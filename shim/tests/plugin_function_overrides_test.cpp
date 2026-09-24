#include "plugin_function_overrides.h"
#include <json.hpp>
#include <cassert>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <sys/stat.h>
#include <unistd.h>
using nlohmann::json;
namespace fs = std::filesystem;

int main() {
    using namespace s2::function_overrides;
    assert(PluginDirectory("@demo/fire") == "id-QGRlbW8vZmlyZQ");
    assert(PluginDirectory("A") != PluginDirectory("a"));
    assert(PluginDirectory("é") == "id-w6k");
    assert(PluginDirectory("é") != PluginDirectory("é")); // No Unicode normalization.
    char temp[] = "/tmp/s2-overrides-XXXXXX";
    auto root = fs::canonical(fs::path(mkdtemp(temp)));
    auto read = [&]() { return json::parse(Snapshot(root.string(), "@demo/fire")); };
    assert(read()["records"].empty());
    assert(read()["error"].is_null());
    assert(!json::parse(Snapshot(root.string(), std::string(200, 'a')))["error"].is_null());
    auto dir = root / "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom";
    fs::create_directories(dir);
    std::ofstream(dir / "b.jsonc") << "abc";
    std::ofstream(dir / "a.jsonc") << "{}";
    auto value = read();
    assert(value["records"].size() == 2);
    assert(value["records"][0]["relative_path"] == "gamedata/plugins/id-QGRlbW8vZmlyZQ/custom/a.jsonc");
    assert(value["records"][1]["sha256"] == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert(value["records"][1]["content"] == "abc");
    // SHA padding boundary: the FIPS 180-4 56-byte known-answer vector spans two blocks.
    std::ofstream(dir / "c.jsonc") << "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    assert(read()["records"][2]["sha256"] == "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
    fs::remove(dir / "c.jsonc");
    fs::create_symlink(dir / "a.jsonc", dir / "c.jsonc");
    assert(!read()["error"].is_null());
    fs::remove(dir / "c.jsonc");
    assert(mkfifo((dir / "c.jsonc").c_str(), 0600) == 0);
    assert(!read()["error"].is_null());
    fs::remove(dir / "c.jsonc");
    std::ofstream(dir / "c.jsonc") << std::string(MaxFileBytes + 1, 'x');
    assert(!read()["error"].is_null());
    fs::remove(dir / "c.jsonc");
    for (size_t i = 0; i < MaxFiles; ++i) {
        std::ofstream(dir / (std::to_string(i) + ".jsonc")) << "{}";
    }
    assert(!read()["error"].is_null());
    fs::remove_all(dir);
    fs::create_directories(dir);
    for (size_t i = 0; i < MaxTotalBytes / MaxFileBytes + 1; ++i) {
        std::ofstream(dir / (std::to_string(i) + ".jsonc")) << std::string(MaxFileBytes, 'x');
    }
    assert(!read()["error"].is_null());
    fs::remove_all(dir);
    fs::create_directories(dir);
    for (size_t i = 0; i < MaxDirectoryEntries + 1; ++i) {
        std::ofstream file(dir / std::to_string(i));
    }
    assert(!read()["error"].is_null());
    fs::remove_all(dir);
    fs::create_symlink(root, dir);
    assert(!read()["error"].is_null());
    fs::remove(dir);
    fs::create_directories(dir);
    fs::create_directory(dir / "directory.jsonc");
    assert(!read()["error"].is_null());
    // Refuse both traversal components and a symlink in the addon-root ancestry.
    assert(!json::parse(Snapshot((root / "gamedata/..").string(), "@demo/fire"))["error"].is_null());
    fs::create_directory_symlink(root, root / "alias");
    assert(!json::parse(Snapshot((root / "alias").string(), "@demo/fire"))["error"].is_null());
    fs::remove_all(root);
    std::cout << "PASS plugin function overrides: encoding, sorting, SHA256, missing dirs, symlink/regular-file and bounded count/byte/entry limits\n";
}
