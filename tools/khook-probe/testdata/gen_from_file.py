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
sys.path.insert(0, str(Path(__file__).resolve().parent))
import khook_acceptance as ka  # noqa: E402
import r6_state_machine as r6  # noqa: E402


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

    sm_errs = r6.selftest()
    if sm_errs:
        print("error: r6_state_machine selftest: " + "; ".join(sm_errs), file=sys.stderr)
        return 1

    write_r6_host_fixtures(out_dir, rev)
    if not judge_r6_host_fixtures(out_dir):
        return 1
    return 0


R6_NATIVE_JS_EXPECTED = {
    ("sdkhooks_one_of_two_entities", "native_spawn_a_ok"): {"spawned": True},
    ("sdkhooks_one_of_two_entities", "native_spawn_b_ok"): {"spawned": True},
    ("sdkhooks_one_of_two_entities", "js_hook_a_delivered"): {"count": 1},
    ("sdkhooks_one_of_two_entities", "js_hook_b_filtered"): {"count": 0},
    ("sdkhooks_phase_removal", "native_phase_subscribe_pre_post"): r6.PHASE_EXPECTED["subscribe_pre_post"],
    ("sdkhooks_phase_removal", "native_phase_remove_pre"): r6.PHASE_EXPECTED["remove_pre"],
    ("sdkhooks_phase_removal", "native_phase_remove_post"): r6.PHASE_EXPECTED["remove_post"],
    ("sdkhooks_phase_removal", "native_phase_self_unsubscribe"): r6.PHASE_EXPECTED["self_unsubscribe"],
    ("sdkhooks_phase_removal", "native_phase_final_unsubscribe"): r6.PHASE_EXPECTED["final_unsubscribe"],
    ("sdkhooks_phase_removal", "js_phase_subscribe_pre_post"): r6.js_phase_expected("subscribe_pre_post"),
    ("sdkhooks_phase_removal", "js_phase_remove_pre"): r6.js_phase_expected("remove_pre"),
    ("sdkhooks_phase_removal", "js_phase_remove_post"): r6.js_phase_expected("remove_post"),
    ("sdkhooks_phase_removal", "js_phase_self_unsubscribe"): r6.js_phase_expected("self_unsubscribe"),
    ("sdkhooks_phase_removal", "js_phase_final_unsubscribe"): r6.js_phase_expected("final_unsubscribe"),
    ("entity_slot_reuse_map_teardown", "native_identity_persisted"): {"persisted": True, "index": 5, "serial": 3},
    ("entity_slot_reuse_map_teardown", "native_slot_reuse_no_stale"): {"stale": False, "reused": True},
    ("entity_slot_reuse_map_teardown", "native_map_teardown_clears"): {"cleared": True},
    ("entity_slot_reuse_map_teardown", "native_unload_reload"): {"reloaded": True, "peer_loaded": True},
    ("entity_slot_reuse_map_teardown", "js_identity_persisted"): {"persisted": True, "index": 5, "id": 11},
    ("entity_slot_reuse_map_teardown", "js_slot_reuse_no_stale"): {"stale": False, "reused": True},
    ("entity_slot_reuse_map_teardown", "js_map_teardown_clears"): {"cleared": True},
    ("entity_slot_reuse_map_teardown", "js_fresh_subscription_after_reload"): {"fresh": True, "count": 1},
    ("voice_recall", "native_voice_listen_bits"): {"allowed": True, "denied": False},
    ("voice_recall", "native_voice_original_once"): {"orig": 1},
    ("voice_recall", "js_voice_policy_applied"): {"applied": True, "allowed_slot": 1, "denied_slot": 2},
    ("check_transmit", "native_first_fire_layout"): {"layout_ok": True},
    ("check_transmit", "native_per_recipient_filter"): {"a": True, "b": False},
    ("check_transmit", "js_visibility_policy"): {"a": True, "b": False},
    ("fire_event_handled_recipient_mask", "native_handled_original_once"): {
        "orig": 1,
        "automatic_skipped": True,
        "listener": 1,
    },
    ("fire_event_handled_recipient_mask", "native_outgoing_recipient_decisions"): {
        "subset": True,
        "excluded": True,
        "all_suppressed": True,
        "call_original_supersede": True,
    },
    ("fire_event_handled_recipient_mask", "js_handled_set_recipients"): {
        "handled": True,
        "subset": True,
        "all_suppressed": True,
    },
}


def r6_base(identity: dict, *, r6_result: str) -> list:
    """R5-owned triples pass; R6 native/js use r6_result; humans stay pending."""
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
            continue
        expected = R6_NATIVE_JS_EXPECTED.get((sc.case, sc.subcheck), {"ok": True})
        if sc.producer == "human":
            out.append(rec(sc, identity, "pending", expected if expected else {"ok": True}, {}, r6.pending_reason(
                "three_clients" if sc.case == "voice_recall"
                else "pvs_entity" if sc.case == "check_transmit"
                else "recipient_subset"
            )))
            continue
        if r6_result == "pending":
            out.append(rec(sc, identity, "pending", expected, {}, r6.pending_reason("sdkhooks_adapter") if "phase" in sc.subcheck or "spawn" in sc.subcheck or "hook" in sc.subcheck else r6.pending_reason("slot_reuse") if "reuse" in sc.subcheck or "identity" in sc.subcheck or "map" in sc.subcheck or "reload" in sc.subcheck else r6.pending_reason("three_clients") if "voice" in sc.subcheck else r6.pending_reason("pvs_entity") if "transmit" in sc.subcheck or "layout" in sc.subcheck or "visibility" in sc.subcheck or sc.subcheck.endswith("filter") else r6.pending_reason("recipient_subset")))
        elif r6_result == "pass":
            out.append(
                rec(
                    sc,
                    identity,
                    "pass",
                    expected,
                    expected,
                    "r6 host transition fixture; not live evidence",
                )
            )
        else:
            out.append(rec(sc, identity, r6_result, expected, {}, "r6 host fixture"))
    return out


def write_r6_host_fixtures(out_dir: Path, rev: str) -> None:
    # Completed automated transitions; humans remain pending (exit 2).
    ident = {"run_id": "khook-a-r6-transitions", "source_revision": rev}
    write_jsonl(out_dir / "r6-phase-transitions.jsonl", r6_base(ident, r6_result="pass"))

    # Filtering failure: B delivered.
    ident_b = {"run_id": "khook-a-r6-filter-b", "source_revision": rev}
    rows = r6_base(ident_b, r6_result="pass")
    overlay(
        rows,
        ("js", "sdkhooks_one_of_two_entities", "js_hook_b_filtered"),
        result="fail",
        expected={"count": 0},
        actual={"count": 1},
        evidence="r6 host fixture: filtering failure, B delivered",
    )
    write_jsonl(out_dir / "r6-negative-filter.jsonl", rows)

    # Stale post-map record.
    ident_map = {"run_id": "khook-a-r6-stale-map", "source_revision": rev}
    rows = r6_base(ident_map, r6_result="pass")
    overlay(
        rows,
        ("native", "entity_slot_reuse_map_teardown", "native_map_teardown_clears"),
        result="fail",
        expected={"cleared": True},
        actual={"cleared": False, "pre_map_count": 4, "post_map_count": 4},
        evidence="r6 host fixture: stale post-map record reused a pre-map counter",
    )
    write_jsonl(out_dir / "r6-stale-post-map.jsonl", rows)

    # Cleanup/reprepare leak.
    ident_leak = {"run_id": "khook-a-r6-reprepare-leak", "source_revision": rev}
    rows = r6_base(ident_leak, r6_result="pass")
    overlay(
        rows,
        ("native", "sdkhooks_one_of_two_entities", "native_spawn_a_ok"),
        result="fail",
        expected={"spawned": True},
        actual={"spawned": True, "count": 2},
        evidence="r6 host fixture: repeat prepare leaked counters (A=2)",
    )
    write_jsonl(out_dir / "r6-cleanup-reprepare.jsonl", rows)

    # Slot reuse not achieved stays pending, not pass.
    ident_reuse = {"run_id": "khook-a-r6-reuse-pending", "source_revision": rev}
    rows = r6_base(ident_reuse, r6_result="pass")
    overlay(
        rows,
        ("native", "entity_slot_reuse_map_teardown", "native_slot_reuse_no_stale"),
        result="pending",
        expected={"stale": False, "reused": True},
        actual={"attempts": 64, "max_attempts": 64},
        evidence=r6.pending_reason("slot_reuse"),
    )
    overlay(
        rows,
        ("js", "entity_slot_reuse_map_teardown", "js_slot_reuse_no_stale"),
        result="pending",
        expected={"stale": False, "reused": True},
        actual={},
        evidence=r6.pending_reason("slot_reuse"),
    )
    write_jsonl(out_dir / "r6-slot-reuse-pending.jsonl", rows)

    # Missing actor human pass must not judge as pass (exit 1 invalid/fail).
    ident_actor = {"run_id": "khook-a-r6-missing-actor", "source_revision": rev}
    rows = r6_base(ident_actor, r6_result="pass")
    overlay(
        rows,
        ("human", "voice_recall", "human_voice_allowed_hears"),
        result="pass",
        expected={"hears": True, "role": "allowed"},
        actual={"hears": True, "role": "allowed"},
        evidence="r6 host fixture: missing actors must not pass",
        actors=[],
        capture_path="captures/forged.txt",
        timestamp="2026-09-15T00:00:00Z",
    )
    write_jsonl(out_dir / "r6-missing-actor.jsonl", rows)

    # Map ended but no post-map EntByIndex invoke — pending, not pass (we stopped touching).
    ident_ni = {"run_id": "khook-a-r6-map-no-invoke", "source_revision": rev}
    rows = r6_base(ident_ni, r6_result="pass")
    overlay(
        rows,
        ("native", "entity_slot_reuse_map_teardown", "native_map_teardown_clears"),
        result="pending",
        expected={"cleared": True},
        actual={},
        evidence=r6.pending_reason("map_invoke"),
    )
    overlay(
        rows,
        ("js", "entity_slot_reuse_map_teardown", "js_map_teardown_clears"),
        result="pending",
        expected={"cleared": True},
        actual={},
        evidence=r6.pending_reason("map_invoke"),
    )
    write_jsonl(out_dir / "r6-map-no-invoke.jsonl", rows)

    # Reload registered a hook but it has not delivered once.
    ident_rd = {"run_id": "khook-a-r6-reload-no-delivery", "source_revision": rev}
    rows = r6_base(ident_rd, r6_result="pass")
    overlay(
        rows,
        ("js", "entity_slot_reuse_map_teardown", "js_fresh_subscription_after_reload"),
        result="pending",
        expected={"fresh": True, "count": 1},
        actual={},
        evidence=r6.pending_reason("reload_delivery"),
    )
    write_jsonl(out_dir / "r6-reload-no-delivery.jsonl", rows)

    # Human pending schema/example (never prefilled pass).
    ident_human = {"run_id": "khook-a-r6-human-pending", "source_revision": rev}
    write_jsonl(out_dir / "r6-human-pending.jsonl", r6_base(ident_human, r6_result="pass"))


def judge_r6_host_fixtures(out_dir: Path) -> bool:
    checks = [
        ("r6-phase-transitions.jsonl", 2),
        ("r6-human-pending.jsonl", 2),
        ("r6-slot-reuse-pending.jsonl", 2),
        ("r6-negative-filter.jsonl", 1),
        ("r6-stale-post-map.jsonl", 1),
        ("r6-cleanup-reprepare.jsonl", 1),
        ("r6-missing-actor.jsonl", 1),
        ("r6-map-no-invoke.jsonl", 2),
        ("r6-reload-no-delivery.jsonl", 2),
    ]
    ok = True
    for name, want in checks:
        path = out_dir / name
        result = ka.judge_from_file(path, suite="A")
        if result.exit_code != want:
            print(
                f"error: {name} expected exit {want}, got {result.exit_code}: {result.messages}",
                file=sys.stderr,
            )
            ok = False
        else:
            print(f"ok:   r6 host fixture {name} exit {result.exit_code}")
    return ok


if __name__ == "__main__":
    raise SystemExit(main())
