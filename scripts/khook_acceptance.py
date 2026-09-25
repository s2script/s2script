#!/usr/bin/env python3
"""Source-bound KHook A/B/C acceptance controller, registry, and judge.

This module is the frozen R5/R6 contract. It does not implement probe or JS
fixture bodies. JSON examples here are the executable fixtures imported by
``scripts/test-khook-acceptance.py`` (``EXAMPLE_RECORD`` /
``EXAMPLE_HUMAN_OBSERVATIONS``). They use ``result=pending``. They are not live
passes.

Command syntax (shell entry point)
---------------------------------
::

    bash scripts/test-khook-live.sh --self-test
    bash scripts/test-khook-live.sh A --from-file FILE --identity runtime-identity.json
    bash scripts/test-khook-live.sh A --prepare --run-dir build/khook-acceptance/run-001 \\
      --identity runtime-identity.json
    bash scripts/test-khook-live.sh A --collect --run-dir build/khook-acceptance/run-001
    bash scripts/test-khook-live.sh A --judge --run-dir build/khook-acceptance/run-001 \\
      --observations build/khook-acceptance/run-001/human.json

``run-001`` is a directory, not the identity. Prepare writes a unique ``run_id``
plus build/revision/host/server identity into that directory. Collect advances
documented actions and appends observations without resetting the run or
re-sending prepare. Judge is read-only. B/C distinguish controlled mechanics from main-runtime delivery.

The independently verified operator receipt supplied by ``--identity`` records
the installed shim/core/probe/fixture paths and SHA256 hashes, s2script commit and
build hash, verified host manifest digest, fixture revision, observed server build,
initial map, server endpoint, verification timestamp and evidence capture hash.
``source_revision`` is the target revision; it is never an installed binary hash.
The receipt uses ``schema=1``, ``kind=khook-runtime-identity`` and this run's
``run_id``. ``artifact_identity`` is SHA256 of sorted compact JSON for the receipt
with only its own digest removed. ``s2script_build_hash`` is SHA256 of sorted compact
JSON for ``{core: CORE_SHA256, shim: SHIM_SHA256}``. The receipt is unsigned operator
evidence; its generator must measure the installed runtime rather than the checkout.
Initial map remains fixed while lifecycle observations record subsequent maps.

Absent receipt or record binding stays pending. A malformed or mismatched receipt
is invalid. Supply a receipt on a later collect to bind an existing pending run;
the controller sends ``bind RUN_ID ARTIFACT_IDENTITY`` without resetting fixtures.
Each record, including the human envelope, must carry ``artifact_identity``.
Every raw response has a unique capture file and is appended verbatim to
``records.jsonl``. Offline replay of that stream uses the live parser and judge.
Identical reports are idempotent, pending may complete, terminal failures remain
visible, and contradictory terminal records are invalid. Controller input errors
are retained as ``{khook_acceptance_error: REASON}`` in the replay stream.

Native/JS command protocol (resident native shim and probe)
----------------------------------------------------------------
::

    s2_khook_probe prepare <run_id> [artifact_identity]
    s2_khook_probe bind <run_id> <artifact_identity>
    s2_khook_probe collect <run_id>
    s2_khook_probe report <run_id>
    s2_khook_accept prepare <run_id> [artifact_identity]
    s2_khook_accept bind <run_id> <artifact_identity>
    s2_khook_accept collect <run_id>
    s2_khook_accept report <run_id>
    s2_khook_accept teardown <run_id>
    s2_khook_probe reload-arm <run_id>
    s2_khook_accept reload-arm <run_id>
    s2_khook_accept resume <run_id> <artifact_identity>

Unknown or mismatched ``run_id`` values are invalid evidence. Never silently
reuse a prior run. Run IDs are 1-64 ASCII letters/digits, underscores or hyphens,
starting with a letter/digit. ``report`` is read-only.

Finish and collect all other stages before arming script reload. Reload only the
`.s2sp` fixture, then resume its matching state handoff. `script_hot_reload` requires
observed old/new generation callbacks and resource retirement on the same resident
native target; neither a controller reset nor native reload supplies that evidence.
Native updates use a server restart and a fresh acceptance run.

Record schema (schema=1)
-------------------------
One JSON object per record. Typed ``expected`` / ``actual``, nonempty
``evidence`` for pass/fail, one terminal ``result``. Example (pending)::

    {
      "schema": 1,
      "suite": "A",
      "run_id": "khook-a-test-0001",
      "source_revision": "d66d7bd45721b3be2a44da358f33b6cef1b5d594",
      "artifact_identity": "",
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
import hashlib
import json
import re
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Sequence, Tuple

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

UNAVAILABLE_SUITES: Dict[str, str] = {}  # Compatibility alias; every suite is registered.

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
    _sc("entity_slot_reuse_map_teardown", "script_hot_reload", "native"),
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


@dataclass(frozen=True)
class EvidenceRule:
    group: str
    provenance: str
    callback_owner: str
    target: str
    join: str = ""


@dataclass(frozen=True)
class SuiteSpec:
    cases: Tuple[str, ...]
    subchecks: Tuple[Subcheck, ...]
    client_actions: Dict[str, str]
    rules: Dict[Tuple[str, str, str], EvidenceRule] = field(default_factory=dict)


# Controlled callback mechanics and main-runtime delivery are separate proof
# groups. A private production-TU copy must never supply a main-runtime join.
_B_ROWS = {
    "declarative_this_void": (
        "native_this_void_continue_original_once native_this_void_handled_original_zero native_this_void_peer_orders",
        "js_this_void_continue_delivery js_this_void_handled_delivery"),
    "declarative_mutable_narrow": (
        "native_narrow_edits_reach_original native_narrow_peer_orders", "js_narrow_all_fields_mutated"),
    "declarative_mutable_wide": (
        "native_wide_edits_reach_original native_wide_opaque_values_preserved native_wide_peer_orders", "js_wide_mutation_delivery"),
    "declarative_nesting_bypass": (
        "native_same_and_different_id_nesting native_stale_forged_views_rejected native_bypass_hit_then_next_delivered native_rejected_nested_scope_restored",
        "js_different_id_nested_delivery js_same_id_reentry_named_skip js_bypass_absent_then_next_delivered"),
    "damage_named_hook": (
        "native_damage_valid_pre_post native_damage_nested_scopes native_damage_peer_orders_original_state", "js_damage_pre_post_correct_victim"),
    "chat_named_hook": (
        "native_chat_continue_original_once native_chat_suppressed_original_zero native_chat_peer_orders", "js_chat_continue_delivery js_chat_suppression_vote"),
    "output_named_hook": (
        "native_output_01_original_once native_output_23_original_zero native_output_peer_orders", "js_output_delivery_and_suppression"),
    "usercmd_named_hook": (
        "native_usercmd_abi_batch_mutation native_usercmd_original_once_return_preserved native_usercmd_peer_orders", "js_usercmd_batch_delivery_neutralization"),
    "script_generation_lifetime": (
        "native_binding_resident_across_reload native_no_disposed_generation_callback", "js_old_generation_retired js_new_generation_callback"),
    "binding_peer_order_retirement": (
        "native_all_sites_both_peer_orders native_active_removal_refused native_async_removal_peer_survives", ""),
}
_C_ROWS = {
    "precache_peer_slot_original_once": (
        "native_precache_real_receiver native_precache_other_vtable_filtered native_precache_peer_same_slot_both_orders native_precache_original_once native_precache_checked_retirement", ""),
    "precache_manifest_nesting": (
        "native_precache_outer_inner_outer_manifest native_precache_dispatch_once_each native_precache_expired_manifest_rejected", ""),
    "precache_map_transition": (
        "native_precache_before_after_map_callback native_precache_map_generation_receiver native_precache_live_peer_both_orders",
        "js_precache_before_after_map_delivery js_precache_resource_each_generation js_precache_stale_context_rejected"),
}


# Frozen outcomes: a producer cannot choose {ok: true} or make its own failed
# actual the expected result. Raw per-invocation facts remain alongside these
# aggregate assertions; native owns original counts, JS owns handler/add returns.
INTEGRATION_EXPECTED = {
    "native_this_void_continue_original_once": {"pre": 1, "original": 1},
    "native_this_void_handled_original_zero": {"pre": 1, "original": 0, "skipped": True},
    "native_narrow_edits_reach_original": {"original": 1, "value": 7.25, "a": -17, "b": 29, "c": -31},
    "native_wide_edits_reach_original": {"original": 1, "value": 7.25, "integer": -17},
    "native_wide_opaque_values_preserved": {"opaque_a": "17375808098319191535", "opaque_b": "9305357566071262703"},
    "native_same_and_different_id_nesting": {"same_restored": 2, "different_restored": 2},
    "native_stale_forged_views_rejected": {"stale_rejected": True, "forged_rejected": True},
    "native_bypass_hit_then_next_delivered": {"bypass_pre": 0, "next_pre": 1, "original": 2},
    "native_rejected_nested_scope_restored": {"rejected": True, "outer_restored": True},
    "native_damage_valid_pre_post": {"pre": 3, "post": 3, "original": 3, "arguments": 3,
        "result_null": 1, "result_nonnull": 2, "output_writes": 2, "output_preserved": 2,
        "pre_ignore": 3, "post_ignore": 3, "peer_post": 3, "skipped": 0, "return_kind": "void"},
    "native_damage_nested_scopes": {"restored": 2, "expired": True},
    "native_damage_peer_orders_original_state": {"orders": ["peer-first", "s2script-first"], "arguments_preserved": True, "output_preserved": True, "skipped": False, "return_kind": "void"},
    "native_chat_continue_original_once": {"dispatch": 1, "original": 1, "skipped": False},
    "native_chat_suppressed_original_zero": {"dispatch": 1, "original": 0, "skipped": True},
    "native_output_01_original_once": {"actions": [0, 1], "originals": [1, 1], "dispatches": [1, 1], "skipped": [False, False]},
    "native_output_23_original_zero": {"actions": [2, 3], "originals": [0, 0], "dispatches": [1, 1], "skipped": [True, True]},
    "native_usercmd_abi_batch_mutation": {"batch_delivered": True, "neutralized": True, "arguments_preserved": True},
    "native_usercmd_original_once_return_preserved": {"originals_by_batch": [1, 1, 1], "return": 37},
    "native_binding_resident_across_reload": {"native_address_same": True, "generations": 3},
    "native_no_disposed_generation_callback": {"old_callbacks_after_retire": 0, "new_callbacks": 2},
    "native_all_sites_both_peer_orders": {"sites": ["this_void", "narrow", "wide", "damage", "chat", "output", "usercmd", "precache"], "orders": ["peer-first", "s2script-first"]},
    "native_active_removal_refused": {"active_refused": True},
    "native_async_removal_peer_survives": {"completion_observed": True, "retired_callbacks": 0, "peer_callbacks": 1},
    "native_precache_real_receiver": {"delivered_receiver_matches": True, "retained_vtable_matches": True},
    "native_precache_other_vtable_filtered": {"dispatch": 0, "original": 1},
    "native_precache_original_once": {"dispatch": 2, "original": 2},
    "native_precache_checked_retirement": {"active_refused": True, "completion_observed": True, "peer_survived": True},
    "native_precache_outer_inner_outer_manifest": {"trace": ["outer", "inner", "outer"], "restored": True},
    "native_precache_dispatch_once_each": {"dispatch": 2, "original": 2},
    "native_precache_expired_manifest_rejected": {"expired": True},
    "native_precache_before_after_map_callback": {"virtual_callbacks_before": True, "virtual_callbacks_after": True},
    "native_precache_map_generation_receiver": {"distinct_generations": True, "receiver_vtable_manifest_observed": True},
    "js_this_void_continue_delivery": {"pre": 1},
    "js_this_void_handled_delivery": {"pre": 1, "action": 2},
    "js_narrow_all_fields_mutated": {"value": 7.25, "a": -17, "b": 29, "c": -31},
    "js_wide_mutation_delivery": {"value": 7.25, "integer": -17},
    "js_different_id_nested_delivery": {"outer": 1, "inner": 1, "restored": True},
    "js_same_id_reentry_named_skip": {"delivered": 1, "nested_safe_skip": True},
    "js_bypass_absent_then_next_delivered": {"bypass": 0, "next": 1},
    "js_damage_pre_post_correct_victim": {"pre": 1, "post": 1, "victim_matches": True},
    "js_chat_continue_delivery": {"continue": 1},
    "js_chat_suppression_vote": {"suppressed": 1, "action": 2},
    "js_output_delivery_and_suppression": {"actions": [0, 1, 2, 3], "deliveries": 4},
    "js_usercmd_batch_delivery_neutralization": {"delivered": True, "neutralized": True},
    "js_old_generation_retired": {"generations_retired": 2},
    "js_new_generation_callback": {"new_generations_delivered": 2},
    "js_precache_before_after_map_delivery": {"virtual_before": True, "virtual_after": True},
    "js_precache_resource_each_generation": {"added_before": True, "added_after": True},
    "js_precache_stale_context_rejected": {"stale_add": False},
}
for _suite_rows in (_B_ROWS, _C_ROWS):
    for _native_names, _js_names in _suite_rows.values():
        for _name in _native_names.split():
            if _name not in INTEGRATION_EXPECTED and ("peer_orders" in _name or "both_orders" in _name):
                INTEGRATION_EXPECTED[_name] = {"orders": ["peer-first", "s2script-first"]}
        for _name in _js_names.split():
            INTEGRATION_EXPECTED["native_main_" + _name[3:]] = {"same_invocation_markers": True, "target_calls_observed": True}

# Coverage-summary sites driven through the main runtime bridge; the rest are stock-provider peers.
MAIN_RUNTIME_SITES = ("this_void", "narrow", "wide")

INTEGRATION_EXPECTED["native_main_bypass_absent_then_next_delivered"] = {
    "same_invocation_markers": True, "target_calls_observed": True, "both_phases_observed": True,
}


def _integration_spec(rows: dict, suite: str) -> SuiteSpec:
    checks = []
    rules = {}
    for case, (native, js) in rows.items():
        for producer, names in (("native", native), ("js", js)):
            for name in names.split():
                main = case.startswith("declarative") and producer == "js"
                main_mechanics = producer == "native" and case in (
                    "declarative_this_void", "declarative_mutable_narrow", "declarative_mutable_wide") and name != "native_wide_opaque_values_preserved"
                real = suite == "C" and case == "precache_map_transition"
                live_named = case.endswith("named_hook") and producer == "js"
                lifetime = case == "script_generation_lifetime"
                group = "main-runtime-bridge" if main or main_mechanics or real or lifetime else "live-named" if live_named else "controlled-mechanics"
                provenance = "live-engine" if real or live_named else "main-runtime" if main or main_mechanics or lifetime else "controlled-stock-provider"
                owner = "named_hooks" if suite == "C" or case.endswith("named_hook") else "engine_hooks"
                join = "script-generation" if lifetime else name[3:] if main else "precache-map" if real and name != "native_precache_live_peer_both_orders" and name != "js_precache_stale_context_rejected" else ""
                if name == "native_all_sites_both_peer_orders":
                    group, provenance, owner = "coverage-summary", "mixed-observed", "stock-khook"
                check = _sc(case, name, producer)
                checks.append(check)
                rules[(case, name, producer)] = EvidenceRule(group, provenance, owner, case, join)
                if main:
                    # SAME scenario native target original/marker observations,
                    # never the independently invoked controlled snapshot.
                    native_name = "native_main_" + name[3:]
                    checks.append(_sc(case, native_name, "native"))
                    rules[(case, native_name, "native")] = rules[(case, name, producer)]
    actions = {"chat_named_hook": "a real client sends run-bound continue and suppress chat tokens",
               "usercmd_named_hook": "a real connected client supplies usercmd input"} if suite == "B" else {}
    return SuiteSpec(tuple(rows), tuple(checks), actions, rules)


SUITES = {"A": SuiteSpec(SUITE_A_CASES, SUBCHECKS, CLIENT_ACTIONS),
          "B": _integration_spec(_B_ROWS, "B"), "C": _integration_spec(_C_ROWS, "C")}
SUITE_B_CASES = SUITES["B"].cases
SUITE_C_CASES = SUITES["C"].cases


def required_subchecks(suite: str = "A") -> List[Subcheck]:
    return list(SUITES[suite].subchecks)


GAMEDATA_SUBCHECKS = {"native_precache_live_peer_both_orders"}


def _fnv1a64(data: bytes) -> str:
    value = 14695981039346656037
    for byte in data:
        value = ((value ^ byte) * 1099511628211) & ((1 << 64) - 1)
    return format(value, "x")


def capture_gamedata_inputs(native: dict, identity: dict) -> dict:
    """Hash the actual native-inspected inputs on the controller's server filesystem.

    An unavailable remote path is pending, never a checkout-file substitution.
    The native fingerprint is diagnostic; SHA-256 is computed here from bytes.
    """
    result = {"run_id": identity["run_id"], "artifact_identity": identity.get("runtime_identity", {}).get("artifact_identity"), "files": []}
    if native.get("run_id") != result["run_id"] or native.get("artifact_identity") != result["artifact_identity"]:
        return dict(result, status="fail", reason="native gamedata identity mismatch")
    if not native.get("prepared"):
        return dict(result, status="pending", reason="native deployed gamedata preparation unavailable")
    if not native.get("unchanged"):
        return dict(result, status="fail", reason="native retained gamedata bytes changed")
    mapped_root = identity.get("gamedata_root")
    if not isinstance(mapped_root, str) or not mapped_root:
        return dict(result, status="pending", reason="explicit --gamedata-root host mapping required")
    result["mapped_root"] = mapped_root
    files = native.get("files")
    if not isinstance(files, list) or not files:
        return dict(result, status="pending", reason="native effective input list unavailable")
    try:
        root = Path(native["addon_root"]) / "gamedata"
        local_root = Path(mapped_root).resolve(strict=True)
        for entry in files:
            native_path = Path(entry["path"])
            if not native_path.is_absolute() or root not in native_path.parents:
                return dict(result, status="fail", reason="gamedata input outside measured addon tree")
            relative = native_path.relative_to(root)
            if ".." in relative.parts:
                return dict(result, status="fail", reason="gamedata path traversal")
            path = (local_root / relative).resolve(strict=True)
            if local_root not in path.parents:
                return dict(result, status="fail", reason="gamedata symlink escapes mapped root")
            before = path.stat()
            data = path.read_bytes()
            after = path.stat()
            if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
                return dict(result, status="fail", reason="gamedata input changed while hashing")
            fingerprint = _fnv1a64(data)
            if entry.get("size") != len(data) or entry.get("fingerprint_algorithm") != "fnv1a64-diagnostic" or entry.get("fingerprint") != fingerprint:
                return dict(result, status="fail", reason="controller bytes differ from retained native input")
            result["files"].append(dict(path=str(native_path), host_path=str(path), size=len(data), sha256=hashlib.sha256(data).hexdigest(), fingerprint=fingerprint))
    except (OSError, KeyError, TypeError, ValueError) as error:
        return dict(result, status="pending", reason="installed gamedata files unavailable on controller host: " + str(error))
    return dict(result, status="pass")


def _gamedata_provenance(rec: dict, identity: dict) -> Tuple[str, str]:
    native = rec.get("gamedata")
    if not isinstance(native, dict) or not native.get("prepared"):
        return "pending", "native effective gamedata provenance unavailable"
    if not native.get("unchanged") or native.get("run_id") != rec.get("run_id") or native.get("artifact_identity") != rec.get("artifact_identity"):
        return "fail", "changed or mismatched native gamedata provenance"
    capture = identity.get("gamedata_capture", {})
    if not isinstance(capture, dict):
        return "fail", "malformed gamedata capture"
    phases = [capture.get(phase) for phase in ("before", "after")]
    for phase in phases:
        if not isinstance(phase, dict) or phase.get("status") == "pending":
            return "pending", "controller before/after gamedata SHA-256 capture required"
        if phase.get("status") != "pass" or phase.get("run_id") != rec.get("run_id") or phase.get("artifact_identity") != rec.get("artifact_identity"):
            return "fail", "failed or mismatched controller gamedata capture"
    before, after = phases
    for files in (native.get("files"), before.get("files"), after.get("files")):
        if not isinstance(files, list) or not files or any(not isinstance(entry, dict) or not isinstance(entry.get("path"), str) or not Path(entry["path"]).is_absolute() or type(entry.get("size")) is not int or entry["size"] < 0 for entry in files):
            return "fail", "malformed gamedata file inventory"
        if len({entry["path"] for entry in files}) != len(files):
            return "fail", "duplicate gamedata input path"
    if _canonical(before["files"]) != _canonical(after["files"]):
        return "fail", "deployed gamedata hashes changed across capture"
    expected = {entry["path"]: entry for entry in native.get("files", [])}
    measured = {entry["path"]: entry for entry in before["files"]}
    if not expected or expected.keys() != measured.keys():
        return "fail", "effective gamedata input inventory differs"
    for path, entry in measured.items():
        if not _hex(entry.get("sha256"), 64) or entry.get("size") != expected[path].get("size") or entry.get("fingerprint") != expected[path].get("fingerprint"):
            return "fail", "effective gamedata bytes/hash provenance differs"
    return "pass", ""


def _registered(suite: str = "A") -> Dict[Tuple[str, str, str], Subcheck]:
    return {(s.case, s.subcheck, s.producer): s for s in required_subchecks(suite)}


def _integration_record_error(rec: dict, rule: EvidenceRule) -> Optional[str]:
    if rec.get("result") == "pending":
        return None
    if _canonical(rec.get("expected")) != _canonical(INTEGRATION_EXPECTED[rec["subcheck"]]):
        return "expected outcome differs from frozen subcheck contract"
    if rec.get("evidence_class") != "observed":
        return "synthetic or unclassified evidence is not live acceptance"
    for name in ("group", "provenance", "callback_owner", "target"):
        if rec.get(name) != getattr(rule, name):
            return f"wrong {name}; expected {getattr(rule, name)}"
    observations = rec.get("observations")
    if not isinstance(observations, list) or not observations:
        return "observed callback scenarios required (registration alone is insufficient)"
    seen = set()
    for obs in observations:
        if not isinstance(obs, dict):
            return "malformed observation"
        for name in ("scenario_id", "invocation", "peer_order"):
            if not isinstance(obs.get(name), str) or not obs[name]:
                return f"observation requires {name}"
        for name in ("sequence", "generation", "callbacks"):
            if type(obs.get(name)) is not int or obs[name] <= 0:
                return f"observation requires positive {name}"
        if obs["peer_order"] not in ("peer-first", "s2script-first", "none"):
            return "unsupported observed peer_order"
        key = (obs["scenario_id"], obs["sequence"], obs["generation"])
        if key in seen:
            return "duplicate invocation scenario"
        seen.add(key)
        if not isinstance(obs.get("facts"), dict) or not obs["facts"]:
            return "typed callback facts required"
        if rec["subcheck"] == "native_main_bypass_absent_then_next_delivered" and rec.get("result") == "pass":
            for name, expected in dict(bypass_original=1, bypass_peer_pre=1, bypass_peer_post=1,
                    bypass_js=0, next_original=1, next_peer_pre=1, next_peer_post=1, next_js=1).items():
                if type(obs["facts"].get(name)) is not int or obs["facts"][name] != expected:
                    return f"bypass requires exact measured phase counter {name}={expected}"
        if rule.provenance == "live-engine" and obs.get("stimulus") not in ("engine", "real-client"):
            return "real engine/client stimulus required; synthetic/selftest callback rejected"
        if rule.target == "precache_map_transition":
            if obs.get("route") != "main-virtual-precache":
                return "actual main virtual precache frame required"
            for name in ("frame_token", "map_generation"):
                if type(obs.get(name)) is not int or obs[name] <= 0:
                    return f"precache requires positive {name}"
            for name in ("receiver", "vtable", "manifest"):
                if not isinstance(obs.get(name), str) or not obs[name]:
                    return f"precache requires opaque {name} label"
    if rule.target == "script_generation_lifetime" and len({o["generation"] for o in observations}) != 3:
        return "three actual script generations required"
    if rec["subcheck"] == "native_all_sites_both_peer_orders":
        expected_sites = INTEGRATION_EXPECTED[rec["subcheck"]]["sites"]
        measured = []
        for observation in observations:
            facts = observation["facts"]
            site = facts.get("site")
            if site not in expected_sites:
                return "coverage summary requires a known observed site"
            origin = "main-runtime" if site in MAIN_RUNTIME_SITES else "controlled-stock-provider"
            if facts.get("origin") != origin:
                return "coverage summary site ownership mismatch"
            measured.append((site, observation["peer_order"]))
        wanted = {(site, order) for site in expected_sites for order in ("peer-first", "s2script-first")}
        if len(measured) != len(wanted) or set(measured) != wanted:
            return "coverage summary requires each site in both observed registration orders"
    if "peer_orders" in rec["subcheck"] or "both_orders" in rec["subcheck"]:
        if {o["peer_order"] for o in observations} != {"peer-first", "s2script-first"}:
            return "both observed peer orders required"
    if rule.target == "precache_map_transition" and "stale_context" not in rec["subcheck"]:
        if len({o["map_generation"] for o in observations}) < 2:
            return "callbacks in two actual map generations required"
    return None


def _join_errors(records: Sequence[dict], spec: SuiteSpec) -> List[str]:
    groups: Dict[Tuple[str, str], Dict[str, list]] = {}
    for rec in records:
        key = (rec.get("case"), rec.get("subcheck"), rec.get("producer"))
        rule = spec.rules.get(key)
        if rule and rule.join and rec.get("result") == "pass" and rec.get("artifact_identity"):
            groups.setdefault((key[0], rule.join), {}).setdefault(key[2], []).append(rec)
    errors = []
    for (case, join), producers in groups.items():
        if set(producers) != {"native", "js"}:
            continue  # Missing counterpart already remains a required pending row.
        def signature(rec):
            return sorted(_canonical({k: o.get(k) for k in
                ("scenario_id", "sequence", "generation", "invocation", "peer_order",
                 "frame_token", "map_generation", "receiver", "vtable", "manifest")})
                for o in rec.get("observations", []))
        signatures = { _canonical(signature(rec)) for rows in producers.values() for rec in rows }
        if len(signatures) != 1:
            errors.append(f"{case}/{join}: cross-producer invocation/scenario/sequence/generation mismatch")
    return errors


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
    "artifact_identity": "",
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
    "artifact_identity": "",
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


def valid_run_id(value: Any) -> bool:
    """One bounded RCON argument and safe filename component."""
    return isinstance(value, str) and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}", value) is not None


def probe_cmd(verb: str, run_id: str, artifact_identity: Optional[str] = None, *, suite: str = "A") -> str:
    if suite not in SUITES:
        raise ValueError("unsupported suite " + suite)
    if artifact_identity is not None and not _hex(artifact_identity, 64):
        raise ValueError("invalid artifact_identity")
    if not valid_run_id(run_id):
        raise ValueError("invalid run_id: expected 1-64 ASCII letters/digits, underscores or hyphens")
    if verb not in ("prepare", "collect", "report", "bind", "resume", "reload-arm", "gamedata"):
        raise ValueError(verb)
    return f"s2_khook_probe {verb} {run_id}" + (f" {artifact_identity}" if artifact_identity else "") + (f" {suite}" if suite != "A" else "")


def accept_cmd(verb: str, run_id: str, artifact_identity: Optional[str] = None, *, suite: str = "A") -> str:
    if suite not in SUITES:
        raise ValueError("unsupported suite " + suite)
    if artifact_identity is not None and not _hex(artifact_identity, 64):
        raise ValueError("invalid artifact_identity")
    if not valid_run_id(run_id):
        raise ValueError("invalid run_id: expected 1-64 ASCII letters/digits, underscores or hyphens")
    if verb not in ("prepare", "collect", "report", "teardown", "bind", "restore", "resume", "reload-arm"):
        raise ValueError(verb)
    return f"s2_khook_accept {verb} {run_id}" + (f" {artifact_identity}" if artifact_identity else "") + (f" {suite}" if suite != "A" else "")


def new_run_id(suite: str = "A") -> str:
    if suite not in SUITES:
        raise ValueError("unsupported suite " + suite)
    return "khook-" + suite.lower() + "-" + uuid.uuid4().hex


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


def _json_loads(text: str) -> Any:
    def object_pairs(pairs):
        obj = {}
        for key, value in pairs:
            if key in obj:
                raise ValueError(f"duplicate JSON field {key}")
            obj[key] = value
        return obj
    return json.loads(text, object_pairs_hook=object_pairs)


def runtime_identity_digest(receipt: dict) -> str:
    """Digest of the frozen operator receipt, excluding its own digest field."""
    payload = {key: value for key, value in receipt.items() if key != "artifact_identity"}
    return hashlib.sha256(_canonical(payload).encode("utf-8")).hexdigest()


def _hex(value: Any, length: int) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{" + str(length) + r"}", value) is not None


def _timestamp(value: Any) -> bool:
    if not isinstance(value, str):
        return False
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00")).tzinfo is not None
    except ValueError:
        return False


def _read_identity(receipt: Any) -> Any:
    if isinstance(receipt, (str, Path)):
        return _json_loads(Path(receipt).read_text())
    return receipt


def validate_runtime_identity(receipt: Any, identity: dict) -> List[str]:
    """Validate an independently verified installed-runtime receipt.

    The generator/qualified operator measures the installed files and running
    server. This unsigned receipt is not an adversarial attestation. The judge
    checks its complete contents, digest and run/server/source binding; it never
    derives an installed binary identity from the controller checkout.
    """
    if not isinstance(receipt, dict):
        return ["malformed runtime identity: expected receipt object"]
    errors = []
    if type(receipt.get("schema")) is not int or receipt["schema"] != SCHEMA:
        errors.append("unsupported runtime identity schema")
    if receipt.get("kind") != "khook-runtime-identity":
        errors.append("unsupported runtime identity kind")
    for name in ("source_revision", "s2script_commit", "fixture_revision"):
        if not _hex(receipt.get(name), 40):
            errors.append(f"runtime identity requires {name} SHA")
    for name in ("s2script_build_hash", "host_manifest_digest", "evidence_sha256", "artifact_identity"):
        if not _hex(receipt.get(name), 64):
            errors.append(f"runtime identity requires {name} SHA256")
    for name in ("run_id", "server", "server_build", "initial_map", "evidence_path"):
        if not isinstance(receipt.get(name), str) or not receipt[name].strip():
            errors.append(f"runtime identity requires {name}")
    if not valid_run_id(receipt.get("run_id")):
        errors.append("runtime identity requires command-safe run_id (1-64 ASCII letters/digits, underscores or hyphens)")
    if not _timestamp(receipt.get("verified_at")):
        errors.append("runtime identity requires verified_at timestamp with timezone")
    for name in ("run_id", "source_revision", "server"):
        if identity.get(name) is not None and receipt.get(name) != identity[name]:
            errors.append(f"wrong runtime identity {name}")
    artifacts = receipt.get("artifacts")
    if not isinstance(artifacts, dict):
        errors.append("runtime identity requires installed artifacts")
        artifacts = {}
    for name in ("shim", "core", "probe", "fixture"):
        artifact = artifacts.get(name)
        if not isinstance(artifact, dict):
            errors.append(f"runtime identity missing installed {name}")
            continue
        if not isinstance(artifact.get("path"), str) or not artifact["path"].startswith("/"):
            errors.append(f"runtime identity requires installed {name} absolute path")
        if not _hex(artifact.get("sha256"), 64):
            errors.append(f"runtime identity requires installed {name} SHA256")
    if not errors:
        build = {name: artifacts[name]["sha256"] for name in ("core", "shim")}
        if receipt["s2script_build_hash"] != hashlib.sha256(_canonical(build).encode()).hexdigest():
            errors.append("wrong runtime identity s2script_build_hash")
    if receipt.get("artifact_identity") != runtime_identity_digest(receipt):
        errors.append("wrong runtime identity artifact_identity digest")
    return errors


def _with_runtime_identity(meta: dict, supplied: Any) -> Tuple[dict, List[str]]:
    meta = dict(meta)
    if supplied is None:
        return meta, []
    try:
        receipt = _read_identity(supplied)
    except (OSError, ValueError) as error:
        return meta, [f"malformed runtime identity: {error}"]
    errors = validate_runtime_identity(receipt, meta)
    existing = meta.get("runtime_identity")
    if isinstance(existing, dict) and isinstance(receipt, dict):
        if existing.get("artifact_identity") != receipt.get("artifact_identity"):
            errors.append("runtime identity changed for already bound run")
    if errors:
        return meta, errors
    meta["runtime_identity"] = receipt
    for name in ("s2script_commit", "s2script_build_hash", "host_manifest_digest", "fixture_revision", "server_build"):
        meta[name] = receipt[name]
    meta["map"] = receipt["initial_map"]
    return meta, []


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
            obj = _json_loads(blob)
        except ValueError as error:
            errors.append(f"malformed JSON at line {lineno}: {error}")
            continue
        if not isinstance(obj, dict):
            errors.append(f"malformed JSON at line {lineno}: not an object")
            continue
        if "khook_acceptance_error" in obj:
            errors.append(f"recorded controller error: {obj['khook_acceptance_error']}")
            continue
        if "case" not in obj:
            continue
        recs.append(obj)
    return recs, errors


def _need_clients_messages(case_status: Dict[str, str], suite: str = "A") -> List[str]:
    out = []
    for case, status in case_status.items():
        if status != "pending":
            continue
        action = SUITES[suite].client_actions.get(case)
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


def _normalize_observations(observations: Any, identity: dict, suite: str = "A") -> Tuple[List[dict], List[str]]:
    if observations is None:
        return [], []
    if isinstance(observations, (str, Path)):
        path = Path(observations)
        try:
            observations = _json_loads(path.read_text())
        except (OSError, ValueError) as e:
            return [], [f"malformed observations: {e}"]
    if not isinstance(observations, dict):
        return [], ["malformed observations: expected object envelope"]
    errors: List[str] = []
    schema = observations.get("schema")
    if type(schema) is not int or schema != SCHEMA:
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
    registered = _registered(suite)
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
            **row,
            "schema": SCHEMA,
            "suite": suite,
            "run_id": identity.get("run_id"),
            "source_revision": identity.get("source_revision"),
            "artifact_identity": observations.get("artifact_identity"),
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
    if "expected" not in rec or rec["expected"] is None:
        return "invalid", "missing expected"
    if "actual" not in rec or rec["actual"] is None:
        return "invalid", "missing actual"
    evidence = rec.get("evidence")
    if result in ("pass", "fail"):
        if not isinstance(evidence, str) or not evidence.strip():
            return "invalid", "evidence nonempty required for pass/fail"
    if rec.get("producer") == "human" and result == "pass":
        for name in ("expected", "actual"):
            value = rec[name]
            if isinstance(value, (dict, list, str)) and not value:
                return "invalid", f"human pass requires nonempty {name} outcome"
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
    if suite not in SUITES:
        return JudgeResult(1, "fail", [f"unsupported suite {suite}"], {})
    spec = SUITES[suite]

    errors = list(parse_errors or [])
    receipt = identity.get("runtime_identity")
    missing_identity = receipt is None
    if not missing_identity:
        errors.extend(validate_runtime_identity(receipt, identity))
    artifact_identity = receipt.get("artifact_identity") if isinstance(receipt, dict) else None
    extra, obs_errors = _normalize_observations(observations, identity, suite)
    errors.extend(obs_errors)
    all_recs = list(records) + extra

    registered = _registered(suite)
    envelopes: Dict[str, List[dict]] = {c: [] for c in spec.cases}

    for rec in all_recs:
        if not isinstance(rec, dict):
            errors.append("malformed record: expected object")
            continue
        if type(rec.get("schema")) is not int or rec["schema"] != SCHEMA:
            errors.append(f"unsupported schema {rec.get('schema')!r}")
            continue
        if str(rec.get("suite") or "A").upper() != suite:
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
        binding = rec.get("artifact_identity")
        if binding not in (None, ""):
            if not _hex(binding, 64):
                errors.append("malformed record artifact_identity")
                continue
            if artifact_identity and binding != artifact_identity:
                errors.append("wrong record artifact_identity")
                continue
        if rec.get("kind") == "diagnostic" and rec.get("case") == "process_terminal" and rec.get("producer") == "native":
            verdict, reason = _subcheck_verdict(rec)
            if verdict == "invalid": errors.append(f"malformed terminal diagnostic: {reason}")
            else: messages.append(f"DIAGNOSTIC: process terminal result={rec.get('result')} actual={rec.get('actual')!r}; known shutdown-only139 is non-blocking by user disposition")
            continue
        case = rec.get("case")
        subcheck = rec.get("subcheck")
        producer = rec.get("producer")
        if case not in spec.cases:
            errors.append(f"unsupported case {case!r}")
            continue
        if producer not in PRODUCERS:
            errors.append(f"unsupported producer {producer!r}")
            continue
        key = (case, subcheck, producer)
        if not isinstance(subcheck, str):
            errors.append(f"malformed subcheck in {case}")
            continue
        if key not in registered:
            errors.append(f"unsupported subcheck {case}/{subcheck} producer={producer}")
            continue
        rule = spec.rules.get(key)
        if rule:
            reason = _integration_record_error(rec, rule)
            if reason:
                errors.append(f"{case}/{subcheck}: {reason}")
                continue
        envelopes[case].append(rec)

    errors.extend(_join_errors([r for rows in envelopes.values() for r in rows], spec))
    case_status: Dict[str, str] = {}
    missing = [c for c in spec.cases if not envelopes[c]]
    if missing:
        errors.append("missing case records: " + ", ".join(missing))

    for case in spec.cases:
        recs_for_case = envelopes[case]
        if not recs_for_case:
            case_status[case] = "invalid"
            continue
        required = [s for s in spec.subchecks if s.case == case]
        by_key: Dict[Tuple[str, str], List[dict]] = {}
        for rec in recs_for_case:
            by_key.setdefault((rec["producer"], rec["subcheck"]), []).append(rec)
        failed = []
        pending = []
        invalid_sc = []
        for sc in required:
            history = by_key.get((sc.producer, sc.subcheck), [])
            if not history:
                pending.append(sc.subcheck)
                continue
            terminal = None
            for rec in history:
                verdict, reason = _subcheck_verdict(rec)
                if verdict == "invalid":
                    invalid_sc.append(f"{sc.subcheck}: {reason}")
                    continue
                if verdict == "pending":
                    continue
                if verdict == "pass" and sc.subcheck in GAMEDATA_SUBCHECKS:
                    provenance, why = _gamedata_provenance(rec, identity)
                    if provenance == "pending":
                        messages.append("PENDING: " + sc.subcheck + ": " + why)
                        continue
                    if provenance == "fail":
                        invalid_sc.append(sc.subcheck + ": " + why)
                        continue
                if verdict == "pass" and (missing_identity or not rec.get("artifact_identity")):
                    continue
                # Identical reports may be replayed. A pending observation may
                # finish, but no later snapshot can erase terminal evidence.
                if terminal is not None and _canonical(rec) != _canonical(terminal):
                    invalid_sc.append(f"duplicate producer-subcheck {sc.producer}/{sc.subcheck}: contradictory terminal observations")
                terminal = rec
                if verdict == "fail":
                    failed.append((sc, rec, reason))
            if terminal is None:
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
    if missing_identity:
        messages.append("PENDING: installed runtime identity missing; supply --identity FILE from verified runtime artifacts")
    messages.extend(_need_clients_messages(case_status, suite))

    if errors:
        failed_n = sum(1 for v in case_status.values() if v == "fail")
        pending_n = sum(1 for v in case_status.values() if v == "pending")
        passed_n = sum(1 for v in case_status.values() if v == "pass")
        messages.append(
            f"FAIL: suite {suite} required={len(spec.cases)} pass={passed_n} "
            f"pending={pending_n} fail={failed_n}"
        )
        return JudgeResult(1, "fail", messages, case_status)

    if any(v == "fail" for v in case_status.values()):
        failed_n = sum(1 for v in case_status.values() if v == "fail")
        pending_n = sum(1 for v in case_status.values() if v == "pending")
        passed_n = sum(1 for v in case_status.values() if v == "pass")
        messages.append(
            f"FAIL: suite {suite} required={len(spec.cases)} pass={passed_n} "
            f"pending={pending_n} fail={failed_n}"
        )
        return JudgeResult(1, "fail", messages, case_status)

    if any(v == "pending" for v in case_status.values()):
        pending_n = sum(1 for v in case_status.values() if v == "pending")
        passed_n = sum(1 for v in case_status.values() if v == "pass")
        messages.append(
            f"PENDING: suite {suite} required={len(spec.cases)} pass={passed_n} "
            f"pending={pending_n} — not a green gate"
        )
        return JudgeResult(2, "pending", messages, case_status)

    messages.append(f"PASS: suite {suite} ({len(spec.cases)}/{len(spec.cases)})")
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
    return _json_loads((run_dir / "run.json").read_text())


def _records_path(run_dir: Path) -> Path:
    return run_dir / "records.jsonl"


def _load_stored_records(run_dir: Path) -> List[dict]:
    path = _records_path(run_dir)
    if not path.exists():
        return []
    # Polling only asks whether envelopes arrived. The final judge reads the
    # complete stream again and retains every parse error.
    recs, _errors = parse_records(path.read_text())
    return recs


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
    # Each response survives later reports, including malformed data. Keep the
    # replayable stream intact so --from-file applies exactly the live judge.
    with (raw / f"{time.time_ns()}-{uuid.uuid4().hex}-{phase}-{safe}.txt").open("x") as fh:
        fh.write(out)
    with _records_path(run_dir).open("a") as fh:
        fh.write(out)
        if out and not out.endswith("\n"):
            fh.write("\n")


def prepare_run(
    run_dir: Path | str,
    *,
    suite: str = "A",
    identity_fields: Optional[dict] = None,
    rcon_send: Optional[Callable[[str], str]] = None,
    port: int = 27015,
) -> JudgeResult:
    suite = (suite or "A").upper()
    if suite not in SUITES:
        return JudgeResult(1, "fail", [f"unsupported suite {suite}"], {})
    run_dir = Path(run_dir)
    if (run_dir / "run.json").exists():
        return JudgeResult(1, "fail", ["run_already_prepared: use collect or a new run directory"], {})
    fields = dict(identity_fields or {})
    try:
        receipt = _read_identity(fields.get("runtime_identity"))
    except (OSError, ValueError) as error:
        return JudgeResult(1, "fail", [f"malformed runtime identity: {error}"], {})
    run_id = fields.get("run_id") if "run_id" in fields else (receipt.get("run_id") if isinstance(receipt, dict) else new_run_id(suite))
    if not valid_run_id(run_id):
        return JudgeResult(1, "fail", ["invalid run_id: expected 1-64 ASCII letters/digits, underscores or hyphens"], {})
    source_revision = fields.get("source_revision") or current_source_revision()
    meta = {
        "schema": SCHEMA,
        "suite": suite,
        "run_id": run_id,
        "source_revision": source_revision,
        "s2script_commit": fields.get("s2script_commit"),
        "s2script_build_hash": fields.get("s2script_build_hash"),
        "host_manifest_digest": fields.get("host_manifest_digest"),
        "fixture_revision": fields.get("fixture_revision"),
        "server_build": fields.get("server_build"),
        "map": fields.get("map"),
        "host": fields.get("host") or "127.0.0.1",
        "server": fields.get("server") or f"127.0.0.1:{port}",
        "started_at": fields.get("started_at") or _utc_now(),
        "phase": "prepared",
        "reason": None,
        "identity_bound": False,
    }
    meta, errors = _with_runtime_identity(meta, receipt)
    if errors:
        return JudgeResult(1, "fail", errors, {})
    run_dir.mkdir(parents=True, exist_ok=True)
    _write_json(run_dir / "run.json", meta)
    if not _records_path(run_dir).exists():
        _records_path(run_dir).write_text("")
    (run_dir / "commands.jsonl").touch()
    (run_dir / "raw").mkdir(exist_ok=True)

    send = rcon_send or (lambda cmd: default_rcon_send(cmd, port=port))
    binding = receipt.get("artifact_identity") if isinstance(receipt, dict) else None
    try:
        for cmd in (probe_cmd("prepare", run_id, binding, suite=suite), accept_cmd("prepare", run_id, binding, suite=suite)):
            _append_jsonl(run_dir / "commands.jsonl", {"cmd": cmd, "ts": _utc_now()})
            out = send(cmd)
            _store_raw(run_dir, "prepare", cmd, out)
        meta["identity_bound"] = bool(binding)
        _write_json(run_dir / "run.json", meta)
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
    if not binding:
        messages.append("PENDING: installed runtime identity missing; supply --identity FILE on collect")
    messages.extend(f"NEED_CLIENTS: {case}: {action}" for case, action in SUITES[suite].client_actions.items())
    return JudgeResult(2, "pending", messages, {})


def _capture_live_gamedata(send: Callable[[str], str], meta: dict, run_dir: Path, phase: str) -> None:
    command = probe_cmd("gamedata", meta["run_id"], suite=meta["suite"])
    _append_jsonl(run_dir / "commands.jsonl", {"cmd": command, "ts": _utc_now()})
    output = send(command)
    (run_dir / ("gamedata-native-" + phase + ".txt")).write_text(output)
    native = None
    for line in output.splitlines():
        start = line.find("{")
        if start < 0:
            continue
        try:
            candidate = json.loads(line[start:])
        except (ValueError, TypeError):
            continue
        if candidate.get("kind") == "khook-gamedata":
            native = candidate
    result = capture_gamedata_inputs(native, meta) if native is not None else {"status": "pending", "reason": "native gamedata query unavailable"}
    capture = meta.setdefault("gamedata_capture", {})
    # Preserve the first successful baseline, so later collects cannot erase a change.
    if capture.get(phase, {}).get("status") != "fail" and (phase != "before" or capture.get("before", {}).get("status") != "pass"):
        capture[phase] = result
    _write_json(run_dir / "gamedata-capture.json", capture)


def collect_run(
    run_dir: Path | str,
    *,
    rcon_send: Optional[Callable[[str], str]] = None,
    port: int = 27015,
    deadline_s: float = COLLECT_DEADLINE_S,
    interval_s: float = COLLECT_INTERVAL_S,
    max_attempts: int = COLLECT_MAX_ATTEMPTS,
    identity_receipt: Any = None,
    suite: Optional[str] = None,
    gamedata_root: Optional[str] = None,
) -> JudgeResult:
    run_dir = Path(run_dir)
    if not (run_dir / "run.json").exists():
        return JudgeResult(1, "fail", ["run_not_prepared"], {})
    meta = _load_run(run_dir)
    stored_suite = meta.get("suite", "A")
    if stored_suite not in SUITES or (suite is not None and suite != stored_suite):
        return JudgeResult(1, "fail", ["suite mismatch or unsupported stored suite"], {})
    suite = stored_suite
    if not valid_run_id(meta.get("run_id")):
        return JudgeResult(1, "fail", ["invalid stored run_id: expected bounded command-safe token"], {})
    meta, errors = _with_runtime_identity(meta, identity_receipt)
    if errors:
        for error in errors:
            _append_jsonl(_records_path(run_dir), {"khook_acceptance_error": error})
        return judge_run_dir(run_dir)
    if gamedata_root:
        mapped = str(Path(gamedata_root).resolve())
        if meta.get("gamedata_root") not in (None, mapped):
            return JudgeResult(1, "fail", ["gamedata root mapping changed for bound run"], {})
        meta["gamedata_root"] = mapped
    run_id = meta["run_id"]
    meta["phase"] = "collected"
    send = rcon_send or (lambda cmd: default_rcon_send(cmd, port=port))
    # Collect never re-sends prepare. Report remains a read-only path.
    cmds = (
        probe_cmd("collect", run_id, suite=suite),
        accept_cmd("collect", run_id, suite=suite),
        probe_cmd("report", run_id, suite=suite),
        accept_cmd("report", run_id, suite=suite),
    )
    deadline = time.monotonic() + deadline_s
    last_err: Optional[Exception] = None
    attempt = 0
    receipt = meta.get("runtime_identity")
    binding = receipt.get("artifact_identity") if isinstance(receipt, dict) else None
    _write_json(run_dir / "run.json", meta)
    while attempt < max_attempts and time.monotonic() <= deadline:
        attempt += 1
        try:
            if binding and not meta.get("identity_bound"):
                for cmd in (probe_cmd("bind", run_id, binding, suite=suite), accept_cmd("bind", run_id, binding, suite=suite)):
                    _append_jsonl(run_dir / "commands.jsonl", {"cmd": cmd, "ts": _utc_now()})
                    _store_raw(run_dir, "bind", cmd, send(cmd))
                meta["identity_bound"] = True
            if suite != "A":
                _capture_live_gamedata(send, meta, run_dir, "before")
            for cmd in cmds:
                verb = cmd.split()[1]
                if verb == "prepare":
                    raise RuntimeError("collect must not send prepare")
                _append_jsonl(run_dir / "commands.jsonl", {"cmd": cmd, "ts": _utc_now()})
                out = send(cmd)
                _store_raw(run_dir, "collect", cmd, out)
            if suite != "A":
                _capture_live_gamedata(send, meta, run_dir, "after")
            last_err = None
        except RconUnreachable as e:
            last_err = e
            break
        recs = _load_stored_records(run_dir)
        present = {r.get("case") for r in recs if r.get("suite", "A") == suite and r.get("run_id") == run_id}
        if all(c in present for c in SUITES[suite].cases):
            break
        if attempt < max_attempts and time.monotonic() + interval_s <= deadline:
            time.sleep(interval_s)
            continue
        break
    if last_err is not None:
        meta["reason"] = "rcon_unreachable"
        _write_json(run_dir / "run.json", meta)
        stored = judge_run_dir(run_dir)
        if _records_path(run_dir).read_text().strip() and stored.exit_code == 1:
            stored.messages.append(f"rcon_unreachable: {last_err}; earlier invalid/failed evidence retained")
            return stored
        return JudgeResult(
            2,
            "pending",
            [f"rcon_unreachable: {last_err}", f"run retained at {run_dir} run_id={run_id}"],
            {},
        )
    meta["reason"] = None
    _write_json(run_dir / "run.json", meta)
    judged = judge_run_dir(run_dir)
    messages = [f"collected run_id={run_id} dir={run_dir}"] + judged.messages
    return JudgeResult(judged.exit_code, judged.status, messages, judged.case_status)


def judge_run_dir(
    run_dir: Path | str,
    observations: Any = None,
    rcon_send: Optional[Callable[[str], str]] = None,
    identity_receipt: Any = None,
) -> JudgeResult:
    """Read-only judge. Does not send RCON or rewrite records."""
    del rcon_send  # judge never talks to the server
    run_dir = Path(run_dir)
    meta = _load_run(run_dir)
    meta, identity_errors = _with_runtime_identity(meta, identity_receipt)
    if not valid_run_id(meta.get("run_id")):
        identity_errors.append("invalid stored run_id: expected bounded command-safe token")
    if meta.get("source_revision") != current_source_revision():
        identity_errors.append("wrong source revision: stored run does not target current revision")
    text = _records_path(run_dir).read_text() if _records_path(run_dir).exists() else ""
    recs, errors = parse_records(text)
    return judge_records(
        recs,
        identity=meta,
        suite=meta.get("suite", "A"),
        observations=observations,
        parse_errors=errors + identity_errors,
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


def synthetic_fixture(kind: str, identity: dict, *, suite: str = "A") -> List[dict]:
    """Host self-test records. Never live evidence; evidence strings say so."""
    recs: List[dict] = []
    for sc in required_subchecks(suite):
        rec = {
            "schema": SCHEMA,
            "suite": suite,
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
        if suite != "A":
            rec["evidence_class"] = "synthetic"
            rec["expected"] = INTEGRATION_EXPECTED[sc.subcheck]
            rec["actual"] = dict(rec["expected"])
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
        return [r for r in recs if r["case"] != SUITES[suite].cases[-1]]
    if kind == "duplicate":
        recs.append(dict(recs[0], result="fail", actual={"ok": False}))
        return recs
    if kind == "fail":
        for rec in recs:
            if rec["subcheck"] == ("native_one_pre_post_orig" if suite == "A" else required_subchecks(suite)[0].subcheck):
                rec["result"] = "fail"
                rec["actual"] = {"ok": False}
                rec["evidence"] = "self-test fixture fail"
        return recs
    if kind == "native-fail-js-pass":
        for rec in recs:
            if rec["producer"] == "native" and rec["case"] == ("frame_client_command_hooks" if suite == "A" else SUITES[suite].cases[0]):
                rec["result"] = "fail"
                rec["actual"] = {"ok": False}
                rec["evidence"] = "self-test native fail"
        return recs
    raise ValueError(f"unknown fixture {kind}")


def print_result(result: JudgeResult) -> int:
    for msg in result.messages:
        print(msg)
    return result.exit_code


def judge_from_file(path: str | Path, suite: str = "A", *, identity_receipt: Any = None, observations: Any = None) -> JudgeResult:
    text = Path(path).read_text()
    recs, errors = parse_records(text)
    run_id = recs[0].get("run_id") if recs else ""
    identity = {
        "run_id": run_id,
        "source_revision": current_source_revision(),
    }
    identity, identity_errors = _with_runtime_identity(identity, identity_receipt)
    return from_file_judge(
        recs,
        identity=identity,
        suite=suite,
        observations=observations,
        parse_errors=errors + identity_errors,
    )


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="KHook A/B/C acceptance controller", allow_abbrev=False)
    p.add_argument("suite", nargs="?", default="A")
    p.add_argument("--from-file")
    p.add_argument("--prepare", action="store_true")
    p.add_argument("--collect", action="store_true")
    p.add_argument("--judge", action="store_true")
    p.add_argument("--run-dir")
    p.add_argument("--observations")
    p.add_argument("--identity", help="independently verified installed-runtime receipt JSON")
    p.add_argument("--gamedata-root", help="host bind-mounted gamedata root matching the probe-listed deployed inputs (B/C)")
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
        for rec in synthetic_fixture(args.emit_fixture, identity, suite=suite):
            print(json.dumps(rec, sort_keys=True))
        return 0
    if args.from_file:
        return print_result(judge_from_file(args.from_file, suite=suite, identity_receipt=args.identity, observations=args.observations))
    staged = args.prepare or args.collect or args.judge
    run_dir = Path(args.run_dir) if args.run_dir else (REPO / "build" / "khook-acceptance" / new_run_id(suite))
    if not staged:
        args.prepare = args.collect = args.judge = True
    code = 0
    if args.prepare:
        code = print_result(prepare_run(run_dir, suite=suite, port=args.port, identity_fields={"runtime_identity": args.identity}))
        if code == 1:
            return code
    if args.collect:
        code = print_result(collect_run(run_dir, port=args.port, identity_receipt=args.identity, suite=suite, gamedata_root=args.gamedata_root))
        if code == 1:
            return code
    if args.judge:
        if _load_run(run_dir).get("suite", "A") != suite:
            return print_result(JudgeResult(1, "fail", ["suite mismatch"], {}))
        code = print_result(judge_run_dir(run_dir, observations=args.observations, identity_receipt=args.identity))
    return code


if __name__ == "__main__":
    sys.exit(main())
