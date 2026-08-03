# Detour relocation — a near-allocated short jump, and relocating what we steal

**Status:** Implemented — this document describes what shipped. Three things changed during
implementation and are marked in place: the near allocator probes with `MAP_FIXED_NOREPLACE` instead
of parsing `/proc/self/maps` (§4, which also retires a risk in §9); the near tier jumps to an
**island** in the trampoline page rather than to the handler (§4 — the handler is in our `.so` and
need not be within 2GB of the game's, which the first draft missed); and the test plan gained
end-to-end coverage of `Install` itself on both tiers (§8.5).
**Audience:** shim maintainers; anyone adding a detour target.
**Builds on:** `s2detour::Install` (`shim/src/detour.{h,cpp}`, Slice 6.6); the vendored HDE64 length
disassembler (`third_party/hde`); `S2_HookInstall`'s patch-site guard
(`shim/src/engine_hooks.cpp:244`).
**Blocks:** the two hooks shipped by
`docs/superpowers/specs/2026-08-02-declarative-inbound-hooks-design.md`, which resolve and arm on the
live server and are then refused at install.
**Reference:** `alliedmodders/sourcemod` — `public/CDetour/detours.cpp`; `cursey/safetyhook` —
`src/inline_hook.cpp`, `src/allocator.cpp`. Both read directly.

---

## 1. Why

`s2detour::Install` refuses to patch any function whose prologue contains a relative or rip-relative
instruction inside its steal window, because it copies the stolen bytes into the trampoline
**blindly** (`detour.cpp:47,54`). That is correct — copying a `lea rsi,[rip+disp32]` to a new address
silently changes what it loads — but it is a refusal where it should be a relocation.

It has now blocked three real targets:

| target | what lands in the window | outcome |
| --- | --- | --- |
| `CCSGameRules::TerminateRound` | `48 8D 35 disp32` — `lea rsi,[rip+disp32]` | `ctx.gameRules.onTerminateRound` degraded at the live gate |
| `CCSPlayerController::Respawn` | `E8 rel32` — `call rel32` | `ctx.players.onRespawn` degraded at the live gate |
| `OnPrecacheResource` | `mov [rip+disp32],rdi` **at offset 0** | abandoned; worked around with a class-vtable patch (`s2script_mm.cpp:3304`) |

The four detours that do work (`DispatchTraceAttack`, `HostSay`, `FireOutputInternal`,
`ProcessUsercmds`) work only because their prologues happen to be relocatable. That is luck, not
design, and every future hook target is a coin flip on it — which is the opposite of what the
declarative-hooks slice promised, where adding a hook was supposed to be a gamedata entry.

## 2. What SourceMod does (read from the source)

**SourceMod does not hand-roll this any more.** `public/CDetour/detours.cpp` delegates the whole
operation to `safetyhook::InlineHook::create(pAddress, callbackFunction, ...)` and translates its
error enum, which is itself the design vocabulary worth stealing:

```
Error::BAD_ALLOCATION                        Error::UNSUPPORTED_INSTRUCTION_IN_TRAMPOLINE
Error::FAILED_TO_DECODE_INSTRUCTION          Error::FAILED_TO_UNPROTECT
Error::SHORT_JUMP_IN_TRAMPOLINE              Error::NOT_ENOUGH_SPACE
Error::IP_RELATIVE_INSTRUCTION_OUT_OF_RANGE
```

On any of them `CDetourManager::CreateDetour()` deletes the detour and returns `NULL` — SM refuses
by name and keeps running, which is already our doctrine.

The decisive finding is in `safetyhook/src/inline_hook.cpp`: it hooks in **two tiers**, and tries the
cheap one first.

```
if (auto e9_result = e9_hook(allocator); !e9_result) {
    ...
    if (auto ff_result = ff_hook(allocator); !ff_result)
```

- **`e9_hook`** — steal only `sizeof(JmpE9)` = **5 bytes** (`E9 rel32`), and allocate the trampoline
  with `allocate_near(desired_addresses, m_trampoline_size)` so the `rel32` reaches it.
- **`ff_hook`** — the x86-64 fallback: `sizeof(JmpFF) + sizeof(uintptr_t)` = **14 bytes**, plain
  `allocate(...)` anywhere. And in *this* tier it refuses relative instructions:
  `if (ix.attributes & ZYDIS_ATTRIB_IS_RELATIVE) return unexpected{Error::ip_relative_instruction_out_of_range(ip)};`

**Our `detour.cpp` is only the fallback tier.** We wrote `ff_hook` and never wrote `e9_hook`, which
is precisely why we hit refusals SourceMod does not.

Relocation, when it is needed, is a displacement recompute against the new address:

```
std::copy_n(ip, ix.length, tramp_ip);
const auto target_address = ip + ix.length + ix.raw.disp.value;
const auto new_disp = target_address - (tramp_ip + ix.length);
store(tramp_ip + ix.raw.disp.offset, static_cast<int32_t>(new_disp));
```

with the same shape applied to `imm[0]` for `call`/`jmp rel32`, and short branches expanded to their
`rel32` forms (`0x0F 0x1X` = 6 bytes, `0xE9` = 5 bytes).

## 3. The two tiers, and why tier 1 alone unblocks the hooks slice

Shrinking the steal window from 14 to 5 is not a marginal improvement — it moves the offending
instruction *out of the window entirely* for both blocked hook targets. Decoded from the shipped
byte-signatures:

| target | tier-1 steal (≥5) | what tier 1 steals | tier-2 steal (≥14) | relative instr in the 14-B window? |
| --- | --- | --- | --- | --- |
| `DispatchTraceAttack` | 6 | `push rbp; mov rbp,rsp; push r15` | 15 | none |
| `HostSay` | 6 | `push rbp; mov rbp,rsp; push r15` | 16 | none |
| `FireOutputInternal` | 6 | `push rbp; mov rbp,rsp; push r15` | 15 | none |
| `ProcessUsercmds` | 6 | `push rbp; mov rbp,rsp; push r15` | 16 | none |
| **`TerminateRound`** | **6** | `push rbp; mov rbp,rsp; push r15` | 18 | `lea rsi,[rip+d]` @11 → **refused today** |
| **`Respawn`** | **6** | `push rbp; mov rbp,rsp; push r12` | 15 | `call rel32` @10 → **refused today** |
| `OnPrecacheResource` | 7 | `mov [rip+d],rdi` | — | rip-relative **at offset 0** |

Every function CS2 gives us opens with the same `push rbp; mov rbp,rsp; push <r>` — 6 position-
independent bytes. **Tier 1 needs no relocation at all for six of the seven targets**, including both
that are currently blocked.

`OnPrecacheResource` is the case that still needs real relocation, in either tier, because its
rip-relative store *is* the first instruction. It is the motivating case for tier 2 and the reason
tier 2 is in this slice rather than deferred — one tier without the other leaves a known target
unreachable and leaves us guessing again on the next one.

## 4. Tier 1 — the near allocator

`jmp rel32` reaches ±2GB. The trampoline must land in that window or the tier fails and we fall
through to tier 2.

- **Probe candidate addresses outward from the target**, stepping 1MB at a time, alternating below
  and above, out to just under 2GB. `mmap` each with `MAP_FIXED_NOREPLACE` (Linux ≥ 4.17; in glibc
  headers since 2.28, so present on the Steam Runtime 3 target), which maps *exactly there or not at
  all*. Inter-library gaps are far larger than the step, so this lands on the first or second try.
- If `MAP_FIXED_NOREPLACE` is unavailable at runtime (`EINVAL`) or silently honoured as a plain hint,
  latch that once and fall back to a hinted `mmap`, **verifying the returned address is in range** —
  never trust the hint. A far result on that path means the kernel is placing mappings where it likes
  and will keep doing so, so stop probing rather than spin.
- One page per install. A page holds many trampolines and sub-allocating them was the original plan,
  but with at most a handful of detours in the process it buys nothing and adds a free-list.
- On exhaustion: **not an error** — return "no near page" and let tier 2 run. Tier 1 can only ever
  *add* successes.

Probing beats the `/proc/self/maps` parse this spec originally called for: the kernel is the
authority on what is free, the answer cannot go stale between the read and the map, and there is no
parser to get wrong. It also makes the race in §9 disappear rather than merely survivable.

**The near tier's jump must target an island, not the handler.** `E9 rel32` reaches ±2GB of the
*target*, but the handler lives in **our** `.so`, which the loader need not place within 2GB of the
game's. So the trampoline page carries a third component and the `E9` lands on that:

```
  +0              relocated prologue        <- *origTrampoline: the "call original" entry
  +emitted        FF 25 <target+steal>      <- ...falls through to here and returns
  +emitted+14     FF 25 <handler>           <- the E9 lands HERE (unused by the far tier)
```

The near page is within 2GB of the target by construction, and the island reaches the handler
absolutely. Pointing the `E9` straight at the handler instead yields a hook that works only when the
loader happens to place the two modules close together — i.e. one that passes every test and then
fails on someone else's machine.

This is safetyhook's `Allocator::allocate_near` reduced to what one process on one platform needs.
We are not vendoring safetyhook: it requires Zydis (a full decoder, a large vendored dependency and a
new licence obligation) where HDE64 is already in-tree and 1 file. We are taking its *structure*.

## 5. Tier 2 — relocating what we steal

Applies to whichever tier is running; tier 1 needs it only for `OnPrecacheResource`-shaped
prologues, tier 2 needs it whenever a relative instruction is in the wider window.

**Relocated:**

1. **rip-relative operand** — `F_MODRM` with `modrm_mod == 0 && modrm_rm == 5` and `F_DISP32`.
   `newDisp = (oldTargetAddr) − (trampIp + len)`. Refuse if it does not fit `int32_t`.
2. **`call rel32` (E8), `jmp rel32` (E9), `jcc rel32` (0F 80–8F)** — same recompute against the
   instruction's `imm32`. Refuse if out of `int32_t` range. With a near trampoline this always fits;
   with a far one it may not, and then the refusal is honest.

**Refused, by name, with the reason surfaced to the caller:**

- `jcc rel8` (70–7F), `jmp rel8` (EB), `loop`/`jrcxz` (E0–E3). safetyhook widens these; we do not,
  because widening changes the copied instruction's size and a prologue containing one is
  hypothetical — no s2script target has ever had one. Named as
  `"short branch in stolen prologue (not widened)"` so if one ever appears we get a precise report
  rather than a mystery, and adding widening later is a contained change.
- Any `F_ERROR*` decode.
- Anything else carrying `F_RELATIVE` that is not case 2 above.

**The HDE64 gotcha that will bite the implementation.** safetyhook gets `ix.raw.disp.offset` and
`ix.raw.imm[0].offset` from Zydis. HDE64 gives us the decoded *values* (`hs.disp.disp32`,
`hs.imm.imm32`) but **not their byte offsets**. They must be derived:

```
immBytes = (F_IMM8 ? 1 : 0) + (F_IMM16 ? 2 : 0) + (F_IMM32 ? 4 : 0) + (F_IMM64 ? 8 : 0)
disp32Offset = len − immBytes − 4        // rip-relative operand
rel32Offset  = len − 4                   // E8 / E9 / 0F 8x  (no displacement on these forms)
```

Getting this wrong writes four bytes into the wrong place in an instruction that still decodes and
still runs. It is the single highest-risk line in the slice, and §8 gates it byte-exactly.

## 6. The latent guard bug this slice must also fix

`shim/src/engine_hooks.cpp:244` proves the patch site executable before letting `hde64_disasm` read
it — a real and well-reasoned guard — but it proves only `[target, target + 14)`:

```cpp
constexpr int kPatchWindow = 14;
```

`s2detour` steals **whole instructions until it has ≥14 bytes**, so it routinely reads past 14 (18
for `TerminateRound`) — the disassembler can read bytes that were never proven mapped, which is
exactly the SEGV-inside-hde64 the guard exists to prevent. It has been latent because the overrun is
small and the following page has always been mapped.

Fix it properly rather than bumping the constant: `s2detour::Install` takes an injected
`bool (*isExecutable)(const void*)` probe and calls it for each instruction's byte range *before*
decoding it. That removes the duplicated width constant, makes the guard exact instead of
approximate, and — because the probe is injected — makes it drivable from an off-server test.

## 7. API and call sites

`s2detour::Install`'s signature grows a probe and a reason-out; the four existing call sites and
`engine_hooks.cpp`'s pass their existing values through. Per the atomic-PR rule this lands with its
callers.

```cpp
namespace s2detour {
struct InstallResult { bool ok; const char* reason; int stolen; bool usedNearJump; };
InstallResult Install(void* target, void* handler, void** origTrampoline,
                      int (*isExecutable)(const void*));   // int, to be `S2_AddressIsExecutable`
void RemoveAll();
void SetForceFarTierForTest(bool on);   // see §8.5
}
```

`reason` is a static string, surfaced verbatim by `S2_HookInstall` into the existing degrade WARN, so
`onTerminateRound` stops saying "refused this prologue" and starts saying which instruction and why.
`usedNearJump` is logged once per install — it is the difference between a 5-byte and a 14-byte
patch, and we should never have to guess which one a live server took.

## 8. Testing

The point of this design is that **nearly all of it is provable off-server**, the way
`defer_queue.cpp` and `call_validate.cpp` already are. `Install` never has to run against a real
function to prove its relocation is right — it has to produce the right *bytes*.

Off-server (`scripts/test-detour-reloc.sh`, new, wired into `ci-native.sh`):

1. **Byte-exact relocation.** Feed hand-authored buffers — a `lea rsi,[rip+d]`, a `call rel32`, a
   `jmp rel32`, a `jcc rel32` — at a known address, relocate to a known trampoline address, and
   assert the emitted bytes *exactly*, including that `disp32Offset`/`rel32Offset` landed where §5
   says. Include an instruction carrying **both** a disp32 and an imm32 (`cmpl $imm,[rip+d]`), which
   is where the offset arithmetic breaks if `immBytes` is ignored.
2. **Semantic round-trip.** Relocate a real snippet into an mmap'd page, call it, assert it reads the
   same global / calls the same function as the original. This catches a correct-looking encoding
   that computes the wrong address.
3. **Every refusal fires, by name** — `jcc rel8`, a decode error, an out-of-`int32` displacement.
   Each asserted on the reason string, not just on `ok == false`.
4. **The executability probe is consulted for every stolen instruction**, proven by a probe that
   returns false on the second instruction and asserting the install refuses and touches nothing.
5. **Tier selection, and BOTH tiers end-to-end.** Install a real detour on a hand-written-asm function
   in the test binary (hand-written so its prologue is known, rather than whatever the compiler
   emitted at this optimisation level), call it, and assert the handler ran *and* that the handler's
   call through the trampoline reached the original body. Then force the far tier via
   `SetForceFarTierForTest` and assert the same. On a normally-loaded process the near tier always
   wins, so without that switch the 14-byte fallback — the path that runs precisely when address
   space is tight — would ship never having been executed once.

   This is the test that covers `Install` itself: the patch, the page layout, the jump back, and the
   island. Everything else here covers `BuildTrampolineBody`, which is bytes-to-bytes.
6. **Prologue corpus.** All seven prologues in §3 as byte literals, asserting the tier-1 steal length
   and, for the four already-working targets, that tier 1 steals a strict prefix of what tier 2
   steals today — the concrete statement of "this cannot regress an existing detour".

On-server (the live gate — the human runs it):

7. All four existing detours still fire: a damage event, a chat message, an entity output, a usercmd.
   **This is the non-negotiable one.** This slice changes the patch width of every detour in the
   framework; an off-server suite cannot prove the engine still runs.
8. The two blocked hooks install and pass declarative-inbound-hooks §9 checks 1–4 and 6, which have
   never run on a server.
9. The boot log reports `usedNearJump` for each, so we know which tier the live server actually took.

## 9. Risks

- **This touches every detour in the framework.** The blast radius is larger than the two hooks it
  unblocks, which is why §8.6 and §8.7 exist. If the live gate shows any regression, the honest
  fallback is a flag that pins tier 2 (today's exact behaviour) while the tier-1 path is fixed.
- **Patching is not atomic against concurrent execution.** Writing 5 bytes over a prologue another
  thread is executing is a torn-instruction hazard. It is not a *new* hazard — today's 14-byte patch
  is strictly worse — and installs happen on the main thread at first-subscribe, but it is the
  reason not to move installs off the main thread later.
- **A jump into the middle of a stolen prologue** is undetectable at patch time and breaks silently.
  safetyhook does not detect it either. Accepted, recorded here so it is not rediscovered.
- ~~**`/proc/self/maps` is a snapshot.**~~ Resolved by dropping the parse: probing with
  `MAP_FIXED_NOREPLACE` asks the kernel and acts atomically, so there is no window to race in.
- **A near page raises the odds a stale-gamedata address is "mapped but wrong".** Unchanged in kind —
  §6's per-instruction probe tightens the existing guard rather than loosening it.

## 10. Scope

**In:** both tiers; the near allocator; relocation of the two instruction classes in §5; the §6 guard
fix; the reason plumbing; the off-server suite; updating the four existing call sites.

**Out:** widening short branches (§5, named refusal); vendoring safetyhook or Zydis; Windows or x86;
unhooking individual detours (`RemoveAll` stays all-or-nothing); revisiting `OnPrecacheResource`'s
vtable workaround — this slice makes a detour *possible* there, it does not change that code.
