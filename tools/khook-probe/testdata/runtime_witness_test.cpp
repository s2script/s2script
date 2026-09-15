#include "runtime_witness.h"
#include <cassert>
#include <iostream>
#include <sstream>

using namespace s2khook::runtime;

int main() {
    assert(RoleForPath("/srv/addons/metamod/bin/linuxsteamrt64/libserver.so") == "metamod_loader");
    assert(RoleForPath("/srv/csgo/bin/linuxsteamrt64/libserver.so").empty());
    assert(RoleForPath("/srv/addons/metamod/bin/linuxsteamrt64/metamod.2.cs2.so") == "metamod");
    assert(RoleForPath("/srv/wrong/metamod.2.cs2.so").empty());
    assert(RoleForPath("/srv/s2script.so") == "shim");
    assert(RoleForPath("addons/metamod/bin/linuxsteamrt64/./libserver.so") == "metamod_loader");
    std::istringstream input(
        "1000-2000 r--p 00000000 00:0A 00042 /srv/s2script.so\n"
        "2000-3000 r-xp 00001000 00:0a 42 /srv/s2script.so\n"
        "4000-5000 r--p 00000000 00:0a 43 /srv/deleted.so (deleted)\n");
    const auto maps = ParseMaps(input);
    assert(maps.size() == 3);
    FileIdentity file{"/srv/s2script.so", 0, 10, 42};
    Image image{"/srv/s2script.so", {{0x1100, 0x100}, {0x2100, 0x1100}}};
    auto row = MatchImage(image, maps, file);
    assert(row && row->Device() == "0:a" && row->Inode() == "42");
    assert(!MatchImage(image, {}, file));
    assert(!MatchImage(image, maps, std::nullopt));
    auto replaced = file; replaced.inode = 99;
    assert(!MatchImage(image, maps, replaced));
    auto wrong_device = file; wrong_device.minor = 11;
    assert(!MatchImage(image, maps, wrong_device));
    auto wrong_offset = image; wrong_offset.segments[0].offset = 0;
    assert(!MatchImage(wrong_offset, maps, file));
    Image deleted{"/srv/deleted.so", {{0x4000, 0}}};
    assert(!MatchImage(deleted, maps, FileIdentity{"/srv/deleted.so", 0, 10, 43}));
    auto ambiguous_maps = maps; ambiguous_maps.push_back(maps.front());
    assert(!MatchImage(image, ambiguous_maps, file));
    auto bad_segment = image; bad_segment.segments.push_back({0x6000, 0});
    assert(!MatchImage(bad_segment, maps, file));
    Modules modules;
    AddImage(modules, image, maps, [&](const std::string&) { return std::optional<FileIdentity>(file); });
    assert(UniqueModule(modules, "shim")); // Multiple PT_LOAD segments are one DSO.
    assert(!UniqueModule(modules, "core"));
    AddImage(modules, image, maps, [&](const std::string&) { return std::optional<FileIdentity>(file); });
    assert(!UniqueModule(modules, "shim")); // Two actual images are ambiguous.
    Modules invalid;
    AddImage(invalid, image, maps, [](const std::string&) { return std::optional<FileIdentity>(); });
    assert(invalid.at("shim").size() == 1 && !UniqueModule(invalid, "shim"));
    Modules missing_alias;
    Image lost_alias{"/srv/now-missing-alias.so", image.segments};
    AddImage(missing_alias, lost_alias, maps, [](const std::string&) { return std::optional<FileIdentity>(); });
    assert(missing_alias.at("shim").size() == 1 && !UniqueModule(missing_alias, "shim"));
    Modules symlinks;
    Image reported_host{"/srv/addons/metamod/bin/linuxsteamrt64/libserver.so", image.segments};
    auto symlink_target = file; symlink_target.path = "/srv/releases/loader-build-1467.so";
    AddImage(symlinks, reported_host, maps, [&](const std::string&) { return std::optional<FileIdentity>(symlink_target); });
    assert(UniqueModule(symlinks, "metamod_loader")->path == symlink_target.path);
    Modules canonical;
    Image alias{"/srv/alias.so", image.segments};
    symlink_target.path = "/srv/addons/metamod/bin/linuxsteamrt64/libserver.so";
    AddImage(canonical, alias, maps, [&](const std::string&) { return std::optional<FileIdentity>(symlink_target); });
    assert(UniqueModule(canonical, "metamod_loader"));
    Modules wrong_loader;
    Image engine{"/srv/csgo/bin/linuxsteamrt64/libserver.so", image.segments};
    symlink_target.path = engine.path;
    AddImage(wrong_loader, engine, maps, [&](const std::string&) { return std::optional<FileIdentity>(symlink_target); });
    assert(!UniqueModule(wrong_loader, "metamod_loader"));
    std::istringstream malformed("junk\n1000-2000 r--p nope 00:0a 42 /x\n");
    assert(ParseMaps(malformed).empty());
    std::cout << "PASS: runtime identity helper cases\n";
}
