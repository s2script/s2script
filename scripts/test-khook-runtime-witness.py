#!/usr/bin/env python3
"""Compile the probe's actual identity helper; Linux also checks loaded ELF files."""
from pathlib import Path
import os
import json
import platform
import shlex
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="khook-runtime-witness-") as directory:
    directory = Path(directory)
    compiler = shlex.split(os.environ.get("CXX", "c++"))
    flags = ["-std=c++17", "-Wall", "-Wextra", "-Werror", "-O1", "-g",
                   "-fsanitize=address,undefined", "-fno-sanitize-recover=all",
                   "-I", str(ROOT / "tools/khook-probe")]
    def compile_test(source, name):
        output = directory / name
        link = ["-ldl"] if platform.system() == "Linux" else []
        subprocess.run(compiler + flags + [str(source), "-o", str(output)] + link, check=True)
        return output

    testdata = ROOT / "tools/khook-probe/testdata"
    subprocess.run([str(compile_test(testdata / "runtime_witness_test.cpp", "unit"))], check=True)

    # Compile the actual command formatter with only engine state and collection
    # substituted; inspect JSON rather than mirroring its serialization logic.
    source = (ROOT / "tools/khook-probe/plugin.cpp").read_text()
    def function(name):
        start = source.index("static " + name)
        return source[start:source.index("\n}", start) + 2]
    command = directory / "command.cpp"
    command.write_text(r'''
#include "runtime_witness.h"
#include <cstdio>
#include <unistd.h>
namespace s2khook::runtime {
Modules fixture;
Modules MockCollectModules() { return fixture; }
}
struct Engine { int build = 12345; int GetBuildVersion() { return build; } } engine;
struct INetworkGameServer { const char* map = "de_test"; const char* GetMapName() { return map; } } game;
struct Network { INetworkGameServer* GetIGameServer() { return &game; } } network;
Engine* g_engine2 = &engine;
Network* g_network_server = &network;
std::string g_probe_generation = "123:456";
#define S2_KHOOK_SOURCE_REVISION "0123456789012345678901234567890123456789"
#define META_CONPRINTF std::printf
#define CollectModules MockCollectModules
''' + function("std::string JsonEscape(") + '\n' + function("void PrintRuntime()") + r'''
int main() {
    using namespace s2khook::runtime;
    for (const char* role : kRoles) fixture[role].push_back(FileIdentity{std::string("/srv/") + role, 0, 10, 42});
    PrintRuntime();
    fixture["shim"].clear(); PrintRuntime();
    fixture["shim"].push_back(std::nullopt); PrintRuntime();
    fixture["shim"] = {FileIdentity{"/srv/shim", 0, 10, 42}};
    fixture["core"].push_back(FileIdentity{"/srv/other-core", 0, 10, 43}); PrintRuntime();
    fixture["core"].pop_back(); engine.build = 0; PrintRuntime();
    engine.build = 12345; game.map = nullptr; PrintRuntime();
}
''')
    rows = [json.loads(line) for line in subprocess.check_output(
        [str(compile_test(command, "command"))], text=True).splitlines()]
    assert len(rows) == 6 and rows[0]["result"] == "ready"
    assert all(row["schema"] == 2 and row["kind"] == "khook-runtime" for row in rows)
    assert all(set(row["modules"]) == {"probe", "shim", "core", "metamod", "metamod_loader"} for row in rows)
    assert rows[0]["modules"]["probe"] == {"path": "/srv/probe", "device": "0:a", "inode": "42"}
    assert rows[0]["source_revision"] == "0123456789012345678901234567890123456789"
    assert rows[0]["process_id"] > 0 and rows[0]["probe_generation"] == "123:456"
    assert rows[0]["server_build"] == 12345 and rows[0]["map"] == "de_test"
    assert all(row["result"] == "pending" for row in rows[1:])
    assert rows[1]["modules"]["shim"] is None and rows[2]["modules"]["shim"] is None
    assert rows[3]["modules"]["core"] is None
    print("PASS: actual runtime command schema 2 ready/pending records")

    if platform.system() == "Linux":
        fixture = directory / "fixture.cpp"
        fixture.write_text('extern "C" int witness_value() { return 42; }\n')
        library = directory / "fixture.so"
        subprocess.run(compiler + ["-shared", "-fPIC", str(fixture), "-o", str(library)], check=True)
        native = compile_test(testdata / "runtime_witness_linux_test.cpp", "linux")
        subprocess.run([str(native), str(library), str(directory / "loaded")], check=True)
    else:
        print("SKIP: Linux loaded ELF identity tests require /proc/self/maps and dl_iterate_phdr")
