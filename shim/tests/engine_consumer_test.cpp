#include "engine_consumer.h"

#include <algorithm>
#include <cstring>
#include <iostream>
#include <string>
#include <tuple>
#include <vector>

static int failures = 0;
#define CHECK(c, label) do { if (!(c)) { std::cerr << "FAIL: " << label << "\n"; ++failures; } } while (0)

namespace {

void put(std::vector<uint8_t>& bytes, size_t at, uint64_t value, size_t count) {
    for (size_t i = 0; i < count; ++i) bytes.at(at + i) = uint8_t(value >> (8 * i));
}

struct Fixture {
    static constexpr uintptr_t base = 0x100000;
    std::vector<uint8_t> file = std::vector<uint8_t>(0x2200);
    std::vector<uint8_t> live = std::vector<uint8_t>(0x5000);
    std::shared_ptr<const s2original::Image> image;
    std::string reason;

    Fixture() {
        std::memcpy(file.data(), "\177ELF\2\1\1", 7);
        put(file, 16, 3, 2); put(file, 18, 62, 2); put(file, 20, 1, 4);
        put(file, 32, 64, 8); put(file, 52, 64, 2); put(file, 54, 56, 2); put(file, 56, 4, 2);
        segment(0, 1, 4, 0, 0, 0x800, 0x800);
        segment(1, 1, 5, 0x1000, 0x1000, 0x400, 0x400);
        segment(2, 1, 6, 0x2000, 0x3000, 0x200, 0x1000);
        segment(3, 4, 4, 0x300, 0x300, 20, 20);
        put(file, 0x300, 4, 4); put(file, 0x304, 4, 4); put(file, 0x308, 3, 4);
        std::memcpy(file.data() + 0x30c, "GNU", 4); put(file, 0x310, 0x3412cdab, 4);
        std::fill(file.begin() + 0x1000, file.begin() + 0x1400, 0x90);
        std::memcpy(live.data() + 0x500, "ConsumerAnchor", 15);
        bytes(0x1200, {0x55, 0x48, 0x89, 0xe5, 0xc3});
        bytes(0x1300, {0x41, 0x56, 0x55, 0xc3});
    }

    void segment(int i, int type, int flags, int off, int addr, int size, int mem) {
        const size_t p = 64 + i * 56;
        put(file, p, type, 4); put(file, p + 4, flags, 4); put(file, p + 8, off, 8);
        put(file, p + 16, addr, 8); put(file, p + 32, size, 8); put(file, p + 40, mem, 8);
        put(file, p + 48, 1, 8);
    }

    void bytes(size_t at, std::initializer_list<uint8_t> value) {
        std::copy(value.begin(), value.end(), file.begin() + at);
    }

    void lea(size_t at, size_t target) {
        bytes(at, {0x4c, 0x8d, 0x35});
        put(file, at + 3, uint32_t(int32_t(target - (at + 7))), 4);
    }

    void call(size_t at, size_t target, size_t string) {
        file[at] = 0xe8;
        put(file, at + 1, uint32_t(int32_t(target - (at + 5))), 4);
        lea(at + 5, string);
    }

    bool mapped(uintptr_t at, size_t count) const {
        if (!count || at < base) return false;
        const size_t off = at - base;
        for (auto range : {std::pair<size_t, size_t>{0, 0x800}, {0x1000, 0x1400}, {0x3000, 0x4000}})
            if (off >= range.first && off < range.second && count <= range.second - off) return true;
        return false;
    }

    void freeze() {
        s2original::Identity id{7, 9, base, "abcd1234"};
        image = s2original::FromElf(file, id, id, {
            {base, base + 0x800, 0, 7, 9},
            {base + 0x1000, base + 0x1400, 0x1000, 7, 9},
            {base + 0x3000, base + 0x4000, 0x2000, 7, 9},
        }, reason);
        CHECK(image, "consumer fixture has a verified image");
        std::copy(file.begin() + 0x1000, file.begin() + 0x1400, live.begin() + 0x1000);
    }

    bool read_live(uintptr_t at, void* out, size_t count) const {
        if (!mapped(at, count)) return false;
        std::memcpy(out, live.data() + at - base, count);
        return true;
    }

    s2resolve::Sources sources() {
        s2resolve::Sources result;
        result.image = image;
        result.mapped = [this](uintptr_t at, size_t count) { return mapped(at, count); };
        result.read_live = [this](uintptr_t at, void* out, size_t count) {
            return read_live(at, out, count);
        };
        return result;
    }

    s2consumer::Resolver evaluator(s2resolve::Sources* used = nullptr) {
        return [this, used](const s2resolve::TargetRecipe& recipe,
                            s2resolve::Resolution& out, std::string& why) {
            auto contacts = sources();
            if (used) contacts = *used;
            return s2resolve::Evaluate(recipe, contacts, out, why);
        };
    }
};

struct Reports {
    std::vector<std::tuple<std::string, bool, std::string>> rows;
    s2consumer::Reporter callback() {
        return [this](const char* name, bool ok, const char* reason) {
            rows.emplace_back(name ? name : "", ok, reason ? reason : "");
        };
    }
};

void engine_descriptor_delegation() {
    Fixture f;
    f.call(0x1000, 0x1200, 0x500);
    f.freeze();
    s2resolve::Resolution out;
    std::string why;

    CHECK(s2consumer::ResolveEngineTarget("signature", "fixture", "41 56 55 C3", "direct",
              "", -1, R"({"prologue":"41 56 55"})", out, why, f.evaluator()) &&
              out.address == Fixture::base + 0x1300,
          "engine signature delegates to the shipped evaluator");
    CHECK(out.recipe.find("Executable") != std::string::npos,
          "engine signatures are explicitly executable");

    CHECK(s2consumer::ResolveEngineTarget("signature", "fixture",
              "E8 ? ? ? ? 4C 8D 35 ? ? ? ?", "validated-call", "", -1,
              R"({"string-xref":{"at":5,"dispOff":3,"instrLen":7,"expect":"ConsumerAnchor"}})",
              out, why, f.evaluator()) && out.address == Fixture::base + 0x1200,
          "engine validated-call descriptors use call-site validation");

    s2resolve::Sources virtual_sources = f.sources();
    put(f.live, 0x3000, 0xf00000, 8);
    virtual_sources.ops.vtable_from_image = [](const char*) -> void** {
        return reinterpret_cast<void**>(Fixture::base + 0x3000);
    };
    virtual_sources.ops.original_virtual = [](void**, int slot) -> void* {
        return slot == 0 ? reinterpret_cast<void*>(Fixture::base + 0x1300) : nullptr;
    };
    CHECK(s2consumer::ResolveEngineTarget("vtable", "fixture", "", "direct",
              "ConsumerClass", 0, R"({"prologue":"41 56 55"})",
              out, why, f.evaluator(&virtual_sources)) && out.address == Fixture::base + 0x1300,
          "engine virtual descriptor uses original peer-patched target");

    CHECK(!s2consumer::ResolveEngineTarget("signature", "fixture", "41 56 55 C3", "bogus",
              "", -1, "{}", out, why, f.evaluator()) && why.find("unknown resolver strategy") != std::string::npos,
          "engine malformed strategy returns the evaluator reason");
    f.bytes(0x1100, {0x41, 0x56, 0x55, 0xc3});
    f.freeze();
    CHECK(!s2consumer::ResolveEngineTarget("signature", "fixture", "41 56 55 C3", "direct",
              "", -1, "{}", out, why, f.evaluator()) && why.find("ambiguous") != std::string::npos,
          "engine ambiguous target returns the evaluator reason");
}

void named_failure_and_builtin_offsets() {
    Fixture bad;
    bad.freeze();
    SigSpec malformed{"fixture", "41 56 55 C3", "bogus", "{}"};
    Reports report;
    s2resolve::Resolution out;
    std::string why;
    CHECK(!s2consumer::ResolveNamedSignature("Touch", malformed, s2resolve::TargetUse::Executable,
              out, why, report.callback(), bad.evaluator()),
          "named SDKHook failure is rejected");
    CHECK(report.rows.size() == 1 && std::get<0>(report.rows[0]) == "Touch" &&
              !std::get<1>(report.rows[0]) && std::get<2>(report.rows[0]) == why &&
              why.find("unknown resolver strategy") != std::string::npos,
          "named SDKHook failure preserves exact owner name and resolver reason");

    std::vector<s2resolve::Resolution> failed_retained;
    int64_t failed_offset = 0;
    Reports failed_builtin;
    CHECK(!s2consumer::ResolveBuiltinOffset("GameEventManager", malformed, failed_retained,
              failed_offset, why, failed_builtin.callback(), bad.evaluator()) &&
              failed_builtin.rows.size() == 1 &&
              std::get<0>(failed_builtin.rows[0]) == "GameEventManager" &&
              !std::get<1>(failed_builtin.rows[0]) && std::get<2>(failed_builtin.rows[0]) == why,
          "failed built-in recipe preserves its own name and resolver reason");

    Fixture data;
    data.lea(0x1000, 0x3800);
    data.file[0x1007] = 0xe8;
    put(data.file, 0x1008, uint32_t(int32_t(0x1200 - 0x100c)), 4);
    data.freeze();
    std::vector<s2resolve::Resolution> retained;
    int64_t offset = 0;
    SigSpec lea{"fixture", "4C 8D 35 ? ? ? ?", "lea-disp", ""};
    Reports builtins;
    CHECK(s2consumer::ResolveBuiltinOffset("IGameSystem_InitAllSystems_pFirst", lea, retained,
              offset, why, builtins.callback(), data.evaluator()) && offset == 0x2800,
          "lea-disp built-in resolves mapped BSS and returns legacy offset");
    CHECK(retained.size() == 1 && retained[0].image == data.image &&
              retained[0].validation_receipt.find("MappedAddress") != std::string::npos,
          "built-in retains mapped resolution image and receipt");
    CHECK(builtins.rows.size() == 1 && std::get<0>(builtins.rows[0]) ==
              "IGameSystem_InitAllSystems_pFirst" && std::get<1>(builtins.rows[0]),
          "successful built-in reports its own name");

    Fixture below;
    below.lea(0x1000, 0x500);
    below.freeze();
    retained.clear(); builtins.rows.clear();
    CHECK(s2consumer::ResolveBuiltinOffset("BelowText", lea, retained, offset, why,
              builtins.callback(), below.evaluator()) && offset == -0xb00,
          "mapped built-in below text returns a checked negative legacy offset");

    Fixture ctor;
    ctor.lea(0x1000, 0x3100);
    ctor.file[0x1007] = 0xe8;
    put(ctor.file, 0x1008, uint32_t(int32_t(0x1200 - 0x100c)), 4);
    ctor.freeze();
    SigSpec ctor_spec{"fixture", "55 48 89 E5 C3", "ctor-body-xref", ""};
    retained.clear(); builtins.rows.clear();
    CHECK(s2consumer::ResolveBuiltinOffset("GameEventManager", ctor_spec, retained, offset, why,
              builtins.callback(), ctor.evaluator()) && offset == 0x2100 &&
              retained[0].validation_receipt.find("MappedAddress") != std::string::npos,
          "ctor-body-xref built-in explicitly resolves mapped data");

    SigSpec direct{"fixture", "41 56 55 C3", "direct", "{}"};
    retained.clear(); builtins.rows.clear();
    CHECK(s2consumer::ResolveBuiltinOffset("DirectFunction", direct, retained, offset, why,
              builtins.callback(), data.evaluator()) && offset == 0x300 &&
              retained[0].validation_receipt.find("Executable") != std::string::npos,
          "direct built-in remains executable");
}

void sdkhook_slot_and_call_records() {
    Fixture f;
    f.freeze();
    put(f.live, 0x3000, 0xf00000, 8);
    put(f.live, 0x3008, Fixture::base + 0x1200, 8);
    s2resolve::Resolution resolved;
    resolved.address = Fixture::base + 0x1300;
    resolved.image = f.image;
    resolved.recipe = "recipe";
    resolved.validation_receipt = "receipt";
    bool slot_read = false;
    int original_calls = 0;
    int slot = -1;
    std::string why;
    CHECK(s2consumer::FindSdkhookSlot(resolved,
              reinterpret_cast<void**>(Fixture::base + 0x3000), 2, slot, why,
              [&f, &slot_read](uintptr_t at, void* out, size_t count) {
                  slot_read = true;
                  return f.read_live(at, out, count);
              },
              [&slot_read, &original_calls](void**, int index) -> void* {
                  CHECK(slot_read, "SDKHook slot is bounded-readable before original lookup");
                  ++original_calls;
                  return index == 0 ? reinterpret_cast<void*>(Fixture::base + 0x1300) : nullptr;
              }) && slot == 0 && original_calls == 1,
          "SDKHook slot discovery compares the original peer-patched virtual");

    slot_read = false; original_calls = 0; slot = -1;
    CHECK(!s2consumer::FindSdkhookSlot(resolved,
              reinterpret_cast<void**>(Fixture::base + 0x3ff8), 2, slot, why,
              [&f, &slot_read](uintptr_t at, void* out, size_t count) {
                  slot_read = true;
                  return f.read_live(at, out, count);
              },
              [&original_calls](void**, int index) -> void* {
                  ++original_calls;
                  return index == 0 ? reinterpret_cast<void*>(Fixture::base + 0x1200) : nullptr;
              }) &&
              original_calls == 1 && why.find("not a vtable slot") != std::string::npos,
          "SDKHook slot discovery stops before an unreadable next slot");

    s2consumer::CallRecords calls(2);
    CHECK(calls.Add(resolved, why) == 0, "first retained call gets id zero");
    auto duplicate = resolved;
    duplicate.validation_receipt = "replacement";
    CHECK(calls.Add(duplicate, why) == 0, "same address reuses numeric call id");
    auto second = resolved;
    second.address = Fixture::base + 0x1200;
    second.validation_receipt = "second";
    CHECK(calls.Add(second, why) == 1 && calls.Address(1) == second.address,
          "second retained call gets the next numeric id");
    s2resolve::Resolution copy;
    CHECK(calls.CopyForAddress(reinterpret_cast<const void*>(resolved.address), copy) &&
              copy.image == f.image && copy.validation_receipt == "receipt",
          "address lookup returns a stable copied resolution after vector growth");
    auto third = resolved; third.address = Fixture::base + 0x1210;
    CHECK(calls.Add(third, why) == -1 && why == "engine-call table is full",
          "retained call table preserves its configured cap");
}

} // namespace

int main() {
    engine_descriptor_delegation();
    named_failure_and_builtin_offsets();
    sdkhook_slot_and_call_records();
    if (failures) {
        std::cerr << "engine_consumer_test: " << failures << " failures\n";
        return 1;
    }
    std::cout << "engine_consumer_test: all passed\n";
}
