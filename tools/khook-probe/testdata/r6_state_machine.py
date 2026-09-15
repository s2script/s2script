#!/usr/bin/env python3
"""Host transition spec for R6 stateful SDKHooks / human-assisted cases.

These are legacy synthetic parser inputs, not an executable engine fixture.
Behavioral regressions execute plugin.ts and plugin.cpp in fixture.test.mjs and
native_fixture_test.py; the shared native original observer has a C++ test.
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


def stale_post_map_record(
    pre_map_count: int,
    post_map: bool,
    post_map_count: int,
    post_map_invoke_attempted: bool = False,
) -> Tuple[str, dict, str]:
    """A cached pre-map counter cannot satisfy a post-map assertion.

    Zero post-map callbacks without a live EntByIndex invoke after changelevel
    is pending, not pass (that is "we stopped touching").
    """
    if not post_map:
        return (
            "pending",
            {"cleared": True},
            "need operator changelevel while this run stays prepared; then collect again",
        )
    if not post_map_invoke_attempted:
        return (
            "pending",
            {"cleared": True},
            "need post-map Touch invoke via live EntByIndex of a remaining trigger_push "
            "(not whatever now occupies the saved index); if no live trigger remains, pending",
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


def js_phase_expected(action: str) -> dict:
    """JS may snapshot PRE/POST only. Native owns original. Do not fabricate original:1."""
    if action == "self_unsubscribe":
        return {"first_pre": 1, "first_post": 1, "second_pre": 0, "second_post": 1}
    native = dict(PHASE_EXPECTED[action])
    return {k: v for k, v in native.items() if not k.startswith("original")}


def layout_verdict(has_tx_ent: bool, first_fire_on_tx_ent: bool, client_int_ok: bool) -> Tuple[str, dict, str]:
    """Layout is the first CheckTransmit fire that inspects the case entity bitvec."""
    exp = {"layout_ok": True}
    if not has_tx_ent or not first_fire_on_tx_ent:
        return (
            "pending",
            exp,
            "need a networked visible entity in both clients' PVS (not logic_relay); "
            "layout is first fire of that entity, not any CheckTransmit int32 @576",
        )
    if client_int_ok:
        return ("pass", exp, "first-fire layout on transmit entity")
    return ("fail", exp, "CheckTransmitInfo client int @576 out of range on transmit-entity fire")


def filter_invokes_for_frames(frames: int) -> int:
    """Filter A and B are invoked once, not once per GameFrame."""
    return 1 if frames >= 1 else 0


def post_map_invoke_allowed(classname: str, vtable_is_touch: bool) -> bool:
    """Do not call CTriggerPush::Touch on a non-trigger occupant of a reused index."""
    return classname == "trigger_push" or vtable_is_touch


def post_map_invokes_for_frames(frames: int, map_ended_at: int = 0) -> int:
    """One post-map attempt, not one Touch per remaining GameFrame."""
    if frames <= map_ended_at:
        return 0
    return 1


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
        "script_hot_reload": (
            "need .s2sp reload while native s2script/probe stay loaded "
            "and matching before/after callback/resource observations"
        ),
        "map_change": "need operator changelevel while this run stays prepared; then collect again",
        "map_invoke": (
            "need post-map Touch invoke via live EntByIndex of a remaining trigger_push "
            "(not whatever now occupies the saved index); if no live trigger remains, pending"
        ),
        "slot_reuse": "slot reuse not achieved within bounded attempts; not a false pass",
        "reload_delivery": "fresh SDKHook registered; need one Touch delivery on the new subscription",
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
    v = filter_verdict(True, True, 48, 0)
    if v["js_hook_a_delivered"][0] != "fail":
        errors.append("48-frame hammer must not pass A=1")
    if filter_invokes_for_frames(48) != 1:
        errors.append("filter must be invoked once, not per GameFrame")
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

    st, _, _ = stale_post_map_record(4, True, 4, True)
    if st != "fail":
        errors.append("stale post-map counter must fail")
    st, _, _ = stale_post_map_record(4, False, 0)
    if st != "pending":
        errors.append("no map change must pending")
    st, _, _ = stale_post_map_record(4, True, 0, False)
    if st != "pending":
        errors.append("zero post-map without invoke must pending, not pass")
    st, _, _ = stale_post_map_record(4, True, 0, True)
    if st != "pass":
        errors.append("post-map invoke silent is pass")
    if post_map_invoke_allowed("worldspawn", False) or post_map_invoke_allowed("player", False):
        errors.append("must not Touch world/pawn occupants after changelevel")
    if not post_map_invoke_allowed("trigger_push", False):
        errors.append("remaining trigger_push must be invokable")
    if not post_map_invoke_allowed("", True):
        errors.append("vtable-matched Touch is an allowed gate")
    if post_map_invokes_for_frames(48, map_ended_at=5) != 1:
        errors.append("post-map Touch must run once, not every GameFrame")

    js_sub = js_phase_expected("subscribe_pre_post")
    if "original" in js_sub or js_sub != {"pre": 1, "post": 1}:
        errors.append("JS phase must not fabricate original")
    if "original" in js_phase_expected("final_unsubscribe"):
        errors.append("JS final must not include original")

    st, _, _ = layout_verdict(False, False, True)
    if st != "pending":
        errors.append("layout without transmit entity must pending")
    st, _, _ = layout_verdict(True, True, True)
    if st != "pass":
        errors.append("layout on transmit-entity first fire")

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
