#!/usr/bin/env python3
"""Execute selected production probe functions against the missing engine boundary.

The target functions are extracted unchanged from plugin.cpp. This catches the
stage-5 omission and >=1 command predicates without compiling the Linux-only SDK
on the developer's Mac. The optimized Linux probe build remains required.
"""
from pathlib import Path
import os
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[3]
source = (ROOT / "tools/khook-probe/plugin.cpp").read_text()


def function(name):
    start = source.index("static ", source.index("static void R6DriveJsPhase()") if name == "R6DriveJsPhase" else source.index("static bool " + name + "()"))
    end = source.index("\n}", start) + 2
    return source[start:end]


program = r'''
#include "acceptance_observer.h"
#include <array>
#include <cassert>
#include <map>
#include <string>
#include <vector>
struct CEntityInstance {} entity;
std::map<std::string, std::string> cvars;
std::string g_run_id = "run";
std::array<bool, 6> g_js_phase_observed{};
std::array<s2khook::OriginalObservation, 6> g_js_phase_original;
int invokes = 0;
bool present = true;
bool suppress = false;
int g_cc_cont_pre = 0, g_cc_cont_post = 0, g_engine_continue = 0, g_cc_cont_skip = 0;
int g_cc_hand_pre = 0, g_cc_hand_post = 0, g_engine_handled = 0, g_cc_hand_skip = 0;
bool JsAcceptPresent() { return present; }
std::string ProbeCvarStr(const char* n) { return cvars[n]; }
int ProbeCvarInt(const char* n, int fallback) { return cvars.count(n) ? std::stoi(cvars[n]) : fallback; }
bool ProbeSetCvarString(const char* n, const std::string& v) { cvars[n] = v; return true; }
std::string JsonEscape(const char* v) { return v; }
CEntityInstance* EntByIndex(int i) { return i == 42 ? &entity : nullptr; }
bool R6EntityIsTriggerPush(CEntityInstance* e) { return e == &entity; }
s2khook::OriginalObservation R6InvokeTouch(CEntityInstance* e) {
    assert(e == &entity); ++invokes;
    s2khook::OriginalObservation result;
    result.Pre(); result.Post(suppress); return result;
}
'''
program += "\n" + function("R6DriveJsPhase")
program += "\n" + function("ContinueOriginalOk")
program += "\n" + function("HandledOriginalOk")
program += r'''
int main() {
    cvars["s2_khook_accept_run"] = "run";
    cvars["s2_khook_accept_phase_ent"] = "42";
    cvars["s2_khook_accept_phase_stage"] = "5";
    R6DriveJsPhase();
    assert(invokes == 1 && g_js_phase_observed[5]);
    assert(g_js_phase_original[5].Once());
    assert(cvars["s2_khook_accept_phase_ack"].find("\"stage\":5") != std::string::npos);
    R6DriveJsPhase(); assert(invokes == 1);
    cvars["s2_khook_accept_phase_stage"] = "0";
    cvars["s2_khook_accept_run"] = "old-run";
    R6DriveJsPhase(); assert(invokes == 1 && !g_js_phase_observed[0]);
    cvars["s2_khook_accept_run"] = "run";
    suppress = true; R6DriveJsPhase();
    assert(invokes == 2 && !g_js_phase_original[0].Once());
    assert(cvars["s2_khook_accept_phase_ack"].find("\"original\":0") != std::string::npos);
    g_cc_cont_pre = g_cc_cont_post = g_engine_continue = 1;
    assert(ContinueOriginalOk());
    g_engine_continue = 2; assert(!ContinueOriginalOk());
    g_engine_continue = 1; g_cc_cont_pre = 2; assert(!ContinueOriginalOk());
    g_cc_hand_pre = g_cc_hand_post = g_cc_hand_skip = 1;
    assert(HandledOriginalOk());
    g_cc_hand_skip = 2; assert(!HandledOriginalOk());
    g_cc_hand_skip = 1; g_engine_handled = 1; assert(!HandledOriginalOk());
}
'''
with tempfile.TemporaryDirectory(prefix="khook-native-fixture-") as temp:
    path = Path(temp)
    (path / "test.cpp").write_text(program)
    subprocess.run([os.environ.get("CXX", "g++"), "-std=c++17", "-O2", "-I", str(ROOT / "tools/khook-probe"), str(path / "test.cpp"), "-o", str(path / "test")], check=True)
    subprocess.run([str(path / "test")], check=True)
print("PASS: production native phase driver and exact command verdicts")
