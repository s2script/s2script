// Focused contract tests for the OS-neutral shim platform boundary. These run on Linux, exercise
// the real Linux module/memory backends, and drive the shared PE parser with a synthetic loaded
// image so Windows image logic is testable without a Windows host.
#include "../src/platform/memory.h"
#include "../src/platform/module.h"
#include "../src/platform/paths.h"
#include "../src/platform/pe_image.h"

#include <cstdint>
#include <cstring>
#include <iostream>
#include <limits>
#include <string>
#include <vector>

static int g_fail = 0;
#define CHECK(cond, msg)                                                     \
    do {                                                                     \
        if (!(cond)) { std::cerr << "FAIL: " << (msg) << "\n"; g_fail++; }   \
        else         { std::cout << "ok:   " << (msg) << "\n"; }             \
    } while (0)

static void Put16(std::vector<uint8_t>& image, size_t at, uint16_t value) {
    std::memcpy(image.data() + at, &value, sizeof(value));
}

static void Put32(std::vector<uint8_t>& image, size_t at, uint32_t value) {
    std::memcpy(image.data() + at, &value, sizeof(value));
}

static void Put64(std::vector<uint8_t>& image, size_t at, uint64_t value) {
    std::memcpy(image.data() + at, &value, sizeof(value));
}

static void PutSection(std::vector<uint8_t>& image, size_t at, const char* name,
                       uint32_t virtualSize, uint32_t virtualAddress,
                       uint32_t characteristics) {
    std::memcpy(image.data() + at, name, std::strlen(name));
    Put32(image, at + 8, virtualSize);
    Put32(image, at + 12, virtualAddress);
    Put32(image, at + 16, virtualSize);
    Put32(image, at + 36, characteristics);
}

// A synthetic PE32+ image in its LOADED layout: section bytes live at VirtualAddress, exactly as
// they do in a process after the Windows loader has mapped the image.
static std::vector<uint8_t> SyntheticPe(bool executableNameDecoy = false,
                                        bool secondaryLocator = false) {
    constexpr uint32_t kPe = 0x80;
    constexpr uint32_t kOptional = kPe + 24;
    constexpr uint32_t kSections = kOptional + 0xF0;
    constexpr uint32_t kExecute = 0x20000000;
    constexpr uint32_t kRead = 0x40000000;

    std::vector<uint8_t> image(0x1000);
    image[0] = 'M'; image[1] = 'Z';
    Put32(image, 0x3c, kPe);
    image[kPe] = 'P'; image[kPe + 1] = 'E';
    Put16(image, kPe + 4, 0x8664);       // AMD64
    Put16(image, kPe + 6, 3);            // section count
    Put16(image, kPe + 20, 0xF0);        // optional-header size
    Put16(image, kOptional, 0x20b);      // PE32+
    Put32(image, kOptional + 56, static_cast<uint32_t>(image.size())); // SizeOfImage

    PutSection(image, kSections, ".text", 0x100, 0x200, kExecute | kRead);
    PutSection(image, kSections + 40, ".xlong", 0x180, 0x400, kExecute | kRead);
    PutSection(image, kSections + 80, ".rdata", 0x300, 0x700, kRead);

    // MSVC x64 RTTI chain for class CTest:
    // TypeDescriptor{name at +16} <- CompleteObjectLocator::pTypeDescriptor (RVA)
    // <- vtable[-1] absolute pointer. FindMsvcVTableInPe returns vtable[0].
    const uintptr_t base = reinterpret_cast<uintptr_t>(image.data());
    constexpr uint32_t kTypeDescriptor = 0x720;
    constexpr uint32_t kCol = 0x780;
    constexpr uint32_t kVtableLocator = 0x7c0;
    const char decorated[] = ".?AVCTest@@";
    if (executableNameDecoy)
        std::memcpy(image.data() + 0x230, decorated, sizeof(decorated));
    std::memcpy(image.data() + kTypeDescriptor + 16, decorated, sizeof(decorated));
    if (secondaryLocator) {
        constexpr uint32_t kSecondaryCol = 0x760;
        constexpr uint32_t kSecondaryVtableLocator = 0x7a0;
        Put32(image, kSecondaryCol, 1);
        Put32(image, kSecondaryCol + 4, 8);              // non-primary offset-to-complete-object
        Put32(image, kSecondaryCol + 12, kTypeDescriptor);
        Put32(image, kSecondaryCol + 20, kSecondaryCol);
        Put64(image, kSecondaryVtableLocator, base + kSecondaryCol);
        Put64(image, kSecondaryVtableLocator + 8, base + 0x220);
    }
    Put32(image, kCol, 1);                         // x64 COL signature
    Put32(image, kCol + 4, 0);                     // primary-vtable complete-object offset
    Put32(image, kCol + 12, kTypeDescriptor);      // pTypeDescriptor RVA
    Put32(image, kCol + 20, kCol);                 // pSelf RVA
    Put64(image, kVtableLocator, base + kCol);      // vtable[-1]
    Put64(image, kVtableLocator + 8, base + 0x210); // vtable[0], arbitrary .text fn
    return image;
}

static void test_addon_root_is_derived_without_host_cwd() {
    CHECK(s2platform::AddonRootFromModulePath(
              "/srv/csgo/addons/s2script/bin/linuxsteamrt64/s2script.so") ==
              "/srv/csgo/addons/s2script",
          "Linux addon root is three parent directories above the shim");
    CHECK(s2platform::AddonRootFromModulePath(
              R"(C:\cs2\game\csgo\addons\s2script\bin\win64\s2script.dll)") ==
              R"(C:\cs2\game\csgo\addons\s2script)",
          "Windows addon root preserves the native separator");
    CHECK(s2platform::AddonRootFromModulePath("s2script.so").empty(),
          "a path without three parents degrades to no addon root");
}

static void test_synthetic_pe_ranges_and_rtti() {
    std::vector<uint8_t> image = SyntheticPe();
    s2platform::ModuleView view;
    CHECK(s2platform::ParsePeImage(image.data(), image.size(), &view),
          "a bounds-checked PE32+ image parses");
    CHECK(view.text == image.data() + 0x400 && view.textSize == 0x180,
          "the largest executable PE section wins");
    CHECK(view.lo == image.data() && view.hi == image.data() + image.size(),
          "the PE module view spans SizeOfImage");

    s2platform::ModuleView containing;
    CHECK(s2platform::PeModuleViewForAddress(
              image.data(), image.size(), image.data() + 0x210, &containing),
          "an address in a smaller executable PE section is recognized");
    CHECK(containing.text == image.data() + 0x200 && containing.textSize == 0x100,
          "address lookup returns its containing executable section, not the largest one");
    CHECK(!s2platform::PeModuleViewForAddress(
              image.data(), image.size(), image.data() + 0x710, &containing),
          "address lookup rejects a readable non-executable PE section");

    void** vtable = s2platform::FindMsvcVTableInPe(image.data(), image.size(), "CTest");
    CHECK(vtable == reinterpret_cast<void**>(image.data() + 0x7c8),
          "MSVC RTTI resolves decorated name through COL to primary vtable");
    CHECK(vtable && (*vtable == image.data() + 0x210),
          "the resolved synthetic vtable exposes its first function slot");

    std::vector<uint8_t> withSecondary = SyntheticPe(false, true);
    CHECK(s2platform::FindMsvcVTableInPe(
              withSecondary.data(), withSecondary.size(), "CTest") ==
              reinterpret_cast<void**>(withSecondary.data() + 0x7c8),
          "a secondary COL before the primary is skipped by its nonzero object offset");

    std::vector<uint8_t> withExecutableDecoy = SyntheticPe(true, false);
    CHECK(s2platform::FindMsvcVTableInPe(
              withExecutableDecoy.data(), withExecutableDecoy.size(), "CTest") ==
              reinterpret_cast<void**>(withExecutableDecoy.data() + 0x7c8),
          "an RTTI-looking name in executable code cannot shadow readable image RTTI");

    image.resize(0x90);
    CHECK(!s2platform::ParsePeImage(image.data(), image.size(), &view),
          "a truncated PE image degrades without an out-of-bounds read");
}

static void test_live_linux_module_queries() {
    s2platform::ModuleView byAddress;
    CHECK(s2platform::ModuleViewForAddress(
              reinterpret_cast<const void*>(&test_live_linux_module_queries), &byAddress),
          "the backend finds the module containing a live function");
    CHECK(byAddress.ContainsExecutable(
              reinterpret_cast<const void*>(&test_live_linux_module_queries)),
          "the returned executable range contains that function");
    CHECK(s2platform::IsExecutableAddress(
              reinterpret_cast<const void*>(&test_live_linux_module_queries)),
          "the module-agnostic executable-address query accepts code");
    CHECK(!s2platform::IsExecutableAddress(nullptr),
          "the executable-address query rejects null");

    int readable = 0;
    CHECK(s2platform::IsReadableAddress(&readable, sizeof(readable)),
          "the readable-address query accepts a mapped object");
    const uintptr_t overflowing = std::numeric_limits<uintptr_t>::max() - 7;
    CHECK(!s2platform::IsReadableAddress(reinterpret_cast<const void*>(overflowing), 16),
          "the readable-address query rejects a range that wraps uintptr_t");
}

static void test_executable_memory_round_trip() {
    const size_t size = s2platform::PageSize();
    CHECK(size >= 4096, "the platform reports a usable page size");
    void* allocation = s2platform::AllocateExecutable(size);
    CHECK(allocation != nullptr, "an executable trampoline page allocates");
    if (!allocation) return;
    CHECK(!s2platform::ProtectMemory(
              reinterpret_cast<void*>(std::numeric_limits<uintptr_t>::max() - 7), 16,
              s2platform::MemoryProtection::ReadOnly),
          "memory protection rejects a range whose aligned end would overflow");

#if defined(__x86_64__) || defined(_M_X64)
    // mov eax,42; ret
    const uint8_t code[] = { 0xB8, 42, 0, 0, 0, 0xC3 };
    std::memcpy(allocation, code, sizeof(code));
    CHECK(s2platform::ProtectMemory(allocation, size, s2platform::MemoryProtection::ReadExecute),
          "a trampoline transitions from writable to executable");
    s2platform::FlushInstructionCache(allocation, sizeof(code));
    using Fn = int (*)();
    CHECK(reinterpret_cast<Fn>(allocation)() == 42,
          "generated code executes after protection and cache synchronization");
#endif

    s2platform::FreeExecutable(allocation, size);
}

int main() {
    test_addon_root_is_derived_without_host_cwd();
    test_synthetic_pe_ranges_and_rtti();
    test_live_linux_module_queries();
    test_executable_memory_round_trip();
    if (g_fail) { std::cerr << "\n" << g_fail << " check(s) FAILED\n"; return 1; }
    std::cout << "\nall platform backend checks passed\n";
    return 0;
}
