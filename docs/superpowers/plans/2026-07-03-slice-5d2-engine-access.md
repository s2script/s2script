# Slice 5D.2 — engine-backed access: live game events + engine identity — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Light up live game-event delivery (the deferred 5D.1b) and engine identity (`player.userId`, `Player.fromUserId`, connected-but-pawnless enumeration) by reaching two un-exported CS2 engine facilities through committed gamedata facts — one signature (event manager) and six offsets (client list).

**Architecture:** Two independent vertical threads over one "layout-is-data" spine. Thread A: a pure byte-pattern scanner (shim) resolves the `IGameEventManager2*` global from `libserver.so` and populates `s_pGameEventManager` (replacing the always-MISSING factory acquisition) — every downstream 5D.1 mechanism is unchanged. Thread B: engine-generic client-list natives read the connected-client list (via `INetworkServerService`) at gamedata offsets; the CS2 `Player` layer maps them to `userId`/`fromUserId`/`allConnected`. Mechanisms live engine-generic in core/shim; the signature/offset values live in `gamedata/core.gamedata.jsonc`; the `Player.*` API lives in the CS2 game layer.

**Tech Stack:** C++ Metamod shim (links hl2sdk + `libs2script_core.so` via DT_NEEDED), Rust `cdylib` core (rusty_v8), `nlohmann::json` (JSONC), CS2 JS (`games/cs2/js/pawn.js`), the `@s2script/cs2` `.d.ts`, Docker CS2 live gate.

**Spec:** `docs/superpowers/specs/2026-07-03-slice-5d2-engine-access-design.md`. **RE facts:** `docs/superpowers/specs/2026-07-03-slice-5d1b-sigscan-spike-findings.md` (the authority for every offset and signature).

## Global Constraints

- **Core stays engine-generic.** No CS2 identifier in `core/src` (`IGameEventManager2`/`INetworkServerService`/`CServerSideClient` are Source2 ENGINE types, not game types → allowed in shim). Both gates green: `bash scripts/check-core-boundary.sh` (EXIT 0), `bash scripts/test-boundary-nameleak.sh` (PASS).
- **Layout is data.** Every offset/signature lives in `gamedata/core.gamedata.jsonc`, never hardcoded in C++/Rust. Offsets are DECIMAL integers (JSON has no hex; hex goes in a `//` comment). Dotted keys (`"Class.field"`) are valid JSON object keys.
- **Degrade per-descriptor, never crash globally.** A missing signature → `s_pGameEventManager` null → event ops no-op. A null service/game-server/out-of-range slot/bad offset → identity natives return `false`/`-1`/`null`. No path dereferences an unvalidated pointer.
- **ABI append-only.** New engine ops are APPENDED to `S2EngineOps` — in the C header (`shim/include/s2script_core.h`) AND its Rust mirror (`core/src/v8host.rs`) in the SAME order. Never reorder or insert above existing fields.
- **Naming:** PascalCase events + types (`Player`), camelCase functions + properties (`userId`, `fromUserId`, `allConnected`).
- **Exact offset values (spike-confirmed, `linuxsteamrt64`):** `NetworkServerService.gameServer`=336 (0x150), `NetworkGameServer.clientCount`=592 (0x250), `NetworkGameServer.clientElems`=600 (0x258), `ServerSideClient.name`=64 (0x40), `ServerSideClient.signon`=100 (0x64), `ServerSideClient.userId`=168 (0xa8).
- **Event-manager signature (`libserver.so`, ctor-body, unique in `.text`):** `55 48 8D 05 ? ? ? ? BE 40 00 00 00 48 89 E5 41 54 4C 8D 65 E8 53 48 89 FB`, resolve strategy `ctor-body-xref`.
- **Commit trailer:** every commit message ends with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`.
- **Test runners:** core = `cargo test -p s2script-core -- --test-threads=1`; CLI/JS = `cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs`; sigscan = `bash scripts/test-sigscan.sh`.
- **Build/live is controller-driven** (Task 5): ONE sniper build covers both threads' native changes; JS changes need only re-packaging.

## File Structure

| File | Create/Modify | Responsibility |
|---|---|---|
| `shim/src/sigscan.h` | Create | Pure pattern parse/find/resolve declarations (no SDK deps) |
| `shim/src/sigscan.cpp` | Create | Pure pattern parse/find/resolve implementation |
| `shim/tests/sigscan_test.cpp` | Create | Host-g++ unit test over synthetic buffers |
| `scripts/test-sigscan.sh` | Create | Compile + run the sigscan unit test (host g++) |
| `shim/CMakeLists.txt` | Modify | Add `src/sigscan.cpp` to the shim sources |
| `shim/src/gamedata.h` / `.cpp` | Modify | `LoadSignatures(path, platform, error)` |
| `gamedata/core.gamedata.jsonc` | Modify | Add `.signatures.GameEventManager`; add 6 `.offsets` keys; delete the `GameEventManager` interface entry |
| `shim/src/s2script_mm.cpp` | Modify | `FindModuleText` glue; sig-scan wiring for `s_pGameEventManager`; store `s_pNetworkServerService`; client-list reader + 5 op impls + wire into `S2EngineOps` |
| `shim/include/s2script_core.h` | Modify | Append 5 client-op typedefs + struct fields |
| `core/src/v8host.rs` | Modify | Append 5 client-op type aliases + struct fields; add 5 client natives + register; in-isolate degrade tests |
| `games/cs2/js/pawn.js` | Modify | `player.userId`, `Player.fromUserId`, `Player.allConnected`, `Player._fromSlotUnchecked` |
| `packages/cs2/index.d.ts` | Modify | `userId` + `fromUserId` + `allConnected` types |
| `packages/cli/test/player-identity.test.mjs` | Create | vm-compose test for the identity JS |
| `examples/demo-plugin/src/plugin.ts` | Modify | Subscribe to a real event + read identity (live gates) |
| `README.md` / `CLAUDE.md` | Modify | Document the slice |

---

## Task 1: Pure pattern scanner + host-g++ unit test

**Files:**
- Create: `shim/src/sigscan.h`, `shim/src/sigscan.cpp`, `shim/tests/sigscan_test.cpp`, `scripts/test-sigscan.sh`
- Modify: `shim/CMakeLists.txt`

**Interfaces:**
- Produces (namespace `s2sig`, consumed by Task 2's shim wiring):
  - `std::vector<int> ParsePattern(const std::string& pattern)` — tokens: `0..255` literal byte, `-1` wildcard; empty on malformed input.
  - `int64_t FindPattern(const uint8_t* text, size_t len, const std::vector<int>& pat)` — first match offset, or `-1`.
  - `int64_t ResolveLeaDisp(const uint8_t* text, size_t len, int64_t matchOff, int dispOff, int instrLen)` — RIP-relative target offset (`matchOff + instrLen + int32(disp@matchOff+dispOff)`), or `s2sig::kFail`.
  - `int64_t ResolveCtorXref(const uint8_t* text, size_t len, int64_t ctorOff)` — find the unique `E8` call whose target == `ctorOff`; walk back ≤32 bytes to the nearest `4C 8D 35`/`4C 8D 2D` lea; return `leaOff + 7 + int32(disp@leaOff+3)`, or `s2sig::kFail`.
  - `constexpr int64_t kFail = INT64_MIN;`
- Note on offsets: all returned "target offsets" are relative to the `text` buffer base; the caller (Task 2) computes the absolute pointer as `text_ptr + offset` (correct across segments because RVA math is module-absolute — a `.bss` target lands beyond `.text`, which is expected).

- [ ] **Step 1: Write the failing test**

Create `shim/tests/sigscan_test.cpp`:

```cpp
// Host-g++ unit test for the pure pattern scanner (no SDK deps). Run via scripts/test-sigscan.sh.
#include "../src/sigscan.h"
#include <cstdio>
#include <cstring>
#include <vector>

static int g_fail = 0;
#define CHECK(cond, msg) do { if (!(cond)) { std::printf("FAIL: %s\n", msg); g_fail = 1; } } while (0)

int main() {
    using namespace s2sig;

    // --- ParsePattern ---
    auto p = ParsePattern("4C 8D 35 ?");
    CHECK(p.size() == 4, "ParsePattern size");
    CHECK(p[0] == 0x4C && p[1] == 0x8D && p[2] == 0x35 && p[3] == -1, "ParsePattern tokens");
    CHECK(ParsePattern("").empty(), "ParsePattern empty");
    CHECK(ParsePattern("ZZ").empty(), "ParsePattern malformed");
    CHECK(ParsePattern("4C 8D 35 ??")[3] == -1, "ParsePattern double-? wildcard");

    // --- FindPattern (literal + wildcard + miss) ---
    std::vector<uint8_t> buf(256, 0x90);           // nop-filled
    buf[0x10] = 0x4C; buf[0x11] = 0x8D; buf[0x12] = 0x35; buf[0x13] = 0xAB;
    CHECK(FindPattern(buf.data(), buf.size(), ParsePattern("4C 8D 35")) == 0x10, "FindPattern literal");
    CHECK(FindPattern(buf.data(), buf.size(), ParsePattern("4C 8D 35 ?")) == 0x10, "FindPattern wildcard");
    CHECK(FindPattern(buf.data(), buf.size(), ParsePattern("DE AD BE EF")) == -1, "FindPattern miss");

    // --- ResolveLeaDisp: lea r14,[rip+disp] at 0x80, disp=0x9C -> target = 0x80 + 7 + 0x9C = 0x123 ---
    std::vector<uint8_t> b2(512, 0x90);
    b2[0x80] = 0x4C; b2[0x81] = 0x8D; b2[0x82] = 0x35;
    b2[0x83] = 0x9C; b2[0x84] = 0x00; b2[0x85] = 0x00; b2[0x86] = 0x00;   // disp32 = 0x9C (LE)
    CHECK(ResolveLeaDisp(b2.data(), b2.size(), 0x80, 3, 7) == 0x123, "ResolveLeaDisp");
    CHECK(ResolveLeaDisp(b2.data(), 0x82, 0x80, 3, 7) == kFail, "ResolveLeaDisp oob");

    // --- ResolveCtorXref: ctor at 0x40; lea r14 at 0x80 (target 0x123); E8 call at 0x87 -> 0x40 ---
    std::vector<uint8_t> b3(512, 0x90);
    b3[0x80] = 0x4C; b3[0x81] = 0x8D; b3[0x82] = 0x35;
    b3[0x83] = 0x9C; b3[0x84] = 0x00; b3[0x85] = 0x00; b3[0x86] = 0x00;   // lea disp -> 0x123
    // call rel32 at 0x87: target 0x40 = 0x87 + 5 + rel32 => rel32 = 0x40 - 0x8C = -0x4C = 0xFFFFFFB4
    b3[0x87] = 0xE8; b3[0x88] = 0xB4; b3[0x89] = 0xFF; b3[0x8A] = 0xFF; b3[0x8B] = 0xFF;
    CHECK(ResolveCtorXref(b3.data(), b3.size(), 0x40) == 0x123, "ResolveCtorXref");
    CHECK(ResolveCtorXref(b3.data(), b3.size(), 0x41) == kFail, "ResolveCtorXref no-caller");

    if (g_fail) { std::printf("sigscan_test: FAILURES\n"); return 1; }
    std::printf("sigscan_test: all passed\n");
    return 0;
}
```

Create `scripts/test-sigscan.sh`:

```bash
#!/usr/bin/env bash
# Compile + run the pure pattern-scanner unit test with the host compiler (no sniper container,
# no SDK — sigscan.{h,cpp} are self-contained). Slice 5D.2 Thread A gate.
set -euo pipefail
cd "$(dirname "$0")/.."
out="$(mktemp -d)/sigscan_test"
g++ -std=c++17 -O2 -Wall -Wextra -o "$out" shim/src/sigscan.cpp shim/tests/sigscan_test.cpp
"$out"
```

- [ ] **Step 2: Run to verify it fails**

Run: `chmod +x scripts/test-sigscan.sh && bash scripts/test-sigscan.sh`
Expected: FAIL — `shim/src/sigscan.h: No such file or directory` (compile error; the header does not exist yet).

- [ ] **Step 3: Write `shim/src/sigscan.h`**

```cpp
#pragma once
#include <cstdint>
#include <cstddef>
#include <string>
#include <vector>

// Pure byte-pattern scanning + RIP-relative extraction (engine-generic; no SDK deps).
// All "offset" returns are relative to the `text` buffer base; the caller adds the runtime
// text pointer to get an absolute address.  Slice 5D.2 (event-manager signature).
namespace s2sig {

constexpr int64_t kFail = INT64_MIN;

// "4C 8D 35 ?" -> {0x4C,0x8D,0x35,-1}. Byte tokens are 2 hex digits; "?"/"??" is a wildcard (-1).
// Returns an empty vector on any malformed token.
std::vector<int> ParsePattern(const std::string& pattern);

// First offset in [text, text+len) where every non-wildcard token matches; -1 if none.
int64_t FindPattern(const uint8_t* text, size_t len, const std::vector<int>& pat);

// Instruction at matchOff has a little-endian int32 displacement at matchOff+dispOff and total
// length instrLen; returns the RIP-relative target offset (matchOff + instrLen + disp), or kFail
// if the displacement bytes are out of bounds.
int64_t ResolveLeaDisp(const uint8_t* text, size_t len, int64_t matchOff, int dispOff, int instrLen);

// ctorOff = a function's start offset. Find the unique 0xE8 rel32 call whose target == ctorOff;
// from that call site, walk back up to 32 bytes to the nearest `4C 8D 35`/`4C 8D 2D`
// (lea r14/r13,[rip+d]); return leaOff + 7 + d. kFail if no caller or no preceding lea.
int64_t ResolveCtorXref(const uint8_t* text, size_t len, int64_t ctorOff);

} // namespace s2sig
```

- [ ] **Step 4: Write `shim/src/sigscan.cpp`**

```cpp
#include "sigscan.h"
#include <cctype>

namespace s2sig {

static int hexNibble(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

std::vector<int> ParsePattern(const std::string& pattern) {
    std::vector<int> out;
    size_t i = 0, n = pattern.size();
    while (i < n) {
        while (i < n && std::isspace((unsigned char)pattern[i])) i++;
        if (i >= n) break;
        if (pattern[i] == '?') {
            out.push_back(-1);
            i++;
            if (i < n && pattern[i] == '?') i++;      // accept "??"
        } else {
            int hi = hexNibble(pattern[i]);
            if (hi < 0 || i + 1 >= n) return {};        // malformed
            int lo = hexNibble(pattern[i + 1]);
            if (lo < 0) return {};                       // malformed
            out.push_back((hi << 4) | lo);
            i += 2;
        }
        // token must be followed by whitespace or end
        if (i < n && !std::isspace((unsigned char)pattern[i])) return {};
    }
    return out;
}

int64_t FindPattern(const uint8_t* text, size_t len, const std::vector<int>& pat) {
    if (!text || pat.empty() || pat.size() > len) return -1;
    const size_t last = len - pat.size();
    for (size_t off = 0; off <= last; off++) {
        bool ok = true;
        for (size_t j = 0; j < pat.size(); j++) {
            if (pat[j] >= 0 && text[off + j] != (uint8_t)pat[j]) { ok = false; break; }
        }
        if (ok) return (int64_t)off;
    }
    return -1;
}

static bool readI32(const uint8_t* text, size_t len, int64_t at, int32_t& out) {
    if (at < 0 || (size_t)at + 4 > len) return false;
    out = (int32_t)((uint32_t)text[at] | ((uint32_t)text[at + 1] << 8) |
                    ((uint32_t)text[at + 2] << 16) | ((uint32_t)text[at + 3] << 24));
    return true;
}

int64_t ResolveLeaDisp(const uint8_t* text, size_t len, int64_t matchOff, int dispOff, int instrLen) {
    int32_t disp;
    if (!readI32(text, len, matchOff + dispOff, disp)) return kFail;
    return matchOff + instrLen + disp;
}

int64_t ResolveCtorXref(const uint8_t* text, size_t len, int64_t ctorOff) {
    if (!text || len < 5) return kFail;
    // Find the unique E8 rel32 call whose target == ctorOff.
    int64_t caller = -1;
    for (size_t c = 0; c + 5 <= len; c++) {
        if (text[c] != 0xE8) continue;
        int32_t rel;
        if (!readI32(text, len, (int64_t)c + 1, rel)) continue;
        if ((int64_t)c + 5 + rel == ctorOff) {
            if (caller >= 0) return kFail;         // not unique -> degrade
            caller = (int64_t)c;
        }
    }
    if (caller < 0) return kFail;
    // Walk back up to 32 bytes to the nearest lea r14/r13,[rip+d]: 4C 8D 35 / 4C 8D 2D.
    for (int64_t p = caller - 3; p >= caller - 32 && p >= 0; p--) {
        if (text[p] == 0x4C && text[p + 1] == 0x8D &&
            (text[p + 2] == 0x35 || text[p + 2] == 0x2D)) {
            int32_t disp;
            if (!readI32(text, len, p + 3, disp)) return kFail;
            return p + 7 + disp;
        }
    }
    return kFail;
}

} // namespace s2sig
```

- [ ] **Step 5: Add sigscan.cpp to the shim build**

In `shim/CMakeLists.txt`, add `src/sigscan.cpp` to the `add_library(s2script SHARED ...)` source list (after `src/gamedata.cpp`):

```cmake
add_library(s2script SHARED
    src/s2script_mm.cpp
    src/gamedata.cpp
    src/sigscan.cpp
    # Slice 5D.1: a self-contained MurmurHash2LowerCase ...
    src/tier1_shims.cpp
)
```

- [ ] **Step 6: Run to verify it passes**

Run: `bash scripts/test-sigscan.sh`
Expected: `sigscan_test: all passed`

- [ ] **Step 7: Commit**

```bash
git add shim/src/sigscan.h shim/src/sigscan.cpp shim/tests/sigscan_test.cpp scripts/test-sigscan.sh shim/CMakeLists.txt
git commit -m "$(printf 'feat(slice5d2): pure byte-pattern scanner + host-g++ unit test\n\nEngine-generic ParsePattern/FindPattern/ResolveLeaDisp/ResolveCtorXref (no SDK deps),\ntested over synthetic buffers via scripts/test-sigscan.sh. Thread A foundation for\nresolving the un-exported IGameEventManager2 global from libserver.so.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Task 2: `LoadSignatures` + gamedata signature + sig-scan wiring (event manager)

**Files:**
- Modify: `shim/src/gamedata.h`, `shim/src/gamedata.cpp`, `gamedata/core.gamedata.jsonc`, `shim/src/s2script_mm.cpp`

**Interfaces:**
- Consumes: `s2sig::*` (Task 1).
- Produces: a populated `s_pGameEventManager` (an `IGameEventManager2*`) when the scan succeeds; the 5D.1 event path (unchanged) then delivers events. Introduces `struct SigSpec { std::string module, pattern, resolve; }` and `std::map<std::string, SigSpec> LoadSignatures(const std::string& path, const std::string& platform, std::string& error)`.

- [ ] **Step 1: Add the `.signatures` section + remove the obsolete interface entry in `gamedata/core.gamedata.jsonc`**

Delete the `"GameEventManager": "GAMEEVENTSMANAGER002"` line from `"interfaces"` (and its preceding comment). Add a new top-level `"signatures"` section (sibling of `interfaces`/`offsets`):

```jsonc
  // Byte signatures for un-exported engine globals (Slice 5D.2). Layout-is-data: the pattern +
  // module are regenerable per CS2 patch from the documented RTTI->vtable->xref anchors (see
  // docs/superpowers/specs/2026-07-03-slice-5d1b-sigscan-spike-findings.md). A missing/stale
  // signature degrades (s_pGameEventManager stays null -> event ops no-op), never crashes.
  "signatures": {
    // IGameEventManager2 global in libserver.so. GAMEEVENTSMANAGER002 is NOT a registered
    // interface in CS2 (in zero modules), so the manager is reached via this ctor-body signature:
    // find the ctor, its sole E8 caller, the preceding lea r14 -> the global instance (offset-to-top 0).
    "GameEventManager": {
      "linuxsteamrt64": {
        "module": "libserver.so",
        "pattern": "55 48 8D 05 ? ? ? ? BE 40 00 00 00 48 89 E5 41 54 4C 8D 65 E8 53 48 89 FB",
        "resolve": "ctor-body-xref"
      }
    }
  }
```

- [ ] **Step 2: Verify the gamedata still parses + has the new section**

Run:
```bash
node -e 'const fs=require("fs");let s=fs.readFileSync("gamedata/core.gamedata.jsonc","utf8").replace(/(^|[^:])\/\/.*$/gm,"$1");const j=JSON.parse(s);if(!j.signatures.GameEventManager.linuxsteamrt64.pattern)throw new Error("missing sig");if(j.interfaces.GameEventManager)throw new Error("interface entry not removed");console.log("gamedata OK: sig present, interface entry removed");'
```
Expected: `gamedata OK: sig present, interface entry removed`
(Note: the naive `//`-strip is safe here — no pattern/string contains `//`.)

- [ ] **Step 3: Declare `LoadSignatures` in `shim/src/gamedata.h`**

Append after the `LoadOffsets` declaration:

```cpp
// A byte-signature spec: which module to scan, the IDA-style pattern, and the resolve strategy.
struct SigSpec {
    std::string module;
    std::string pattern;
    std::string resolve;
};

// Reads platform-keyed byte signatures from the "signatures" section of a gamedata .jsonc file.
// Returns a map of signature-name → SigSpec for `platform`, or an empty map. `error` is left empty
// on success (including when the "signatures" section is absent); set on parse failure.
std::map<std::string, SigSpec> LoadSignatures(const std::string& path,
                                              const std::string& platform,
                                              std::string& error);
```

Add `#include <vector>` is not needed; ensure `<string>`/`<map>` already included (they are).

- [ ] **Step 4: Implement `LoadSignatures` in `shim/src/gamedata.cpp`**

Append (mirrors `LoadOffsets`):

```cpp
std::map<std::string, SigSpec> LoadSignatures(const std::string& path,
                                              const std::string& platform,
                                              std::string& error) {
    std::map<std::string, SigSpec> out;
    std::ifstream f(path);
    if (!f) {
        error = "gamedata file not found: " + path;
        return out;
    }
    try {
        auto j = nlohmann::json::parse(f, nullptr, /*allow_exceptions=*/true, /*ignore_comments=*/true);
        if (!j.contains("signatures")) return out;      // absent is not an error
        for (auto& [key, platforms] : j.at("signatures").items()) {
            if (!platforms.contains(platform)) continue;
            auto& p = platforms.at(platform);
            SigSpec s;
            s.module  = p.value("module", "");
            s.pattern = p.value("pattern", "");
            s.resolve = p.value("resolve", "");
            out[key] = s;
        }
    } catch (const std::exception& e) {
        error = std::string("gamedata parse error: ") + e.what();
        out.clear();
    }
    return out;
}
```

- [ ] **Step 5: Add the `FindModuleText` glue in `shim/src/s2script_mm.cpp`**

Near the top includes, add:

```cpp
#include <link.h>       // dl_iterate_phdr, ElfW
#include "sigscan.h"
```

Add a helper (place it above `Load`, near the other static helpers):

```cpp
// Slice 5D.2: locate the largest executable segment of a loaded module by soname substring.
// Returns {nullptr, 0} if not found. Live-only (dl_iterate_phdr); the pure match/extract is sigscan.
struct ModText { const uint8_t* text; size_t size; };
static ModText FindModuleText(const char* soname) {
    struct Ctx { const char* name; ModText out; } ctx{ soname, { nullptr, 0 } };
    dl_iterate_phdr([](struct dl_phdr_info* info, size_t, void* data) -> int {
        auto* c = static_cast<Ctx*>(data);
        if (!info->dlpi_name || !std::strstr(info->dlpi_name, c->name)) return 0;  // keep scanning
        for (int i = 0; i < info->dlpi_phnum; i++) {
            const ElfW(Phdr)& ph = info->dlpi_phdr[i];
            if (ph.p_type == PT_LOAD && (ph.p_flags & PF_X) && ph.p_filesz > c->out.size) {
                c->out.text = reinterpret_cast<const uint8_t*>(info->dlpi_addr + ph.p_vaddr);
                c->out.size = ph.p_filesz;                                          // pick the largest PF_X seg
            }
        }
        return 1;   // module found; stop
    }, &ctx);
    return ctx.out;
}
```

- [ ] **Step 6: Replace the event-manager factory block with the sig-scan**

Replace the entire Slice-5D.1 `IGameEventManager2*` acquisition block (the `{ auto it = versions.find("GameEventManager"); ... }` block that tries `serverFactory`/`engineFactory` and logs MISSING) with:

```cpp
        // Acquire IGameEventManager2* via signature scan (Slice 5D.2). GAMEEVENTSMANAGER002 is NOT a
        // registered interface in CS2 (in zero modules), so the global is resolved from libserver.so
        // by pattern. Signature + module are gamedata (layout-is-data). Degrade-never-crash: any
        // failure leaves s_pGameEventManager null -> event ops no-op.
        {
            std::string sigErr;
            auto sigs = LoadSignatures(GamedataPath(), "linuxsteamrt64", sigErr);
            if (!sigErr.empty()) {
                META_CONPRINTF("[s2script] WARN: %s — GameEventManager sig unavailable\n", sigErr.c_str());
            }
            auto it = sigs.find("GameEventManager");
            if (it == sigs.end()) {
                META_CONPRINTF("[s2script] WARN: no GameEventManager signature in gamedata — events degrade\n");
            } else {
                const SigSpec& sig = it->second;
                ModText mt = FindModuleText(sig.module.c_str());
                std::vector<int> pat = s2sig::ParsePattern(sig.pattern);
                if (!mt.text || pat.empty()) {
                    META_CONPRINTF("[s2script] WARN: GameEventManager sig-scan setup failed (module=%s, patTokens=%zu) — events degrade\n",
                                   sig.module.c_str(), pat.size());
                } else {
                    int64_t matchOff = s2sig::FindPattern(mt.text, mt.size, pat);
                    int64_t targetOff = s2sig::kFail;
                    if (matchOff >= 0) {
                        if (sig.resolve == "ctor-body-xref")
                            targetOff = s2sig::ResolveCtorXref(mt.text, mt.size, matchOff);
                        else if (sig.resolve == "lea-disp")
                            targetOff = s2sig::ResolveLeaDisp(mt.text, mt.size, matchOff, 3, 7);
                    }
                    if (targetOff != s2sig::kFail) {
                        s_pGameEventManager = reinterpret_cast<IGameEventManager2*>(
                            const_cast<uint8_t*>(mt.text) + targetOff);
                        META_CONPRINTF("[s2script] interface OK: GameEventManager (sig-scan %s, %p)\n",
                                       sig.resolve.c_str(), (void*)s_pGameEventManager);
                    } else {
                        s_pGameEventManager = nullptr;
                        META_CONPRINTF("[s2script] WARN: GameEventManager sig-scan no match (matchOff=%lld) — events degrade\n",
                                       (long long)matchOff);
                    }
                }
            }
        }
```

- [ ] **Step 7: Verify (compile is deferred to the Task 5 sniper build)**

The shim compiles only in the sniper container (Task 5). For this task, verify:
- `bash scripts/test-sigscan.sh` still passes (Task 1 unchanged).
- The gamedata parse check from Step 2 passes.
- Re-read the replaced block: no remaining reference to the old `versions.find("GameEventManager")`; `#include <link.h>` and `#include "sigscan.h"` are present.

Run:
```bash
bash scripts/test-sigscan.sh
grep -c 'versions.find("GameEventManager")' shim/src/s2script_mm.cpp   # expect 0
grep -c 'LoadSignatures' shim/src/s2script_mm.cpp                       # expect >=1
```
Expected: sigscan passes; first grep prints `0`; second prints `1` or more.

- [ ] **Step 8: Commit**

```bash
git add shim/src/gamedata.h shim/src/gamedata.cpp gamedata/core.gamedata.jsonc shim/src/s2script_mm.cpp
git commit -m "$(printf 'feat(slice5d2): LoadSignatures + sig-scan the IGameEventManager2 global\n\nReplaces the always-MISSING GAMEEVENTSMANAGER002 factory acquisition with a gamedata-driven\nlibserver.so ctor-body-xref signature scan (FindModuleText via dl_iterate_phdr + s2sig). All of\n5D.1 downstream (listener, event_mux, ops, natives, typed catalog) is unchanged. Degrade-never-crash.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Task 3: Engine-identity client-list ops + natives

**Files:**
- Modify: `shim/include/s2script_core.h`, `core/src/v8host.rs`, `gamedata/core.gamedata.jsonc`, `shim/src/s2script_mm.cpp`

**Interfaces:**
- Consumes: the stored `INetworkServerService*` (this task adds `s_pNetworkServerService`); the 6 `.offsets` keys (this task adds them + reads them into statics).
- Produces (engine-generic natives the CS2 JS layer in Task 4 calls):
  - `__s2_client_valid(slot: number) -> boolean`
  - `__s2_client_userid(slot: number) -> number` (i32; `-1` if unassigned/absent)
  - `__s2_client_signon(slot: number) -> number` (i32; `-1` if absent)
  - `__s2_client_name(slot: number) -> string | null`
  - `__s2_client_find_by_userid(userid: number) -> number` (slot, or `-1`)

- [ ] **Step 1: Write the failing in-isolate degrade tests (`core/src/v8host.rs`)**

In the `#[cfg(test)] mod tests` block, add (uses the existing `eval_in_context_string` harness; with no engine ops installed the natives must degrade):

Add this test next to `ent_ref_natives_degrade_without_engine_ops` (copy its exact setup — `init(dummy_logger())` / `set_engine_ops(None)` / `create_plugin_context("p")` / `shutdown()`):

```rust
    /// Slice 5D.2: the five engine-identity client natives degrade safely with no engine-ops table
    /// (no crash — false/-1/null as documented).
    #[test]
    fn client_natives_degrade_without_ops() {
        let _ = init(dummy_logger());
        set_engine_ops(None);                 // no ops table → every client op is a safe miss
        create_plugin_context("p");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_valid(0))"), "false");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_userid(0))"), "-1");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_signon(0))"), "-1");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_name(0))"), "null");
        assert_eq!(eval_in_context_string("p", "String(__s2_client_find_by_userid(5))"), "-1");
        shutdown();
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p s2script-core client_natives_degrade -- --test-threads=1`
Expected: FAIL — `__s2_client_valid is not defined` (natives not registered yet).

- [ ] **Step 3: Append the client-op typedefs + struct fields in `shim/include/s2script_core.h`**

After the `s2_event_get_player_slot_fn` typedef, add:

```c
/* Engine-identity ops (Slice 5D.2) — read the connected-client list (INetworkServerService ->
 * game server -> CServerSideClient[]) at gamedata offsets. All degrade to safe misses on any null. */
typedef int          (*s2_client_valid_fn)(int slot);          /* 0/1: connected client at slot */
typedef int          (*s2_client_userid_fn)(int slot);         /* engine user-id, or -1 */
typedef int          (*s2_client_signon_fn)(int slot);         /* signon state, or -1 */
typedef const char*  (*s2_client_name_fn)(int slot);           /* valid during call; core copies now */
typedef int          (*s2_client_find_by_userid_fn)(int userid); /* slot, or -1 */
```

In the `S2EngineOps` struct, APPEND after `event_get_player_slot`:

```c
    /* Engine-identity ops (Slice 5D.2) — APPENDED after the event ops; order is the ABI. */
    s2_client_valid_fn          client_valid;
    s2_client_userid_fn         client_userid;
    s2_client_signon_fn         client_signon;
    s2_client_name_fn           client_name;
    s2_client_find_by_userid_fn client_find_by_userid;
```

- [ ] **Step 4: Append the Rust mirror + natives + registration in `core/src/v8host.rs`**

Add the type aliases (after `EventGetPlayerSlotFn`):

```rust
// --- Slice 5D.2: engine-identity ops (C-ABI; the C header must match exactly) ---
pub type ClientValidFn        = extern "C" fn(slot: c_int) -> c_int;
pub type ClientUseridFn       = extern "C" fn(slot: c_int) -> i32;
pub type ClientSignonFn       = extern "C" fn(slot: c_int) -> i32;
pub type ClientNameFn         = extern "C" fn(slot: c_int) -> *const c_char;
pub type ClientFindByUseridFn = extern "C" fn(userid: c_int) -> i32;
```

APPEND to `struct S2EngineOps` (after `event_get_player_slot`):

```rust
    // --- Slice 5D.2: engine-identity ops (APPENDED — order is the ABI; do not reorder above) ---
    pub client_valid:          Option<ClientValidFn>,
    pub client_userid:         Option<ClientUseridFn>,
    pub client_signon:         Option<ClientSignonFn>,
    pub client_name:           Option<ClientNameFn>,
    pub client_find_by_userid: Option<ClientFindByUseridFn>,
```

Add the five natives (model the scalar ones on `s2_event_get_int`, the name one on `s2_event_get_string`). The `slot`/`userid` arg is an i32 read from `args.get(0)`:

```rust
/// Native `__s2_client_valid(slot) -> boolean`. Calls `client_valid`; degrades to false.
fn s2_client_valid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_bool(false);
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.client_valid else { return };
        rv.set_bool(func(slot) != 0);
    }));
}

/// Native `__s2_client_userid(slot) -> i32`. Calls `client_userid`; degrades to -1.
fn s2_client_userid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.client_userid else { return };
        rv.set_int32(func(slot));
    }));
}

/// Native `__s2_client_signon(slot) -> i32`. Calls `client_signon`; degrades to -1.
fn s2_client_signon(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.client_signon else { return };
        rv.set_int32(func(slot));
    }));
}

/// Native `__s2_client_find_by_userid(userid) -> i32`. Calls `client_find_by_userid`; degrades to -1.
fn s2_client_find_by_userid(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(-1);
        if args.length() < 1 { return; }
        let id = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.client_find_by_userid else { return };
        rv.set_int32(func(id));
    }));
}

/// Native `__s2_client_name(slot) -> string | null`. Calls `client_name`; copies the C string now.
fn s2_client_name(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        if args.length() < 1 { return; }
        let slot = args.get(0).int32_value(scope).unwrap_or(-1);
        let Some(ops) = ENGINE_OPS.with(|o| o.get()) else { return };
        let Some(func) = ops.client_name else { return };
        let ptr = func(slot);
        if ptr.is_null() { return; }
        let s = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        if let Some(js) = v8::String::new(scope, &s) { rv.set(js.into()); }
    }));
}
```

Note to implementer: match the EXACT `set_null()` / string-return idiom used by `s2_event_get_string` in this file (it may use a helper for the null/return-value pattern). If `int32_value` returns an `Option`, the `.unwrap_or(-1)` above is correct; if the codebase uses `to_int32(scope)`, mirror the neighbouring natives.

Register all five where the event natives are registered (near `set_native(scope, global_obj, "__s2_event_subscribe", ...)`):

```rust
    set_native(scope, global_obj, "__s2_client_valid", s2_client_valid);
    set_native(scope, global_obj, "__s2_client_userid", s2_client_userid);
    set_native(scope, global_obj, "__s2_client_signon", s2_client_signon);
    set_native(scope, global_obj, "__s2_client_name", s2_client_name);
    set_native(scope, global_obj, "__s2_client_find_by_userid", s2_client_find_by_userid);
```

- [ ] **Step 5: Run to verify the degrade tests pass**

Run: `cargo test -p s2script-core client_natives_degrade -- --test-threads=1`
Expected: PASS (natives registered; with no ops they degrade to false/-1/null).

- [ ] **Step 6: Add the 6 identity offsets to `gamedata/core.gamedata.jsonc`**

In the `"offsets"` section, after `GameEntitySystem`, add:

```jsonc
    // Slice 5D.2 engine identity (libengine2.so). Decimal = hex; regenerable per patch from the
    // documented anchors (spike findings §Target 1-3). A wrong offset degrades (null/-1), never crashes.
    "NetworkServerService.gameServer": { "linuxsteamrt64": 336 },   // 0x150: *(svc+off) = INetworkGameServer*
    "NetworkGameServer.clientCount":   { "linuxsteamrt64": 592 },   // 0x250: slot-indexed CUtlVector count
    "NetworkGameServer.clientElems":   { "linuxsteamrt64": 600 },   // 0x258: CServerSideClient** elems
    "ServerSideClient.name":           { "linuxsteamrt64": 64  },   // 0x40 : char* (null -> empty)
    "ServerSideClient.signon":         { "linuxsteamrt64": 100 },   // 0x64 : int32 signon state
    "ServerSideClient.userId":         { "linuxsteamrt64": 168 }    // 0xa8 : int16 CPlayerUserId (-1 = unassigned)
```

Verify parse: re-run the Step-2 JSON check from Task 2 form (adapt to assert one of the new offset keys):
```bash
node -e 'const fs=require("fs");let s=fs.readFileSync("gamedata/core.gamedata.jsonc","utf8").replace(/(^|[^:])\/\/.*$/gm,"$1");const j=JSON.parse(s);if(j.offsets["ServerSideClient.userId"].linuxsteamrt64!==168)throw new Error("userId offset");console.log("offsets OK");'
```
Expected: `offsets OK`

- [ ] **Step 7: Store `s_pNetworkServerService` + implement the reader + op impls in `shim/src/s2script_mm.cpp`**

(a) Add the static + offset statics near `s_pGameEventManager`:

```cpp
// Slice 5D.2: engine identity. INetworkServerService is acquired in Load() (was only logged);
// the client-list offsets come from gamedata. Any -1 offset / null pointer degrades safely.
static void* s_pNetworkServerService = nullptr;
static int s_offGameServer  = -1, s_offClientCount = -1, s_offClientElems = -1;
static int s_offSscName     = -1, s_offSscSignon   = -1, s_offSscUserId   = -1;
static const int kSignonConnected = 2;  // SIGNONSTATE_CONNECTED; >=2 = connected (incl. pawnless). Pin on gate.
```

(b) Add the reader + op impls (place with the other `s2_*` op impls, e.g. after the event op impls):

```cpp
// C-ABI, called by the Rust core through the S2EngineOps table. Degrade-never-crash.
static void* S2_ClientAt(int slot) {
    if (!s_pNetworkServerService || s_offGameServer < 0) return nullptr;
    void* gs = *reinterpret_cast<void**>(reinterpret_cast<char*>(s_pNetworkServerService) + s_offGameServer);
    if (!gs || s_offClientCount < 0 || s_offClientElems < 0) return nullptr;
    int n = *reinterpret_cast<int*>(reinterpret_cast<char*>(gs) + s_offClientCount);
    if (slot < 0 || slot >= n) return nullptr;
    void** elems = *reinterpret_cast<void***>(reinterpret_cast<char*>(gs) + s_offClientElems);
    return elems ? elems[slot] : nullptr;
}
static int s2_client_signon(int slot) {
    void* c = S2_ClientAt(slot);
    return (c && s_offSscSignon >= 0)
        ? *reinterpret_cast<int*>(reinterpret_cast<char*>(c) + s_offSscSignon) : -1;
}
static int s2_client_userid(int slot) {
    void* c = S2_ClientAt(slot);
    if (!c || s_offSscUserId < 0) return -1;
    return static_cast<int>(*reinterpret_cast<int16_t*>(reinterpret_cast<char*>(c) + s_offSscUserId));
}
static int s2_client_valid(int slot) {
    int s = s2_client_signon(slot);
    return (s >= kSignonConnected) ? 1 : 0;
}
static const char* s2_client_name(int slot) {
    void* c = S2_ClientAt(slot);
    if (!c || s_offSscName < 0) return nullptr;
    return *reinterpret_cast<const char**>(reinterpret_cast<char*>(c) + s_offSscName);  // core copies now
}
static int s2_client_find_by_userid(int id) {
    if (!s_pNetworkServerService || s_offGameServer < 0) return -1;
    void* gs = *reinterpret_cast<void**>(reinterpret_cast<char*>(s_pNetworkServerService) + s_offGameServer);
    if (!gs || s_offClientCount < 0) return -1;
    int n = *reinterpret_cast<int*>(reinterpret_cast<char*>(gs) + s_offClientCount);
    for (int slot = 0; slot < n; slot++) {
        if (s2_client_valid(slot) && s2_client_userid(slot) == id) return slot;
    }
    return -1;
}
```

(c) Store `s_pNetworkServerService` — change the existing `tryGet("NetworkServerService", engineFactory);` (log-only) to acquire-and-store, mirroring the `SchemaSystem` block:

```cpp
        // Acquire + STORE INetworkServerService* (Slice 5D.2 engine identity; was log-only).
        {
            auto it = versions.find("NetworkServerService");
            const char* verStr = (it != versions.end()) ? it->second.c_str() : "NetworkServerService_001";
            int ret = 0;
            s_pNetworkServerService = engineFactory ? engineFactory(verStr, &ret) : nullptr;
            if (s_pNetworkServerService && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: NetworkServerService (%s)\n", verStr);
            } else {
                s_pNetworkServerService = nullptr;
                META_CONPRINTF("[s2script] WARN: interface MISSING: NetworkServerService (%s) — identity natives degrade\n", verStr);
            }
        }
```

(Remove the old `tryGet("NetworkServerService", engineFactory);` line.)

(d) Load the 6 offsets into the statics. In the existing offsets-loading area (where `GameEntitySystem` is read via `LoadOffsets(GamedataPath(), "linuxsteamrt64", ...)`), extend to also read the identity offsets. If the existing code only loads offsets inside the GameResourceService block, add a dedicated load near the NetworkServerService block:

```cpp
        // Load the engine-identity offsets (Slice 5D.2). Absent/typoed keys stay -1 -> degrade.
        {
            std::string offErr;
            auto offs = LoadOffsets(GamedataPath(), "linuxsteamrt64", offErr);
            auto pick = [&](const char* k) { auto i = offs.find(k); return i != offs.end() ? i->second : -1; };
            s_offGameServer  = pick("NetworkServerService.gameServer");
            s_offClientCount = pick("NetworkGameServer.clientCount");
            s_offClientElems = pick("NetworkGameServer.clientElems");
            s_offSscName     = pick("ServerSideClient.name");
            s_offSscSignon   = pick("ServerSideClient.signon");
            s_offSscUserId   = pick("ServerSideClient.userId");
            META_CONPRINTF("[s2script] identity offsets: gs=%d cnt=%d elems=%d name=%d signon=%d uid=%d\n",
                           s_offGameServer, s_offClientCount, s_offClientElems,
                           s_offSscName, s_offSscSignon, s_offSscUserId);
        }
```

(e) Wire the 5 ops into the `S2EngineOps ops = {}` table (after `ops.event_get_player_slot = ...`):

```cpp
    // Engine-identity ops (Slice 5D.2): order MUST match S2EngineOps in s2script_core.h + Rust v8host.rs.
    ops.client_valid          = &s2_client_valid;
    ops.client_userid         = &s2_client_userid;
    ops.client_signon         = &s2_client_signon;
    ops.client_name           = &s2_client_name;
    ops.client_find_by_userid = &s2_client_find_by_userid;
```

- [ ] **Step 8: Verify (core tests + boundary gates; shim compile deferred to Task 5)**

Run:
```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
```
Expected: all core tests pass (including `client_natives_degrade_without_ops`); both boundary gates green (no CS2 name entered `core/src` — the client ops are engine-generic; `CServerSideClient`/`INetworkServerService` appear only in the shim).

- [ ] **Step 9: Commit**

```bash
git add shim/include/s2script_core.h core/src/v8host.rs gamedata/core.gamedata.jsonc shim/src/s2script_mm.cpp
git commit -m "$(printf 'feat(slice5d2): engine-identity client-list ops + natives\n\n5 engine-generic ops (client_valid/userid/signon/name/find_by_userid) appended to S2EngineOps\n(C header + Rust mirror, ABI order). Shim reads the connected-client list via INetworkServerService\n(stored) + gamedata offsets; core natives degrade to false/-1/null. In-isolate degrade tests green.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Task 4: CS2 player identity API (JS + types + vm test)

**Files:**
- Modify: `games/cs2/js/pawn.js`, `packages/cs2/index.d.ts`
- Create: `packages/cli/test/player-identity.test.mjs`

**Interfaces:**
- Consumes: the Task-3 natives `__s2_client_userid`/`__s2_client_valid`/`__s2_client_find_by_userid`; the existing `EntityRef`, `__s2_ent_current_serial`, `Player`, `Player.fromSlot`, `MAX_PLAYERS`.
- Produces: `player.userId: number`; `Player.fromUserId(userId): Player | null`; `Player.allConnected(): Player[]`; `Player._fromSlotUnchecked(slot): Player | null` (internal — controller-valid, pawn NOT required).

- [ ] **Step 1: Write the failing vm-compose test**

Create `packages/cli/test/player-identity.test.mjs`:

```javascript
import { test } from "node:test";
import assert from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import vm from "node:vm";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const genJs = readFileSync(join(repo, "games/cs2/js/schema.generated.js"), "utf8");
const pawnJs = readFileSync(join(repo, "games/cs2/js/pawn.js"), "utf8");

function runWith(clientMock) {
  function EntityRef(i, s) { this.index = i; this.serial = s; }
  EntityRef.prototype.isValid = function () { return true; };
  EntityRef.prototype.readInt32 = function () { return 2; };
  EntityRef.prototype.readUInt8 = function () { return 2; };
  EntityRef.prototype.readFloat32 = function () { return 0.25; };
  EntityRef.prototype.readBool = function () { return false; };
  EntityRef.prototype.readHandle = function () { return new EntityRef(this.index + 100, 7); };
  const math = { Vector: function (x, y, z) { this.x = x; this.y = y; this.z = z; },
                 QAngle: function (x, y, z) { this.x = x; this.y = y; this.z = z; } };
  const ctx = {
    __s2require: (n) => (n === "@s2script/entity" ? { EntityRef } : n === "@s2script/math" ? math
                       : n === "@s2script/events" ? {} : null),
    __s2_schema_offset: () => 8,
    __s2_ent_current_serial: () => 7,
    __s2_handle_decode: (h) => [h & 0x7fff, 0],
    ...clientMock,
  };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext(genJs + "\n" + pawnJs, ctx);
  return ctx.__s2pkg_cs2;
}

test("Player.allConnected + userId (offline vm): connected slots regardless of pawn", () => {
  const { Player } = runWith({
    __s2_client_valid: (slot) => slot < 2,                       // slots 0,1 connected
    __s2_client_userid: (slot) => (slot === 0 ? 5 : slot === 1 ? 6 : -1),
    __s2_client_find_by_userid: (id) => (id === 6 ? 1 : -1),
  });
  const conn = Player.allConnected();
  assert.equal(conn.length, 2, "two connected slots");
  assert.equal(conn[0].slot, 0);
  assert.equal(conn[0].userId, 5, "userId reads the engine native, not schema");
  assert.equal(conn[1].userId, 6);
});

test("Player.fromUserId (offline vm): round-trips to the right slot, null on miss", () => {
  const { Player } = runWith({
    __s2_client_valid: () => true,
    __s2_client_userid: () => 6,
    __s2_client_find_by_userid: (id) => (id === 6 ? 1 : -1),
  });
  const p = Player.fromUserId(6);
  assert.ok(p, "found");
  assert.equal(p.slot, 1);
  assert.equal(Player.fromUserId(999), null, "miss -> null");
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/player-identity.test.mjs`
Expected: FAIL — `Player.allConnected is not a function`.

- [ ] **Step 3: Add the identity API to `games/cs2/js/pawn.js`**

After the `Player.all = function () { ... };` block (around line 54), add:

```javascript
  // --- Slice 5D.2: engine identity (the connected/pawnless follow promised at Player.fromSlot) ---
  // player.userId — the engine user-id (NOT a schema field); -1 if unassigned/absent.
  Object.defineProperty(Player.prototype, "userId", {
    get: function () { return __s2_client_userid(this.slot); },
    enumerable: true, configurable: true,
  });
  // Construct a Player from a slot when the CONTROLLER entity is valid — pawn NOT required
  // (unlike Player.fromSlot, which pawn-gates for the in-game-only Player.all()).
  Player._fromSlotUnchecked = function (slot) {
    var idx = slot + 1;                                          // controller entity index
    var ref = new EntityRef(idx, __s2_ent_current_serial(idx));
    return ref.isValid() ? new Player(ref) : null;
  };
  // Player.fromUserId(userId) — engine-userid lookup -> Player (pawnless-safe), or null.
  Player.fromUserId = function (userId) {
    var slot = __s2_client_find_by_userid(userId | 0);
    return slot < 0 ? null : Player._fromSlotUnchecked(slot);
  };
  // Player.allConnected() — every CONNECTED player regardless of pawn (the pawnless enumeration),
  // complementing the pawn-gated Player.all(). Uses the engine client list as the occupancy oracle.
  Player.allConnected = function () {
    var out = [];
    for (var s = 0; s < MAX_PLAYERS; s++) {
      if (__s2_client_valid(s)) { var p = Player._fromSlotUnchecked(s); if (p) out.push(p); }
    }
    return out;
  };
```

- [ ] **Step 4: Run to verify the test passes**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/player-identity.test.mjs`
Expected: PASS (2 tests).

- [ ] **Step 5: Add the types to `packages/cs2/index.d.ts`**

In `export interface Player extends Omit<CCSPlayerController, "pawn"> {` add the instance member:

```typescript
  /** The engine user-id (session-stable; NOT a schema field). `-1` if unassigned/absent. */
  readonly userId: number;
```

Find the `Player` static side (the `export const Player: { ... }` / namespace declaration that carries `fromSlot`/`all`) and add:

```typescript
  /** Look up a connected player by engine user-id. `null` if no such player. Pawnless-safe. */
  fromUserId(userId: number): Player | null;
  /** Every connected player regardless of pawn (the pawnless enumeration). Complements `all()`. */
  allConnected(): Player[];
```

Note to implementer: match the EXACT declaration shape already used for `fromSlot`/`all` in this file (interface vs. const-object vs. namespace) — read the surrounding lines and mirror them. Do NOT expose `_fromSlotUnchecked` in the types (it is internal).

- [ ] **Step 6: Verify the full CLI/JS suite + boundary gates**

Run:
```bash
cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs
cd - && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
```
Expected: all CLI/JS tests pass (existing + the 2 new); both gates green (CS2 identity JS lives in `games/cs2` + `packages/cs2`, not core).

- [ ] **Step 7: Commit**

```bash
git add games/cs2/js/pawn.js packages/cs2/index.d.ts packages/cli/test/player-identity.test.mjs
git commit -m "$(printf 'feat(slice5d2): Player.userId / fromUserId / allConnected (engine identity)\n\nCS2 player API over the Task-3 client-list natives: player.userId (engine, not schema),\nPlayer.fromUserId (pawnless-safe lookup), Player.allConnected (connected-but-pawnless enumeration\ncomplementing the pawn-gated Player.all()). vm-compose test green; Player.all() unchanged.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Task 5: Demo plugin + sniper build + live gates (events + identity) + docs

**Files:**
- Modify: `examples/demo-plugin/src/plugin.ts`, `README.md`, `CLAUDE.md`

**Interfaces:**
- Consumes: everything from Tasks 1–4 (the sig-scanned event manager; the identity natives + JS).
- Produces: the live-gate evidence + the spike-findings doc cross-link.

This task is CONTROLLER-DRIVEN (the sniper build + Docker CS2 server are heavy operations the controller runs). The implementer writes the demo plugin + docs; the controller performs the build + live gate and records results.

- [ ] **Step 1: Rewrite the demo plugin to exercise both threads**

Replace `examples/demo-plugin/src/plugin.ts` with:

```typescript
import { Player } from "@s2script/cs2";
import { Events } from "@s2script/cs2";

// Slice 5D.2 live gate:
//  A) game events deliver (sig-scanned IGameEventManager2): subscribe to round_start + player_spawn.
//  B) engine identity: Player.allConnected() (pawnless-safe), player.userId, Player.fromUserId round-trip.
export function onLoad(): void {
  console.log("[demo] onLoad (5D.2 events + identity)");

  Events.on("round_start", (ev) => {
    console.log("[demo] EVENT round_start timelimit=" + ev.getInt("timelimit"));
    reportPlayers();
  });
  Events.on("player_spawn", (ev) => {
    const slot = ev.getPlayerSlot("userid");
    console.log("[demo] EVENT player_spawn slot=" + slot);
  });
}

function reportPlayers(): void {
  const conn = Player.allConnected();
  console.log("[demo] allConnected=" + conn.length);
  for (const p of conn) {
    const uid = p.userId;
    const back = Player.fromUserId(uid);
    console.log("  slot=" + p.slot + " userId=" + uid
      + " teamNum=" + p.teamNum
      + " pawn=" + (p.pawn ? "yes" : "none")
      + " fromUserId(uid).slot=" + (back ? back.slot : "null"));
  }
}

export function onUnload(): void {
  console.log("[demo] onUnload");
}
```

Note to implementer: confirm `Events` is exported from `@s2script/cs2` (pawn.js line 92 re-exports it; `packages/cs2/index.d.ts` should declare it — if the import shape differs, match the 5D.1 demo/tests). Build the demo with `npx s2script build` from `examples/demo-plugin` per the existing README runbook; do NOT hand-edit the `.s2sp`.

- [ ] **Step 2: Controller — one sniper build (both threads' native changes)**

Run (controller):
```bash
docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
bash scripts/package-addon.sh
# re-copy the demo .s2sp (package-addon.sh rm -rf's the addon dir)
```
Expected: the shim + core build clean in the sniper container (this is the FIRST compile of the Task-1/2/3 shim changes). If the shim fails to compile, fix inline and rebuild (do not proceed to the live gate).

- [ ] **Step 3: Controller — live gate A (events) on de_inferno, bot_quota 2**

Bring up the Docker CS2 server (per README runbook), wait PAST the boot window (server ticking, `simulating` false), load the demo, then:
```bash
python3 scripts/rcon.py "bot_quota 2"
python3 scripts/rcon.py "mp_restartgame 1"    # forces round_start + player_spawn
```
Expected in the server log: `[s2script] interface OK: GameEventManager (sig-scan ctor-body-xref, 0x...)` AND `[demo] EVENT round_start ...` / `[demo] EVENT player_spawn slot=...` — the FIRST proof of 5D.1 event delivery. Record the exact lines.

- [ ] **Step 4: Controller — live gate B (identity), same server**

With the 2 bots spawned:
Expected: `[demo] allConnected=2`; each line shows a valid `userId` (>=0), `teamNum` 2/3, `pawn=yes`, and `fromUserId(uid).slot` equal to `slot` (round-trip). Then:
```bash
python3 scripts/rcon.py "bot_kick"
```
Expected: `allConnected=0` on the next `round_start`, server ticking, no crash. Record the signon threshold behaviour (if `allConnected` is empty while bots are present, lower `kSignonConnected` per §7 and note it).

- [ ] **Step 5: Write the live-gate findings + cross-link the spike doc**

Append a "Live-gate results" section to `docs/superpowers/specs/2026-07-03-slice-5d1b-sigscan-spike-findings.md` with the exact recorded log lines (events delivered, identity round-trip, degrade on bot_kick), and note the confirmed `kSignonConnected` value + whether `clientElems[slot]` index == player slot (the soft points from §7).

- [ ] **Step 6: Update README.md + CLAUDE.md**

- README: add a short "Slice 5D.2" note under the runbook — events now deliver (sig-scan), `player.userId`/`Player.fromUserId`/`Player.allConnected` available.
- CLAUDE.md `## Current state`: append a `5D.2` paragraph (mechanism + the live-gate result + what stays deferred), and update `Current focus` (the engine-RE bundle is now DONE; note the remaining deferred items as the next candidates). Keep the entry consistent with the existing per-slice style.

- [ ] **Step 7: Full verification sweep + commit**

Run:
```bash
bash scripts/test-sigscan.sh
cargo test -p s2script-core -- --test-threads=1
cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs && cd -
for g in check-nav-generated check-schema-generated check-events-generated check-core-boundary test-boundary-nameleak; do bash scripts/$g.sh >/dev/null 2>&1 && echo "$g PASS" || echo "$g FAIL"; done
```
Expected: sigscan passes; core green; CLI green; all 5 gates PASS.

```bash
git add examples/demo-plugin README.md CLAUDE.md docs/superpowers/specs/2026-07-03-slice-5d1b-sigscan-spike-findings.md
git commit -m "$(printf 'feat(slice5d2): live gate PASSED — event delivery + engine identity\n\nSig-scanned IGameEventManager2 delivers round_start/player_spawn (first proof of 5D.1 delivery);\nPlayer.allConnected/userId/fromUserId round-trip live on de_inferno (bot_quota 2), degrade on\nbot_kick, server ticking. Demo + README/CLAUDE + spike-findings live results.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Self-Review notes (author checklist — completed)

- **Spec coverage:** §2 spine → T2 (signatures) + T3 (offsets); §3 events/scanner → T1 (pure) + T2 (wiring); §4 identity ops → T3, player API → T4; §5 boundary → gates in T3/T4; §6 degrade → degrade paths in every native/op + T3 tests; §7 signon predicate → `kSignonConnected` + T5 live-pin; §8 tests/gates → per-task + T5 sweep; §9 tasks → T1–T5; §10 out-of-scope → not built.
- **Type consistency:** native names identical across T3 (Rust/C/register) and T4 (JS callers): `__s2_client_valid/userid/signon/name/find_by_userid`. Op field order identical in the C header + Rust mirror. Offset keys identical in the gamedata (T3 Step 6) and the shim reader (T3 Step 7d).
- **No placeholders:** every code step carries complete code; the two "match the neighbouring pattern" notes (Rust test lock/init in T3 Step 1; `.d.ts` declaration shape in T4 Step 5) point at concrete existing code the implementer reads, not invented content.
