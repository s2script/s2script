#include "engine_function_copy.h"
#include <algorithm>
#include <cassert>
#include <cmath>
#include <cstring>
#include <iostream>
#include <limits>
#include <string>
#include <vector>
#ifdef __linux__
#include <sys/mman.h>
#include <unistd.h>
#include <cerrno>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#endif
using namespace s2fn::copy;
static std::size_t checks = 0;
#define CHECK(x) do { ++checks; if (!(x)) { std::cerr << __LINE__ << ": " #x "\n"; std::abort(); } } while (0)
template<class T> static void error(const s2fn::Result<T>& r, const char* name) {
    CHECK(!r); CHECK(r.error.find(name) != std::string::npos);
}
static StableOwner owner(unsigned id, Domain domain = Domain::Plugin) {
    StableOwner key; key.domain = domain; std::memcpy(key.digest.data(), &id, sizeof id); return key;
}
static Operation begin(Budget& budget, unsigned id = 1, unsigned generation = 1) {
    auto r = budget.Begin({owner(id), generation}); CHECK(r); return std::move(r.value);
}
static Snapshot str(const Operation& op, const std::string& text) {
    auto r = Snapshot::FromBytes(op, Kind::String, text.data(), text.size()); CHECK(r); return std::move(r.value);
}
struct Fake {
    std::uintptr_t base = 0x10000;
    std::vector<std::uint8_t> bytes;
    std::size_t calls = 0, largest = 0;
    bool deny = false;
    static std::size_t read(void* ctx, std::uintptr_t address, void* out, std::size_t n) {
        auto& f = *static_cast<Fake*>(ctx); ++f.calls; f.largest = std::max(f.largest, n);
        CHECK(n <= 4096 && address / 4096 == (address + n - 1) / 4096);
        if (f.deny || address < f.base || address - f.base >= f.bytes.size()) return 0;
        n = std::min(n, f.bytes.size() - (address - f.base));
        std::memcpy(out, f.bytes.data() + (address - f.base), n); return n;
    }
    Reader reader() { return {read, this, 4096, true}; }
};
static void capture_tests() {
    Budget budget; auto host = budget.Begin({}); CHECK(host); auto op = std::move(host.value);
    auto empty = str(op, ""); CHECK(empty.size() == 0 && empty.data()[0] == 0);
    auto text = str(op, "hello\xf0\x9f\x98\x80"); CHECK(text.size() == 9);
    const char* invalid[] = {"\x80", "\xc0\x80", "\xe0\x80\x80", "\xed\xa0\x80", "\xf4\x90\x80\x80", "\xf5\x80\x80\x80", "\xe2\x82"};
    for (auto s : invalid) error(Snapshot::FromBytes(op, Kind::String, s, std::strlen(s)), "FunctionCopyLifetimeUnsupported");
    error(Snapshot::FromBytes(op, Kind::String, "a\0b", 3), "FunctionCopyLifetimeUnsupported");
    error(Snapshot::FromBytes(op, Kind::String, nullptr, 0), "FunctionCopyLifetimeUnsupported");
    error(Snapshot::FromBytes(op, Kind::String, "", SIZE_MAX), "FunctionCopyTooLarge");
    Fake f; f.base += 4094; f.bytes = {'a', 0};
    auto plugin_operation = begin(budget);
    error(Snapshot::Capture(plugin_operation, Kind::String, f.base, f.reader()), "FunctionCopyLifetimeUnsupported");
    CHECK(f.calls == 0);
    auto capture = Snapshot::Capture(op, Kind::String, f.base, f.reader());
    CHECK(capture && capture.value.size() == 1 && f.calls == 1);
    f.base = 0x10000; f.bytes.assign(65536, 'x'); f.bytes.back() = 0; f.calls = 0;
    capture = Snapshot::Capture(op, Kind::String, f.base, f.reader());
    CHECK(capture && capture.value.size() == 65535 && f.calls == 16 && f.largest == 4096);
    f.bytes.back() = 'x'; error(Snapshot::Capture(op, Kind::String, f.base, f.reader()), "FunctionCopyTooLarge");
    f.bytes = {'o', 'k', 0}; // short read with terminator succeeds
    CHECK(Snapshot::Capture(op, Kind::String, f.base, f.reader()));
    f.bytes = {'o', 'k'}; error(Snapshot::Capture(op, Kind::String, f.base, f.reader()), "FunctionCopyLifetimeUnsupported");
    auto calls = f.calls;
    error(Snapshot::Capture(op, Kind::String, 0, f.reader()), "FunctionCopyLifetimeUnsupported");
    error(Snapshot::Capture(op, Kind::String, UINTPTR_MAX, f.reader()), "FunctionCopyLifetimeUnsupported");
    CHECK(f.calls == calls);
    f.deny = true; error(Snapshot::Capture(op, Kind::String, f.base, f.reader()), "FunctionCopyLifetimeUnsupported");
    auto unavailable = f.reader(); unavailable.available = false; calls = f.calls;
    error(Snapshot::Capture(op, Kind::String, reinterpret_cast<std::uintptr_t>("safe"), unavailable), "FunctionCopyLifetimeUnsupported"); CHECK(f.calls == calls);
    float values[] = {-0.0f, 1.25f, -2.5f};
    auto vec = Snapshot::FromBytes(op, Kind::Vector, values, 12); CHECK(vec && std::memcmp(vec.value.data(), values, 12) == 0);
    error(Snapshot::FromBytes(op, Kind::Vector, values, 11), "FunctionCopyLifetimeUnsupported");
    values[2] = std::numeric_limits<float>::infinity(); error(Snapshot::FromBytes(op, Kind::Vector, values, 12), "FunctionCopyLifetimeUnsupported");
    values[2] = std::numeric_limits<float>::quiet_NaN(); error(Snapshot::FromBytes(op, Kind::Vector, values, 12), "FunctionCopyLifetimeUnsupported");
    values[2] = 3.0f; f.deny = false; f.bytes.resize(12); std::memcpy(f.bytes.data(), values, 12);
    CHECK(Snapshot::Capture(op, Kind::Vector, f.base, f.reader()));
    f.bytes.resize(11); error(Snapshot::Capture(op, Kind::Vector, f.base, f.reader()), "FunctionCopyLifetimeUnsupported");
    auto before = budget.Read().bytes;
    { auto shared = text; auto moved = std::move(shared); CHECK(budget.Read().bytes == before); CHECK(moved.size() == 9); }
    CHECK(budget.Read().bytes == before);
    Budget full; auto full_host = full.Begin({}); CHECK(full_host);
    auto fill = full_host.value.Reserve(8 * MiB - full.Read().bytes); CHECK(fill);
    calls = f.calls;
    error(Snapshot::Capture(full_host.value, Kind::String, f.base, f.reader()), "FunctionCopyBudgetExceeded");
    CHECK(f.calls == calls);
    // Later mutation/free of a valid capture source cannot change the owned copy.
    CHECK(capture.value.size() == 65535 && capture.value.data()[0] == 'x');
}
static void budget_tests() {
    Budget budget;
    {
        auto first = begin(budget); auto first_charge = budget.Read().bytes;
        auto second = begin(budget); auto second_charge = budget.Read().bytes - first_charge;
        CHECK(first_charge > second_charge); // occupied owner-row retention is charged once
        first = {}; CHECK(budget.Read().bytes == first_charge);
    }
    CHECK(budget.Read().bytes == 0);
    auto op = begin(budget); auto base = budget.Read().bytes; CHECK(base > 0);
    error(op.Reserve(8 * MiB), "FunctionCopyBudgetExceeded");
    auto all = op.Reserve(8 * MiB - base); CHECK(all); CHECK(budget.Read().bytes == 8 * MiB);
    auto sibling = begin(budget, 1, 2); CHECK(sibling.Reserve(1));
    error(budget.Begin({owner(1), 1}), "FunctionCopyBudgetExceeded");
    all.value = {}; CHECK(budget.Read().bytes == 2 * base);
    {
        auto copy = op; op = {}; CHECK(budget.Read().items == 2);
        auto lease = copy.Reserve(321); CHECK(lease); copy = {};
        CHECK(budget.Read().items == 2); lease.value = {}; CHECK(budget.Read().items == 1);
    }
    sibling = {}; CHECK(budget.Read().bytes == 0 && budget.Read().owners == 0);
    std::vector<Operation> operations;
    for (unsigned i = 0; i < 128; ++i) operations.push_back(begin(budget));
    error(budget.Begin({owner(1), 1}), "FunctionCopyBudgetExceeded");
    for (unsigned i = 128; i < 1024; ++i) operations.push_back(begin(budget, i));
    error(budget.Begin({owner(2000), 1}), "FunctionCopyBudgetExceeded");
    operations.clear(); CHECK(budget.Read().items == 0 && budget.Read().owners == 0);
    // Row reuse is tied to final live leases, independent of generation churn.
    for (unsigned i = 0; i < 2000; ++i) { auto one = begin(budget, i, i); }
    std::vector<Lease> leases;
    for (unsigned i = 0; i < 4; ++i) {
        auto one = begin(budget, i); auto reserve = one.Reserve(8 * MiB - base); CHECK(reserve);
        leases.push_back(std::move(reserve.value));
    }
    CHECK(budget.Read().bytes == 32 * MiB); error(budget.Begin({owner(9999), 1}), "FunctionCopyBudgetExceeded");
    leases.clear(); auto metrics = budget.Read();
    CHECK(metrics.bytes == 0 && metrics.items == 0 && metrics.owners == 0);
    CHECK(metrics.high_bytes == 32 * MiB && metrics.high_items == 1024 && metrics.rejected >= 5);
    Snapshot saved;
    { auto one = begin(budget); saved = str(one, "retained"); }
    CHECK(budget.Read().items == 1 && budget.Read().bytes > base);
    saved = {}; CHECK(budget.Read().bytes == 0);
}
static void same_counters(PermanentMetrics a, PermanentMetrics b) {
    CHECK(a.bytes == b.bytes && a.values == b.values && a.owners == b.owners);
}
static s2fn::Result<bool> batch(Arena& arena, const Operation& op, StableOwner key, const Snapshot* values, std::size_t count, const void** outputs) {
    std::array<StableOwner, MaxBatch> owners; owners.fill(key);
    return arena.Intern(op, owners.data(), values, count, outputs);
}
static const void* intern(Arena& arena, const Operation& op, StableOwner key, const Snapshot& s) {
    const void* output = nullptr; CHECK(batch(arena, op, key, &s, 1, &output)); return output;
}
static void arena_tests(const std::string& mode) {
    Budget budget; auto op = begin(budget); auto& arena = Arena::Resident();
    if (mode == "arena") {
        auto text = str(op, "saved"); Snapshot values[] = {text, text, str(op, "other")};
        const void* outputs[3]{}; CHECK(batch(arena, op, owner(1), values, 3, outputs));
        CHECK(outputs[0] == outputs[1] && outputs[0] != outputs[2]);
        CHECK(reinterpret_cast<std::uintptr_t>(outputs[0]) % 16 == 0);
        auto initial = arena.Read(); CHECK(initial.values == 2 && initial.bytes == 32 && initial.owners == 1);
        auto generation2 = begin(budget, 1, 2); CHECK(intern(arena, generation2, owner(2), text) == outputs[0]); same_counters(initial, arena.Read());
        Snapshot invalid[] = {str(op, "unpublished"), {}};
        const void* unchanged[] = {outputs[0], outputs[2]};
        error(batch(arena, op, owner(1), invalid, 2, unchanged), "FunctionCopyLifetimeUnsupported");
        CHECK(unchanged[0] == outputs[0] && unchanged[1] == outputs[2]); same_counters(initial, arena.Read());
        // Equal exact bytes in different kinds must never alias (12 ASCII bytes
        // happen to represent three finite f32 values).
        auto bytes = str(op, "abcdefghijkl"); auto vector = Snapshot::FromBytes(op, Kind::Vector, bytes.data(), 12); CHECK(vector);
        CHECK(intern(arena, op, owner(1), bytes) != intern(arena, op, owner(1), vector.value));
        const void* escaped;
        { auto scope = begin(budget, 3); auto temp = str(scope, "survives heap scope"); escaped = intern(arena, scope, owner(3), temp); }
        for (int i = 0; i < 10000; ++i) { std::string churn(10000, static_cast<char>(i)); CHECK(churn.size() == 10000); }
        CHECK(std::strcmp(static_cast<const char*>(escaped), "survives heap scope") == 0);
        CHECK(&Arena::Resident() == &arena && intern(Arena::Resident(), op, owner(5), text) == outputs[0]);
        Snapshot mixed[] = {str(op, "mixed A"), str(op, "mixed B"), str(op, "mixed A")};
        StableOwner mixed_owners[] = {owner(20), owner(21), owner(22)};
        const void* mixed_out[3]{}; auto mixed_before = arena.Read();
        CHECK(arena.Intern(op, mixed_owners, mixed, 3, mixed_out));
        CHECK(mixed_out[0] == mixed_out[2] && mixed_out[0] != mixed_out[1]);
        CHECK(arena.Read().owners == mixed_before.owners + 2 && arena.Read().values == mixed_before.values + 2);
        std::array<Snapshot, 33> complete; complete.fill(text);
        std::array<StableOwner, 33> complete_owners; complete_owners.fill(owner(30));
        const void* complete_out[33]{};
        CHECK(arena.Intern(op, complete_owners.data(), complete.data(), 33, complete_out));
        CHECK(complete_out[32] == outputs[0]);
        error(arena.Intern(op, complete_owners.data(), complete.data(), 34, complete_out), "FunctionCopyLifetimeUnsupported");
        auto before = arena.Read(); auto charge = budget.Read().bytes; auto fill = op.Reserve(8 * MiB - charge); CHECK(fill);
        const void* output = escaped; error(batch(arena, op, owner(1), &text, 1, &output), "FunctionCopyBudgetExceeded");
        CHECK(output == escaped); same_counters(before, arena.Read());
    } else if (mode == "owner-bytes") {
        for (unsigned i = 0; i < 64; ++i) { std::string s(65535, 'x'); s.replace(0, std::to_string(i).size(), std::to_string(i)); intern(arena, op, owner(1), str(op, s)); }
        CHECK(arena.Read().bytes == 4 * MiB);
        auto old = str(op, std::string(65535, 'x').replace(0, 1, "0")); auto existing = intern(arena, op, owner(2), old);
        auto extra = str(op, "new"); Snapshot values[] = {old, extra}; const void* out[2] = {existing, existing}; auto before = arena.Read();
        auto reload = begin(budget, 1, 77); error(batch(arena, reload, owner(1), values, 2, out), "owner payload");
        CHECK(out[0] == existing && out[1] == existing); same_counters(before, arena.Read());
        CHECK(intern(arena, reload, owner(1), old) == existing);
        CHECK(intern(arena, op, owner(1, Domain::HostPackage), extra));
    } else if (mode == "owner-rows") {
        auto shared = str(op, "0"); const void* first = nullptr;
        for (unsigned i = 0; i < 1024; ++i) { auto p = intern(arena, op, owner(i), str(op, std::to_string(i))); if (!i) first = p; }
        auto before = arena.Read(); CHECK(before.owners == 1024); auto extra = str(op, "new"); const void* out = first;
        error(batch(arena, op, owner(1024), &extra, 1, &out), "owner rows"); same_counters(before, arena.Read());
        CHECK(intern(arena, op, owner(1024), shared) == first);
    } else if (mode == "values") {
        for (unsigned i = 0; i < 32768; ++i) intern(arena, op, owner(i / 8192), str(op, std::to_string(i)));
        auto before = arena.Read(); CHECK(before.values == 32768); auto extra = str(op, "new"); const void* out = nullptr;
        error(batch(arena, op, owner(4), &extra, 1, &out), "value count"); same_counters(before, arena.Read());
        CHECK(intern(arena, op, owner(4), str(op, "0")));
    } else if (mode == "owner-values") {
        for (unsigned i = 0; i < 8192; ++i) intern(arena, op, owner(1), str(op, std::to_string(i)));
        auto before = arena.Read(); auto extra = str(op, "new"); const void* out = nullptr;
        error(batch(arena, op, owner(1), &extra, 1, &out), "owner values"); same_counters(before, arena.Read());
        CHECK(intern(arena, op, owner(1), str(op, "0")));
        Snapshot mixed[] = {str(op, "from new owner"), str(op, "from exhausted owner")};
        StableOwner mixed_owners[] = {owner(2), owner(1)};
        const void* untouched[] = {reinterpret_cast<void*>(1), reinterpret_cast<void*>(2)};
        auto current = arena.Read();
        error(arena.Intern(op, mixed_owners, mixed, 2, untouched), "owner values");
        same_counters(current, arena.Read());
        CHECK(untouched[0] == reinterpret_cast<void*>(1) && untouched[1] == reinterpret_cast<void*>(2));
    } else if (mode == "payload") {
        for (unsigned i = 0; i < 224; ++i) { std::string s(65535, 'x'); s.replace(0, std::to_string(i).size(), std::to_string(i)); intern(arena, op, owner(i / 64), str(op, s)); }
        auto before = arena.Read(); CHECK(before.bytes == 14 * MiB); auto extra = str(op, "new"); const void* out = nullptr;
        error(batch(arena, op, owner(4), &extra, 1, &out), "global payload"); same_counters(before, arena.Read());
        CHECK(intern(arena, op, owner(4), str(op, std::string(65535, 'x').replace(0, 1, "0"))));
    } else { CHECK(false); }
}
static void linux_denied_tests(bool missing) {
#ifdef __linux__
    // A real kernel filter denies the actual self-reader syscall, before its
    // availability probe. Each mode is a separate short-lived test process.
    const unsigned refusal = missing ? ENOSYS : EPERM;
    sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(seccomp_data, nr)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_process_vm_readv, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | refusal),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog program{static_cast<unsigned short>(sizeof filter / sizeof filter[0]), filter};
    CHECK(prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == 0);
    CHECK(prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program) == 0);
    auto reader = SystemReader(); CHECK(!reader.available);
    auto op = NativeBudget().Begin({}); CHECK(op);
    error(Snapshot::Capture(op.value, Kind::String, reinterpret_cast<std::uintptr_t>("valid accessible string"), reader), "FunctionCopyLifetimeUnsupported");
    std::cout << "Actual Linux seccomp denial: " << (missing ? "ENOSYS" : "EPERM") << ", no fallback\n";
#else
    (void)missing;
    std::cout << "SKIP actual Linux seccomp denial on macOS\n";
#endif
}
static void linux_tests() {
#ifdef __linux__
    auto reader = SystemReader(); CHECK(reader.available); Budget budget; auto host = budget.Begin({}); CHECK(host); auto op = std::move(host.value);
    auto page = static_cast<std::size_t>(sysconf(_SC_PAGESIZE));
    auto* memory = static_cast<std::uint8_t*>(mmap(nullptr, 65536 + 2 * page, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0));
    CHECK(memory != MAP_FAILED); CHECK(mprotect(memory + 65536, page * 2, PROT_NONE) == 0);
    auto capture = [&](Kind kind, void* p) { return Snapshot::Capture(op, kind, reinterpret_cast<std::uintptr_t>(p), reader); };
    std::memset(memory, 'x', 65536); memory[65535] = 0;
    auto max = capture(Kind::String, memory); CHECK(max && max.value.size() == 65535);
    memory[65535] = 'x'; error(capture(Kind::String, memory), "FunctionCopyTooLarge");
    memory[65535] = 0; CHECK(capture(Kind::String, memory + 65535));
    error(capture(Kind::String, memory + 65536), "FunctionCopyLifetimeUnsupported");
    memory[65534] = 0x80; error(capture(Kind::String, memory + 65534), "FunctionCopyLifetimeUnsupported");
    float vec[] = {-0.0f, 2.0f, 3.0f}; std::memcpy(memory + 65536 - 12, vec, 12);
    auto v = capture(Kind::Vector, memory + 65536 - 12); CHECK(v && std::memcmp(v.value.data(), vec, 12) == 0);
    error(capture(Kind::Vector, memory + 65536 - 11), "FunctionCopyLifetimeUnsupported");
    error(capture(Kind::String, reinterpret_cast<void*>(UINTPTR_MAX)), "FunctionCopyLifetimeUnsupported");
    CHECK(munmap(memory, 65536 + 2 * page) == 0);
    std::cout << "Actual Linux process_vm_readv guarded-page evidence\n";
#else
    CHECK(!SystemReader().available);
    std::cout << "SKIP actual Linux reader: macOS production reader explicitly unsupported\n";
#endif
}
static void indirect_tests() {
    Budget budget; auto host = budget.Begin({}); CHECK(host); auto op = std::move(host.value);
    Fake f; f.bytes.assign(0x20000, 0);
    auto word = [&](std::size_t at, std::uintptr_t value) { std::memcpy(f.bytes.data() + at, &value, sizeof value); };
    auto text = [&](std::size_t at, const std::string& s) { std::memcpy(f.bytes.data() + at, s.data(), s.size()); f.bytes[at + s.size()] = 0; };
    // Null char* word: an empty, NUL-terminated string without reading any text.
    auto empty = Snapshot::CaptureIndirect(op, f.base, f.reader());
    CHECK(empty && empty.value.kind() == Kind::String && empty.value.size() == 0 && empty.value.data()[0] == 0);
    // Pointer word -> captured bytes, immune to later native mutation.
    text(0x100, "hello\xf0\x9f\x98\x80"); word(0, f.base + 0x100);
    auto ok = Snapshot::CaptureIndirect(op, f.base, f.reader());
    CHECK(ok && ok.value.size() == 9 && std::memcmp(ok.value.data(), "hello", 5) == 0);
    f.bytes[0x100] = 'j'; CHECK(ok.value.data()[0] == 'h');
    // A word straddling a page boundary is read in two page-bounded pieces.
    word(0x0ffc, f.base + 0x100); auto calls = f.calls;
    auto straddle = Snapshot::CaptureIndirect(op, f.base + 0x0ffc, f.reader());
    CHECK(straddle && straddle.value.size() == 9 && f.calls == calls + 3);
    // Engine-domain capture only: a plugin operation performs no native read.
    auto plugin = begin(budget); calls = f.calls;
    error(Snapshot::CaptureIndirect(plugin, f.base, f.reader()), "FunctionCopyLifetimeUnsupported"); CHECK(f.calls == calls);
    // Null / overflowing / unreadable object and unavailable reader are denials, never dereferences.
    error(Snapshot::CaptureIndirect(op, 0, f.reader()), "FunctionCopyLifetimeUnsupported");
    error(Snapshot::CaptureIndirect(op, UINTPTR_MAX - 3, f.reader()), "FunctionCopyLifetimeUnsupported");
    CHECK(f.calls == calls);
    error(Snapshot::CaptureIndirect(op, f.base + f.bytes.size() - 4, f.reader()), "FunctionCopyLifetimeUnsupported");
    error(Snapshot::CaptureIndirect(op, 0x20, f.reader()), "FunctionCopyLifetimeUnsupported");
    auto unavailable = f.reader(); unavailable.available = false; calls = f.calls;
    error(Snapshot::CaptureIndirect(op, f.base, unavailable), "FunctionCopyLifetimeUnsupported"); CHECK(f.calls == calls);
    f.deny = true; error(Snapshot::CaptureIndirect(op, f.base, f.reader()), "FunctionCopyLifetimeUnsupported"); f.deny = false;
    // Word points at unreadable memory.
    word(0, 0x40); error(Snapshot::CaptureIndirect(op, f.base, f.reader()), "FunctionCopyLifetimeUnsupported");
    // Oversize: no NUL within 65536 bytes of the pointed-to text.
    std::memset(f.bytes.data() + 0x1000, 'x', 0x10000); word(0, f.base + 0x1000);
    error(Snapshot::CaptureIndirect(op, f.base, f.reader()), "FunctionCopyTooLarge");
    f.bytes[0x1000 + 65535] = 0;
    auto max = Snapshot::CaptureIndirect(op, f.base, f.reader()); CHECK(max && max.value.size() == 65535);
    // Invalid UTF-8 (overlong NUL, lone continuation, surrogate) is refused.
    for (auto bad : {"\xc0\x80", "\x80", "\xed\xa0\x80", "ok\xe2\x82"}) {
        text(0x200, bad); word(0, f.base + 0x200);
        error(Snapshot::CaptureIndirect(op, f.base, f.reader()), "FunctionCopyLifetimeUnsupported");
    }
    std::cout << "PASS string-indirect null word, capture, straddle, denial, oversize and UTF-8 rejection\n";
}
int main(int argc, char** argv) {
    const std::string mode = argc > 1 ? argv[1] : "capture";
    if (mode == "capture") capture_tests(); else if (mode == "indirect") indirect_tests(); else if (mode == "budget") budget_tests(); else if (mode == "linux") linux_tests(); else if (mode == "linux-denied" || mode == "linux-missing") linux_denied_tests(mode == "linux-missing"); else arena_tests(mode);
    std::cout << mode << ": " << checks << " checks passed\n";
}
