#!/usr/bin/env python3
"""Bounded mixed-workload collector for the isolated hardening CS2 server."""
from __future__ import annotations

import argparse
import datetime as dt
import importlib.util
import json
import math
import os
import re
import selectors
import stat
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any


def _load_jsonc_stripper():
    """Load the repository's shared JSONC semantics from any supported fixture depth."""
    for parent in Path(__file__).resolve().parents:
        path = parent / "scripts" / "lib" / "jsonc.py"
        if not path.is_file():
            continue
        spec = importlib.util.spec_from_file_location("_s2script_jsonc", path)
        if spec is None or spec.loader is None:
            break
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module.strip_jsonc_comments
    raise RuntimeError("cannot locate repository scripts/lib/jsonc.py for fixture config parsing")


strip_jsonc_comments = _load_jsonc_stripper()


RESOURCE_KEYS = ("jobs", "completion", "sockets", "sqlite", "pools", "inbound", "outbound", "timers")
RESOURCE_FIELDS = ("items", "bytes", "rejected")
QUEUED_KEYS = ("worker", "http", "db", "ws", "net")
STAGED_KEYS = ("timers", "ws", "net", "cookies", "http", "db")
CACHE_KEYS = ("accounts", "entries", "bytes")
FRAME_KEYS = ("items", "bytes", "polls")
LOADER_WORKER_KEYS = (
    "obligations", "queued", "inFlight", "results", "controls", "config", "baselines", "proposals",
)
LOADER_REJECTED_KEYS = (
    "requestItems", "requestBytes", "resultItems", "resultBytes", "controlItems", "controlBytes",
    "configPaths", "configBytes", "pendingItems", "pendingBytes", "retainedItems", "retainedBytes",
)
LOADER_HIGH_WATER_KEYS = (
    "obligations", "requestBytes", "resultBytes", "queued", "inFlight", "results", "controlItems",
    "controlBytes", "configPaths", "configBytes", "baselineItems", "baselineBytes", "proposalItems",
    "proposalBytes", "pendingItems", "pendingBytes", "retainedItems", "retainedBytes",
)
LOADER_LIMIT_KEYS = (
    "requestItems", "requestBytes", "resultItems", "resultBytes", "preparedItems", "preparedBytes",
    "scanEntries", "scanCandidates", "pathBytes", "archiveBytes", "configBytes", "configBaselineItems",
    "configBaselineBytes", "parse", "drainItems", "drainBytes", "drainMicros",
)
LOADER_PARSE_LIMIT_KEYS = (
    "zipEntries", "memberNameBytes", "manifestBytes", "pluginJsBytes", "gamedataBytes",
)
EXPECTED_COMPONENTS = {"timers": 12, "http": 2, "tcp": 2, "hooks": 16}
ERROR_RE = re.compile(r"\b(?:error|panic|fatal|assert(?:ion)? failed|segmentation fault|crash)\b", re.I)
STATS_PART_PREFIX = "[mixed-live] STATS_PART "
STATS_ERROR_PREFIX = "[mixed-live] STATS_ERROR "
STATS_PART_RE = re.compile(
    r"snapshot=(?P<snapshot>[1-9][0-9]*) part=(?P<part>[1-9][0-9]*)/"
    r"(?P<count>[1-9][0-9]*) bytes=(?P<bytes>[0-9]+) data=(?P<data>.*)"
)
STATUS_RE = re.compile(r"\[mixed-live\] STATUS (?P<body>[^\r\n]+)")
PRESSURE_RE = re.compile(r"\[mixed-live\] PRESSURE (?P<body>[^\r\n]+)")
MAX_OUTPUT = 262144
MAX_STATS_BYTES = 65536
MAX_STATS_PART_BYTES = 1024
MAX_STATS_PARTS = 64
SLOT_PENDING_CAP = 64
PEER_STOP_RESERVE = 10.0
PEER_STOP_TIMEOUT = 10.0
# Keep every measured generation at the same on-disk size.  57 is the largest
# generation used by the acceptance soak; trailing spaces remain valid JSON.
CONFIG_CAPACITY = 17


def _nonnegative_int(value: Any, path: str) -> int:
    if type(value) is not int or value < 0:
        raise ValueError(f"{path} must be a non-negative integer")
    return value


def _positive_int(value: Any, path: str) -> int:
    value = _nonnegative_int(value, path)
    if value == 0:
        raise ValueError(f"{path} must be a positive integer")
    return value


def _exact_object(value: Any, path: str, keys: tuple[str, ...]) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(f"{path} must be an object")
    missing = sorted(set(keys) - value.keys())
    extra = sorted(value.keys() - set(keys))
    if missing:
        raise ValueError(f"{path}.{missing[0]} is required")
    if extra:
        raise ValueError(f"{path}.{extra[0]} is not in the frozen schema")
    return value


def _at_most(value: int, cap: int, path: str, cap_path: str) -> None:
    if value > cap:
        raise ValueError(f"{path}={value} exceeds {cap_path}={cap}")


def _loader_pair(section: Any, path: str) -> dict[str, int]:
    value = _exact_object(section, path, ("items", "bytes"))
    return {key: _nonnegative_int(value[key], f"{path}.{key}") for key in ("items", "bytes")}


def _loader_config(section: Any) -> dict[str, int]:
    value = _exact_object(section, "loader.worker.config", ("paths", "bytes"))
    return {
        "items": _nonnegative_int(value["paths"], "loader.worker.config.paths"),
        "bytes": _nonnegative_int(value["bytes"], "loader.worker.config.bytes"),
    }


def validate_loader(loader_value: Any) -> dict[str, Any]:
    loader = _exact_object(
        loader_value, "loader", ("running", "worker", "main", "rejected", "highWater", "limits"),
    )
    if type(loader["running"]) is not bool:
        raise ValueError("loader.running must be a boolean")
    if not loader["running"]:
        raise ValueError("loader.running must remain true during the live soak")

    worker = _exact_object(loader["worker"], "loader.worker", LOADER_WORKER_KEYS)
    obligations = _exact_object(
        worker["obligations"], "loader.worker.obligations", ("items", "requestBytes", "resultBytes"),
    )
    for key in ("items", "requestBytes", "resultBytes"):
        _nonnegative_int(obligations[key], f"loader.worker.obligations.{key}")
    for key in ("queued", "inFlight", "results"):
        _nonnegative_int(worker[key], f"loader.worker.{key}")
    controls = _exact_object(worker["controls"], "loader.worker.controls", ("items", "bytes", "pending"))
    for key in ("items", "bytes", "pending"):
        _nonnegative_int(controls[key], f"loader.worker.controls.{key}")
    config = _loader_config(worker["config"])
    baselines = _loader_pair(worker["baselines"], "loader.worker.baselines")
    proposals = _loader_pair(worker["proposals"], "loader.worker.proposals")

    main = _exact_object(
        loader["main"], "loader.main", ("pending", "active", "ready", "waiting", "applying", "retained"),
    )
    pending = _loader_pair(main["pending"], "loader.main.pending")
    retained = _loader_pair(main["retained"], "loader.main.retained")
    for key in ("active", "ready", "waiting", "applying"):
        _nonnegative_int(main[key], f"loader.main.{key}")

    rejected = _exact_object(loader["rejected"], "loader.rejected", LOADER_REJECTED_KEYS)
    for key in LOADER_REJECTED_KEYS:
        _nonnegative_int(rejected[key], f"loader.rejected.{key}")
    high_water = _exact_object(loader["highWater"], "loader.highWater", LOADER_HIGH_WATER_KEYS)
    for key in LOADER_HIGH_WATER_KEYS:
        _nonnegative_int(high_water[key], f"loader.highWater.{key}")
    limits = _exact_object(loader["limits"], "loader.limits", LOADER_LIMIT_KEYS)
    for key in LOADER_LIMIT_KEYS:
        if key != "parse":
            _positive_int(limits[key], f"loader.limits.{key}")
    parse_limits = _exact_object(limits["parse"], "loader.limits.parse", LOADER_PARSE_LIMIT_KEYS)
    for key in LOADER_PARSE_LIMIT_KEYS:
        _positive_int(parse_limits[key], f"loader.limits.parse.{key}")

    obligation_item_cap = min(limits["requestItems"], limits["resultItems"])
    _at_most(obligations["items"], obligation_item_cap, "loader.worker.obligations.items",
             "min(loader.limits.requestItems,loader.limits.resultItems)")
    _at_most(obligations["requestBytes"], limits["requestBytes"],
             "loader.worker.obligations.requestBytes", "loader.limits.requestBytes")
    _at_most(obligations["resultBytes"], limits["resultBytes"],
             "loader.worker.obligations.resultBytes", "loader.limits.resultBytes")
    for key in ("queued", "inFlight", "results"):
        _at_most(worker[key], obligations["items"], f"loader.worker.{key}",
                 "loader.worker.obligations.items")
    _at_most(controls["items"], limits["requestItems"], "loader.worker.controls.items",
             "loader.limits.requestItems")
    _at_most(controls["bytes"], limits["requestBytes"], "loader.worker.controls.bytes",
             "loader.limits.requestBytes")
    _at_most(controls["pending"], controls["items"], "loader.worker.controls.pending",
             "loader.worker.controls.items")
    _at_most(config["items"], limits["configBaselineItems"], "loader.worker.config.paths",
             "loader.limits.configBaselineItems")
    _at_most(config["bytes"], limits["configBaselineBytes"], "loader.worker.config.bytes",
             "loader.limits.configBaselineBytes")
    for name, section in (("baselines", baselines), ("proposals", proposals)):
        _at_most(section["items"], config["items"], f"loader.worker.{name}.items",
                 "loader.worker.config.paths")
        _at_most(section["bytes"], config["bytes"], f"loader.worker.{name}.bytes",
                 "loader.worker.config.bytes")
    if config["bytes"] != baselines["bytes"] + proposals["bytes"]:
        raise ValueError("loader.worker.config.bytes must equal baselines.bytes plus proposals.bytes")
    if config["items"] < max(baselines["items"], proposals["items"]) or \
            config["items"] > baselines["items"] + proposals["items"]:
        raise ValueError("loader.worker.config.paths must be the union of baseline and proposal paths")
    _at_most(pending["items"], limits["requestItems"], "loader.main.pending.items",
             "loader.limits.requestItems")
    _at_most(pending["bytes"], limits["requestBytes"], "loader.main.pending.bytes",
             "loader.limits.requestBytes")
    if retained["items"] != main["active"] + main["ready"] + main["waiting"] + main["applying"]:
        raise ValueError("loader.main.retained.items must equal active plus ready plus waiting plus applying")
    _at_most(retained["items"], limits["preparedItems"], "loader.main.retained.items",
             "loader.limits.preparedItems")
    _at_most(retained["bytes"], limits["preparedBytes"], "loader.main.retained.bytes",
             "loader.limits.preparedBytes")

    high_water_caps = {
        "obligations": (obligation_item_cap, "min(loader.limits.requestItems,loader.limits.resultItems)"),
        "requestBytes": (limits["requestBytes"], "loader.limits.requestBytes"),
        "resultBytes": (limits["resultBytes"], "loader.limits.resultBytes"),
        "queued": (obligation_item_cap, "min(loader.limits.requestItems,loader.limits.resultItems)"),
        "inFlight": (obligation_item_cap, "min(loader.limits.requestItems,loader.limits.resultItems)"),
        "results": (obligation_item_cap, "min(loader.limits.requestItems,loader.limits.resultItems)"),
        "controlItems": (limits["requestItems"], "loader.limits.requestItems"),
        "controlBytes": (limits["requestBytes"], "loader.limits.requestBytes"),
        "configPaths": (limits["configBaselineItems"], "loader.limits.configBaselineItems"),
        "configBytes": (limits["configBaselineBytes"], "loader.limits.configBaselineBytes"),
        "baselineItems": (limits["configBaselineItems"], "loader.limits.configBaselineItems"),
        "baselineBytes": (limits["configBaselineBytes"], "loader.limits.configBaselineBytes"),
        "proposalItems": (limits["configBaselineItems"], "loader.limits.configBaselineItems"),
        "proposalBytes": (limits["configBaselineBytes"], "loader.limits.configBaselineBytes"),
        "pendingItems": (limits["requestItems"], "loader.limits.requestItems"),
        "pendingBytes": (limits["requestBytes"], "loader.limits.requestBytes"),
        "retainedItems": (limits["preparedItems"], "loader.limits.preparedItems"),
        "retainedBytes": (limits["preparedBytes"], "loader.limits.preparedBytes"),
    }
    for key, (cap, cap_path) in high_water_caps.items():
        _at_most(high_water[key], cap, f"loader.highWater.{key}", cap_path)
    for key in ("queued", "inFlight", "results"):
        _at_most(high_water[key], high_water["obligations"], f"loader.highWater.{key}",
                 "loader.highWater.obligations")
    current_high_water = {
        "obligations": obligations["items"], "requestBytes": obligations["requestBytes"],
        "resultBytes": obligations["resultBytes"], "queued": worker["queued"],
        "inFlight": worker["inFlight"], "results": worker["results"],
        "controlItems": controls["items"], "controlBytes": controls["bytes"],
        "configPaths": config["items"], "configBytes": config["bytes"],
        "baselineItems": baselines["items"], "baselineBytes": baselines["bytes"],
        "proposalItems": proposals["items"], "proposalBytes": proposals["bytes"],
        "pendingItems": pending["items"], "pendingBytes": pending["bytes"],
        "retainedItems": retained["items"], "retainedBytes": retained["bytes"],
    }
    for key, current in current_high_water.items():
        _at_most(current, high_water[key], f"loader current {key}", f"loader.highWater.{key}")
    return loader


def parse_stats(output: str) -> dict[str, Any]:
    errors = [
        line.split(STATS_ERROR_PREFIX, 1)[1]
        for line in output.splitlines() if STATS_ERROR_PREFIX in line
    ]
    if errors:
        raise ValueError("native stats fixture error: " + "; ".join(errors))
    parts: list[tuple[int, int, int, int, str]] = []
    for line in output.splitlines():
        if STATS_PART_PREFIX not in line:
            continue
        body = line.split(STATS_PART_PREFIX, 1)[1]
        match = STATS_PART_RE.fullmatch(body)
        if match is None:
            raise ValueError(f"malformed native stats part: {body[:160]}")
        snapshot = int(match.group("snapshot"))
        part = int(match.group("part"))
        count = int(match.group("count"))
        declared_bytes = int(match.group("bytes"))
        data = match.group("data")
        data_bytes = len(data.encode("utf-8"))
        if count > MAX_STATS_PARTS:
            raise ValueError(f"native stats part count {count} exceeds cap {MAX_STATS_PARTS}")
        if declared_bytes > MAX_STATS_BYTES:
            raise ValueError(f"native stats bytes {declared_bytes} exceeds cap {MAX_STATS_BYTES}")
        if data_bytes > MAX_STATS_PART_BYTES:
            raise ValueError(
                f"native stats part data bytes {data_bytes} exceeds cap {MAX_STATS_PART_BYTES}"
            )
        if part > count:
            raise ValueError(f"native stats part {part} exceeds declared count {count}")
        parts.append((snapshot, part, count, declared_bytes, data))
    if not parts:
        raise ValueError("expected one multipart native stats snapshot, got 0 parts")
    snapshots = {item[0] for item in parts}
    if len(snapshots) != 1:
        raise ValueError(f"mixed native stats snapshots: {sorted(snapshots)}")
    counts = {item[2] for item in parts}
    declared_lengths = {item[3] for item in parts}
    if len(counts) != 1 or len(declared_lengths) != 1:
        raise ValueError("inconsistent native stats part counts or byte lengths")
    count = parts[0][2]
    by_index: dict[int, str] = {}
    for _snapshot, part, _count, _declared_bytes, data in parts:
        if part in by_index:
            raise ValueError(f"duplicate native stats part {part}")
        by_index[part] = data
    missing = sorted(set(range(1, count + 1)) - by_index.keys())
    if missing:
        raise ValueError(f"missing native stats parts: {missing}")
    if len(parts) != count:
        raise ValueError(f"native stats part count mismatch: expected {count}, got {len(parts)}")
    payload = "".join(by_index[index] for index in range(1, count + 1))
    actual_bytes = len(payload.encode("utf-8"))
    declared_bytes = parts[0][3]
    if actual_bytes != declared_bytes:
        raise ValueError(f"native stats length mismatch: declared={declared_bytes} actual={actual_bytes}")
    try:
        stats = json.loads(payload)
    except json.JSONDecodeError as exc:
        raise ValueError(f"invalid native stats JSON: {exc}") from exc
    if not isinstance(stats, dict):
        raise ValueError("native stats must be an object")
    for key in RESOURCE_KEYS:
        section = stats.get(key)
        if not isinstance(section, dict):
            raise ValueError(f"{key} must be an object")
        for field in RESOURCE_FIELDS:
            _nonnegative_int(section.get(field), f"{key}.{field}")
    for group, keys in (("queued", QUEUED_KEYS), ("staged", STAGED_KEYS), ("cache", CACHE_KEYS)):
        section = stats.get(group)
        if not isinstance(section, dict):
            raise ValueError(f"{group} must be an object")
        for key in keys:
            _nonnegative_int(section.get(key), f"{group}.{key}")
    frame = stats.get("frame")
    if frame is not None:
        if not isinstance(frame, dict):
            raise ValueError("frame must be an object or null")
        for key in FRAME_KEYS:
            _nonnegative_int(frame.get(key), f"frame.{key}")
    for key in ("timerExamined", "lastNs", "maxNs"):
        _nonnegative_int(stats.get(key), key)
    stats["loader"] = validate_loader(stats.get("loader"))
    return stats


def compare_quiescent(baseline: dict[str, Any], current: dict[str, Any]) -> list[str]:
    """Compare leak-relevant gauges; cumulative rejection/time counters may increase."""
    errors: list[str] = []
    for key in RESOURCE_KEYS:
        for field in ("items", "bytes"):
            if current[key][field] != baseline[key][field]:
                errors.append(f"{key}.{field}: baseline={baseline[key][field]} current={current[key][field]}")
    for group, keys in (("queued", QUEUED_KEYS), ("staged", STAGED_KEYS), ("cache", CACHE_KEYS)):
        for key in keys:
            if current[group][key] != baseline[group][key]:
                errors.append(f"{group}.{key}: baseline={baseline[group][key]} current={current[group][key]}")
    return errors


def compare_rejections(baseline: dict[str, Any], current: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    for key in RESOURCE_KEYS:
        if current[key]["rejected"] != baseline[key]["rejected"]:
            errors.append(
                f"{key}.rejected: baseline={baseline[key]['rejected']} current={current[key]['rejected']}"
            )
    return errors


def loader_idle_errors(stats: dict[str, Any]) -> list[str]:
    loader = stats["loader"]
    worker = loader["worker"]
    main = loader["main"]
    expected_zero = {
        "loader.worker.obligations.items": worker["obligations"]["items"],
        "loader.worker.obligations.requestBytes": worker["obligations"]["requestBytes"],
        "loader.worker.obligations.resultBytes": worker["obligations"]["resultBytes"],
        "loader.worker.queued": worker["queued"],
        "loader.worker.inFlight": worker["inFlight"],
        "loader.worker.results": worker["results"],
        "loader.worker.controls.pending": worker["controls"]["pending"],
        "loader.worker.proposals.items": worker["proposals"]["items"],
        "loader.worker.proposals.bytes": worker["proposals"]["bytes"],
        "loader.main.pending.items": main["pending"]["items"],
        "loader.main.pending.bytes": main["pending"]["bytes"],
        "loader.main.active": main["active"],
        "loader.main.ready": main["ready"],
        "loader.main.waiting": main["waiting"],
        "loader.main.applying": main["applying"],
        "loader.main.retained.items": main["retained"]["items"],
        "loader.main.retained.bytes": main["retained"]["bytes"],
    }
    errors = [f"{path}={value}, expected 0" for path, value in expected_zero.items() if value != 0]
    if worker["config"]["paths"] != worker["baselines"]["items"]:
        errors.append(
            "loader.worker.config.paths=" + str(worker["config"]["paths"]) +
            " differs from loader.worker.baselines.items=" + str(worker["baselines"]["items"])
        )
    if worker["config"]["bytes"] != worker["baselines"]["bytes"]:
        errors.append(
            "loader.worker.config.bytes=" + str(worker["config"]["bytes"]) +
            " differs from loader.worker.baselines.bytes=" + str(worker["baselines"]["bytes"])
        )
    return errors


def loader_plateau(stats: dict[str, Any]) -> dict[str, Any]:
    worker = stats["loader"]["worker"]
    return {
        "worker": {
            "controls": {key: worker["controls"][key] for key in ("items", "bytes")},
            "config": dict(worker["config"]),
            "baselines": dict(worker["baselines"]),
        }
    }


def compare_loader_plateau(baseline: dict[str, Any], current: dict[str, Any]) -> list[str]:
    left = loader_plateau(baseline)
    right = loader_plateau(current)
    errors: list[str] = []
    for section in ("controls", "config", "baselines"):
        for key, expected in left["worker"][section].items():
            actual = right["worker"][section][key]
            if actual != expected:
                errors.append(
                    f"loader.worker.{section}.{key}: baseline={expected} current={actual}"
                )
    return errors


def compare_loader_warm_transition(
    original: dict[str, Any], measured: dict[str, Any], payload_delta: int,
) -> list[str]:
    left = loader_plateau(original)["worker"]
    right = loader_plateau(measured)["worker"]
    expected = {
        "controls.items": left["controls"]["items"],
        "controls.bytes": left["controls"]["bytes"],
        "config.paths": left["config"]["paths"],
        "config.bytes": left["config"]["bytes"] + payload_delta,
        "baselines.items": left["baselines"]["items"],
        "baselines.bytes": left["baselines"]["bytes"] + payload_delta,
    }
    errors: list[str] = []
    for dotted, expected_value in expected.items():
        section, key = dotted.split(".", 1)
        actual = right[section][key]
        if actual != expected_value:
            errors.append(
                f"loader.worker.{dotted}: expected={expected_value} current={actual} "
                f"payloadDelta={payload_delta}"
            )
    return errors


def compare_loader_rejections(baseline: dict[str, Any], current: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    for key in LOADER_REJECTED_KEYS:
        expected = baseline["loader"]["rejected"][key]
        actual = current["loader"]["rejected"][key]
        if actual != expected:
            errors.append(f"loader.rejected.{key}: baseline={expected} current={actual}")
    return errors


def parse_config_generation(content: bytes) -> int:
    try:
        source = content.decode("utf-8")
        value = json.loads(strip_jsonc_comments(source))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"original fixture config is not valid UTF-8 JSONC: {exc}") from exc
    if not isinstance(value, dict):
        raise ValueError("original fixture config must be an object")
    return _nonnegative_int(value.get("generation"), "original fixture config generation")


def parse_status_generation(output: str) -> int:
    matches = list(STATUS_RE.finditer(output))
    if len(matches) != 1:
        raise ValueError(f"expected exactly one STATUS line, got {len(matches)}")
    for token in matches[0].group("body").split():
        if token.startswith("config="):
            try:
                return _nonnegative_int(int(token.split("=", 1)[1]), "STATUS config")
            except ValueError as exc:
                raise ValueError("STATUS config must be a non-negative integer") from exc
    raise ValueError("STATUS config is missing")


def measurement_summary(values: list[int]) -> dict[str, int]:
    if not values:
        raise ValueError("cannot summarize an empty measurement set")
    ordered = sorted(values)
    def nearest_rank(percent: int) -> int:
        return ordered[max(0, math.ceil(len(ordered) * percent / 100) - 1)]
    return {
        "count": len(ordered), "min": ordered[0], "p50": nearest_rank(50),
        "p95": nearest_rank(95), "p99": nearest_rank(99), "max": ordered[-1],
    }


def parse_rss_kib(output: str) -> int:
    total = 0
    lines = output.splitlines()
    if not lines or "RSS" not in lines[0].split():
        raise ValueError("docker top output has no RSS header")
    header = lines[0].split()
    rss_index = header.index("RSS")
    for line in lines[1:]:
        fields = line.split(None, len(header) - 1)
        if len(fields) <= rss_index or not fields[rss_index].isdigit():
            raise ValueError(f"invalid docker top RSS row: {line}")
        total += int(fields[rss_index])
    return total


def parse_cycle_status(output: str, cycle: int) -> dict[str, Any]:
    matches = list(STATUS_RE.finditer(output))
    if len(matches) != 1:
        raise ValueError(f"expected exactly one STATUS line, got {len(matches)}")
    fields: dict[str, str] = {}
    for token in matches[0].group("body").split():
        if "=" in token:
            key, value = token.split("=", 1)
            fields[key] = value
    if fields.get("cycle") != str(cycle):
        raise ValueError(f"expected cycle={cycle}, got cycle={fields.get('cycle')}")
    required = {"state", "pass", "timers", "http", "tcp", "hooks", "db", "config",
                "slotAttempts", "slotReuses", "slotFailures", "slotPending"}
    missing = sorted(required - fields.keys())
    if missing:
        raise ValueError("missing status fields: " + ",".join(missing))
    if fields["state"] != "done" or fields["pass"] != "true" or fields["db"] != "pass":
        raise ValueError("completed cycle must report state=done pass=true db=pass")
    for field, expected_count in EXPECTED_COMPONENTS.items():
        left, sep, right = fields[field].partition("/")
        if not sep or not left.isdigit() or not right.isdigit():
            raise ValueError(f"{field} must report non-negative completed/expected counts")
        if int(left) != expected_count or int(right) != expected_count:
            raise ValueError(f"{field} must report exact {expected_count}/{expected_count}")
    result: dict[str, Any] = dict(fields)
    for field in ("config", "slotAttempts", "slotReuses", "slotFailures", "slotPending"):
        try:
            result[field] = _nonnegative_int(int(fields[field]), field)
        except ValueError as exc:
            raise ValueError(f"{field} must be a non-negative integer") from exc
    if result["config"] != cycle:
        raise ValueError(f"config generation mismatch: expected {cycle}, got {result['config']}")
    if result["slotFailures"] != 0:
        raise ValueError(f"wrong-client isolation failures: {result['slotFailures']}")
    if result["slotPending"] > SLOT_PENDING_CAP:
        raise ValueError(f"pending stale client handles exceed cap {SLOT_PENDING_CAP}: {result['slotPending']}")
    return result


def parse_pressure_status(output: str, expected: int) -> dict[str, int | str]:
    matches = list(PRESSURE_RE.finditer(output))
    if len(matches) != 1:
        raise ValueError(f"expected exactly one PRESSURE line, got {len(matches)}")
    fields: dict[str, str] = {}
    for token in matches[0].group("body").split():
        if "=" in token:
            key, value = token.split("=", 1)
            fields[key] = value
    required = {"state", "expected", "settled", "success", "rejected", "other"}
    missing = sorted(required - fields.keys())
    if missing:
        raise ValueError("missing pressure fields: " + ",".join(missing))
    result: dict[str, int | str] = {"state": fields["state"]}
    for key in required - {"state"}:
        try:
            result[key] = int(fields[key])
        except ValueError as exc:
            raise ValueError(f"pressure {key} must be an integer") from exc
    if result["state"] != "done" or result["expected"] != expected:
        raise ValueError(f"pressure did not finish expected={expected}")
    if result["settled"] != expected or result["success"] + result["rejected"] + result["other"] != expected:
        raise ValueError("pressure settled accounting mismatch")
    if result["rejected"] <= 0:
        raise ValueError("pressure produced no named AsyncQueueFull rejection")
    if result["other"] != 0:
        raise ValueError(f"pressure saw other errors: {result['other']}")
    return result


def classify(report: dict[str, Any]) -> str:
    if not report["complete"]:
        return "INCOMPLETE"
    exact = (
        not report["errors"]
        and report["cycles_completed"] == report["cycles_requested"]
        and report["reload_acks"] == report["reload_commands"]
        and report["active_transitions"] == report["reload_commands"]
        and report["slot_reuses"] >= 1
        and report["slot_cleanup_complete"]
        and report.get("original_config_applied_at_start", False)
        and report.get("config_restore_verified", False)
        and report.get("loader_final_idle", False)
        and report["final_running_plugins"] == report["expected_running_plugins"]
    )
    return "PASS" if exact else "FAIL"


def run(argv: list[str], root: Path, timeout: float) -> tuple[int, str]:
    proc = subprocess.Popen(argv, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    assert proc.stdout is not None
    os.set_blocking(proc.stdout.fileno(), False)
    selector = selectors.DefaultSelector()
    selector.register(proc.stdout, selectors.EVENT_READ)
    buf = bytearray()
    truncated = False
    deadline = time.monotonic() + timeout
    try:
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                proc.kill()
                proc.wait()
                return 124, bytes(buf).decode("utf-8", "replace") + "\nTIMEOUT\n"
            for key, _ in selector.select(min(0.1, remaining)):
                chunk = os.read(key.fileobj.fileno(), 65536)
                if chunk:
                    buf.extend(chunk)
                    if len(buf) > MAX_OUTPUT:
                        truncated = True
                        del buf[:-MAX_OUTPUT]
                else:
                    selector.unregister(key.fileobj)
            if proc.poll() is not None and not selector.get_map():
                suffix = "\nOUTPUT_TRUNCATED\n" if truncated else ""
                return (125 if truncated else int(proc.returncode)), bytes(buf).decode("utf-8", "replace") + suffix
    finally:
        selector.close()
        proc.stdout.close()


def atomic_write(path: Path, content: str | bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    prior_mode = stat.S_IMODE(path.stat().st_mode) if path.exists() else 0o644
    fd, tmp_name = tempfile.mkstemp(prefix=path.name + ".", dir=path.parent)
    try:
        os.fchmod(fd, prior_mode)
        mode = "wb" if isinstance(content, bytes) else "w"
        encoding = None if isinstance(content, bytes) else "utf-8"
        with os.fdopen(fd, mode, encoding=encoding) as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(tmp_name, path)
    finally:
        try:
            os.unlink(tmp_name)
        except FileNotFoundError:
            pass


def encode_generation_config(generation: int) -> str:
    """Encode one generation into the fixed-capacity JSON fixture payload."""
    content = json.dumps({"generation": generation}, separators=(",", ":"))
    if len(content) > CONFIG_CAPACITY:
        raise ValueError(
            f"generation config requires {len(content)} bytes, capacity is {CONFIG_CAPACITY}"
        )
    return content + (" " * (CONFIG_CAPACITY - len(content)))


def utc() -> dt.datetime:
    return dt.datetime.now(dt.timezone.utc)


def main(argv: list[str] | None = None, *, run_fn=None, clock=None, utc_fn=None) -> int:
    run_fn = run if run_fn is None else run_fn
    clock = time if clock is None else clock
    utc_fn = utc if utc_fn is None else utc_fn
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", type=Path, default=Path("/home/ghirakawa/s2script-hardening"))
    ap.add_argument("--out", type=Path, default=Path(".gate/mixed-soak"))
    ap.add_argument("--duration", type=float, default=3600)
    ap.add_argument("--warmup", type=float, default=180)
    ap.add_argument("--cycle-period", type=float, default=60)
    ap.add_argument("--quiescence", type=float, default=10)
    ap.add_argument("--cycle-timeout", type=float, default=35)
    ap.add_argument("--call-timeout", type=float, default=15)
    ap.add_argument("--port", type=int, default=27016)
    ap.add_argument("--container", default="s2script-cs2-hardening")
    ap.add_argument("--network", default="s2script-cs2-hardening_default")
    ap.add_argument("--peer-container", default="s2script-mixed-slow-peer")
    ap.add_argument("--peer-image", default="python:3.12-alpine")
    ap.add_argument("--reload-plugin", default="@s2script/clientprefs")
    ap.add_argument("--expected-plugins", type=int, default=17)
    ap.add_argument("--source", required=True)
    ap.add_argument("--no-manage-peer", action="store_true")
    args = ap.parse_args(argv)
    if min(args.duration, args.warmup, args.cycle_period, args.quiescence,
           args.cycle_timeout, args.call_timeout) <= 0:
        ap.error("all durations must be positive")
    if args.duration <= args.warmup + args.cycle_period:
        ap.error("duration must leave room for at least one post-warmup cycle")
    if args.quiescence + args.cycle_timeout >= args.cycle_period:
        ap.error("cycle-timeout plus quiescence must be less than cycle-period")

    started = utc_fn()
    run_dir = args.out / ("mixed-soak-" + started.strftime("%Y%m%dT%H%M%SZ"))
    run_dir.mkdir(parents=True, exist_ok=False)
    events_file = run_dir / "events.jsonl"
    fixture_dir = Path(__file__).resolve().parent
    config_path = args.root / "dist/addons/s2script/configs/_fixture_mixed-live.json"
    errors: list[str] = []
    complete = True
    reload_commands = reload_acks = active_transitions = cycles_completed = 0
    slot_reuses = 0
    checkpoints: list[dict[str, Any]] = []
    pressure_report: dict[str, Any] | None = None
    peer_started = False
    peer_stop_attempted = False
    peer_stop_succeeded = False
    peer_cleanup_overrun_seconds = 0.0
    slots_started = False
    slot_cleanup_complete = False
    config_original: bytes | None = None
    original_config_generation: int | None = None
    original_config_applied_at_start = False
    config_restore_verified = False
    loader_final_idle = False
    loader_original_plateau: dict[str, Any] | None = None
    loader_measured_plateau: dict[str, Any] | None = None
    log_cursor = started.timestamp()
    seen_active_lines: set[str] = set()
    seen_error_lines: set[str] = set()
    rss_samples: list[int] = []
    machine = ""
    start_mono = clock.monotonic()
    soak_deadline = start_mono + args.duration
    hard_deadline = soak_deadline + 90

    cycles_requested = int((args.duration - args.warmup) // args.cycle_period)
    if cycles_requested > 99:
        ap.error("measured generation count exceeds fixed-config maximum 99")
    event_serial = 0

    def save_event(kind: str, code: int, output: str, **extra: Any) -> None:
        nonlocal event_serial
        event_serial += 1
        event = {"utc": utc_fn().isoformat(), "kind": kind, "returncode": code, **extra}
        with events_file.open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(event, separators=(",", ":")) + "\n")
        with (run_dir / f"output-{event_serial:04d}-{kind}.txt").open("x", encoding="utf-8") as stream:
            stream.write(output)

    def proof_hard_deadline() -> float:
        return hard_deadline - (PEER_STOP_RESERVE if peer_started else 0.0)

    def call(argv: list[str], kind: str, timeout: float | None = None) -> tuple[int, str]:
        remaining = proof_hard_deadline() - clock.monotonic()
        if remaining <= 0:
            output = "ordinary-operation deadline expired; peer cleanup reserve preserved\n"
            save_event(kind, 124, output, argv=argv, skipped=True)
            return 124, output
        requested = args.call_timeout if timeout is None else timeout
        if requested <= 0:
            output = "operation deadline expired\n"
            save_event(kind, 124, output, argv=argv, skipped=True)
            return 124, output
        code, output = run_fn(argv, args.root, min(requested, remaining))
        save_event(kind, code, output, argv=argv)
        return code, output

    def rcon(*commands: str, timeout: float | None = None) -> tuple[int, str]:
        return call(["python3", "scripts/rcon.py", "--port", str(args.port), *commands], "rcon", timeout)

    def cleanup_slots() -> None:
        nonlocal slot_cleanup_complete, complete
        try:
            code, output = rcon("sm_mixed_slotcleanup")
        except Exception as exc:
            complete = False
            errors.append(f"slot cleanup failed: {type(exc).__name__}: {exc}")
            return
        slot_cleanup_complete = code == 0 and output.count("[mixed-live] SLOT_CLEANUP pending=0 checking=0") == 1
        if not slot_cleanup_complete:
            complete = False
            errors.append(f"slot cleanup did not acknowledge empty state ({code})")

    def collect_logs(label: str) -> None:
        nonlocal log_cursor, active_transitions, complete
        end = utc_fn().timestamp()
        code, output = call(["docker", "logs", "--timestamps", "--since", f"{log_cursor:.6f}",
                             "--until", f"{end:.6f}", args.container], "logs")
        (run_dir / f"logs-{label}.txt").write_text(output, encoding="utf-8")
        if code:
            errors.append(f"log collection {label} failed ({code})")
            complete = False
            return
        log_cursor = end
        for line in output.splitlines():
            if f"[plugins] '{args.reload_plugin}' Active" in line and line not in seen_active_lines:
                seen_active_lines.add(line)
                active_transitions += 1
            if ERROR_RE.search(line) and line not in seen_error_lines:
                seen_error_lines.add(line)
                errors.append(f"server log: {line}")

    loader_idle_samples = 0
    loader_high_water_seen: dict[str, int] | None = None
    loader_limits_baseline: dict[str, Any] | None = None

    def check_loader_continuity(value: dict[str, Any], label: str) -> None:
        nonlocal loader_high_water_seen, loader_limits_baseline
        current_limits = value["loader"]["limits"]
        current_high_water = value["loader"]["highWater"]
        if loader_limits_baseline is None:
            loader_limits_baseline = json.loads(json.dumps(current_limits))
        elif current_limits != loader_limits_baseline:
            errors.append(f"stats {label}: loader.limits changed during continuous worker lifetime")
        if loader_high_water_seen is None:
            loader_high_water_seen = dict(current_high_water)
            return
        for key in LOADER_HIGH_WATER_KEYS:
            previous = loader_high_water_seen[key]
            current = current_high_water[key]
            if current < previous:
                errors.append(
                    f"stats {label}: loader.highWater.{key} decreased from {previous} to {current}"
                )
            else:
                loader_high_water_seen[key] = current

    def stats(label: str, *, timeout: float | None = None) -> dict[str, Any] | None:
        nonlocal complete
        code, output = rcon("sm_mixed_stats", timeout=timeout)
        if code:
            errors.append(f"stats {label} RCON failed ({code})")
            complete = False
            return None
        try:
            value = parse_stats(output)
        except ValueError as exc:
            errors.append(f"stats {label}: {exc}")
            complete = False
            return None
        check_loader_continuity(value, label)
        (run_dir / f"stats-{label}.json").write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
        return value

    def await_loader_idle(
        label: str,
        deadline: float,
        nonloader_baseline: dict[str, Any] | None = None,
    ) -> dict[str, Any] | None:
        nonlocal loader_idle_samples, complete
        sample = 0
        last_idle_errors: list[str] = ["no loader sample captured"]
        poll_interval = min(0.25, args.quiescence / 4)
        while clock.monotonic() < deadline:
            remaining = deadline - clock.monotonic()
            sample += 1
            current = stats(
                f"{label}-sample-{sample:03d}",
                timeout=min(args.call_timeout, remaining),
            )
            loader_idle_samples += 1
            if clock.monotonic() > deadline:
                last_idle_errors = [
                    f"stats sample completed after deadline now={clock.monotonic():.6f} deadline={deadline:.6f}"
                ]
                break
            if current is None:
                if clock.monotonic() >= deadline:
                    break
            else:
                last_idle_errors = loader_idle_errors(current)
                if nonloader_baseline is not None:
                    last_idle_errors.extend(
                        "non-loader " + message
                        for message in compare_quiescent(nonloader_baseline, current)
                    )
                if not last_idle_errors:
                    return current
            remaining = deadline - clock.monotonic()
            if remaining > 0:
                clock.sleep(min(poll_interval, remaining))
        if last_idle_errors == ["no loader sample captured"]:
            complete = False
        errors.append(f"{label} loader idle timeout: " + "; ".join(last_idle_errors))
        return None

    def await_config_application(expected: int, deadline: float, label: str) -> bool:
        nonlocal complete
        last = "no STATUS sample captured"
        poll_interval = min(0.25, args.quiescence / 4)
        while clock.monotonic() < deadline:
            remaining = deadline - clock.monotonic()
            code, output = rcon("sm_mixed_status", timeout=min(args.call_timeout, remaining))
            if clock.monotonic() > deadline:
                last = (
                    f"STATUS call completed after deadline now={clock.monotonic():.6f} "
                    f"deadline={deadline:.6f}"
                )
                break
            if code == 0:
                try:
                    actual = parse_status_generation(output)
                    if actual == expected:
                        return True
                    last = f"expected config={expected}, got config={actual}"
                except ValueError as exc:
                    last = str(exc)
            else:
                last = f"RCON failed ({code})"
                if code == 124:
                    complete = False
            remaining = deadline - clock.monotonic()
            if remaining > 0:
                clock.sleep(min(poll_interval, remaining))
        errors.append(f"config {label} runtime application timeout: {last}")
        return False

    def rss(label: str) -> int | None:
        nonlocal complete
        code, output = call(["docker", "top", args.container, "-eo", "pid,ppid,comm,rss,args"], "rss")
        (run_dir / f"rss-{label}.txt").write_text(output, encoding="utf-8")
        if code:
            complete = False
            errors.append(f"RSS {label} failed ({code})")
            return None
        try:
            value = parse_rss_kib(output)
        except ValueError as exc:
            complete = False
            errors.append(f"RSS {label}: {exc}")
            return None
        rss_samples.append(value)
        return value

    def run_cycle(cycle: int) -> dict[str, Any] | None:
        nonlocal complete, reload_commands, reload_acks, slot_reuses
        deadline = min(clock.monotonic() + args.cycle_timeout, soak_deadline)

        def cycle_rcon(*commands: str) -> tuple[int, str]:
            remaining = deadline - clock.monotonic()
            if remaining <= 0:
                return 124, "cycle deadline expired\n"
            return rcon(*commands, timeout=min(args.call_timeout, remaining))

        atomic_write(config_path, encode_generation_config(cycle))
        reload_commands += 1
        code, output = cycle_rcon(f"sm plugins reload {args.reload_plugin}")
        if code:
            errors.append(f"cycle {cycle} reload RCON failed ({code})")
            complete = False
        ack = output.count("[SM] Reloading")
        reload_acks += ack
        if ack != 1:
            errors.append(f"cycle {cycle} reload expected 1 ACK, got {ack}")
        code, output = cycle_rcon(f"sm_mixed_start {cycle}", f"sm_mixed_slotcycle {cycle}")
        if code:
            errors.append(f"cycle {cycle} start RCON failed ({code})")
            complete = False
            return None
        last = ""
        while clock.monotonic() < deadline:
            clock.sleep(min(1, max(0, deadline - clock.monotonic())))
            if clock.monotonic() >= deadline:
                break
            code, last = cycle_rcon("sm_mixed_status")
            if clock.monotonic() > deadline:
                break
            if code:
                continue
            try:
                result = parse_cycle_status(last, cycle)
            except (ValueError, TypeError):
                continue
            slot_reuses = int(result["slotReuses"])
            return result
        errors.append(f"cycle {cycle} did not complete in {args.cycle_timeout}s; last={last[-300:]}")
        complete = False
        return None

    manifest = {
        "started_utc": started.isoformat(), "source": args.source, "duration_seconds": args.duration,
        "warmup_seconds": args.warmup, "cycle_period_seconds": args.cycle_period,
        "quiescence_seconds": args.quiescence, "cycles_requested": cycles_requested,
        "root": str(args.root), "container": args.container, "network": args.network,
        "rcon": f"127.0.0.1:{args.port}", "reload_plugin": args.reload_plugin,
        "expected_running_plugins": args.expected_plugins,
        "peer_stop_reserve_seconds": PEER_STOP_RESERVE,
        "rss_scope": "whole container process RSS; recorded only, never used as a leak assertion",
    }
    (run_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    baseline: dict[str, Any] | None = None
    final_running = 0
    try:
        code, actual_source = call(["git", "rev-parse", "HEAD"], "source")
        if code:
            raise RuntimeError(f"source verification failed ({code})")
        actual_source = actual_source.strip()
        if actual_source != args.source:
            raise RuntimeError(f"source mismatch: requested={args.source} actual={actual_source}")
        if not config_path.exists():
            raise RuntimeError(f"fixture config not materialized: {config_path}")
        config_original = config_path.read_bytes()
        original_config_generation = parse_config_generation(config_original)
        if not args.no_manage_peer:
            code, _ = call(["docker", "inspect", args.peer_container], "peer-preflight")
            if code == 0:
                raise RuntimeError(f"refusing to replace existing container {args.peer_container}")
            code, output = call([
                "docker", "run", "--rm", "-d", "--name", args.peer_container,
                "--network", args.network, "-v", f"{fixture_dir}:/fixture:ro", args.peer_image,
                "python3", "/fixture/slow-peer.py", "--host", "0.0.0.0",
            ], "peer-start", 60)
            if code:
                raise RuntimeError(f"private peer start failed ({code}): {output[-300:]}")
            peer_started = True
            ready = False
            for _ in range(20):
                code, peer_logs = call(["docker", "logs", args.peer_container], "peer-ready")
                if code == 0 and "[mixed-slow-peer] ready" in peer_logs:
                    ready = True
                    break
                clock.sleep(0.25)
            if not ready:
                raise RuntimeError("private peer did not report readiness")
            code, peer_network = call([
                "docker", "inspect", "--format",
                "{{json .NetworkSettings.Ports}}|{{json .NetworkSettings.Networks}}", args.peer_container,
            ], "peer-network")
            if code or not peer_network.startswith("{}|") or args.network not in peer_network:
                raise RuntimeError(f"private peer network/public-port check failed: {peer_network[-500:]}")
        code, machine = call(["uname", "-a"], "machine")
        if code:
            raise RuntimeError(f"machine identity failed ({code})")
        code, output = rcon("meta list", "sm plugins list", "status")
        if code:
            raise RuntimeError(f"preflight RCON failed ({code})")
        (run_dir / "status-start.txt").write_text(output, encoding="utf-8")
        final_running = sum(1 for line in output.splitlines() if "(running)" in line.lower())
        if final_running != args.expected_plugins:
            raise RuntimeError(f"preflight expected {args.expected_plugins} running plugins, got {final_running}")
        code, output = rcon("sm_mixed_slotreset")
        slots_started = True
        if code or output.count("[mixed-live] SLOT_RESET pending=0 checking=0") != 1:
            raise RuntimeError(f"slot proof reset failed ({code})")
        rcon("bot_quota_mode normal", "bot_join_after_player 0", "mp_limitteams 0", "bot_quota 2")

        original_config_applied_at_start = await_config_application(
            original_config_generation,
            min(clock.monotonic() + args.quiescence, soak_deadline),
            "original preflight",
        )
        if not original_config_applied_at_start:
            raise RuntimeError("original fixture config was not applied before workload edits")

        # Saturate one owner's worker jobs once, then require every success/rejection to settle and
        # every live gauge to return. Cumulative rejection counters after this become the baseline
        # for the non-saturating mixed soak.
        pressure_before = await_loader_idle(
            "pressure-before", min(clock.monotonic() + args.quiescence, soak_deadline)
        )
        rss("pressure-before")
        if pressure_before is None:
            raise RuntimeError("pre-pressure stats unavailable")
        loader_original_plateau = loader_plateau(pressure_before)
        code, output = rcon("sm_mixed_pressure 256")
        if code:
            raise RuntimeError(f"pressure start RCON failed ({code})")
        pressure_deadline = min(clock.monotonic() + args.cycle_timeout, soak_deadline)
        last_pressure = ""
        while clock.monotonic() < pressure_deadline:
            clock.sleep(min(0.5, max(0, pressure_deadline - clock.monotonic())))
            remaining = pressure_deadline - clock.monotonic()
            if remaining <= 0:
                break
            code, last_pressure = rcon("sm_mixed_pressure_status", timeout=min(args.call_timeout, remaining))
            if clock.monotonic() > pressure_deadline:
                break
            if code:
                continue
            try:
                pressure_report = parse_pressure_status(last_pressure, 256)
                break
            except (ValueError, TypeError):
                continue
        if pressure_report is None:
            raise RuntimeError(f"pressure did not settle in {args.cycle_timeout}s; last={last_pressure[-300:]}")
        prewarm = await_loader_idle(
            "pressure-after",
            min(clock.monotonic() + args.quiescence, soak_deadline),
            pressure_before,
        )
        rss("pressure-after")
        if prewarm is None:
            raise RuntimeError("post-pressure stats unavailable")
        pressure_gauges = compare_quiescent(pressure_before, prewarm)
        errors.extend("pressure quiescence " + message for message in pressure_gauges)
        errors.extend(
            "pressure loader plateau " + message
            for message in compare_loader_plateau(pressure_before, prewarm)
        )
        errors.extend(
            "pressure loader rejection " + message
            for message in compare_loader_rejections(pressure_before, prewarm)
        )
        native_rejected = prewarm["jobs"]["rejected"] - pressure_before["jobs"]["rejected"]
        if native_rejected != pressure_report["rejected"]:
            errors.append(
                f"pressure jobs.rejected delta={native_rejected} pluginNamed={pressure_report['rejected']}"
            )

        # Warm-up runs the full mixed cycle once. The post-warmup snapshot becomes the baseline for
        # every measured cycle, after proving that warm-up itself returned to post-pressure idle.
        if clock.monotonic() >= soak_deadline:
            raise RuntimeError("soak deadline expired before warm-up cycle")
        warm = run_cycle(0)
        if warm is None:
            raise RuntimeError("warm-up cycle failed")
        warm_end = min(start_mono + args.warmup, soak_deadline)
        if clock.monotonic() >= warm_end:
            raise RuntimeError("warm-up left no time for bounded loader quiescence")
        baseline = await_loader_idle("baseline", warm_end, prewarm)
        if baseline is None:
            raise RuntimeError("baseline loader did not become idle during warm-up")
        if clock.monotonic() < warm_end:
            clock.sleep(warm_end - clock.monotonic())
        rss("baseline")
        collect_logs("baseline")
        errors.extend("warm-up quiescence " + message for message in compare_quiescent(prewarm, baseline))
        errors.extend("warm-up admission " + message for message in compare_rejections(prewarm, baseline))
        errors.extend(
            "warm-up loader admission " + message
            for message in compare_loader_rejections(prewarm, baseline)
        )
        payload_delta = len(encode_generation_config(0).encode("utf-8")) - len(config_original)
        transition_errors = compare_loader_warm_transition(prewarm, baseline, payload_delta)
        errors.extend("warm-up loader transition " + message for message in transition_errors)
        loader_measured_plateau = loader_plateau(baseline)

        for cycle in range(1, cycles_requested + 1):
            cycle_anchor = start_mono + args.warmup + (cycle - 1) * args.cycle_period
            if clock.monotonic() < cycle_anchor:
                clock.sleep(cycle_anchor - clock.monotonic())
            if clock.monotonic() >= soak_deadline:
                complete = False
                errors.append(f"soak deadline expired before measured cycle {cycle}")
                break
            result = run_cycle(cycle)
            if result is None:
                break
            cycles_completed += 1
            current = await_loader_idle(
                f"cycle-{cycle:03d}",
                min(clock.monotonic() + args.quiescence, soak_deadline),
                baseline,
            )
            rss_value = rss(f"cycle-{cycle:03d}")
            collect_logs(f"cycle-{cycle:03d}")
            if current is None:
                break
            gauge_errors = compare_quiescent(baseline, current)
            rejection_errors = compare_rejections(baseline, current)
            loader_plateau_errors = compare_loader_plateau(baseline, current)
            loader_rejection_errors = compare_loader_rejections(baseline, current)
            errors.extend(f"cycle {cycle} quiescence {message}" for message in gauge_errors)
            errors.extend(f"cycle {cycle} admission {message}" for message in rejection_errors)
            errors.extend(f"cycle {cycle} loader plateau {message}" for message in loader_plateau_errors)
            errors.extend(f"cycle {cycle} loader admission {message}" for message in loader_rejection_errors)
            checkpoints.append({
                "cycle": cycle, "status": result, "gauge_errors": gauge_errors,
                "rejection_errors": rejection_errors,
                "loader_plateau_errors": loader_plateau_errors,
                "loader_rejection_errors": loader_rejection_errors,
                "rejected": {key: current[key]["rejected"] for key in RESOURCE_KEYS},
                "loaderRejected": dict(current["loader"]["rejected"]),
                "loaderHighWater": dict(current["loader"]["highWater"]),
                "loaderPlateau": loader_plateau(current),
                "lastNs": current["lastNs"], "maxNs": current["maxNs"],
                "timerExamined": current["timerExamined"],
                "rssKiB": rss_value,
            })
            (run_dir / "checkpoints.json").write_text(json.dumps(checkpoints, indent=2) + "\n", encoding="utf-8")

        if clock.monotonic() < soak_deadline:
            clock.sleep(soak_deadline - clock.monotonic())
        cleanup_slots()
        if config_original is None or original_config_generation is None:
            raise RuntimeError("original config was not captured")
        atomic_write(config_path, config_original)
        cleanup_proof_deadline = proof_hard_deadline()
        application_deadline = min(clock.monotonic() + args.quiescence, cleanup_proof_deadline)
        config_restore_verified = await_config_application(
            original_config_generation, application_deadline, "restore",
        )
        final_stats = await_loader_idle(
            "final-restored",
            min(clock.monotonic() + args.quiescence, cleanup_proof_deadline),
            baseline,
        )
        loader_final_idle = final_stats is not None
        collect_logs("final")
        if baseline is not None and final_stats is not None:
            errors.extend("final quiescence " + message for message in compare_quiescent(baseline, final_stats))
            errors.extend("final admission " + message for message in compare_rejections(baseline, final_stats))
            errors.extend(
                "final loader admission " + message
                for message in compare_loader_rejections(baseline, final_stats)
            )
        if loader_original_plateau is not None and final_stats is not None:
            original_loader_errors = compare_loader_plateau(pressure_before, final_stats)
            errors.extend("final original loader plateau " + message for message in original_loader_errors)
        code, output = rcon("sm plugins list", "sm_mixed_status")
        if code:
            complete = False
            errors.append(f"final plugin/status RCON failed ({code})")
        final_running = sum(1 for line in output.splitlines() if "(running)" in line.lower())
        (run_dir / "plugins-final.txt").write_text(output, encoding="utf-8")
        rss("final")
    except Exception as exc:
        complete = False
        errors.append(f"harness exception: {type(exc).__name__}: {exc}")
    finally:
        if slots_started and not slot_cleanup_complete:
            cleanup_slots()
        if peer_started:
            remaining = hard_deadline - clock.monotonic()
            peer_stop_attempted = True
            if remaining <= 0:
                complete = False
                errors.append(
                    f"cleanup hard deadline overrun by {-remaining:.6f}s before peer stop; "
                    "using one fresh bounded cleanup attempt"
                )
                stop_timeout = PEER_STOP_TIMEOUT
            else:
                stop_timeout = min(PEER_STOP_TIMEOUT, remaining)
            code, output = run_fn(
                ["docker", "stop", "--time", "2", args.peer_container],
                args.root,
                stop_timeout,
            )
            save_event("peer-stop", code, output, cleanup_only=True)
            peer_stop_succeeded = code == 0
            peer_cleanup_overrun_seconds = max(0.0, clock.monotonic() - hard_deadline)
            if peer_cleanup_overrun_seconds > 0 and remaining > 0:
                complete = False
                errors.append(
                    f"peer cleanup exceeded hard deadline by {peer_cleanup_overrun_seconds:.6f}s"
                )
            if code:
                complete = False
                errors.append(f"private peer cleanup failed ({code})")
        if config_original is not None and not config_restore_verified:
            try:
                atomic_write(config_path, config_original)
            except Exception as exc:
                complete = False
                errors.append(f"config restore failed: {type(exc).__name__}: {exc}")

    report: dict[str, Any] = {
        **manifest, "complete": complete, "errors": errors[:200], "error_count": len(errors),
        "cycles_completed": cycles_completed, "reload_commands": reload_commands,
        "reload_acks": reload_acks, "active_transitions": active_transitions,
        "slot_reuses": slot_reuses, "slot_pending_cap": SLOT_PENDING_CAP,
        "slot_cleanup_complete": slot_cleanup_complete, "final_running_plugins": final_running,
        "config_restore_verified": config_restore_verified,
        "original_config_applied_at_start": original_config_applied_at_start,
        "original_config_generation": original_config_generation,
        "loader_final_idle": loader_final_idle,
        "loader_idle_samples": loader_idle_samples,
        "loader_original_plateau": loader_original_plateau,
        "loader_measured_plateau": loader_measured_plateau,
        "loader_limits": loader_limits_baseline,
        "loader_high_water_seen": loader_high_water_seen,
        "peer_stop_attempted": peer_stop_attempted,
        "peer_stop_succeeded": peer_stop_succeeded,
        "peer_cleanup_overrun_seconds": peer_cleanup_overrun_seconds,
        "elapsed_seconds": clock.monotonic() - start_mono, "directory": str(run_dir),
        "pressure": pressure_report,
        "environment": {"machine": machine.strip(), "status_start_file": "status-start.txt",
                        "status_final_file": "plugins-final.txt"},
        "measurements": {
            "asyncLastNs": measurement_summary([int(row["lastNs"]) for row in checkpoints]) if checkpoints else None,
            "rssKiB": measurement_summary(rss_samples) if rss_samples else None,
            "asyncMaxNsFinal": checkpoints[-1]["maxNs"] if checkpoints else None,
            "timerExaminedFinal": checkpoints[-1]["timerExamined"] if checkpoints else None,
        },
    }
    report["result"] = classify(report)
    (run_dir / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2), flush=True)
    return 0 if report["result"] == "PASS" else 2 if report["result"] == "INCOMPLETE" else 1


if __name__ == "__main__":
    raise SystemExit(main())
