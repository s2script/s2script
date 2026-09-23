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
        [os.environ.get("CXX", "g++"), "-std=c++17", "-O2", "-I", str(ROOT / "tools/khook-probe"),
         str(source), "-o", str(exe)],
        check=True,
    )
    lines = subprocess.run([str(exe)], check=True, capture_output=True, text=True).stdout.splitlines()

records = [json.loads(line) for line in lines]
assert len(records) == 6
assert records.pop(0) == {"installed": True, "pre": 1, "original": 1,
                          "original_in_scope": 1, "receiver_ok": 1, "expired": True}
snapshot = records.pop(0)
assert snapshot["mutation"]["opaque_a"] == str(0xf123456789abcdef)
assert snapshot["mutation"]["opaque_b"] == str(0x8123456789abcdef)
assert snapshot["acquire"][3] == {"pre": 1, "post": 1, "original": 0, "arguments_ok": 0,
                                  "effective_return": 1, "post_result": 1, "skipped": 1}
assert snapshot["nesting"]["post_methods"] == [42, 41, 40]
assert snapshot["bypass"]["returns"] == [6, 6, 6]
assert records[0]["ab_io"] == {"pre_a": 1, "pre_b": 1, "orig": 1, "ret": 42, "pre_order": 21}
assert records[0]["ba_os"] == {"pre_a": 1, "pre_b": 1, "orig": 0, "ret": 99, "pre_order": 12}
assert records[1]["ba_os"]["ret"] == 2139062143
assert records[2] == {"pre": 1, "post": 1, "orig": 1, "return": 15}
assert records[3]["return"] == 16
print("PASS: opaque-call seam, cleanup policy, and strict controlled evidence")
