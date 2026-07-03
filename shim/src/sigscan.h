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
