#pragma once
// Internal native ownership only. No raw address in this API may cross into JS.
#include "engine_function_abi.h"
#include <atomic>
#include <mutex>

namespace s2fn::copy {
constexpr std::size_t KiB = 1024, MiB = 1024 * KiB;
constexpr std::size_t MaxString = 65535, MaxBatch = 33;
constexpr std::size_t ProcessBytes = 32 * MiB, OwnerBytes = 8 * MiB, OperationBytes = 8 * MiB;
constexpr std::size_t ProcessItems = 1024, OwnerItems = 128;
// Only the trusted native/core context constructs these from a verified canonical
// identity digest. Domains separate engine-origin, script, and host-package quotas.
// Digest collisions may share/refuse quota; these keys never grant authority.
enum class Domain : std::uint32_t { Engine, Plugin, HostPackage };
struct StableOwner {
    Domain domain = Domain::Engine;
    std::array<std::uint8_t, 32> digest{};
    bool operator==(const StableOwner& b) const { return domain == b.domain && digest == b.digest; }
};
struct OwnerGeneration {
    StableOwner owner;
    std::uint64_t generation = 0;
    bool operator==(const OwnerGeneration& b) const { return generation == b.generation && owner == b.owner; }
};
struct Metrics { std::size_t bytes = 0, items = 0, owners = 0, high_bytes = 0, high_items = 0, rejected = 0; };
class Budget;
struct OperationState;
class Lease {
public:
    Lease() = default;
    Lease(Lease&&) noexcept;
    Lease& operator=(Lease&&) noexcept;
    ~Lease();
    Lease(const Lease&) = delete;
    Lease& operator=(const Lease&) = delete;
private:
    friend class Operation;
    Lease(OperationState*, std::size_t);
    OperationState* state_ = nullptr;
    std::size_t bytes_ = 0;
};
class Operation {
public:
    Operation() = default;
    Operation(const Operation&);
    Operation& operator=(const Operation&);
    Operation(Operation&&) noexcept;
    Operation& operator=(Operation&&) noexcept;
    ~Operation();
    Result<Lease> Reserve(std::size_t) const;
private:
    friend class Budget;
    friend class Snapshot;
    bool HostOwned() const;
    explicit Operation(OperationState* state) : state_(state) {}
    OperationState* state_ = nullptr;
};
// A Budget must outlive its operations/leases. Production always uses NativeBudget.
class Budget {
public:
    Result<Operation> Begin(const OwnerGeneration&);
    Metrics Read() const;
private:
    friend class Operation;
    friend class Lease;
    friend struct OperationState;
    struct Row { OwnerGeneration key; std::size_t bytes = 0, items = 0; };
    std::array<Row, ProcessItems> owners_{};
    mutable std::mutex mutex_;
    Metrics metrics_;
};
Budget& NativeBudget();
enum class Kind : std::uint32_t { String = 1, Vector = 2 };
// Returns the bytes actually read (0 denotes denial/fault). Never reads by
// dereferencing an untrusted source. Injected readers obey the same contract.
struct Reader {
    using ReadFn = std::size_t (*)(void*, std::uintptr_t, void*, std::size_t);
    ReadFn read = nullptr;
    void* context = nullptr;
    std::size_t page_size = 0;
    bool available = false;
};
Reader SystemReader();
struct SnapshotBlock;
class Snapshot {
public:
    Snapshot() = default;
    Snapshot(const Snapshot&);
    Snapshot& operator=(const Snapshot&);
    Snapshot(Snapshot&&) noexcept;
    Snapshot& operator=(Snapshot&&) noexcept;
    ~Snapshot();
    explicit operator bool() const { return block_ != nullptr; }
    Kind kind() const;
    const std::uint8_t* data() const;
    std::size_t size() const;
    // FromBytes consumes a trusted accessible sidecar span, never an engine
    // address. Capture is the only entry point for untrusted native addresses.
    static Result<Snapshot> FromBytes(const Operation&, Kind, const void*, std::size_t);
    // Native-origin capture requires a distinct Engine-domain operation.
    static Result<Snapshot> Capture(const Operation&, Kind, std::uintptr_t, const Reader&);
    // Admit the complete capture allocation before an irreversible native call.
    // Prepared storage is single-use and must not have been shared.
    static Result<Snapshot> PrepareCapture(const Operation&, Kind);
    static Result<Snapshot> CapturePrepared(Snapshot&&, std::uintptr_t, const Reader&);
private:
    static Result<Snapshot> Allocate(const Operation&, Kind, std::size_t);
    SnapshotBlock* block_ = nullptr;
};
struct PermanentMetrics { std::size_t bytes = 0, values = 0, owners = 0, rejected = 0; };
class Arena {
public:
    static Arena& Resident();
    // Each value has a trusted billing owner. First new occurrence pays when
    // duplicate values in the same batch name different owners.
    // Output storage belongs to caller and remains untouched on failure. Only
    // winning values arrive here; output pointer application must be infallible.
    Result<bool> Intern(const Operation&, const StableOwner*, const Snapshot*, std::size_t, const void**);
    PermanentMetrics Read() const;
private:
    Arena();
    void* mapping_ = nullptr;
};
} // namespace s2fn::copy
