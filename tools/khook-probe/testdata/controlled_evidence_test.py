#!/usr/bin/env python3
"""Test the probe's opaque-call seam, cleanup policy, verdicts, and strict JSON."""

import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[3]

program = r'''
#include "controlled_evidence.h"
#include <cassert>
#include <iostream>

static int plus_one(int value) { return value + 1; }
static int override_42(int) { return 42; }
static char allocator_storage[2][64];
static int allocation_count = 0;
static int release_count = 0;
static char* released_pointer = nullptr;

static char* fixture_allocate(std::size_t size) {
    assert(size <= sizeof(allocator_storage[0]));
    return allocator_storage[allocation_count++];
}
static void fixture_release(char* pointer) {
    ++release_count;
    released_pointer = pointer;
}

int main() {
    s2khook::PrecacheTokens tokens;
    tokens.Reset("run");
    S2NamedPrecacheFrameV1 outer{1,sizeof(S2NamedPrecacheFrameV1),31,41,51,61};
    assert(!tokens.Begin("wrong",1,1,outer));
    const int token=tokens.Begin("run",1,1,outer);
    assert(token>0 && !tokens.Begin("run",1,1,outer));
    auto inner=outer; inner.serial=32; inner.manifest=62;
    assert(!tokens.Finish("run",token,1,inner,"resource",true));
    assert(!tokens.Finish("run",token,2,outer,"resource",true));
    assert(!tokens.Finish("other-run",token,1,outer,"resource",true));
    assert(!tokens.Finish("run",token+1,1,outer,"resource",true));
    assert(tokens.Finish("run",token,1,outer,"resource",true));
    assert(!tokens.Finish("run",token,1,outer,"resource",true));
    assert(tokens.Read(token,0,outer)==1 && tokens.Read(token,2,outer)>0);
    assert(tokens.Read(token,0,inner)==0);
    assert(!tokens.Finish("run",token,1,{},"resource",true));
    tokens.Reset("new-run");
    assert(tokens.Read(token,0,outer)==0);

    s2khook::DeclarativeVoidObservation declarative;
    assert(!declarative.Passed());
    declarative = {true, 1, 1, 1, 1, true};
    assert(declarative.Passed());
    std::cout << declarative.Json() << "\n";
    declarative.original = 2;
    assert(!declarative.Passed());
    declarative.original = 1;
    declarative.original_in_scope = 0;
    assert(!declarative.Passed());
    declarative.original_in_scope = 1;
    declarative.expired = false;
    assert(!declarative.Passed());

    s2khook::DeclarativeSnapshot snapshot;
    assert(!snapshot.Passed());
    snapshot.simple = {true, 1, 1, 1, 1, true};
    snapshot.mutation = {1, 1, 1, 7.25f, -17,
        static_cast<int64_t>(UINT64_C(0xf123456789abcdef)), static_cast<int64_t>(UINT64_C(0x8123456789abcdef))};
    snapshot.acquire = {{{1,1,1,1,6,6,0}, {1,1,1,1,6,6,0}, {1,1,1,1,2,2,0},
                         {1,1,0,0,1,1,1}, {1,1,0,0,0,0,1}}};
    snapshot.nesting = {3,3,3,2,2,2,2,2,0,40,{{42,41,40}}};
    snapshot.bypass = {2,2,3,0,0,2,2,{{6,6,6}}};
    assert(snapshot.Passed());
    std::cout << snapshot.Json() << "\n";
    snapshot.mutation.opaque_b = 0x89abcdef;
    assert(!snapshot.Passed());
    snapshot.mutation.opaque_b = static_cast<int64_t>(UINT64_C(0x8123456789abcdef));
    snapshot.acquire[2].post_result = 0;
    assert(!snapshot.Passed());
    snapshot.acquire[2].post_result = 2;
    snapshot.nesting.post_methods[0] = 40;
    assert(!snapshot.Passed());
    snapshot.nesting.post_methods[0] = 42;
    snapshot.bypass.post_after_bypass = 1;
    assert(!snapshot.Passed());

    s2khook::NamedSnapshot named;
    assert(!named.Passed());
    named.installed = true;
    named.damage_pre=3; named.damage_post=3; named.damage_original=3;
    named.damage_nested_restored=2; named.damage_expired=1;
    named.damage_pre_ignore=3; named.damage_post_ignore=3; named.damage_post_observed=3;
    named.damage_skipped=0; named.damage_current_return=INT64_C(0x1122334455667788);
    named.damage_current_return_matches=3;
    named.chat_dispatch=2; named.chat_original=1; named.chat_peer_before=2;
    named.chat_peer_after=2; named.chat_peer_order=123123;
    named.chat_post_observed=2; named.chat_actions={{0,2}}; named.chat_skipped={{0,1}};
    named.chat_current_return={s2khook::FacetApplicability::Inapplicable,-1};
    named.output_dispatch=2; named.output_original=1;
    named.output_post_observed=2; named.output_actions={{0,2}}; named.output_skipped={{0,1}};
    named.output_current_return={s2khook::FacetApplicability::Inapplicable,-1};
    named.usercmd_dispatch=3; named.usercmd_neutralized=2; named.usercmd_original=3;
    named.usercmd_return=37; named.usercmd_nested_restored=1; named.usercmd_expired=1;
    named.usercmd_ignore=3; named.usercmd_post_observed=3; named.usercmd_skipped=0;
    named.usercmd_current_return=37; named.usercmd_current_return_matches=3;
    named.precache_dispatch=2; named.precache_original=2; named.precache_receiver_ok=2;
    named.precache_nested_restored=1; named.precache_filtered_original=1;
    named.precache_expired=1; named.precache_peer_before=2; named.precache_peer_after=2;
    named.precache_peer_order=121233;
    named.precache_ignore=2; named.precache_post_observed=2; named.precache_skipped=0;
    named.precache_current_return={s2khook::FacetApplicability::Inapplicable,-1};
    named.precache_generation_source=s2khook::MapGenerationSource::LevelLifetime;
    named.precache_map_generation=41; named.precache_observed_map_generation=41;
    named.precache_generation_observations=2;
    named.bypass={s2khook::FacetApplicability::Inapplicable,-1};
    named.removal={s2khook::FacetApplicability::Applicable,1,1,1,1};
    assert(named.Passed());
    std::cout << named.Json() << "\n";
    auto pending=named;
    pending.removal.terminal_preflight=-1; pending.removal.terminal_remove=-1;
    pending.removal.terminal_complete=-1;
    assert(pending.InvocationPassed() && !pending.Passed());
    named.usercmd_original = 2;
    assert(!named.Passed());
    named.usercmd_original = 3;
    named.precache_filtered_original = 0;
    assert(!named.Passed());
    named.precache_filtered_original = 1;
    named.damage_current_return=INT64_C(0x55667788);
    assert(!named.Passed());
    named.damage_current_return=INT64_C(0x1122334455667788);
    named.chat_actions[1]=-1;
    assert(!named.Passed());
    named.chat_actions[1]=2;
    named.output_skipped[1]=0;
    assert(!named.Passed());
    named.output_skipped[1]=1;
    named.chat_current_return.applicability=s2khook::FacetApplicability::Unspecified;
    assert(!named.Passed());
    named.chat_current_return.applicability=s2khook::FacetApplicability::Inapplicable;
    named.chat_current_return.value=0;
    assert(!named.Passed());
    named.chat_current_return.value=-1;
    named.bypass.value=1;
    assert(!named.Passed());
    named.bypass.value=-1;
    named.removal.terminal_complete=-1;
    assert(!named.Passed());
    named.removal.terminal_complete=1;
    named.precache_generation_source=s2khook::MapGenerationSource::Unspecified;
    assert(!named.Passed());
    named.precache_generation_source=s2khook::MapGenerationSource::LevelLifetime;
    named.precache_observed_map_generation=40;
    assert(!named.Passed());

    s2khook::IntTarget volatile target = &plus_one;
    assert(s2khook::InvokeOpaque(target, 10) == 11);
    target = &override_42;
    assert(s2khook::InvokeOpaque(target, 10) == 42);

    char* owned = fixture_allocate(4);
    std::memcpy(owned, "old", 4);
    char* original = owned;
    assert(s2khook::ReplaceOwnedCString(&owned, "new-value", fixture_allocate, fixture_release));
    assert(std::string(owned) == "new-value");
    assert(release_count == 1 && released_pointer == original);
    const auto allocation_before_failure = allocation_count;
    auto fail_allocate = [](std::size_t) -> char* { return nullptr; };
    assert(!s2khook::ReplaceOwnedCString(&owned, "ignored", fail_allocate, fixture_release));
    assert(std::string(owned) == "new-value");
    assert(allocation_count == allocation_before_failure && release_count == 1);

    s2khook::LevelLifetime level;
    assert(!level.MayTouchWorld());
    assert(level.Generation() == 0);
    assert(level.ClaimCurrentWorld() == 0);
    level.OnLevelInit();
    assert(level.MayTouchWorld());
    assert(level.Generation() == 1);
    const auto prepared_generation = level.ClaimCurrentWorld();
    assert(prepared_generation == 1);
    assert(level.MayTouchOwnedWorld(prepared_generation));
    level.OnLevelShutdown();
    level.OnLevelShutdown();
    assert(!level.MayTouchWorld());
    assert(level.ClaimCurrentWorld() == 0);
    level.OnLevelInit();
    assert(level.Generation() == 2);
    const auto reload_target_generation = level.ClaimCurrentWorld();
    assert(reload_target_generation == 2);
    assert(!level.MayTouchOwnedWorld(prepared_generation));
    assert(level.MayTouchOwnedWorld(reload_target_generation));

    s2khook::PeerActionsObservation peers{
        {1, 1, 1, 42, 21}, {1, 1, 1, 99, 21}, {1, 1, 0, 99, 21},
        {1, 1, 1, 42, 12}, {1, 1, 1, 7, 12}, {1, 1, 0, 99, 12},
    };
    assert(peers.Passed());
    assert(peers.Json() == s2khook::PeerActionsObservation::ExpectedJson());
    std::cout << peers.Json() << "\n";
    peers.ab_oo.ret = 7;
    assert(!peers.Passed());
    peers.ab_oo.ret = 99;
    peers.ab_oo.pre_order = 12;
    assert(!peers.Passed());
    peers.ab_oo.pre_order = 21;
    peers.ab_io.original = 0;
    assert(!peers.Passed());
    peers.ab_io.original = 1;
    peers.ba_os.ret = 2139062143;
    assert(!peers.Passed());
    std::cout << peers.Json() << "\n";

    s2khook::OnceObservation once{1, 1, 1, 15};
    assert(once.Passed());
    assert(once.Json() == s2khook::OnceObservation::ExpectedJson());
    std::cout << once.Json() << "\n";
    once.ret = 16;
    assert(!once.Passed());
    std::cout << once.Json() << "\n";
}
'''

with tempfile.TemporaryDirectory(prefix="khook-controlled-evidence-") as temp:
    path = Path(temp)
    source = path / "test.cpp"
    exe = path / "test"
    source.write_text(program)
    subprocess.run(
        [os.environ.get("CXX", "g++"), "-std=c++17", "-O2", "-I", str(ROOT / "tools/khook-probe"), "-I", str(ROOT / "shim/src"), "-I", str(ROOT / "third_party/metamod-source/third_party/khook/include"),
         str(source), "-o", str(exe)],
        check=True,
    )
    lines = subprocess.run([str(exe)], check=True, capture_output=True, text=True).stdout.splitlines()

records = [json.loads(line) for line in lines]
assert len(records) == 7
assert records.pop(0) == {"installed": True, "pre": 1, "original": 1,
                          "original_in_scope": 1, "receiver_ok": 1, "expired": True}
snapshot = records.pop(0)
assert snapshot["mutation"]["opaque_a"] == str(0xf123456789abcdef)
assert snapshot["mutation"]["opaque_b"] == str(0x8123456789abcdef)
assert snapshot["acquire"][3] == {"pre": 1, "post": 1, "original": 0, "arguments_ok": 0,
                                  "effective_return": 1, "post_result": 1, "skipped": 1}
assert snapshot["nesting"]["post_methods"] == [42, 41, 40]
assert snapshot["bypass"]["returns"] == [6, 6, 6]
named = records.pop(0)
assert named["damage"] == {"pre": 3, "post": 3, "original": 3,
                            "nested_restored": 2, "expired": 1,
                            "pre_ignore": 3, "post_ignore": 3,
                            "post_observed": 3, "skipped": 0,
                            "current_return": str(0x1122334455667788),
                            "current_return_matches": 3}
assert named["chat"]["peer_order"] == 123123
assert named["chat"]["actions"] == [0, 2]
assert named["chat"]["skipped"] == [0, 1]
assert named["chat"]["current_return"] == {"applicability": "inapplicable", "value": None}
assert named["usercmd"]["return"] == 37
assert named["usercmd"]["current_return"] == 37
assert named["precache"]["filtered_original"] == 1
assert named["precache"]["peer_order"] == 121233
assert named["precache"]["map_generation"] == 41
assert named["precache"]["generation_source"] == "level_lifetime"
assert named["bypass"] == {"applicability": "inapplicable", "value": None}
assert named["removal"] == {"applicability": "applicable", "state": "complete", "active_refused": 1,
                             "terminal_preflight": 1, "terminal_remove": 1,
                             "terminal_complete": 1}
assert records[0]["ab_io"] == {"pre_a": 1, "pre_b": 1, "orig": 1, "ret": 42, "pre_order": 21}
assert records[0]["ba_os"] == {"pre_a": 1, "pre_b": 1, "orig": 0, "ret": 99, "pre_order": 12}
assert records[1]["ba_os"]["ret"] == 2139062143
assert records[2] == {"pre": 1, "post": 1, "orig": 1, "return": 15}
assert records[3]["return"] == 16
print("PASS: opaque-call seam, cleanup policy, and strict controlled evidence")
