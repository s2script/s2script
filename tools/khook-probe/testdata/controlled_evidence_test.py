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
        {1, 1, 1, 42}, {1, 1, 1, 7}, {1, 1, 0, 99},
        {1, 1, 1, 42}, {1, 1, 1, 99}, {1, 1, 0, 99},
    };
    assert(peers.Passed());
    assert(peers.Json() == s2khook::PeerActionsObservation::ExpectedJson());
    std::cout << peers.Json() << "\n";
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
assert len(records) == 4
assert records[0]["ab_io"] == {"pre_a": 1, "pre_b": 1, "orig": 1, "ret": 42}
assert records[0]["ba_os"] == {"pre_a": 1, "pre_b": 1, "orig": 0, "ret": 99}
assert records[1]["ba_os"]["ret"] == 2139062143
assert records[2] == {"pre": 1, "post": 1, "orig": 1, "return": 15}
assert records[3]["return"] == 16
print("PASS: opaque-call seam, cleanup policy, and strict controlled evidence")
