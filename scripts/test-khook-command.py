#!/usr/bin/env python3
"""Exercise the production client-command handlers at their core dispatch boundary."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "shim/src/s2script_mm.cpp"
text = source.read_text()
start = text.index("KHook::Return<void> S2ScriptPlugin::Hook_DispatchConCommand(")
end = text.index("// Client lifecycle notify-hooks", start)
handlers = text[start:end]

fixture = r'''
#include <cassert>
#include <string>
#include <khook.hpp>
struct ICvar {};
struct ISource2GameClients {};
struct ConCommandRef {};
struct CPlayerSlot { int value; int Get() const { return value; } };
struct CCommandContext { CPlayerSlot GetPlayerSlot() const { return {7}; } };
struct CCommand {
    const char* name; const char* args;
    const char* Arg(int) const { return name; }
    const char* ArgS() const { return args; }
};
static bool enabled = true;
struct Binding { template<typename T> bool Observe(T*) { return enabled; } };
static struct { Binding dispatchConCommand, clientCommand; } g_hk;
bool S2Hook_EnterDispatch(bool observed) { return observed; }
KHook::Return<void> S2_Ignore() { return {KHook::Action::Ignore}; }
KHook::Return<void> S2_Supersede() { return {KHook::Action::Supersede}; }
#define META_CONPRINTF(...) ((void)0)
static int listeners, commands, listener_result, command_result, seen_slot;
static std::string seen_name, seen_args;
int s2script_core_dispatch_command_listeners(int slot, const char* name, const char* args) {
    ++listeners; seen_slot = slot; seen_name = name; seen_args = args;
    return listener_result;
}
int s2script_core_dispatch_client_command(int, const char*, const char*) {
    ++commands; return command_result;
}
class S2ScriptPlugin {
public:
    KHook::Return<void> Hook_DispatchConCommand(ICvar*, ConCommandRef,
                                               const CCommandContext&, const CCommand&);
    KHook::Return<void> Hook_ClientCommand(ISource2GameClients*, CPlayerSlot, const CCommand&);
};
void reset() { listeners = commands = listener_result = command_result = 0; enabled = true; }
'''
checks = r'''
int main() {
    S2ScriptPlugin plugin; ISource2GameClients clients; ICvar cvar;
    reset();
    auto result = plugin.Hook_ClientCommand(&clients, {7}, {"unknown_probe", nullptr});
    assert(result.action == KHook::Action::Ignore && listeners == 1 && commands == 1);
    assert(seen_slot == 7 && seen_name == "unknown_probe" && seen_args.empty());
    reset(); listener_result = 1;
    result = plugin.Hook_ClientCommand(&clients, {7}, {"unknown_probe", "run-token"});
    assert(result.action == KHook::Action::Supersede && listeners == 1 && commands == 0);
    assert(seen_args == "run-token");
    reset(); command_result = 1;
    result = plugin.Hook_ClientCommand(&clients, {7}, {"owned", "arg"});
    assert(result.action == KHook::Action::Supersede && listeners == 1 && commands == 1);
    reset();
    result = plugin.Hook_DispatchConCommand(&cvar, {}, {}, {"registered", "arg"});
    assert(result.action == KHook::Action::Ignore && listeners == 1 && commands == 0);
    reset(); listener_result = 1;
    result = plugin.Hook_DispatchConCommand(&cvar, {}, {}, {"registered", "arg"});
    assert(result.action == KHook::Action::Supersede && listeners == 1 && commands == 0);
    reset(); enabled = false;
    result = plugin.Hook_ClientCommand(&clients, {7}, {"unknown_probe", "arg"});
    assert(result.action == KHook::Action::Ignore && listeners == 0 && commands == 0);
    reset();
    result = plugin.Hook_ClientCommand(&clients, {7}, {"", "arg"});
    assert(result.action == KHook::Action::Ignore && listeners == 0 && commands == 0);
}
'''
with tempfile.TemporaryDirectory(prefix="s2script-command-") as directory:
    cpp = Path(directory) / "command.cpp"
    binary = Path(directory) / "command"
    cpp.write_text(fixture + handlers + checks)
    subprocess.run([os.environ.get("CXX", "g++"), "-std=c++17", "-O1", "-g",
                    "-fsanitize=address,undefined", "-fno-sanitize-recover=all",
                    "-I", str(ROOT / "third_party/metamod-source/third_party/khook/include"),
                    str(cpp), "-o", str(binary)], check=True)
    subprocess.run([str(binary)], check=True, timeout=15)
print("production command dispatch: passed")
