#!/usr/bin/env python3
"""No-server tests for the mixed live-soak collector and private slow peer."""
from __future__ import annotations

import importlib.util
import datetime as dt
import contextlib
import io
import json
import pathlib
import re
import socket
import stat
import subprocess
import tempfile
import threading
import time
import unittest


HERE = pathlib.Path(__file__).resolve().parent


def load(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


mixed = load("mixed_soak", "mixed-soak.py")
peer = load("slow_peer", "slow-peer.py")


SOURCE = "0123456789abcdef0123456789abcdef01234567"
ACTUAL_GENERATED_CONFIG = (
    b'{\n  // int \xe2\x80\x94 Collector-owned generation marker for the mixed live soak.\n'
    b'  "generation": 0\n}\n'
)


def loader_stats(*, config_bytes: int = 24, transient: bool = False,
                 retained_leak: int = 0) -> dict:
    request_items = 128
    request_bytes = 8 * 1024 * 1024
    result_items = 128
    result_bytes = 64 * 1024 * 1024
    prepared_items = 32
    prepared_bytes = 64 * 1024 * 1024
    active = 1 if transient else 0
    obligations = 1 if transient else 0
    retained = active + retained_leak
    return {
        "running": True,
        "worker": {
            "obligations": {
                "items": obligations,
                "requestBytes": 64 if transient else 0,
                "resultBytes": 128 if transient else 0,
            },
            "queued": obligations,
            "inFlight": 0,
            "results": 0,
            "controls": {"items": 1, "bytes": 48, "pending": 0},
            "config": {"paths": 1, "bytes": config_bytes},
            "baselines": {"items": 1, "bytes": config_bytes},
            "proposals": {"items": 0, "bytes": 0},
        },
        "main": {
            "pending": {"items": 0, "bytes": 0},
            "active": active,
            "ready": 0,
            "waiting": retained_leak,
            "applying": 0,
            "retained": {"items": retained, "bytes": retained * 128},
        },
        "rejected": {
            key: 0 for key in (
                "requestItems", "requestBytes", "resultItems", "resultBytes",
                "controlItems", "controlBytes", "configPaths", "configBytes",
                "pendingItems", "pendingBytes", "retainedItems", "retainedBytes",
            )
        },
        "highWater": {
            "obligations": max(1, obligations),
            "requestBytes": max(64, 64 if transient else 0),
            "resultBytes": max(128, 128 if transient else 0),
            "queued": max(1, obligations),
            "inFlight": 1,
            "results": 1,
            "controlItems": 1,
            "controlBytes": 48,
            "configPaths": 1,
            "configBytes": max(24, config_bytes),
            "baselineItems": 1,
            "baselineBytes": max(24, config_bytes),
            "proposalItems": 1,
            "proposalBytes": max(24, config_bytes),
            "pendingItems": 1,
            "pendingBytes": 128,
            "retainedItems": max(1, retained),
            "retainedBytes": max(128, retained * 128),
        },
        "limits": {
            "requestItems": request_items,
            "requestBytes": request_bytes,
            "resultItems": result_items,
            "resultBytes": result_bytes,
            "preparedItems": prepared_items,
            "preparedBytes": prepared_bytes,
            "scanEntries": 4096,
            "scanCandidates": 1024,
            "pathBytes": 4096,
            "archiveBytes": 32 * 1024 * 1024,
            "configBytes": 1024 * 1024,
            "configBaselineItems": 256,
            "configBaselineBytes": 8 * 1024 * 1024,
            "parse": {
                "zipEntries": 256,
                "memberNameBytes": 4096,
                "manifestBytes": 1024 * 1024,
                "pluginJsBytes": 16 * 1024 * 1024,
                "gamedataBytes": 16 * 1024 * 1024,
            },
            "drainItems": 32,
            "drainBytes": 8 * 1024 * 1024,
            "drainMicros": 2000,
        },
    }


def stats_payload(*, jobs: int = 0, rejected: int = 0, max_ns: int = 90,
                  cache_entries: int = 2, loader: dict | None = None) -> str:
    resources = {
        name: {"items": jobs if name == "jobs" else 0, "bytes": 0,
               "rejected": rejected if name == "jobs" else 0}
        for name in ("jobs", "completion", "sockets", "sqlite", "pools", "inbound", "outbound", "timers")
    }
    stats = {
        **resources,
        "queued": {"worker": 0, "http": 0, "db": 0, "ws": 0, "net": 0},
        "staged": {"timers": 0, "ws": 0, "net": 0, "cookies": 0, "http": 0, "db": 0},
        "cache": {"accounts": 1, "entries": cache_entries, "bytes": 64},
        "frame": {"items": 0, "bytes": 0, "polls": 0},
        "timerExamined": 4,
        "lastNs": 20,
        "maxNs": max_ns,
        "loader": loader_stats() if loader is None else loader,
    }
    return json.dumps(stats, separators=(",", ":"))


def multipart_stats(payload: str, *, snapshot: int = 41, chunk_chars: int = 700) -> str:
    count = (len(payload) + chunk_chars - 1) // chunk_chars
    return "\n".join(
        f"[mixed-live] STATS_PART snapshot={snapshot} part={part + 1}/{count} "
        f"bytes={len(payload)} data={payload[part * chunk_chars:(part + 1) * chunk_chars]}"
        for part in range(count)
    ) + "\n"


def stats_line(*, jobs: int = 0, rejected: int = 0, max_ns: int = 90,
               cache_entries: int = 2, loader: dict | None = None) -> str:
    return multipart_stats(stats_payload(
        jobs=jobs, rejected=rejected, max_ns=max_ns, cache_entries=cache_entries, loader=loader,
    ))


class FakeClock:
    def __init__(self):
        self.t = 0.0

    def monotonic(self):
        return self.t

    def sleep(self, seconds):
        self.t += seconds


class FakeRun:
    def __init__(self, clock, source=SOURCE, expire_after_cycle=None, *,
                 malformed_loader=False, retained_leak=False,
                 restoration_failure=False, stats_timeout=False,
                 over_cap_loader=False, delayed_nonloader_cleanup=False,
                 high_water_drop=False, limit_change=False,
                 warmup_config_growth=False, late_stats_success=False,
                 late_restore_success=False, loader_transients=True,
                 final_clock_jump=None):
        self.clock = clock
        self.source = source
        self.expire_after_cycle = expire_after_cycle
        self.calls = []
        self.current_cycle = -1
        self.native_rejected = 0
        self.pending_active = 0
        self.log_serial = 0
        self.malformed_loader = malformed_loader
        self.retained_leak = retained_leak
        self.restoration_failure = restoration_failure
        self.stats_timeout = stats_timeout
        self.over_cap_loader = over_cap_loader
        self.delayed_nonloader_cleanup = delayed_nonloader_cleanup
        self.high_water_drop = high_water_drop
        self.limit_change = limit_change
        self.warmup_config_growth = warmup_config_growth
        self.late_stats_success = late_stats_success
        self.late_restore_success = late_restore_success
        self.loader_transients = loader_transients
        self.final_clock_jump = final_clock_jump
        self.nonloader_cleanup_seen = set()
        self.call_times = []
        self.original_generation = 0
        self.applied_generation = 0
        self.stats_calls = 0
        self.config_bytes_high_water = 0

    def __call__(self, argv, root, timeout):
        self.calls.append((list(argv), timeout))
        self.call_times.append((list(argv), timeout, self.clock.t))
        if argv[:3] == ["git", "rev-parse", "HEAD"]:
            return 0, self.source + "\n"
        if argv[0] == "uname":
            return 0, "FakeOS fixture\n"
        if argv[:2] == ["docker", "inspect"]:
            if "--format" in argv:
                return 0, '{}|{"s2script-cs2-hardening_default":{}}\n'
            return 1, "not found\n"
        if argv[:2] == ["docker", "run"]:
            return 0, "fake-peer-id\n"
        if argv[:2] == ["docker", "stop"]:
            return 0, "s2script-mixed-slow-peer\n"
        if argv[:2] == ["docker", "top"]:
            return 0, "PID PPID COMM RSS ARGS\n1 0 cs2 100 cs2\n"
        if argv[:2] == ["docker", "logs"]:
            if argv[-1] == "s2script-mixed-slow-peer":
                return 0, "[mixed-slow-peer] ready\n"
            lines = []
            while self.pending_active:
                self.log_serial += 1
                lines.append(
                    f"2026-01-01T00:00:{self.log_serial:02d}.000000000Z "
                    "[plugins] '@s2script/clientprefs' Active"
                )
                self.pending_active -= 1
            return 0, "\n".join(lines) + ("\n" if lines else "")
        if argv[0] != "python3":
            return 0, ""
        commands = argv[4:]
        config_path = pathlib.Path(root) / "dist/addons/s2script/configs/_fixture_mixed-live.json"
        if config_path.exists():
            generation_match = re.search(rb'"generation"\s*:\s*([0-9]+)', config_path.read_bytes())
            assert generation_match is not None
            config_generation = int(generation_match.group(1))
            if not (self.restoration_failure and self.current_cycle >= 0 and
                    config_generation == self.original_generation):
                self.applied_generation = config_generation
        if any(command.startswith("sm plugins reload ") for command in commands):
            self.pending_active += 1
            return 0, "[SM] Reloading '@s2script/clientprefs'\n"
        for command in commands:
            if command.startswith("sm_mixed_start "):
                self.current_cycle = int(command.split()[-1])
        if commands == ["sm_mixed_slotreset"]:
            return 0, "[mixed-live] SLOT_RESET pending=0 checking=0\n"
        if commands == ["sm_mixed_slotcleanup"]:
            return 0, "[mixed-live] SLOT_CLEANUP pending=0 checking=0\n"
        if commands == ["sm_mixed_pressure 256"]:
            self.native_rejected = 192
            return 0, "[mixed-live] PRESSURE accepted expected=256\n"
        if commands == ["sm_mixed_pressure_status"]:
            return 0, "[mixed-live] PRESSURE state=done expected=256 settled=256 success=64 rejected=192 other=0\n"
        if commands == ["sm_mixed_stats"]:
            self.stats_calls += 1
            if self.stats_timeout:
                self.clock.t += timeout
                return 124, "TIMEOUT\n"
            content = config_path.read_bytes()
            if self.late_stats_success and self.current_cycle >= 1:
                self.clock.t += timeout + 0.01
            reported_config_bytes = len(content)
            if self.warmup_config_growth and self.current_cycle == 0:
                reported_config_bytes += 5
            loader = loader_stats(
                config_bytes=reported_config_bytes,
                transient=self.loader_transients and self.stats_calls % 3 == 1,
                retained_leak=1 if self.retained_leak and self.current_cycle >= 1 else 0,
            )
            self.config_bytes_high_water = max(self.config_bytes_high_water, reported_config_bytes)
            for key in ("configBytes", "baselineBytes", "proposalBytes"):
                loader["highWater"][key] = max(loader["highWater"][key], self.config_bytes_high_water)
            if self.malformed_loader:
                del loader["highWater"]["retainedBytes"]
            if self.over_cap_loader:
                loader["highWater"]["retainedBytes"] = loader["limits"]["preparedBytes"] + 1
            if self.high_water_drop and self.current_cycle >= 1:
                loader["highWater"]["queued"] = 0
            if self.limit_change and self.current_cycle >= 1:
                loader["limits"]["requestItems"] += 1
            jobs = 0
            if (self.delayed_nonloader_cleanup and self.current_cycle >= 0 and
                    self.current_cycle not in self.nonloader_cleanup_seen):
                self.nonloader_cleanup_seen.add(self.current_cycle)
                jobs = 1
            return 0, stats_line(jobs=jobs, rejected=self.native_rejected, loader=loader)
        if commands == ["sm_mixed_status"]:
            line = self.status_line()
            if (self.late_restore_success and self.current_cycle >= 0 and
                    len(config_path.read_bytes()) != mixed.CONFIG_CAPACITY):
                self.clock.t += timeout + 0.01
            if (self.final_clock_jump is not None and self.current_cycle >= 0 and
                    len(config_path.read_bytes()) != mixed.CONFIG_CAPACITY):
                self.clock.t = self.final_clock_jump
                self.final_clock_jump = None
            if self.expire_after_cycle == self.current_cycle:
                self.clock.t += 20
                self.expire_after_cycle = None
            return 0, line
        if "sm plugins list" in commands:
            return 0, '"@fixture/mixed-live" (running)\n' + self.status_line()
        return 0, "ok\n"

    def status_line(self):
        return (
            f"[mixed-live] STATUS cycle={self.current_cycle} state=done pass=true timers=12/12 "
            "http=2/2 tcp=2/2 hooks=16/16 db=pass "
            f"config={self.applied_generation} slotAttempts=1 slotReuses=1 slotFailures=0 slotPending=0\n"
        )


def main_args(root, out, source=SOURCE):
    return [
        "--root", str(root), "--out", str(out), "--source", source,
        "--duration", "10", "--warmup", "3", "--cycle-period", "3",
        "--cycle-timeout", "1.5", "--quiescence", ".2", "--call-timeout", "15",
        "--expected-plugins", "1", "--no-manage-peer",
    ]


def managed_main_args(root, out, source=SOURCE):
    args = main_args(root, out, source)
    args.remove("--no-manage-peer")
    return args


def materialize_config(root):
    path = root / "dist/addons/s2script/configs/_fixture_mixed-live.json"
    path.parent.mkdir(parents=True)
    path.write_text('{\n  "generation": 0\n}\n', encoding="utf-8")
    return path


class StatsTests(unittest.TestCase):
    def test_parse_stats_round_trips_long_complete_multipart_schema(self):
        payload = stats_payload() + " " * 512
        self.assertGreater(len(payload), 2048)
        parsed = mixed.parse_stats(multipart_stats(payload))
        self.assertEqual(parsed["loader"]["limits"]["resultBytes"], 64 * 1024 * 1024)

    def test_parse_stats_rejects_missing_truncated_duplicate_and_interleaved_parts(self):
        payload = stats_payload()
        parts = multipart_stats(payload, chunk_chars=700).splitlines()
        with self.assertRaisesRegex(ValueError, "missing|part count"):
            mixed.parse_stats("\n".join(parts[:-1]))
        truncated = parts.copy()
        truncated[-1] = truncated[-1][:-10]
        with self.assertRaisesRegex(ValueError, "length"):
            mixed.parse_stats("\n".join(truncated))
        with self.assertRaisesRegex(ValueError, "duplicate"):
            mixed.parse_stats("\n".join([parts[0], parts[0], *parts[1:]]))
        other = multipart_stats(payload, snapshot=42, chunk_chars=700).splitlines()
        with self.assertRaisesRegex(ValueError, "mixed"):
            mixed.parse_stats("\n".join([parts[0], other[1], *parts[2:]]))

    def test_parse_stats_rejects_inconsistent_counts_and_oversized_data(self):
        payload = stats_payload()
        parts = multipart_stats(payload, chunk_chars=700).splitlines()
        inconsistent = parts.copy()
        inconsistent[1] = inconsistent[1].replace(f"/{len(parts)} ", f"/{len(parts) + 1} ", 1)
        with self.assertRaisesRegex(ValueError, "inconsistent"):
            mixed.parse_stats("\n".join(inconsistent))
        oversized = parts.copy()
        oversized[0] = oversized[0].replace(f"bytes={len(payload)} ", "bytes=65537 ", 1)
        with self.assertRaisesRegex(ValueError, "exceeds"):
            mixed.parse_stats("\n".join(oversized))
        with self.assertRaisesRegex(ValueError, "part data bytes.*exceeds"):
            mixed.parse_stats(
                "[mixed-live] STATS_PART snapshot=1 part=1/1 bytes=1025 data=" + "x" * 1025
            )

    def test_parse_stats_requires_the_complete_current_native_schema(self):
        parsed = mixed.parse_stats(stats_line())
        self.assertEqual(parsed["cache"]["entries"], 2)
        broken = json.loads(stats_payload())
        del broken["staged"]["db"]
        with self.assertRaisesRegex(ValueError, "staged.db"):
            mixed.parse_stats(multipart_stats(json.dumps(broken)))

        broken = json.loads(stats_payload())
        del broken["loader"]
        with self.assertRaisesRegex(ValueError, "loader"):
            mixed.parse_stats(multipart_stats(json.dumps(broken)))

    def test_parse_stats_requires_loader_types_caps_and_occupancy_semantics(self):
        raw = json.loads(stats_payload())
        raw["loader"]["worker"]["obligations"]["items"] = True
        with self.assertRaisesRegex(ValueError, "loader.worker.obligations.items"):
            mixed.parse_stats(multipart_stats(json.dumps(raw)))

        raw = json.loads(stats_payload())
        raw["loader"]["highWater"]["retainedBytes"] = raw["loader"]["limits"]["preparedBytes"] + 1
        with self.assertRaisesRegex(ValueError, "highWater.retainedBytes.*preparedBytes"):
            mixed.parse_stats(multipart_stats(json.dumps(raw)))

        raw = json.loads(stats_payload())
        raw["loader"]["worker"]["queued"] = 2
        raw["loader"]["worker"]["obligations"]["items"] = 1
        with self.assertRaisesRegex(ValueError, "queued.*obligations.items"):
            mixed.parse_stats(multipart_stats(json.dumps(raw)))

    def test_loader_idle_and_plateau_distinguish_transients_from_retained_leaks(self):
        baseline = mixed.parse_stats(stats_line(loader=loader_stats(config_bytes=17)))
        transient = mixed.parse_stats(stats_line(loader=loader_stats(config_bytes=17, transient=True)))
        self.assertTrue(mixed.loader_idle_errors(transient))
        self.assertEqual(mixed.compare_loader_plateau(baseline, transient), [])

        leaked = mixed.parse_stats(stats_line(loader=loader_stats(config_bytes=17, retained_leak=1)))
        self.assertTrue(any("main.retained.items" in item for item in mixed.loader_idle_errors(leaked)))

        changed = mixed.parse_stats(stats_line(loader=loader_stats(config_bytes=18)))
        self.assertIn(
            "loader.worker.config.bytes: baseline=17 current=18",
            mixed.compare_loader_plateau(baseline, changed),
        )

    def test_parse_stats_accepts_null_frame_before_instrumented_frame_exists(self):
        raw = json.loads(stats_payload())
        raw["frame"] = None
        self.assertIsNone(mixed.parse_stats(multipart_stats(json.dumps(raw)))["frame"])

    def test_quiescence_detects_resource_queue_stage_and_cache_growth(self):
        baseline = mixed.parse_stats(stats_line())
        current = mixed.parse_stats(stats_line(jobs=1, cache_entries=3))
        current["queued"]["http"] = 1
        current["staged"]["timers"] = 1
        errors = mixed.compare_quiescent(baseline, current)
        self.assertIn("jobs.items: baseline=0 current=1", errors)
        self.assertIn("queued.http: baseline=0 current=1", errors)
        self.assertIn("staged.timers: baseline=0 current=1", errors)
        self.assertIn("cache.entries: baseline=2 current=3", errors)

    def test_quiescence_allows_cumulative_rejections_and_timing_high_water(self):
        baseline = mixed.parse_stats(stats_line(max_ns=90))
        current = mixed.parse_stats(stats_line(max_ns=120))
        current["jobs"]["rejected"] = 7
        self.assertEqual(mixed.compare_quiescent(baseline, current), [])
        self.assertEqual(mixed.compare_rejections(baseline, current), [
            "jobs.rejected: baseline=0 current=7",
        ])

    def test_measurement_summary_uses_literal_nearest_rank_percentiles(self):
        self.assertEqual(mixed.measurement_summary([1, 2, 3, 4, 100]), {
            "count": 5, "min": 1, "p50": 3, "p95": 100, "p99": 100, "max": 100,
        })

    def test_rss_parser_sums_each_container_process_in_kib(self):
        output = "PID PPID COMM RSS ARGS\n1 0 cs2 100 cs2\n2 1 worker 35 worker --x\n"
        self.assertEqual(mixed.parse_rss_kib(output), 135)


class ProtocolTests(unittest.TestCase):
    def test_pressure_parser_requires_every_operation_to_settle_and_named_rejections(self):
        good = "[mixed-live] PRESSURE state=done expected=256 settled=256 success=64 rejected=192 other=0"
        self.assertEqual(mixed.parse_pressure_status(good, 256)["rejected"], 192)
        with self.assertRaisesRegex(ValueError, "settled"):
            mixed.parse_pressure_status(good.replace("settled=256", "settled=255"), 256)
        with self.assertRaisesRegex(ValueError, "rejection"):
            mixed.parse_pressure_status(
                "[mixed-live] PRESSURE state=done expected=256 settled=256 success=256 rejected=0 other=0",
                256,
            )
        with self.assertRaisesRegex(ValueError, "other"):
            mixed.parse_pressure_status(good.replace("success=64", "success=63").replace("other=0", "other=1"), 256)

    def test_cycle_status_parser_rejects_wrong_cycle_or_failed_component(self):
        good = (
            '[mixed-live] STATUS cycle=7 state=done pass=true timers=12/12 http=2/2 '
            'tcp=2/2 hooks=16/16 db=pass config=7 slotAttempts=3 slotReuses=1 slotFailures=0 slotPending=0'
        )
        self.assertEqual(mixed.parse_cycle_status(good, 7)["slotReuses"], 1)
        with self.assertRaisesRegex(ValueError, "cycle"):
            mixed.parse_cycle_status(good, 8)
        with self.assertRaisesRegex(ValueError, "pass=true"):
            mixed.parse_cycle_status(good.replace("db=pass", "db=fail").replace("pass=true", "pass=false"), 7)
        with self.assertRaisesRegex(ValueError, "hooks"):
            mixed.parse_cycle_status(good.replace("hooks=16/16", "hooks=15/16"), 7)
        self.assertEqual(mixed.parse_cycle_status(good.replace("slotPending=0", "slotPending=64"), 7)["slotPending"], 64)
        with self.assertRaisesRegex(ValueError, "pending"):
            mixed.parse_cycle_status(good.replace("slotPending=0", "slotPending=65"), 7)
        with self.assertRaisesRegex(ValueError, "timers"):
            mixed.parse_cycle_status(good.replace("timers=12/12", "timers=0/0"), 7)
        with self.assertRaisesRegex(ValueError, "non-negative"):
            mixed.parse_cycle_status(good.replace("slotAttempts=3", "slotAttempts=-1"), 7)

    def test_final_classification_requires_exact_ack_active_and_completed_cycles(self):
        report = {
            "complete": True,
            "errors": [],
            "cycles_requested": 4,
            "cycles_completed": 4,
            "reload_commands": 4,
            "reload_acks": 4,
            "active_transitions": 4,
            "slot_reuses": 1,
            "slot_cleanup_complete": True,
            "original_config_applied_at_start": True,
            "config_restore_verified": True,
            "loader_final_idle": True,
            "final_running_plugins": 17,
            "expected_running_plugins": 17,
        }
        self.assertEqual(mixed.classify(report), "PASS")
        for key in ("reload_acks", "active_transitions", "cycles_completed"):
            bad = dict(report)
            bad[key] -= 1
            self.assertEqual(mixed.classify(bad), "FAIL", key)
        self.assertEqual(mixed.classify(dict(report, slot_reuses=0)), "FAIL")
        self.assertEqual(mixed.classify(dict(report, slot_cleanup_complete=False)), "FAIL")
        incomplete = dict(report, complete=False)
        self.assertEqual(mixed.classify(incomplete), "INCOMPLETE")


    def test_bundled_plugin_retains_bounded_stale_clients_until_real_slot_wrap(self):
        result = subprocess.run(["node", str(HERE / "test-slot-protocol.cjs")],
                                cwd=HERE, capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("actual stale probe after 16 different slots", result.stdout)


class ProcessTests(unittest.TestCase):
    def test_parse_config_generation_accepts_actual_generated_jsonc(self):
        self.assertEqual(mixed.parse_config_generation(ACTUAL_GENERATED_CONFIG), 0)

    def test_parse_config_generation_preserves_comment_tokens_inside_strings(self):
        content = (
            b'{ /* outside */ "url": "https://example.invalid/a//b", '
            b'"literal": "/* value */", "generation": 7 // tail\n}'
        )
        self.assertEqual(mixed.parse_config_generation(content), 7)

    def test_parse_config_generation_rejects_malformed_jsonc(self):
        with self.assertRaisesRegex(ValueError, "not valid UTF-8 JSONC"):
            mixed.parse_config_generation(b'{"generation": 1,,}')
        with self.assertRaisesRegex(ValueError, "not valid UTF-8 JSONC"):
            mixed.parse_config_generation(b'{"generation": 1}\xff')

    def test_parse_config_generation_rejects_missing_or_noninteger_generation(self):
        for content in (b'{}', b'{"generation": "0"}', b'{"generation": true}'):
            with self.subTest(content=content):
                with self.assertRaisesRegex(ValueError, "generation"):
                    mixed.parse_config_generation(content)

    def test_executable_main_restores_actual_generated_jsonc_byte_exactly(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            out = root / "evidence"
            config_path = materialize_config(root)
            config_path.write_bytes(ACTUAL_GENERATED_CONFIG)
            clock = FakeClock()
            runner = FakeRun(clock)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(main_args(root, out), run_fn=runner, clock=clock, utc_fn=now)
            report = json.loads((next(out.glob("mixed-soak-*")) / "report.json").read_text())
            self.assertEqual(rc, 0)
            self.assertEqual(report["result"], "PASS")
            self.assertEqual(report["original_config_generation"], 0)
            self.assertEqual(config_path.read_bytes(), ACTUAL_GENERATED_CONFIG)

    def test_executable_main_passes_and_retains_every_rcon_artifact(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            out = root / "evidence"
            config_path = materialize_config(root)
            original = config_path.read_bytes()
            clock = FakeClock()
            runner = FakeRun(clock)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(main_args(root, out), run_fn=runner, clock=clock, utc_fn=now)
            run_dir = next(out.glob("mixed-soak-*"))
            report = json.loads((run_dir / "report.json").read_text())
            outputs = sorted(run_dir.glob("output-*.txt"))
            rcon_calls = [call for call, _timeout in runner.calls if call[:2] == ["python3", "scripts/rcon.py"]]
            self.assertEqual(rc, 0)
            self.assertEqual(report["result"], "PASS")
            self.assertTrue(report["slot_cleanup_complete"])
            slot_calls = [call[4:] for call, _ in runner.calls if call[:2] == ["python3", "scripts/rcon.py"]]
            self.assertLess(slot_calls.index(["sm_mixed_slotreset"]),
                            next(i for i, commands in enumerate(slot_calls) if "sm_mixed_start 0" in commands))
            self.assertIn(["sm_mixed_slotcleanup"], slot_calls)
            self.assertEqual(report["expected_running_plugins"], 1)
            self.assertEqual(len(outputs), len(runner.calls))
            self.assertEqual(len(outputs), len(set(path.name for path in outputs)))
            self.assertEqual(len(list(run_dir.glob("output-*-rcon.txt"))), len(rcon_calls))
            self.assertEqual(config_path.read_bytes(), original)
            self.assertTrue(report["config_restore_verified"])
            self.assertTrue(report["original_config_applied_at_start"])
            self.assertEqual(report["original_config_generation"], 0)
            self.assertNotEqual(report["loader_original_plateau"]["worker"]["config"]["bytes"],
                                report["loader_measured_plateau"]["worker"]["config"]["bytes"])
            self.assertGreater(report["loader_idle_samples"], 1)
            status_timeouts = [timeout for call, timeout in runner.calls if call[4:] == ["sm_mixed_status"]]
            self.assertTrue(status_timeouts)
            self.assertTrue(all(0 < timeout <= 0.500001 for timeout in status_timeouts))

    def test_executable_main_rejects_broken_loader_metrics(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            materialize_config(root)
            clock = FakeClock()
            runner = FakeRun(clock, malformed_loader=True)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(main_args(root, root / "evidence"), run_fn=runner, clock=clock, utc_fn=now)
            report = json.loads(next((root / "evidence").glob("mixed-soak-*/report.json")).read_text())
            self.assertNotEqual(rc, 0)
            self.assertTrue(any("loader.highWater.retainedBytes" in item for item in report["errors"]))

    def test_executable_main_rejects_loader_high_water_above_effective_cap(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            materialize_config(root)
            clock = FakeClock()
            runner = FakeRun(clock, over_cap_loader=True)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(main_args(root, root / "evidence"), run_fn=runner, clock=clock, utc_fn=now)
            report = json.loads(next((root / "evidence").glob("mixed-soak-*/report.json")).read_text())
            self.assertNotEqual(rc, 0)
            self.assertTrue(any("retainedBytes" in item and "preparedBytes" in item
                                for item in report["errors"]))

    def test_executable_main_detects_retained_loader_leak_after_bounded_sampling(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            materialize_config(root)
            clock = FakeClock()
            runner = FakeRun(clock, retained_leak=True)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(main_args(root, root / "evidence"), run_fn=runner, clock=clock, utc_fn=now)
            report = json.loads(next((root / "evidence").glob("mixed-soak-*/report.json")).read_text())
            self.assertNotEqual(rc, 0)
            self.assertTrue(any("loader idle timeout" in item and "main.retained.items" in item
                                for item in report["errors"]))

    def test_executable_main_waits_for_delayed_nonloader_cleanup_after_loader_is_idle(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            materialize_config(root)
            clock = FakeClock()
            runner = FakeRun(
                clock,
                delayed_nonloader_cleanup=True,
                loader_transients=False,
            )
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(main_args(root, root / "evidence"), run_fn=runner, clock=clock, utc_fn=now)
            run_dir = next((root / "evidence").glob("mixed-soak-*"))
            delayed = [
                json.loads(path.read_text())["jobs"]["items"]
                for path in run_dir.glob("stats-*.json")
            ]
            self.assertEqual(rc, 0)
            self.assertIn(1, delayed)

    def test_executable_main_rejects_loader_high_water_drop_between_valid_samples(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            materialize_config(root)
            clock = FakeClock()
            runner = FakeRun(clock, high_water_drop=True, loader_transients=False)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(main_args(root, root / "evidence"), run_fn=runner, clock=clock, utc_fn=now)
            report = json.loads(next((root / "evidence").glob("mixed-soak-*/report.json")).read_text())
            self.assertNotEqual(rc, 0)
            self.assertTrue(any("loader.highWater.queued decreased" in item for item in report["errors"]))

    def test_executable_main_rejects_loader_limit_change_between_samples(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            materialize_config(root)
            clock = FakeClock()
            runner = FakeRun(clock, limit_change=True, loader_transients=False)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(main_args(root, root / "evidence"), run_fn=runner, clock=clock, utc_fn=now)
            report = json.loads(next((root / "evidence").glob("mixed-soak-*/report.json")).read_text())
            self.assertNotEqual(rc, 0)
            self.assertTrue(any("loader.limits changed" in item for item in report["errors"]))

    def test_executable_main_rejects_successful_stats_sample_returned_after_phase_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            materialize_config(root)
            clock = FakeClock()
            runner = FakeRun(clock, late_stats_success=True, loader_transients=False)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(main_args(root, root / "evidence"), run_fn=runner, clock=clock, utc_fn=now)
            report = json.loads(next((root / "evidence").glob("mixed-soak-*/report.json")).read_text())
            self.assertNotEqual(rc, 0)
            self.assertTrue(any("completed after deadline" in item for item in report["errors"]))

    def test_executable_main_rejects_config_application_returned_after_cleanup_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            materialize_config(root)
            clock = FakeClock()
            runner = FakeRun(clock, late_restore_success=True, loader_transients=False)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(main_args(root, root / "evidence"), run_fn=runner, clock=clock, utc_fn=now)
            report = json.loads(next((root / "evidence").glob("mixed-soak-*/report.json")).read_text())
            self.assertNotEqual(rc, 0)
            self.assertFalse(report["config_restore_verified"])
            self.assertTrue(any("completed after deadline" in item for item in report["errors"]))

    def test_executable_main_rejects_unexpected_warmup_loader_byte_growth(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            materialize_config(root)
            clock = FakeClock()
            runner = FakeRun(clock, warmup_config_growth=True, loader_transients=False)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(main_args(root, root / "evidence"), run_fn=runner, clock=clock, utc_fn=now)
            report = json.loads(next((root / "evidence").glob("mixed-soak-*/report.json")).read_text())
            self.assertNotEqual(rc, 0)
            self.assertTrue(any("warm-up loader transition" in item and "expected" in item
                                for item in report["errors"]))

    def test_owned_peer_gets_reserved_cleanup_window_before_hard_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            materialize_config(root)
            clock = FakeClock()
            runner = FakeRun(clock, final_clock_jump=90.0, loader_transients=False)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(managed_main_args(root, root / "evidence"),
                                run_fn=runner, clock=clock, utc_fn=now)
            report = json.loads(next((root / "evidence").glob("mixed-soak-*/report.json")).read_text())
            stop_calls = [row for row in runner.call_times if row[0][:2] == ["docker", "stop"]]
            ordinary_after_reserve = [
                row for row in runner.call_times
                if row[2] >= 90.0 and row[0][:2] != ["docker", "stop"]
            ]
            self.assertNotEqual(rc, 0)
            self.assertEqual(len(stop_calls), 1)
            self.assertEqual(ordinary_after_reserve, [])
            self.assertLessEqual(stop_calls[0][1], 10)
            self.assertTrue(report["peer_stop_succeeded"])
            self.assertEqual(report["peer_cleanup_overrun_seconds"], 0)

    def test_owned_peer_still_gets_one_bounded_stop_after_exceptional_hard_deadline_overrun(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            materialize_config(root)
            clock = FakeClock()
            runner = FakeRun(clock, final_clock_jump=101.0, loader_transients=False)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(managed_main_args(root, root / "evidence"),
                                run_fn=runner, clock=clock, utc_fn=now)
            report = json.loads(next((root / "evidence").glob("mixed-soak-*/report.json")).read_text())
            stop_calls = [row for row in runner.call_times if row[0][:2] == ["docker", "stop"]]
            self.assertEqual(rc, 2)
            self.assertEqual(len(stop_calls), 1)
            self.assertEqual(stop_calls[0][1], 10)
            self.assertTrue(report["peer_stop_succeeded"])
            self.assertGreaterEqual(report["peer_cleanup_overrun_seconds"], 1)
            self.assertTrue(any("cleanup hard deadline overrun" in item for item in report["errors"]))

    def test_executable_main_caps_stats_timeout_to_quiescence_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            materialize_config(root)
            clock = FakeClock()
            runner = FakeRun(clock, stats_timeout=True)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(main_args(root, root / "evidence"), run_fn=runner, clock=clock, utc_fn=now)
            stats_timeouts = [timeout for call, timeout in runner.calls if call[4:] == ["sm_mixed_stats"]]
            self.assertNotEqual(rc, 0)
            self.assertTrue(stats_timeouts)
            self.assertTrue(all(0 < timeout <= 0.200001 for timeout in stats_timeouts))

    def test_executable_main_restores_exact_original_when_runtime_application_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            config_path = materialize_config(root)
            original = config_path.read_bytes()
            clock = FakeClock()
            runner = FakeRun(clock, restoration_failure=True)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(main_args(root, root / "evidence"), run_fn=runner, clock=clock, utc_fn=now)
            report = json.loads(next((root / "evidence").glob("mixed-soak-*/report.json")).read_text())
            self.assertNotEqual(rc, 0)
            self.assertFalse(report["config_restore_verified"])
            self.assertEqual(config_path.read_bytes(), original)
            self.assertTrue(any("runtime application" in item for item in report["errors"]))

    def test_executable_main_refuses_source_mismatch_before_rcon(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            materialize_config(root)
            clock = FakeClock()
            runner = FakeRun(clock, source="f" * 40)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(main_args(root, root / "evidence"), run_fn=runner, clock=clock, utc_fn=now)
            self.assertEqual(rc, 2)
            self.assertFalse(any(call[:2] == ["python3", "scripts/rcon.py"] for call, _ in runner.calls))

    def test_executable_main_never_starts_a_cycle_after_soak_deadline(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            materialize_config(root)
            clock = FakeClock()
            runner = FakeRun(clock, expire_after_cycle=1)
            now = lambda: dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc) + dt.timedelta(seconds=clock.t)
            with contextlib.redirect_stdout(io.StringIO()):
                rc = mixed.main(main_args(root, root / "evidence"), run_fn=runner, clock=clock, utc_fn=now)
            starts = [arg for call, _ in runner.calls for arg in call if arg.startswith("sm_mixed_start ")]
            self.assertNotEqual(rc, 0)
            self.assertEqual(starts, ["sm_mixed_start 0", "sm_mixed_start 1"])

    def test_subprocess_timeout_retains_partial_output(self):
        code, output = mixed.run(
            ["python3", "-c", "import sys,time;sys.stdout.write('fragment');sys.stdout.flush();time.sleep(2)"],
            HERE,
            0.15,
        )
        self.assertEqual(code, 124)
        self.assertIn("fragment", output)

    def test_atomic_config_edit_preserves_existing_mode(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "fixture.json"
            path.write_text('{"generation":0}\n', encoding="utf-8")
            path.chmod(0o640)
            mixed.atomic_write(path, '{"generation":1}\n')
            self.assertEqual(path.read_text(encoding="utf-8"), '{"generation":1}\n')
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o640)

    def test_generation_config_has_fixed_capacity_and_rejects_overflow(self):
        encoded = [mixed.encode_generation_config(generation) for generation in (9, 10, 57)]
        self.assertEqual({len(content) for content in encoded}, {mixed.CONFIG_CAPACITY})
        self.assertEqual(mixed.CONFIG_CAPACITY, 17)
        self.assertEqual([json.loads(content)["generation"] for content in encoded], [9, 10, 57])
        self.assertTrue(encoded[0].endswith(" "))
        with self.assertRaisesRegex(ValueError, "capacity"):
            mixed.encode_generation_config(100)


class SlowPeerTests(unittest.TestCase):
    def test_http_endpoint_streams_exact_bounded_body(self):
        server = peer.make_http_server("127.0.0.1", 0, max_bytes=4096, max_delay_ms=20)
        thread = threading.Thread(target=server.handle_request, daemon=True)
        thread.start()
        host, port = server.server_address
        with socket.create_connection((host, port), timeout=1) as conn:
            conn.sendall(b"GET /slow?bytes=23&chunks=4&delay_ms=1 HTTP/1.0\r\nHost: fixture\r\n\r\n")
            data = bytearray()
            while True:
                chunk = conn.recv(4096)
                if not chunk:
                    break
                data.extend(chunk)
        thread.join(1)
        server.server_close()
        header, body = bytes(data).split(b"\r\n\r\n", 1)
        self.assertIn(b"200 OK", header)
        self.assertEqual(body, peer.body_bytes(23))

    def test_tcp_endpoint_reads_slowly_then_returns_exact_ack(self):
        server = peer.make_tcp_server("127.0.0.1", 0, max_bytes=4096, read_delay_ms=1)
        thread = threading.Thread(target=server.handle_request, daemon=True)
        thread.start()
        host, port = server.server_address
        payload = b"cycle=9:" + b"x" * 30
        with socket.create_connection((host, port), timeout=1) as conn:
            conn.sendall(len(payload).to_bytes(4, "big") + payload)
            ack = conn.recv(128)
        thread.join(1)
        server.server_close()
        self.assertEqual(ack, b"ACK 38\n")

    def test_peer_refuses_oversized_requests(self):
        with self.assertRaisesRegex(ValueError, "exceeds"):
            peer.checked_size(4097, 4096)


if __name__ == "__main__":
    unittest.main(verbosity=2)
