#!/usr/bin/env python3
"""Generate schema=1 --from-file fixtures for the frozen R4 judge.

These are host fixtures, never live evidence. source_revision is stamped to
git HEAD so --from-file (which uses current_source_revision()) can judge them.
"""
from __future__ import annotations

import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts"))
import khook_acceptance as ka  # noqa: E402


def rec(sc: ka.Subcheck, identity: dict, result: str, expected, actual, evidence: str) -> dict:
    row = {
        "schema": ka.SCHEMA,
        "suite": "A",
        "run_id": identity["run_id"],
        "source_revision": identity["source_revision"],
        "case": sc.case,
        "subcheck": sc.subcheck,
        "producer": sc.producer,
        "result": result,
        "expected": expected,
        "actual": actual,
        "evidence": evidence,
    }
    if sc.producer == "human":
        if sc.case == "voice_recall":
            row["actors"] = [
                {"slot": 0, "identity": "speaker-0"},
                {"slot": 1, "identity": "allowed-1"},
                {"slot": 2, "identity": "denied-2"},
            ]
        else:
            row["actors"] = [
                {"slot": 0, "identity": "client-a"},
                {"slot": 1, "identity": "client-b"},
            ]
        row["capture_path"] = "captures/r5-fixture.txt" if result == "pass" else ""
        row["timestamp"] = "2026-09-15T00:00:00Z" if result == "pass" else ""
    return row


def default_expected(sc: ka.Subcheck) -> dict:
    return {"ok": True}


R5_OWNED = {
    "new_capsule_registration",
    "shared_capsule_registration",
    "peer_actions_both_orders",
    "one_normal_invocation",
    "frame_client_command_hooks",
    "fire_event_no_suppression",
}


def r5_pass_r6_pending(identity: dict) -> list:
    """R5-owned triples pass; R6/human stay pending. Never invent live/human passes."""
    out = []
    for sc in ka.required_subchecks():
        if sc.case in R5_OWNED:
            out.append(
                rec(
                    sc,
                    identity,
                    "pass",
                    {"ok": True},
                    {"ok": True},
                    "r5 from-file fixture; not live evidence",
                )
            )
        else:
            out.append(rec(sc, identity, "pending", default_expected(sc), {}, ""))
    return out


def overlay(rows: list, key, **fields) -> None:
    for row in rows:
        if (row["producer"], row["case"], row["subcheck"]) == key:
            row.update(fields)
            return
    raise KeyError(key)


def write_jsonl(path: Path, rows: list) -> None:
    path.write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in rows))


def main() -> int:
    if len(sys.argv) < 3:
        print("usage: gen_from_file.py SOURCE_REVISION OUT_DIR", file=sys.stderr)
        return 2
    rev = sys.argv[1]
    out_dir = Path(sys.argv[2])
    out_dir.mkdir(parents=True, exist_ok=True)

    ident_pending = {"run_id": "khook-a-r5-pending", "source_revision": rev}
    ident_missing = {"run_id": "khook-a-r5-missing-js", "source_revision": rev}
    ident_flip = {"run_id": "khook-a-r5-flipped", "source_revision": rev}
    ident_omit = {"run_id": "khook-a-r5-omitted", "source_revision": rev}

    pending_rows = []
    for sc in ka.required_subchecks():
        result = "pending"
        expected = {"ok": True}
        actual: dict = {}
        evidence = ""
        if sc.case in (
            "new_capsule_registration",
            "shared_capsule_registration",
            "peer_actions_both_orders",
            "one_normal_invocation",
            "fire_event_no_suppression",
        ):
            result = "pass"
            actual = {"ok": True}
            evidence = "r5 from-file fixture; not live evidence"
        pending_rows.append(rec(sc, ident_pending, result, expected, actual, evidence))
    write_jsonl(out_dir / "r6-pending.jsonl", pending_rows)

    missing = r5_pass_r6_pending(ident_missing)
    overlay(
        missing,
        ("js", "frame_client_command_hooks", "js_command_continue_delivery"),
        result="fail",
        expected={"js": 1},
        actual={"js": 0},
        evidence="r5 from-file fixture: missing JS hook",
    )
    overlay(
        missing,
        ("native", "frame_client_command_hooks", "native_control_missing_js_hook"),
        result="pass",
        expected={"detected": True, "engine": 1, "js_hook": False},
        actual={"detected": True, "engine": 1, "js_hook": False},
        evidence="r5 from-file fixture: missing JS hook detected",
    )
    write_jsonl(out_dir / "negative-missing-js.jsonl", missing)

    flipped = r5_pass_r6_pending(ident_flip)
    overlay(
        flipped,
        ("native", "frame_client_command_hooks", "native_command_continue_original"),
        result="fail",
        expected={"native_pre": 1, "native_post": 1, "engine": 1, "skipped": False},
        actual={"native_pre": 1, "native_post": 1, "engine": 0, "skipped": True},
        evidence="r5 from-file fixture: Continue was suppressed",
    )
    overlay(
        flipped,
        ("native", "frame_client_command_hooks", "native_command_handled_skipped"),
        result="fail",
        expected={"native_pre": 1, "native_post": 1, "engine": 0, "skipped": True},
        actual={"native_pre": 1, "native_post": 1, "engine": 1, "skipped": False},
        evidence="r5 from-file fixture: Handled was not skipped",
    )
    overlay(
        flipped,
        ("native", "frame_client_command_hooks", "native_control_flipped_decision"),
        result="pass",
        expected={"continue_engine": 0, "handled_engine": 1, "continue_skipped": True, "handled_skipped": False},
        actual={"continue_engine": 0, "handled_engine": 1, "continue_skipped": True, "handled_skipped": False},
        evidence="r5 from-file fixture: flipped decision detected",
    )
    write_jsonl(out_dir / "negative-flipped.jsonl", flipped)

    omitted = r5_pass_r6_pending(ident_omit)
    for row in omitted:
        if row["producer"] == "js" and row["case"] == "frame_client_command_hooks":
            row["result"] = "fail"
            row["expected"] = {"js": 1}
            row["actual"] = {}
            row["evidence"] = "r5 from-file fixture: acceptance plugin omitted"
    overlay(
        omitted,
        ("native", "frame_client_command_hooks", "native_control_omitted_acceptance_plugin"),
        result="pass",
        expected={"js_plugin": False},
        actual={"js_plugin": False},
        evidence="r5 from-file fixture: omitted plugin detected",
    )
    write_jsonl(out_dir / "negative-omitted-plugin.jsonl", omitted)

    ident_partial = {"run_id": "khook-a-r5-partial-continue", "source_revision": rev}
    partial = r5_pass_r6_pending(ident_partial)
    overlay(
        partial,
        ("native", "frame_client_command_hooks", "native_command_handled_skipped"),
        result="pending",
        expected={"native_pre": 1, "native_post": 1, "engine": 0, "skipped": True},
        actual={},
        evidence="",
    )
    overlay(
        partial,
        ("js", "frame_client_command_hooks", "js_command_handled_delivery"),
        result="pending",
        expected={"js": 1},
        actual={},
        evidence="",
    )
    write_jsonl(out_dir / "partial-continue.jsonl", partial)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
