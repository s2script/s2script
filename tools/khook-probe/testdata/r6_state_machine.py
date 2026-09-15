#!/usr/bin/env python3
"""Host transition spec for R6 stateful SDKHooks / human-assisted cases.

This is the TDD oracle: exact counts after each phase action, filter
negatives, stale post-map records, missing actors, and cleanup/reprepare.
Live probe/JS must emit these expected objects. Human rows stay pending
unless a real --observations file supplies them — never invent a pass.
"""
from __future__ import annotations

from typing import Dict, List, Optional, Tuple


PHASE_STEPS = (
    "subscribe_pre_post",
    "remove_pre",
    "remove_post",
    "self_unsubscribe",
    "final_unsubscribe",
)

PHASE_EXPECTED = {
    "subscribe_pre_post": {"pre": 1, "post": 1, "original": 1},
    "remove_pre": {"pre": 0, "post": 1, "original": 1},
    "remove_post": {"pre": 1, "post": 0, "original": 1},
    "self_unsubscribe": {
        "first_pre": 1,
        "first_post": 1,
        "second_pre": 0,
        "second_post": 1,
        "original_first": 1,
        "original_second": 1,
    },
    "final_unsubscribe": {"pre": 0, "post": 0, "original": 1},
}


def phase_after(action: str, pre_sub: bool, post_sub: bool, self_unsub_on_first: bool) -> Dict[str, int]:
    """Pure transition: invoke once (or twice for self-unsub) against current subscriptions."""
    if action == "subscribe_pre_post":
        return dict(PHASE_EXPECTED["subscribe_pre_post"])
    if action == "remove_pre":
        assert pre_sub is False and post_sub is True
        return dict(PHASE_EXPECTED["remove_pre"])
    if action == "remove_post":
        assert pre_sub is True and post_sub is False
        return dict(PHASE_EXPECTED["remove_post"])
    if action == "self_unsubscribe":
        assert self_unsub_on_first
        return dict(PHASE_EXPECTED["self_unsubscribe"])
    if action == "final_unsubscribe":
        assert pre_sub is False and post_sub is False
        return dict(PHASE_EXPECTED["final_unsubscribe"])
    raise ValueError(action)


def filter_verdict(spawn_a: bool, spawn_b: bool, delivered_a: int, delivered_b: int) -> Dict[str, Tuple[str, dict, str]]:
    """Hook only A. Registration failure = neither delivered. Filter failure = B delivered."""
    out: Dict[str, Tuple[str, dict, str]] = {}
    if spawn_a:
        out["native_spawn_a_ok"] = ("pass", {"spawned": True}, "spawn A succeeded")
    else:
        out["native_spawn_a_ok"] = (
            "pending",
            {"spawned": True},
            "need live engine: UTIL_CreateEntityByName + DispatchSpawn of entity A",
        )
    if spawn_b:
        out["native_spawn_b_ok"] = ("pass", {"spawned": True}, "spawn B succeeded")
    else:
        out["native_spawn_b_ok"] = (
            "pending",
            {"spawned": True},
            "need live engine: UTIL_CreateEntityByName + DispatchSpawn of entity B",
        )
    if not spawn_a or not spawn_b:
        out["js_hook_a_delivered"] = (
            "pending",
            {"count": 1},
            "require successful spawn of A and B before judging filtering",
        )
        out["js_hook_b_filtered"] = (
            "pending",
            {"count": 0},
            "require successful spawn of A and B before judging filtering",
        )
        return out
    if delivered_a == 0 and delivered_b == 0:
        out["js_hook_a_delivered"] = (
            "fail",
            {"count": 1},
            "registration failure: neither entity delivered (hook only A)",
        )
        out["js_hook_b_filtered"] = (
            "fail",
            {"count": 0},
            "registration failure: neither entity delivered",
        )
        return out
    if delivered_a == 1:
        out["js_hook_a_delivered"] = ("pass", {"count": 1}, "A delivered once")
    else:
        out["js_hook_a_delivered"] = ("fail", {"count": 1}, "A delivery count mismatch")
    if delivered_b == 0:
        out["js_hook_b_filtered"] = ("pass", {"count": 0}, "B filtered")
    else:
        out["js_hook_b_filtered"] = (
            "fail",
            {"count": 0},
            "filtering failure: B delivered while only A was hooked",
        )
    return out


def reuse_verdict(
    old_index: Optional[int],
    old_serial: Optional[int],
    new_index: Optional[int],
    new_serial: Optional[int],
    stale_deliveries: int,
    attempts: int,
    max_attempts: int = 64,
) -> Tuple[str, dict, str]:
    if old_index is None or old_serial is None:
        return (
            "pending",
            {"stale": False},
            "need live engine: persist index+serial then UTIL_Remove",
        )
    if new_index is None or new_serial is None or new_index != old_index:
        return (
            "pending",
            {"stale": False, "attempts": attempts, "max_attempts": max_attempts},
            "slot reuse not achieved within bounded attempts; not a false pass",
        )
    if new_serial == old_serial:
        return (
            "pending",
            {"stale": False},
            "reused slot kept the same serial; identity did not change",
        )
    if stale_deliveries != 0:
        return ("fail", {"stale": False}, "old subscription delivered for the new occupant")
    return ("pass", {"stale": False, "reused": True}, "old subscription silent on new serial")


def stale_post_map_record(pre_map_count: int, post_map: bool, post_map_count: int) -> Tuple[str, dict, str]:
    """A cached pre-map counter cannot satisfy a post-map assertion."""
    if not post_map:
        return (
            "pending",
            {"cleared": True},
            "need operator changelevel while this run stays prepared; then collect again",
        )
    if pre_map_count > 0 and post_map_count == pre_map_count:
        return (
            "fail",
            {"cleared": True},
            "stale post-map record: pre-map counter reused after map teardown",
        )
    if post_map_count == 0:
        return ("pass", {"cleared": True}, "post-map callbacks cleared")
    return ("fail", {"cleared": True}, "post-map callback still firing on old identity")


def missing_actor_human_row() -> dict:
    return {
        "result": "pass",
        "actors": [],
        "capture_path": "captures/forged.txt",
        "timestamp": "2026-09-15T00:00:00Z",
        "expected": {"hears": True, "role": "allowed"},
        "actual": {"hears": True, "role": "allowed"},
        "evidence": "forged missing-actor row must not pass",
    }


def cleanup_reprepare_leaked(first_prepare_a: int, second_prepare_a: int) -> bool:
    """Repeat prepare must reset counters; leaked A=2 is a failure."""
    return first_prepare_a == 1 and second_prepare_a == 1


def pending_reason(kind: str) -> str:
    return {
        "sdkhooks_adapter": (
            "need live engine: schedulable entity or Touch invoke through the actual "
            "SDKHooks adapter (CTriggerPush::Touch PRE+POST); Dummy Virtuals are supporting "
            "evidence only"
        ),
        "three_clients": (
            "need three real clients (speaker, allowed-listener, denied-listener); "
            "speaker talks in allowed phase, denied phase, then unmuted phase"
        ),
        "pvs_entity": (
            "need a networked visible entity in both clients' PVS (not logic_relay); "
            "client A allowed, client B denied, then restore visibility"
        ),
        "recipient_subset": (
            "need at least two real clients; send an observable event to a strict subset, "
            "then suppress for all. Sending to every human is not a mask test"
        ),
        "unload_reload": (
            "need s2script/probe native unload/reload with the peer still loaded "
            "(R2 pending/retry); missing restoration must stay visible"
        ),
        "map_change": "need operator changelevel while this run stays prepared; then collect again",
        "slot_reuse": "slot reuse not achieved within bounded attempts; not a false pass",
    }[kind]


def selftest() -> List[str]:
    errors: List[str] = []
    got = phase_after("subscribe_pre_post", True, True, False)
    if got != PHASE_EXPECTED["subscribe_pre_post"]:
        errors.append(f"subscribe mismatch {got}")
    got = phase_after("remove_pre", False, True, False)
    if got != PHASE_EXPECTED["remove_pre"]:
        errors.append(f"remove_pre mismatch {got}")
    got = phase_after("remove_post", True, False, False)
    if got != PHASE_EXPECTED["remove_post"]:
        errors.append(f"remove_post mismatch {got}")
    got = phase_after("self_unsubscribe", True, True, True)
    if got != PHASE_EXPECTED["self_unsubscribe"]:
        errors.append(f"self_unsubscribe mismatch {got}")
    got = phase_after("final_unsubscribe", False, False, False)
    if got != PHASE_EXPECTED["final_unsubscribe"]:
        errors.append(f"final mismatch {got}")

    v = filter_verdict(True, True, 1, 0)
    if v["js_hook_a_delivered"][0] != "pass" or v["js_hook_b_filtered"][0] != "pass":
        errors.append("happy-path filter")
    v = filter_verdict(True, True, 1, 1)
    if v["js_hook_b_filtered"][0] != "fail":
        errors.append("B delivered must fail filter")
    v = filter_verdict(True, True, 0, 0)
    if v["js_hook_a_delivered"][0] != "fail":
        errors.append("registration failure must fail")
    v = filter_verdict(False, False, 0, 0)
    if v["native_spawn_a_ok"][0] != "pending":
        errors.append("missing spawn must pending, not pass")

    st, _, _ = reuse_verdict(5, 3, None, None, 0, 64)
    if st != "pending":
        errors.append("unachieved reuse must pending")
    st, _, _ = reuse_verdict(5, 3, 5, 9, 1, 4)
    if st != "fail":
        errors.append("stale delivery must fail")
    st, _, _ = reuse_verdict(5, 3, 5, 9, 0, 4)
    if st != "pass":
        errors.append("reuse happy path")

    st, _, _ = stale_post_map_record(4, True, 4)
    if st != "fail":
        errors.append("stale post-map counter must fail")
    st, _, _ = stale_post_map_record(4, False, 0)
    if st != "pending":
        errors.append("no map change must pending")

    if cleanup_reprepare_leaked(1, 2):
        errors.append("leaked reprepare must not look clean")
    if not cleanup_reprepare_leaked(1, 1):
        errors.append("clean reprepare")

    row = missing_actor_human_row()
    if row["actors"]:
        errors.append("missing-actor fixture must have empty actors")
    return errors


if __name__ == "__main__":
    errs = selftest()
    if errs:
        raise SystemExit("r6_state_machine selftest failed: " + "; ".join(errs))
    print("ok: r6_state_machine selftest")
