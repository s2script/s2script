#include "engine_function_copy.h"
#include <algorithm>
#include <cmath>
#include <limits>
#include <memory>
#include <new>
#include <sys/mman.h>
#include <unistd.h>
#ifdef __linux__
#include <sys/syscall.h>
#include <sys/uio.h>
#endif

namespace s2fn::copy {
namespace {
constexpr const char* Temporary = "FunctionCopyBudgetExceeded";
constexpr const char* Unsupported = "FunctionCopyLifetimeUnsupported";
constexpr const char* TooLarge = "FunctionCopyTooLarge";
constexpr const char* Permanent = "FunctionCopyPermanentBudgetExceeded";
template<class T> Result<T> Fail(const char* name, const char* reason) {
    return {{}, std::string(name) + ": " + reason};
}
constexpr std::size_t Align16(std::size_t n) { return (n + 15) & ~std::size_t(15); }
bool Range(std::uintptr_t address, std::size_t length) {
    return address && length && length - 1 <= std::numeric_limits<std::uintptr_t>::max() - address;
}
bool Valid(Kind kind, const std::uint8_t* bytes, std::size_t length) {
    if (!bytes) return false;
    if (kind == Kind::Vector) {
        static_assert(sizeof(float) == 4 && std::numeric_limits<float>::is_iec559);
        if (length != 12) return false;
        float components[3]; std::memcpy(components, bytes, 12);
        return std::isfinite(components[0]) && std::isfinite(components[1]) && std::isfinite(components[2]);
    }
    if (kind != Kind::String || length > MaxString) return false;
    // Canonical UTF-8: reject NUL, overlong encodings, surrogates and > U+10FFFF.
    for (std::size_t i = 0; i < length;) {
        std::uint32_t cp = bytes[i++];
        if (!cp) return false;
        if (cp < 0x80) continue;
        unsigned count; std::uint32_t minimum;
        if (cp >= 0xc2 && cp <= 0xdf) { count = 1; minimum = 0x80; cp &= 0x1f; }
        else if (cp >= 0xe0 && cp <= 0xef) { count = 2; minimum = 0x800; cp &= 0x0f; }
        else if (cp >= 0xf0 && cp <= 0xf4) { count = 3; minimum = 0x10000; cp &= 7; }
        else return false;
        if (count > length - i) return false;
        while (count--) { auto next = bytes[i++]; if ((next & 0xc0) != 0x80) return false; cp = (cp << 6) | (next & 0x3f); }
        if (cp < minimum || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) return false;
    }
    return true;
}
} // namespace

struct OperationState {
    std::atomic<std::size_t> refs{1};
    Budget* budget;
    std::size_t row, bytes = sizeof(OperationState);
    OperationState(Budget* b, std::size_t r) : budget(b), row(r) {}
    void Retain() { refs.fetch_add(1, std::memory_order_relaxed); }
    void Uncharge(std::size_t amount) {
        std::lock_guard<std::mutex> lock(budget->mutex_);
        bytes -= amount; budget->owners_[row].bytes -= amount; budget->metrics_.bytes -= amount;
    }
    void Release() {
        if (refs.fetch_sub(1, std::memory_order_acq_rel) != 1) return;
        auto* b = budget; auto r = row;
        // Free the owning allocation before making its admission available again.
        delete this;
        std::lock_guard<std::mutex> lock(b->mutex_);
        auto& owner = b->owners_[r]; owner.bytes -= sizeof(OperationState);
        b->metrics_.bytes -= sizeof(OperationState); --b->metrics_.items;
        if (--owner.items == 0) {
            b->metrics_.bytes -= sizeof(Budget::Row);
            owner = {}; --b->metrics_.owners;
        }
    }
};
Budget& NativeBudget() { static Budget budget; return budget; }
Metrics Budget::Read() const { std::lock_guard<std::mutex> lock(mutex_); return metrics_; }
Result<Operation> Budget::Begin(const OwnerGeneration& key) {
    std::lock_guard<std::mutex> lock(mutex_);
    std::size_t row = owners_.size(), empty = owners_.size();
    for (std::size_t i = 0; i < owners_.size(); ++i) {
        if (!owners_[i].items) { if (empty == owners_.size()) empty = i; }
        else if (owners_[i].key == key) { row = i; break; }
    }
    if (row == owners_.size()) row = empty;
    auto reject = [&]() { ++metrics_.rejected; return Fail<Operation>(Temporary, "operation/owner admission"); };
    if (metrics_.items == ProcessItems || row == owners_.size()) return reject();
    auto& owner = owners_[row];
    const auto charge = sizeof(OperationState) + (owner.items ? 0 : sizeof(Row));
    if (charge > ProcessBytes - metrics_.bytes || owner.items == OwnerItems || charge > OwnerBytes - owner.bytes) return reject();
    // Holding the synchronous admission lock prevents a competing reservation
    // between admission and this first allocation. No growing owner heap table.
    auto* state = new (std::nothrow) OperationState(this, row);
    if (!state) return reject();
    if (!owner.items) { owner.key = key; ++metrics_.owners; }
    ++owner.items; owner.bytes += charge;
    ++metrics_.items; metrics_.bytes += charge;
    metrics_.high_bytes = std::max(metrics_.high_bytes, metrics_.bytes);
    metrics_.high_items = std::max(metrics_.high_items, metrics_.items);
    return {Operation(state), {}};
}
Operation::Operation(const Operation& b) : state_(b.state_) { if (state_) state_->Retain(); }
Operation::Operation(Operation&& b) noexcept : state_(std::exchange(b.state_, nullptr)) {}
Operation& Operation::operator=(const Operation& b) { Operation copy(b); std::swap(state_, copy.state_); return *this; }
Operation& Operation::operator=(Operation&& b) noexcept { Operation moved(std::move(b)); std::swap(state_, moved.state_); return *this; }
Operation::~Operation() { if (state_) state_->Release(); }
bool Operation::HostOwned() const {
    return state_ && state_->budget->owners_[state_->row].key.owner.domain == Domain::Engine;
}
Result<Lease> Operation::Reserve(std::size_t bytes) const {
    if (!state_) return Fail<Lease>(Temporary, "missing operation");
    auto& b = *state_->budget; std::lock_guard<std::mutex> lock(b.mutex_);
    auto& row = b.owners_[state_->row];
    if (bytes > OperationBytes - state_->bytes || bytes > OwnerBytes - row.bytes || bytes > ProcessBytes - b.metrics_.bytes) {
        ++b.metrics_.rejected; return Fail<Lease>(Temporary, "operation/owner/process bytes");
    }
    state_->bytes += bytes; row.bytes += bytes; b.metrics_.bytes += bytes;
    b.metrics_.high_bytes = std::max(b.metrics_.high_bytes, b.metrics_.bytes);
    return {Lease(state_, bytes), {}};
}
Lease::Lease(OperationState* state, std::size_t bytes) : state_(state), bytes_(bytes) { state_->Retain(); }
Lease::Lease(Lease&& b) noexcept : state_(std::exchange(b.state_, nullptr)), bytes_(b.bytes_) {}
Lease& Lease::operator=(Lease&& b) noexcept { Lease moved(std::move(b)); std::swap(state_, moved.state_); std::swap(bytes_, moved.bytes_); return *this; }
Lease::~Lease() { if (state_) { state_->Uncharge(bytes_); state_->Release(); } }

struct alignas(16) SnapshotBlock {
    std::atomic<std::size_t> refs{1};
    Lease lease;
    Kind kind;
    std::size_t length = 0;
    bool capture_ready = false;
    SnapshotBlock(Lease&& charge, Kind k) : lease(std::move(charge)), kind(k) {}
    std::uint8_t* Data() { return reinterpret_cast<std::uint8_t*>(this + 1); }
    void Retain() { refs.fetch_add(1, std::memory_order_relaxed); }
    void Release() {
        if (refs.fetch_sub(1, std::memory_order_acq_rel) != 1) return;
        auto charge = std::move(lease);
        this->~SnapshotBlock(); delete[] reinterpret_cast<std::uint8_t*>(this);
        // charge dies after the allocation, including the refcount/lease/padding.
    }
};
Snapshot::Snapshot(const Snapshot& b) : block_(b.block_) { if (block_) block_->Retain(); }
Snapshot::Snapshot(Snapshot&& b) noexcept : block_(std::exchange(b.block_, nullptr)) {}
Snapshot& Snapshot::operator=(const Snapshot& b) { Snapshot copy(b); std::swap(block_, copy.block_); return *this; }
Snapshot& Snapshot::operator=(Snapshot&& b) noexcept { Snapshot moved(std::move(b)); std::swap(block_, moved.block_); return *this; }
Snapshot::~Snapshot() { if (block_) block_->Release(); }
Kind Snapshot::kind() const { return block_ ? block_->kind : Kind{}; }
const std::uint8_t* Snapshot::data() const { return block_ ? block_->Data() : nullptr; }
std::size_t Snapshot::size() const { return block_ ? block_->length : 0; }
Result<Snapshot> Snapshot::Allocate(const Operation& op, Kind kind, std::size_t capacity) {
    auto bytes = sizeof(SnapshotBlock) + Align16(capacity);
    auto admission = op.Reserve(bytes); if (!admission) return {{}, std::move(admission.error)};
    auto* storage = new (std::nothrow) std::uint8_t[bytes];
    if (!storage) return Fail<Snapshot>(Temporary, "snapshot allocation");
    Snapshot result; result.block_ = new (storage) SnapshotBlock(std::move(admission.value), kind);
    return {std::move(result), {}};
}
Result<Snapshot> Snapshot::FromBytes(const Operation& op, Kind kind, const void* source, std::size_t length) {
    if (kind == Kind::String && length > MaxString) return Fail<Snapshot>(TooLarge, "string content");
    if (!Valid(kind, static_cast<const std::uint8_t*>(source), length)) return Fail<Snapshot>(Unsupported, "invalid copied bytes");
    auto result = Allocate(op, kind, length + (kind == Kind::String)); if (!result) return result;
    std::memcpy(result.value.block_->Data(), source, length); result.value.block_->length = length;
    if (kind == Kind::String) result.value.block_->Data()[length] = 0;
    return result;
}
#ifdef __linux__
static std::size_t SelfRead(void*, std::uintptr_t source, void* destination, std::size_t size) {
    if (!Range(source, size)) return 0;
    iovec local{destination, size}, remote{reinterpret_cast<void*>(source), size};
    auto count = syscall(SYS_process_vm_readv, getpid(), &local, 1UL, &remote, 1UL, 0UL);
    return count > 0 ? static_cast<std::size_t>(count) : 0;
}
#endif
Reader SystemReader() {
    static const Reader reader = [] {
#ifdef __linux__
        auto page = sysconf(_SC_PAGESIZE); std::uint8_t source = 0x53, output = 0;
        if (page > 0 && SelfRead(nullptr, reinterpret_cast<std::uintptr_t>(&source), &output, 1) == 1 && output == source)
            return Reader{SelfRead, nullptr, static_cast<std::size_t>(page), true};
#endif
        return Reader{}; // No direct-memory, /proc, or signal-handler fallback.
    }();
    return reader;
}
Result<Snapshot> Snapshot::PrepareCapture(const Operation& op, Kind kind) {
    if(!op.HostOwned() || (kind!=Kind::String && kind!=Kind::Vector))
        return Fail<Snapshot>(Unsupported, "capture requires Engine operation and copied kind");
    auto result=Allocate(op,kind,kind==Kind::String ? MaxString+1 : 12);
    if(result) result.value.block_->capture_ready=true;
    return result;
}
Result<Snapshot> Snapshot::Capture(const Operation& op, Kind kind, std::uintptr_t source, const Reader& reader) {
    auto prepared=PrepareCapture(op,kind);if(!prepared) return prepared;
    return CapturePrepared(std::move(prepared.value),source,reader);
}
Result<Snapshot> Snapshot::CapturePrepared(Snapshot&& prepared, std::uintptr_t source, const Reader& reader) {
    if(!prepared.block_ || !prepared.block_->capture_ready || prepared.block_->refs.load()!=1)
        return Fail<Snapshot>(Unsupported, "capture storage unavailable or shared");
    const auto kind=prepared.kind();
    const std::size_t limit=kind==Kind::String ? MaxString+1 : 12;
    if(!reader.available || !reader.read || !reader.page_size || !Range(source,limit))
        return Fail<Snapshot>(Unsupported, "unavailable reader or invalid address");
    prepared.block_->capture_ready=false;
    Result<Snapshot> result{std::move(prepared),{}};
    std::uint8_t scratch[4096]; std::size_t offset = 0;
    while (offset < limit) {
        auto address = source + offset;
        auto count = std::min({limit - offset, sizeof scratch, reader.page_size - address % reader.page_size});
        auto received = reader.read(reader.context, address, scratch, count);
        if (!received || received > count) return Fail<Snapshot>(Unsupported, "unreadable copied source");
        if (kind == Kind::String) {
            auto* end = static_cast<std::uint8_t*>(std::memchr(scratch, 0, received));
            if (end) {
                auto n = static_cast<std::size_t>(end - scratch);
                std::memcpy(result.value.block_->Data() + offset, scratch, n + 1);
                result.value.block_->length = offset + n;
                if (!Valid(kind, result.value.data(), result.value.size())) return Fail<Snapshot>(Unsupported, "invalid UTF-8 source");
                return result;
            }
        }
        if (received != count) return Fail<Snapshot>(Unsupported, "short copied source");
        std::memcpy(result.value.block_->Data() + offset, scratch, count); offset += count;
    }
    if (kind == Kind::String) return Fail<Snapshot>(TooLarge, "no NUL within 65536 bytes");
    result.value.block_->length = 12;
    if (!Valid(kind, result.value.data(), 12)) return Fail<Snapshot>(Unsupported, "nonfinite vector source");
    return result;
}

namespace {
constexpr std::size_t MappingBytes = 16 * MiB, MetadataBytes = 2 * MiB, PayloadBytes = 14 * MiB;
constexpr std::size_t InternSlots = 65536, MaxValues = 32768, PermanentOwners = 1024;
constexpr std::size_t OwnerPayload = 4 * MiB, OwnerValues = 8192;
struct Slot { std::uint64_t hash = 0; std::uint32_t offset = 0, length = 0; Kind kind{}; };
struct PermanentOwner { StableOwner key; std::uint32_t bytes = 0, values = 0; };
struct Metadata {
    std::mutex mutex;
    PermanentMetrics metrics;
    std::array<Slot, InternSlots> slots{};
    std::array<PermanentOwner, PermanentOwners> owners{};
};
static_assert(sizeof(Metadata) <= MetadataBytes, "fixed intern index and owner rows must fit mapping");
static_assert(MetadataBytes + PayloadBytes == MappingBytes && MetadataBytes % 16 == 0);
struct Pending { Slot entry; std::size_t slot; const Snapshot* source; const StableOwner* owner; };
struct OwnerCharge { StableOwner key; std::size_t row = 0, bytes = 0, values = 0; };
struct Stage {
    std::array<Pending, MaxBatch> entries{};
    std::array<const void*, MaxBatch> outputs{};
    std::array<OwnerCharge, MaxBatch> owners{};
    std::size_t count = 0, bytes = 0, owner_count = 0;
};
std::uint64_t Hash(const Snapshot& s) {
    std::uint64_t hash = 14695981039346656037ULL ^ static_cast<std::uint32_t>(s.kind());
    for (std::size_t i = 0; i < s.size(); ++i) { hash ^= s.data()[i]; hash *= 1099511628211ULL; }
    return hash;
}
bool Equal(const Slot& entry, const Snapshot& s, std::uint64_t hash, const std::uint8_t* bytes) {
    return entry.kind == s.kind() && entry.hash == hash && entry.length == s.size() && std::memcmp(bytes, s.data(), s.size()) == 0;
}
} // namespace
Arena::Arena() {
    auto* memory = mmap(nullptr, MappingBytes, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (memory == MAP_FAILED) return;
    mapping_ = memory; new (memory) Metadata;
    // Deliberately no destructor/munmap/reset. The existing shim is resident;
    // service reinitialization, maps and script generations reuse this instance.
}
Arena& Arena::Resident() { static Arena arena; return arena; }
PermanentMetrics Arena::Read() const {
    if (!mapping_) return {};
    auto& metadata = *static_cast<Metadata*>(mapping_); std::lock_guard<std::mutex> lock(metadata.mutex);
    return metadata.metrics;
}
Result<bool> Arena::Intern(const Operation& op, const StableOwner* keys, const Snapshot* values, std::size_t count, const void** outputs) {
    if (!mapping_) return Fail<bool>(Unsupported, "resident arena mapping unavailable");
    if (count > MaxBatch || (count && (!keys || !values || !outputs))) return Fail<bool>(Unsupported, "invalid copied batch");
    if (!count) return {true, {}};
    auto admission = op.Reserve(sizeof(Stage)); if (!admission) return {{}, std::move(admission.error)};
    auto stage = std::unique_ptr<Stage>(new (std::nothrow) Stage);
    if (!stage) return Fail<bool>(Temporary, "publication staging allocation");
    auto& metadata = *static_cast<Metadata*>(mapping_); std::lock_guard<std::mutex> lock(metadata.mutex);
    auto* payload = static_cast<std::uint8_t*>(mapping_) + MetadataBytes;
    auto reject = [&](const char* reason) { ++metadata.metrics.rejected; return Fail<bool>(Permanent, reason); };
    // Planning consults published slots and transaction-local tentative slots.
    // Nothing in the mapping (except rejection metrics) changes until all values,
    // quotas and staging are admitted. No allocation/fallible work follows commit.
    for (std::size_t i = 0; i < count; ++i) {
        const auto& value = values[i];
        if (!value || !Valid(value.kind(), value.data(), value.size())) return Fail<bool>(Unsupported, "invalid copied batch value");
        auto hash = Hash(value); std::size_t index = hash % InternSlots;
        for (;;) {
            const auto& slot = metadata.slots[index];
            if (slot.kind != Kind{}) {
                if (Equal(slot, value, hash, payload + slot.offset)) { stage->outputs[i] = payload + slot.offset; break; }
            } else {
                const Pending* staged = nullptr;
                for (std::size_t j = 0; j < stage->count; ++j) if (stage->entries[j].slot == index) { staged = &stage->entries[j]; break; }
                if (staged) {
                    if (Equal(staged->entry, value, hash, staged->source->data())) { stage->outputs[i] = payload + staged->entry.offset; break; }
                } else {
                    const auto bytes = Align16(value.size() + (value.kind() == Kind::String));
                    if (metadata.metrics.values + stage->count == MaxValues) return reject("value count");
                    if (bytes > PayloadBytes - metadata.metrics.bytes - stage->bytes) return reject("global payload");
                    const auto offset = metadata.metrics.bytes + stage->bytes;
                    stage->entries[stage->count++] = {{hash, static_cast<std::uint32_t>(offset), static_cast<std::uint32_t>(value.size()), value.kind()}, index, &value, &keys[i]};
                    stage->bytes += bytes; stage->outputs[i] = payload + offset; break;
                }
            }
            index = (index + 1) % InternSlots; // <= half full, so always terminates
        }
    }
    for (std::size_t pending_index = 0; pending_index < stage->count; ++pending_index) {
        const auto& pending = stage->entries[pending_index];
        const auto& key = *pending.owner;
        std::size_t charge_index = 0;
        while (charge_index < stage->owner_count && !(stage->owners[charge_index].key == key)) ++charge_index;
        if (charge_index == stage->owner_count) {
            std::size_t owner_index = PermanentOwners, empty = PermanentOwners;
            for (std::size_t i = 0; i < PermanentOwners; ++i) {
                const auto& row = metadata.owners[i];
                if (!row.values) {
                    bool reserved = false;
                    for (std::size_t j = 0; j < stage->owner_count; ++j) if (stage->owners[j].row == i) { reserved = true; break; }
                    if (!reserved && empty == PermanentOwners) empty = i;
                } else if (row.key == key) { owner_index = i; break; }
            }
            if (owner_index == PermanentOwners) owner_index = empty;
            if (owner_index == PermanentOwners) return reject("owner rows");
            stage->owners[stage->owner_count++] = {key, owner_index, 0, 0};
        }
        auto& charge = stage->owners[charge_index];
        charge.bytes += Align16(pending.source->size() + (pending.source->kind() == Kind::String));
        ++charge.values;
        const auto& row = metadata.owners[charge.row];
        if (charge.bytes > OwnerPayload - row.bytes) return reject("owner payload");
        if (charge.values > OwnerValues - row.values) return reject("owner values");
    }
    // Atomic publication under the mapping mutex. The caller receives its complete
    // pointer batch only after all infallible copies and counter updates finish.
    for (std::size_t i = 0; i < stage->count; ++i) {
        const auto& pending = stage->entries[i]; const auto& source = *pending.source;
        auto capacity = Align16(source.size() + (source.kind() == Kind::String));
        std::memset(payload + pending.entry.offset, 0, capacity);
        std::memcpy(payload + pending.entry.offset, source.data(), source.size());
        metadata.slots[pending.slot] = pending.entry;
    }
    for (std::size_t i = 0; i < stage->owner_count; ++i) {
        const auto& charge = stage->owners[i]; auto& row = metadata.owners[charge.row];
        if (!row.values) { row.key = charge.key; ++metadata.metrics.owners; }
        row.bytes += charge.bytes; row.values += charge.values;
    }
    metadata.metrics.bytes += stage->bytes; metadata.metrics.values += stage->count;
    std::copy_n(stage->outputs.begin(), count, outputs);
    return {true, {}};
}
} // namespace s2fn::copy
