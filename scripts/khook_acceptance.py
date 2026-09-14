#!/usr/bin/env python3
"""One KHook suite A acceptance controller, registry, and judge.

This module is the frozen R5/R6 contract. It does not implement probe or JS
fixture bodies. JSON examples here are the executable fixtures imported by
``scripts/test-khook-acceptance.py`` (``EXAMPLE_RECORD`` /
``EXAMPLE_HUMAN_OBSERVATIONS``). They use ``result=pending``. They are not live
passes.

Command syntax (shell entry point)
---------------------------------
::

    bash scripts/test-khook-live.sh --self-test
    bash scripts/test-khook-live.sh A --from-file FILE
    bash scripts/test-khook-live.sh A --prepare --run-dir build/khook-acceptance/run-001
    bash scripts/test-khook-live.sh A --collect --run-dir build/khook-acceptance/run-001
    bash scripts/test-khook-live.sh A --judge --run-dir build/khook-acceptance/run-001 \\
      --observations build/khook-acceptance/run-001/human.json

``run-001`` is a directory, not the identity. Prepare writes a unique ``run_id``
plus build/revision/host/server identity into that directory. Collect advances
documented actions and upserts observations without resetting the run or
re-sending prepare. Judge is read-only. Suites B and C are not authored.

Native/JS command protocol (R5/R6 implement the bodies later)
----------------------------------------------------------------
::

    s2_khook_probe prepare <run_id>
    s2_khook_probe collect <run_id>
    s2_khook_probe report <run_id>
    s2_khook_accept prepare <run_id>
    s2_khook_accept collect <run_id>
    s2_khook_accept report <run_id>
    s2_khook_accept teardown <run_id>

Unknown or mismatched ``run_id`` values are invalid evidence. Never silently
reuse a prior run. ``report`` is read-only.

Record schema (schema=1)
-------------------------
One JSON object per record. Typed ``expected`` / ``actual``, nonempty
``evidence`` for pass/fail, one terminal ``result``. Example (pending)::

    {
      "schema": 1,
      "suite": "A",
      "run_id": "khook-a-test-0001",
      "source_revision": "d66d7bd45721b3be2a44da358f33b6cef1b5d594",
      "case": "new_capsule_registration",
      "subcheck": "native_null_configure_failed",
      "producer": "native",
      "result": "pending",
      "expected": {"state": "Failed", "id": "INVALID_HOOK"},
      "actual": {},
      "evidence": ""
    }

Human observations file (pending example; never prefilled pass data)
----------------------------------------------------------------------
See ``EXAMPLE_HUMAN_OBSERVATIONS``. Human rows supplement required native/JS
subchecks and cannot override a native failure.

Exit codes: 0 complete pass / 1 fail or invalid / 2 pending (incomplete).
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Dict, Iterable, List, Optional, Sequence, Tuple

REPO = Path(__file__).resolve().parent.parent
SCHEMA = 1
PRODUCERS = ("native", "js", "human")
RESULTS = ("pass", "fail", "pending")
COLLECT_DEADLINE_S = 30.0
COLLECT_INTERVAL_S = 0.5
COLLECT_MAX_ATTEMPTS = 8

SUITE_A_CASES = (
    "new_capsule_registration",
    "shared_capsule_registration",
    "peer_actions_both_orders",
    "one_normal_invocation",
    "frame_client_command_hooks",
    "fire_event_no_suppression",
    "fire_event_handled_recipient_mask",
    "voice_recall",
    "sdkhooks_one_of_two_entities",
    "sdkhooks_phase_removal",
    "entity_slot_reuse_map_teardown",
    "check_transmit",
)

UNAVAILABLE_SUITES = {
    "B": "not authored",
    "C": "not authored",
}

EXAMPLE_IDENTITY = {
    "run_id": "khook-a-test-0001",
    "source_revision": "d66d7bd45721b3be2a44da358f33b6cef1b5d594",
}


@dataclass(frozen=True)
class Subcheck:
    """Named producer-owned subcheck. R5/R6 must emit this exact triple."""

    case: str
    subcheck: str
    producer: str


def _sc(case: str, subcheck: str, producer: str) -> Subcheck:
    if producer not in PRODUCERS:
        raise ValueError(producer)
    return Subcheck(case, subcheck, producer)


# Producer ownership is the F2/F4/F5 contract: a single probe counter cannot
# satisfy a case that lists both native and js (and human, where listed).
SUBCHECKS: Tuple[Subcheck, ...] = (
    _sc("new_capsule_registration", "native_null_configure_failed", "native"),
    _sc("new_capsule_registration", "native_first_valid_invoke_activates", "native"),
    _sc("shared_capsule_registration", "native_two_consumers_active", "native"),
    _sc("shared_capsule_registration", "native_gameframe_shared", "native"),
    _sc("peer_actions_both_orders", "native_peer_actions_both_orders", "native"),
    _sc("one_normal_invocation", "native_one_pre_post_orig", "native"),
    # frame_client_command_hooks: native engine observation AND JS delivery,
    # joined by client identity/generation; Continue vs Handled tokens;
    # negative controls are required, not optional.
    _sc("frame_client_command_hooks", "native_gameframe_observed", "native"),
    _sc("frame_client_command_hooks", "js_gameframe_delivery", "js"),
    _sc("frame_client_command_hooks", "native_client_connected", "native"),
    _sc("frame_client_command_hooks", "js_client_connected", "js"),
    _sc("frame_client_command_hooks", "native_client_identity_join", "native"),
    _sc("frame_client_command_hooks", "js_client_identity_join", "js"),
    _sc("frame_client_command_hooks", "native_command_continue_original", "native"),
    _sc("frame_client_command_hooks", "js_command_continue_delivery", "js"),
    _sc("frame_client_command_hooks", "native_command_handled_skipped", "native"),
    _sc("frame_client_command_hooks", "js_command_handled_delivery", "js"),
    _sc("frame_client_command_hooks", "native_control_missing_js_hook", "native"),
    _sc("frame_client_command_hooks", "native_control_flipped_decision", "native"),
    _sc("frame_client_command_hooks", "native_control_omitted_acceptance_plugin", "native"),
    # fire_event_no_suppression: native owns original/listener counts; JS must
    # not invent original counts.
    _sc("fire_event_no_suppression", "native_invocation_scope", "native"),
    _sc("fire_event_no_suppression", "native_original_once", "native"),
    _sc("fire_event_no_suppression", "native_listener_delivery", "native"),
    _sc("fire_event_no_suppression", "native_broadcast_unsuppressed", "native"),
    _sc("fire_event_no_suppression", "js_no_handled_on_unsuppressed_event", "js"),
    # fire_event_handled_recipient_mask: native/JS independent; human extra.
    _sc("fire_event_handled_recipient_mask", "native_handled_original_once", "native"),
    _sc("fire_event_handled_recipient_mask", "native_outgoing_recipient_decisions", "native"),
    _sc("fire_event_handled_recipient_mask", "js_handled_set_recipients", "js"),
    _sc("fire_event_handled_recipient_mask", "human_subset_receipt", "human"),
    _sc("fire_event_handled_recipient_mask", "human_excluded_nonreceipt", "human"),
    _sc("fire_event_handled_recipient_mask", "human_all_suppressed", "human"),
    _sc("voice_recall", "native_voice_listen_bits", "native"),
    _sc("voice_recall", "native_voice_original_once", "native"),
    _sc("voice_recall", "js_voice_policy_applied", "js"),
    _sc("voice_recall", "human_voice_allowed_hears", "human"),
    _sc("voice_recall", "human_voice_denied_silent", "human"),
    _sc("voice_recall", "human_voice_unmuted_hears", "human"),
    _sc("sdkhooks_one_of_two_entities", "native_spawn_a_ok", "native"),
    _sc("sdkhooks_one_of_two_entities", "native_spawn_b_ok", "native"),
    _sc("sdkhooks_one_of_two_entities", "js_hook_a_delivered", "js"),
    _sc("sdkhooks_one_of_two_entities", "js_hook_b_filtered", "js"),
    _sc("sdkhooks_phase_removal", "native_phase_subscribe_pre_post", "native"),
    _sc("sdkhooks_phase_removal", "native_phase_remove_pre", "native"),
    _sc("sdkhooks_phase_removal", "native_phase_remove_post", "native"),
    _sc("sdkhooks_phase_removal", "native_phase_self_unsubscribe", "native"),
    _sc("sdkhooks_phase_removal", "native_phase_final_unsubscribe", "native"),
    _sc("sdkhooks_phase_removal", "js_phase_subscribe_pre_post", "js"),
    _sc("sdkhooks_phase_removal", "js_phase_remove_pre", "js"),
    _sc("sdkhooks_phase_removal", "js_phase_remove_post", "js"),
    _sc("sdkhooks_phase_removal", "js_phase_self_unsubscribe", "js"),
    _sc("sdkhooks_phase_removal", "js_phase_final_unsubscribe", "js"),
    _sc("entity_slot_reuse_map_teardown", "native_identity_persisted", "native"),
    _sc("entity_slot_reuse_map_teardown", "native_slot_reuse_no_stale", "native"),
    _sc("entity_slot_reuse_map_teardown", "native_map_teardown_clears", "native"),
    _sc("entity_slot_reuse_map_teardown", "native_unload_reload", "native"),
    _sc("entity_slot_reuse_map_teardown", "js_identity_persisted", "js"),
    _sc("entity_slot_reuse_map_teardown", "js_slot_reuse_no_stale", "js"),
    _sc("entity_slot_reuse_map_teardown", "js_map_teardown_clears", "js"),
    _sc("entity_slot_reuse_map_teardown", "js_fresh_subscription_after_reload", "js"),
    _sc("check_transmit", "native_first_fire_layout", "native"),
    _sc("check_transmit", "native_per_recipient_filter", "native"),
    _sc("check_transmit", "js_visibility_policy", "js"),
    _sc("check_transmit", "human_client_a_visible", "human"),
    _sc("check_transmit", "human_client_b_denied", "human"),
    _sc("check_transmit", "human_visibility_restored", "human"),
)

CLIENT_ACTIONS = {
    "frame_client_command_hooks": (
        "a real connected client must issue the continue token then the handled "
        "token through ClientCommand (RCON is not ClientCommand evidence)"
    ),
    "voice_recall": (
        "three clients (speaker, allowed listener, denied listener): apply listen "
        "policy, observe allowed/denied audio, unmute, observe the formerly denied listener"
    ),
    "check_transmit": (
        "two clients in PVS of a networked visible entity; A-only visibility then restore all"
    ),
    "fire_event_handled_recipient_mask": (
        "at least two real clients; send an observable event to a strict subset, then suppress for all"
    ),
}


def required_subchecks() -> List[Subcheck]:
    return list(SUBCHECKS)


def _registered() -> Dict[Tuple[str, str, str], Subcheck]:
    return {(s.case, s.subcheck, s.producer): s for s in SUBCHECKS}


def _human_pending_observation(sc: Subcheck) -> Dict[str, Any]:
    if sc.case == "voice_recall":
        actors = [
            {"slot": 0, "identity": "speaker"},
            {"slot": 1, "identity": "allowed-listener"},
            {"slot": 2, "identity": "denied-listener"},
        ]
    else:
        actors = [
            {"slot": 0, "identity": "client-a"},
            {"slot": 1, "identity": "client-b"},
        ]
    expected = {
        "voice_recall": {
            "human_voice_allowed_hears": {"hears": True, "role": "allowed"},
            "human_voice_denied_silent": {"hears": False, "role": "denied"},
            "human_voice_unmuted_hears": {"hears": True, "role": "formerly-denied"},
        },
        "check_transmit": {
            "human_client_a_visible": {"visible": True, "client": "a"},
            "human_client_b_denied": {"visible": False, "client": "b"},
            "human_visibility_restored": {"visible": True, "client": "both"},
        },
        "fire_event_handled_recipient_mask": {
            "human_subset_receipt": {"received": True, "set": "subset"},
            "human_excluded_nonreceipt": {"received": False, "set": "excluded"},
            "human_all_suppressed": {"received": False, "set": "all"},
        },
    }[sc.case][sc.subcheck]
    return {
        "case": sc.case,
        "subcheck": sc.subcheck,
        "result": "pending",
        "expected": expected,
        "actual": {},
        "evidence": "",
        "actors": actors,
        "capture_path": "",
        "timestamp": "",
    }


EXAMPLE_RECORD = {
    "schema": SCHEMA,
    "suite": "A",
    "run_id": EXAMPLE_IDENTITY["run_id"],
    "source_revision": EXAMPLE_IDENTITY["source_revision"],
    "case": "new_capsule_registration",
    "subcheck": "native_null_configure_failed",
    "producer": "native",
    "result": "pending",
    "expected": {"state": "Failed", "id": "INVALID_HOOK"},
    "actual": {},
    "evidence": "",
}

EXAMPLE_HUMAN_OBSERVATIONS = {
    "schema": SCHEMA,
    "run_id": EXAMPLE_IDENTITY["run_id"],
    "source_revision": EXAMPLE_IDENTITY["source_revision"],
    "observations": [
        _human_pending_observation(s) for s in SUBCHECKS if s.producer == "human"
    ],
}


class RconUnreachable(Exception):
    """Named live-server failure; never invent native records to cover it."""


@dataclass
class JudgeResult:
    exit_code: int
    status: str
    messages: List[str] = field(default_factory=list)
    case_status: Dict[str, str] = field(default_factory=dict)


def probe_cmd(verb: str, run_id: str) -> str:
    if verb not in ("prepare", "collect", "report"):
        raise ValueError(verb)
    return f"s2_khook_probe {verb} {run_id}"


def accept_cmd(verb: str, run_id: str) -> str:
    if verb not in ("prepare", "collect", "report", "teardown"):
        raise ValueError(verb)
    return f"s2_khook_accept {verb} {run_id}"


def new_run_id() -> str:
    return "khook-a-" + uuid.uuid4().hex


def current_source_revision() -> str:
    try:
        out = subprocess.check_output(
            ["git", "rev-parse", "HEAD"],
            cwd=str(REPO),
            text=True,
            stderr=subprocess.DEVNULL,
        )
        return out.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def _canonical(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), default=str)


def parse_records(text: str) -> Tuple[List[dict], List[str]]:
    """Parse JSONL (or RCON-prefixed JSON) into record dicts.

    Lines without ``{`` are ignored (RCON chatter). A line that contains ``{``
    but does not parse is malformed — it is not skipped.
    """
    recs: List[dict] = []
    errors: List[str] = []
    for lineno, raw in enumerate(text.splitlines(), 1):
        s = raw.strip()
        if not s:
            continue
        start = s.find("{")
        end = s.rfind("}")
        if start < 0:
            continue
        if end <= start:
            errors.append(f"malformed JSON at line {lineno}")
            continue
        blob = s[start : end + 1]
        try:
            obj = json.loads(blob)
        except json.JSONDecodeError:
            errors.append(f"malformed JSON at line {lineno}")
            continue
        if not isinstance(obj, dict):
            errors.append(f"malformed JSON at line {lineno}: not an object")
            continue
        if "case" not in obj:
            continue
        recs.append(obj)
    return recs, errors


def _need_clients_messages(case_status: Dict[str, str]) -> List[str]:
    out = []
    for case, status in case_status.items():
        if status != "pending":
            continue
        action = CLIENT_ACTIONS.get(case)
        if action:
            out.append(f"NEED_CLIENTS: {case}: {action}")
    return out


def _legacy_contradicts(rec: dict) -> Optional[str]:
    if "pass" not in rec:
        return None
    legacy = rec["pass"]
    if not isinstance(legacy, bool):
        return "contradictory legacy pass field (not boolean)"
    result = str(rec.get("result") or "").lower()
    if result == "pass" and legacy is not True:
        return "contradictory result=pass with pass=false"
    if result == "fail" and legacy is not False:
        return "contradictory result=fail with pass=true"
    if result == "pending" and legacy is True:
        return "contradictory result=pending with pass=true"
    return None


def _normalize_observations(observations: Any, identity: dict) -> Tuple[List[dict], List[str]]:
    if observations is None:
        return [], []
    if isinstance(observations, (str, Path)):
        path = Path(observations)
        try:
            observations = json.loads(path.read_text())
        except (OSError, json.JSONDecodeError) as e:
            return [], [f"malformed observations: {e}"]
    if not isinstance(observations, dict):
        return [], ["malformed observations: expected object envelope"]
    errors: List[str] = []
    schema = observations.get("schema")
    if schema != SCHEMA:
        errors.append("unsupported observations schema")
        return [], errors
    if observations.get("run_id") != identity.get("run_id"):
        errors.append("wrong run in observations")
        return [], errors
    if observations.get("source_revision") != identity.get("source_revision"):
        errors.append("wrong source revision in observations")
        return [], errors
    rows = observations.get("observations")
    if not isinstance(rows, list):
        errors.append("malformed observations: observations[] required")
        return [], errors
    recs: List[dict] = []
    registered = _registered()
    for row in rows:
        if not isinstance(row, dict):
            errors.append("malformed observations: row is not an object")
            continue
        case = row.get("case")
        subcheck = row.get("subcheck")
        key = (case, subcheck, "human")
        if key not in registered:
            errors.append(f"unsupported human subcheck {case}/{subcheck}")
            continue
        rec = {
            "schema": SCHEMA,
            "suite": "A",
            "run_id": identity.get("run_id"),
            "source_revision": identity.get("source_revision"),
            "case": case,
            "subcheck": subcheck,
            "producer": "human",
            "result": row.get("result"),
            "expected": row.get("expected"),
            "actual": row.get("actual"),
            "evidence": row.get("evidence", ""),
            "actors": row.get("actors"),
            "capture_path": row.get("capture_path", ""),
            "timestamp": row.get("timestamp", ""),
        }
        recs.append(rec)
    return recs, errors


def _subcheck_verdict(rec: dict) -> Tuple[str, Optional[str]]:
    """Return (pass|fail|pending|invalid, reason)."""
    result = str(rec.get("result") or "").lower()
    if result not in RESULTS:
        return "invalid", "unsupported result"
    contra = _legacy_contradicts(rec)
    if contra:
        return "invalid", contra
    if "expected" not in rec:
        return "invalid", "missing expected"
    if "actual" not in rec:
        return "invalid", "missing actual"
    evidence = rec.get("evidence")
    if result in ("pass", "fail"):
        if not isinstance(evidence, str) or not evidence.strip():
            return "invalid", "evidence nonempty required for pass/fail"
    if rec.get("producer") == "human" and result == "pass":
        actors = rec.get("actors")
        if not isinstance(actors, list) or not actors:
            return "invalid", "absent actors cannot pass"
        if not rec.get("capture_path"):
            return "invalid", "human pass requires capture_path"
        if not rec.get("timestamp"):
            return "invalid", "human pass requires timestamp"
        for actor in actors:
            if not isinstance(actor, dict) or "slot" not in actor or not actor.get("identity"):
                return "invalid", "human pass requires actor slot and identity"
    if result == "pass" and _canonical(rec.get("expected")) != _canonical(rec.get("actual")):
        return "fail", "actual mismatch"
    return result, None


def judge_records(
    records: Sequence[dict],
    *,
    identity: dict,
    suite: str = "A",
    observations: Any = None,
    parse_errors: Optional[Sequence[str]] = None,
) -> JudgeResult:
    suite = (suite or "A").upper()
    messages: List[str] = []
    if suite in UNAVAILABLE_SUITES:
        reason = UNAVAILABLE_SUITES[suite]
        return JudgeResult(1, "fail", [f"suite {suite} {reason}"], {})
    if suite != "A":
        return JudgeResult(1, "fail", [f"unsupported suite {suite}"], {})

    errors = list(parse_errors or [])
    extra, obs_errors = _normalize_observations(observations, identity)
    errors.extend(obs_errors)
    all_recs = list(records) + extra

    registered = _registered()
    seen_keys: Dict[Tuple[str, str, str], dict] = {}
    envelopes: Dict[str, List[dict]] = {c: [] for c in SUITE_A_CASES}

    for rec in all_recs:
        if rec.get("schema") != SCHEMA:
            errors.append(f"unsupported schema {rec.get('schema')!r}")
            continue
        if str(rec.get("suite") or "A").upper() != "A":
            errors.append(f"unsupported suite in record {rec.get('suite')!r}")
            continue
        if rec.get("run_id") != identity.get("run_id"):
            errors.append(
                f"wrong run: record {rec.get('run_id')!r} != {identity.get('run_id')!r}"
            )
            continue
        if rec.get("source_revision") != identity.get("source_revision"):
            errors.append(
                f"wrong source revision: record {rec.get('source_revision')!r} "
                f"!= {identity.get('source_revision')!r}"
            )
            continue
        case = rec.get("case")
        subcheck = rec.get("subcheck")
        producer = rec.get("producer")
        if case not in SUITE_A_CASES:
            errors.append(f"unsupported case {case!r}")
            continue
        if producer not in PRODUCERS:
            errors.append(f"unsupported producer {producer!r}")
            continue
        key = (case, subcheck, producer)
        if key not in registered:
            errors.append(f"unsupported subcheck {case}/{subcheck} producer={producer}")
            continue
        if key in seen_keys:
            errors.append(f"duplicate producer-subcheck {producer}/{case}/{subcheck}")
            continue
        seen_keys[key] = rec
        envelopes[case].append(rec)

    case_status: Dict[str, str] = {}
    missing = [c for c in SUITE_A_CASES if not envelopes[c]]
    if missing:
        errors.append("missing case records: " + ", ".join(missing))

    for case in SUITE_A_CASES:
        recs_for_case = envelopes[case]
        if not recs_for_case:
            case_status[case] = "invalid"
            continue
        required = [s for s in SUBCHECKS if s.case == case]
        by_key = {(r["producer"], r["subcheck"]): rec for r in recs_for_case for rec in [r]}
        failed = []
        pending = []
        invalid_sc = []
        for sc in required:
            rec = by_key.get((sc.producer, sc.subcheck))
            if rec is None:
                pending.append(sc.subcheck)
                continue
            verdict, reason = _subcheck_verdict(rec)
            if verdict == "invalid":
                invalid_sc.append(f"{sc.subcheck}: {reason}")
            elif verdict == "fail":
                failed.append((sc, rec, reason))
            elif verdict == "pending":
                pending.append(sc.subcheck)
        if invalid_sc:
            case_status[case] = "invalid"
            errors.extend(f"{case}: {m}" for m in invalid_sc)
        elif failed:
            case_status[case] = "fail"
            for sc, rec, reason in failed:
                why = reason or "failed subcheck"
                messages.append(
                    f"FAIL: {case}/{sc.subcheck}: expected={rec.get('expected')!r} "
                    f"actual={rec.get('actual')!r} ({why})"
                )
        elif pending:
            case_status[case] = "pending"
            messages.append(f"PENDING: {case}: waiting on {', '.join(pending)}")
        else:
            case_status[case] = "pass"
            messages.append(f"PASS: {case}")

    messages.extend(f"FAIL: {e}" for e in errors)
    messages.extend(_need_clients_messages(case_status))

    if errors:
        failed_n = sum(1 for v in case_status.values() if v == "fail")
        pending_n = sum(1 for v in case_status.values() if v == "pending")
        passed_n = sum(1 for v in case_status.values() if v == "pass")
        messages.append(
            f"FAIL: suite {suite} required={len(SUITE_A_CASES)} pass={passed_n} "
            f"pending={pending_n} fail={failed_n}"
        )
        return JudgeResult(1, "fail", messages, case_status)

    if any(v == "fail" for v in case_status.values()):
        failed_n = sum(1 for v in case_status.values() if v == "fail")
        pending_n = sum(1 for v in case_status.values() if v == "pending")
        passed_n = sum(1 for v in case_status.values() if v == "pass")
        messages.append(
            f"FAIL: suite {suite} required={len(SUITE_A_CASES)} pass={passed_n} "
            f"pending={pending_n} fail={failed_n}"
        )
        return JudgeResult(1, "fail", messages, case_status)

    if any(v == "pending" for v in case_status.values()):
        pending_n = sum(1 for v in case_status.values() if v == "pending")
        passed_n = sum(1 for v in case_status.values() if v == "pass")
        messages.append(
            f"PENDING: suite {suite} required={len(SUITE_A_CASES)} pass={passed_n} "
            f"pending={pending_n} — not a green gate"
        )
        return JudgeResult(2, "pending", messages, case_status)

    messages.append(f"PASS: suite {suite} ({len(SUITE_A_CASES)}/{len(SUITE_A_CASES)})")
    return JudgeResult(0, "pass", messages, case_status)


def judge_text(
    text: str,
    *,
    identity: dict,
    suite: str = "A",
    observations: Any = None,
) -> JudgeResult:
    recs, errors = parse_records(text)
    return judge_records(
        recs,
        identity=identity,
        suite=suite,
        observations=observations,
        parse_errors=errors,
    )


# --from-file and live mode share these exact callables.
from_file_parse = parse_records
from_file_judge = judge_records
live_parse = parse_records
live_judge = judge_records


def _utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _write_json(path: Path, obj: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, indent=2, sort_keys=True) + "\n")


def _append_jsonl(path: Path, obj: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as fh:
        fh.write(json.dumps(obj, sort_keys=True) + "\n")


def _load_run(run_dir: Path) -> dict:
    return json.loads((run_dir / "run.json").read_text())


def _records_path(run_dir: Path) -> Path:
    return run_dir / "records.jsonl"


def _load_stored_records(run_dir: Path) -> List[dict]:
    path = _records_path(run_dir)
    if not path.exists():
        return []
    recs, errors = parse_records(path.read_text())
    if errors:
        return recs
    return recs


def _upsert_records(run_dir: Path, incoming: Iterable[dict]) -> None:
    existing = _load_stored_records(run_dir)
    by: Dict[Tuple[str, str, str], dict] = {}
    for rec in existing:
        key = (rec.get("case"), rec.get("subcheck"), rec.get("producer"))
        by[key] = rec
    for rec in incoming:
        key = (rec.get("case"), rec.get("subcheck"), rec.get("producer"))
        by[key] = rec
    lines = [json.dumps(by[k], sort_keys=True) for k in sorted(by, key=lambda t: (str(t[0]), str(t[1]), str(t[2])))]
    _records_path(run_dir).write_text("".join(line + "\n" for line in lines))


def default_rcon_send(cmd: str, port: int = 27015) -> str:
    try:
        proc = subprocess.run(
            ["python3", str(REPO / "scripts" / "rcon.py"), "--port", str(port), cmd],
            cwd=str(REPO),
            capture_output=True,
            text=True,
            timeout=25,
        )
    except (OSError, subprocess.TimeoutExpired) as e:
        raise RconUnreachable("rcon_unreachable") from e
    if proc.returncode != 0:
        raise RconUnreachable("rcon_unreachable")
    return (proc.stdout or "") + (proc.stderr or "")


def _store_raw(run_dir: Path, phase: str, cmd: str, out: str) -> None:
    raw = run_dir / "raw"
    raw.mkdir(parents=True, exist_ok=True)
    safe = cmd.replace(" ", "_")[:80]
    (raw / f"{phase}-{safe}.txt").write_text(out)
    recs, _errs = parse_records(out)
    if recs:
        _upsert_records(run_dir, recs)


def prepare_run(
    run_dir: Path | str,
    *,
    suite: str = "A",
    identity_fields: Optional[dict] = None,
    rcon_send: Optional[Callable[[str], str]] = None,
    port: int = 27015,
) -> JudgeResult:
    suite = (suite or "A").upper()
    run_dir = Path(run_dir)
    run_dir.mkdir(parents=True, exist_ok=True)
    fields = dict(identity_fields or {})
    run_id = fields.get("run_id") or new_run_id()
    source_revision = fields.get("source_revision") or current_source_revision()
    meta = {
        "schema": SCHEMA,
        "suite": suite,
        "run_id": run_id,
        "source_revision": source_revision,
        "s2script_commit": fields.get("s2script_commit") or source_revision,
        "s2script_build_hash": fields.get("s2script_build_hash"),
        "host_manifest_digest": fields.get("host_manifest_digest"),
        "fixture_revision": fields.get("fixture_revision") or source_revision,
        "server_build": fields.get("server_build"),
        "map": fields.get("map"),
        "host": fields.get("host") or "127.0.0.1",
        "server": fields.get("server") or f"127.0.0.1:{port}",
        "started_at": fields.get("started_at") or _utc_now(),
        "phase": "prepared",
        "reason": None,
    }
    _write_json(run_dir / "run.json", meta)
    if not _records_path(run_dir).exists():
        _records_path(run_dir).write_text("")
    (run_dir / "commands.jsonl").touch()
    (run_dir / "raw").mkdir(exist_ok=True)

    if suite in UNAVAILABLE_SUITES:
        meta["reason"] = f"suite_{suite}_not_authored"
        _write_json(run_dir / "run.json", meta)
        return JudgeResult(1, "fail", [f"suite {suite} {UNAVAILABLE_SUITES[suite]}"], {})

    send = rcon_send or (lambda cmd: default_rcon_send(cmd, port=port))
    try:
        for cmd in (probe_cmd("prepare", run_id), accept_cmd("prepare", run_id)):
            _append_jsonl(run_dir / "commands.jsonl", {"cmd": cmd, "ts": _utc_now()})
            out = send(cmd)
            _store_raw(run_dir, "prepare", cmd, out)
    except RconUnreachable as e:
        meta["reason"] = "rcon_unreachable"
        _write_json(run_dir / "run.json", meta)
        return JudgeResult(
            2,
            "pending",
            [f"rcon_unreachable: {e}", f"run retained at {run_dir} run_id={run_id}"],
            {},
        )
    messages = [f"prepared run_id={run_id} dir={run_dir}"]
    messages.extend(f"NEED_CLIENTS: {case}: {action}" for case, action in CLIENT_ACTIONS.items())
    return JudgeResult(2, "pending", messages, {})


def collect_run(
    run_dir: Path | str,
    *,
    rcon_send: Optional[Callable[[str], str]] = None,
    port: int = 27015,
    deadline_s: float = COLLECT_DEADLINE_S,
    interval_s: float = COLLECT_INTERVAL_S,
    max_attempts: int = COLLECT_MAX_ATTEMPTS,
) -> JudgeResult:
    run_dir = Path(run_dir)
    if not (run_dir / "run.json").exists():
        return JudgeResult(1, "fail", ["run_not_prepared"], {})
    meta = _load_run(run_dir)
    run_id = meta["run_id"]
    meta["phase"] = "collected"
    send = rcon_send or (lambda cmd: default_rcon_send(cmd, port=port))
    # Collect never re-sends prepare. Report remains a read-only path.
    cmds = (
        probe_cmd("collect", run_id),
        accept_cmd("collect", run_id),
        probe_cmd("report", run_id),
        accept_cmd("report", run_id),
    )
    deadline = time.monotonic() + deadline_s
    last_err: Optional[Exception] = None
    attempt = 0
    while attempt < max_attempts and time.monotonic() <= deadline:
        attempt += 1
        try:
            for cmd in cmds:
                verb = cmd.split()[1]
                if verb == "prepare":
                    raise RuntimeError("collect must not send prepare")
                _append_jsonl(run_dir / "commands.jsonl", {"cmd": cmd, "ts": _utc_now()})
                out = send(cmd)
                _store_raw(run_dir, "collect", cmd, out)
            last_err = None
        except RconUnreachable as e:
            last_err = e
            break
        recs = _load_stored_records(run_dir)
        present = {r.get("case") for r in recs}
        if all(c in present for c in SUITE_A_CASES):
            break
        if attempt < max_attempts and time.monotonic() + interval_s <= deadline:
            time.sleep(interval_s)
            continue
        break
    if last_err is not None:
        meta["reason"] = "rcon_unreachable"
        _write_json(run_dir / "run.json", meta)
        return JudgeResult(
            2,
            "pending",
            [f"rcon_unreachable: {last_err}", f"run retained at {run_dir} run_id={run_id}"],
            {},
        )
    recs = _load_stored_records(run_dir)
    present = {r.get("case") for r in recs}
    if not all(c in present for c in SUITE_A_CASES):
        meta["reason"] = "incomplete_records"
        _write_json(run_dir / "run.json", meta)
        messages = [
            "incomplete_records: envelopes still waiting; not invented",
            f"run retained at {run_dir} run_id={run_id}",
        ]
        messages.extend(f"NEED_CLIENTS: {case}: {action}" for case, action in CLIENT_ACTIONS.items())
        return JudgeResult(2, "pending", messages, {})
    meta["reason"] = None
    _write_json(run_dir / "run.json", meta)
    judged = judge_records(recs, identity=meta, suite=meta.get("suite", "A"))
    messages = [f"collected run_id={run_id} dir={run_dir}"] + judged.messages
    return JudgeResult(judged.exit_code, judged.status, messages, judged.case_status)


def judge_run_dir(
    run_dir: Path | str,
    observations: Any = None,
    rcon_send: Optional[Callable[[str], str]] = None,
) -> JudgeResult:
    """Read-only judge. Does not send RCON or rewrite records."""
    del rcon_send  # judge never talks to the server
    run_dir = Path(run_dir)
    meta = _load_run(run_dir)
    text = _records_path(run_dir).read_text() if _records_path(run_dir).exists() else ""
    recs, errors = parse_records(text)
    return judge_records(
        recs,
        identity=meta,
        suite=meta.get("suite", "A"),
        observations=observations,
        parse_errors=errors,
    )


def _human_pass_actors(case: str) -> List[dict]:
    if case == "voice_recall":
        return [
            {"slot": 0, "identity": "speaker-0"},
            {"slot": 1, "identity": "allowed-1"},
            {"slot": 2, "identity": "denied-2"},
        ]
    return [
        {"slot": 0, "identity": "client-a"},
        {"slot": 1, "identity": "client-b"},
    ]


def synthetic_fixture(kind: str, identity: dict) -> List[dict]:
    """Host self-test records. Never live evidence; evidence strings say so."""
    recs: List[dict] = []
    for sc in SUBCHECKS:
        rec = {
            "schema": SCHEMA,
            "suite": "A",
            "run_id": identity["run_id"],
            "source_revision": identity["source_revision"],
            "case": sc.case,
            "subcheck": sc.subcheck,
            "producer": sc.producer,
            "result": "pass",
            "expected": {"ok": True},
            "actual": {"ok": True},
            "evidence": "self-test fixture; not live evidence",
        }
        if sc.producer == "human":
            rec["actors"] = _human_pass_actors(sc.case)
            rec["capture_path"] = "captures/self-test.txt"
            rec["timestamp"] = "2026-09-14T00:00:00Z"
        recs.append(rec)
    if kind == "all-pass":
        return recs
    if kind == "pending":
        for rec in recs:
            rec["result"] = "pending"
            rec["actual"] = {}
            rec["evidence"] = ""
            if rec["producer"] == "human":
                rec["capture_path"] = ""
                rec["timestamp"] = ""
        return recs
    if kind == "missing":
        return [r for r in recs if r["case"] != "check_transmit"]
    if kind == "duplicate":
        recs.append(dict(recs[0]))
        return recs
    if kind == "fail":
        for rec in recs:
            if rec["subcheck"] == "native_one_pre_post_orig":
                rec["result"] = "fail"
                rec["actual"] = {"ok": False}
                rec["evidence"] = "self-test fixture fail"
        return recs
    if kind == "native-fail-js-pass":
        for rec in recs:
            if rec["producer"] == "native" and rec["case"] == "frame_client_command_hooks":
                rec["result"] = "fail"
                rec["actual"] = {"ok": False}
                rec["evidence"] = "self-test native fail"
        return recs
    raise ValueError(f"unknown fixture {kind}")


def print_result(result: JudgeResult) -> int:
    for msg in result.messages:
        print(msg)
    return result.exit_code


def judge_from_file(path: str | Path, suite: str = "A") -> JudgeResult:
    text = Path(path).read_text()
    recs, errors = parse_records(text)
    run_id = recs[0].get("run_id") if recs else ""
    identity = {
        "run_id": run_id,
        "source_revision": current_source_revision(),
    }
    return from_file_judge(
        recs,
        identity=identity,
        suite=suite,
        parse_errors=errors,
    )


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="KHook suite A acceptance controller")
    p.add_argument("suite", nargs="?", default="A")
    p.add_argument("--from-file")
    p.add_argument("--prepare", action="store_true")
    p.add_argument("--collect", action="store_true")
    p.add_argument("--judge", action="store_true")
    p.add_argument("--run-dir")
    p.add_argument("--observations")
    p.add_argument("--port", type=int, default=27015)
    p.add_argument("--emit-fixture", choices=("all-pass", "pending", "missing", "duplicate", "fail", "native-fail-js-pass"))
    p.add_argument("--identity-run-id", default="self-test")
    return p


def main(argv: Optional[Sequence[str]] = None) -> int:
    args = _build_parser().parse_args(argv)
    suite = (args.suite or "A").upper()
    if args.emit_fixture:
        identity = {
            "run_id": args.identity_run_id,
            "source_revision": current_source_revision(),
        }
        for rec in synthetic_fixture(args.emit_fixture, identity):
            print(json.dumps(rec, sort_keys=True))
        return 0
    if args.from_file:
        return print_result(judge_from_file(args.from_file, suite=suite))
    staged = args.prepare or args.collect or args.judge
    run_dir = Path(args.run_dir) if args.run_dir else (REPO / "build" / "khook-acceptance" / new_run_id())
    if not staged:
        args.prepare = args.collect = args.judge = True
    code = 0
    if args.prepare:
        code = print_result(prepare_run(run_dir, suite=suite, port=args.port))
        if code == 1:
            return code
    if args.collect:
        code = print_result(collect_run(run_dir, port=args.port))
        if code == 1:
            return code
    if args.judge:
        if (run_dir / "run.json").exists():
            meta = json.loads((run_dir / "run.json").read_text())
            empty = (
                not _records_path(run_dir).exists()
                or not _records_path(run_dir).read_text().strip()
            )
            if empty and meta.get("reason") in ("rcon_unreachable", "incomplete_records"):
                print(f"PENDING: {meta['reason']}")
                print(f"run retained at {run_dir} run_id={meta.get('run_id')}")
                for case, action in CLIENT_ACTIONS.items():
                    print(f"NEED_CLIENTS: {case}: {action}")
                return 2
        code = print_result(judge_run_dir(run_dir, observations=args.observations))
    return code


if __name__ == "__main__":
    sys.exit(main())
