// Real ELF loader + /proc/self/maps coverage, separate from the CS2 live gate.
#include "runtime_witness.h"
#include <cassert>
#include <dlfcn.h>
#include <iostream>
#include <unistd.h>

using namespace s2khook::runtime;
namespace fs = std::filesystem;

int main(int argc, char** argv) {
    assert(argc == 3);
    const fs::path fixture(argv[1]), root(argv[2]);
    const std::map<std::string, fs::path> names{
        {"probe", "addons/s2script/s2_khook_probe.so"},
        {"shim", "addons/s2script/s2script.so"},
        {"core", "addons/s2script/libs2script_core.so"},
        {"metamod", "addons/metamod/bin/linuxsteamrt64/metamod.2.cs2.so"},
        {"metamod_loader", "addons/metamod/bin/linuxsteamrt64/libserver.so"}
    };
    auto install = [&](const fs::path& relative) {
        const auto path = root / relative;
        fs::create_directories(path.parent_path());
        fs::copy_file(fixture, path);
        return path;
    };
    std::vector<void*> handles;
    auto load = [&](const fs::path& path) {
        void* handle = ::dlopen(path.c_str(), RTLD_NOW | RTLD_LOCAL);
        if (!handle) std::cerr << ::dlerror() << '\n';
        assert(handle);
        auto value = reinterpret_cast<int(*)()>(::dlsym(handle, "witness_value"));
        assert(value && value() == 42);
        handles.push_back(handle);
    };
    assert(!UniqueModule(CollectModules(), "probe"));
    load(install("csgo/bin/linuxsteamrt64/libserver.so"));
    assert(!UniqueModule(CollectModules(), "metamod_loader"));
    for (const auto& name : names) load(install(name.second));
    const auto loaded = CollectModules();
    for (const auto* role : kRoles) {
        const auto row = UniqueModule(loaded, role);
        assert(row);
        const auto file = ReadFileIdentity((root / names.at(role)).string());
        assert(file && row->path == file->path && row->Device() == file->Device() && row->inode == file->inode);
        assert(loaded.at(role).size() == 1);
    }
    // Replace a still-mapped file atomically: stat now names a different inode.
    const auto replacement = install("replacement.so");
    fs::rename(replacement, root / names.at("probe"));
    assert(!UniqueModule(CollectModules(), "probe"));
    assert(UniqueModule(CollectModules(), "core"));
    fs::remove(root / names.at("shim"));
    assert(!UniqueModule(CollectModules(), "shim"));
    const auto other_core = install("another/libs2script_core.so");
    const auto core_alias = root / "core-alias.so";
    fs::create_symlink(other_core, core_alias);
    load(core_alias);
    assert(CollectModules().at("core").size() == 2);
    assert(!UniqueModule(CollectModules(), "core"));
    fs::remove(core_alias);
    assert(CollectModules().at("core").size() == 2);
    assert(!UniqueModule(CollectModules(), "core"));
    for (void* handle : handles) assert(::dlclose(handle) == 0);
    assert(!UniqueModule(CollectModules(), "core"));
    // Reported standard layout may point to a nonstandard canonical filename.
    const auto real_loader = install("release/loader.so");
    fs::remove(root / names.at("metamod_loader"));
    fs::create_symlink(real_loader, root / names.at("metamod_loader"));
    handles.clear();
    load(root / names.at("metamod_loader"));
    assert(UniqueModule(CollectModules(), "metamod_loader")->path == fs::canonical(real_loader));
    for (void* handle : handles) assert(::dlclose(handle) == 0);
    std::cout << "PASS: Linux loaded ELF identity, replacement, deletion, ambiguity, loader layout and symlink\n";
}
