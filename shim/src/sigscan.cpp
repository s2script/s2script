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
