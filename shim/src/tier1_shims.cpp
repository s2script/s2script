// Slice 5D.1 — a self-contained definition of the ONE tier1 symbol the event system needs.
//
// Reading an IGameEvent field by name (`ev->GetInt(CKV3MemberName(key), …)`) constructs a
// CKV3MemberName from the C-string key. That constructor calls MakeStringToken → MurmurHash2LowerCase,
// which lives in tier1 (hl2sdk/tier1/generichash.cpp). The shim otherwise links no tier1, so at
// dlopen time Metamod fails with `undefined symbol: MurmurHash2LowerCase(char const*, unsigned int)`.
//
// Compiling the whole generichash.cpp — or linking tier1.a — cascades into CUtlString / UtlVectorMemory /
// V_tier0_strlen (tier1/tier0), which are not otherwise resolvable. Since CKV3MemberName needs ONLY this
// one function, we provide it here, self-contained: the EXACT MurmurHash2 algorithm from Valve's
// generichash.cpp, with a plain ASCII lowercase in place of CUtlString::ToLowerFast (identical result:
// ToLowerFast is an ASCII-only A–Z → a–z pass). The hash MUST match Valve's byte-for-byte or event field
// lookups silently return defaults — this is verified live (real field values ⇒ the hash matches).

#include <cstdint>
#include <cstring>
#include <cstdlib>

namespace {
// Exact copy of Valve's MurmurHash2 core (generichash.cpp). LittleDWord() is identity on little-endian
// x86-64 (our only target); we use memcpy for the 4-byte load to avoid an unaligned-read UB.
uint32_t s2_MurmurHash2(const void* key, int len, uint32_t seed) {
    const uint32_t m = 0x5bd1e995u;
    const int r = 24;
    uint32_t h = seed ^ (uint32_t)len;
    const unsigned char* data = (const unsigned char*)key;
    while (len >= 4) {
        uint32_t k;
        std::memcpy(&k, data, 4);
        k *= m; k ^= k >> r; k *= m;
        h *= m; h ^= k;
        data += 4; len -= 4;
    }
    switch (len) {
        case 3: h ^= (uint32_t)data[2] << 16; [[fallthrough]];
        case 2: h ^= (uint32_t)data[1] << 8;  [[fallthrough]];
        case 1: h ^= (uint32_t)data[0]; h *= m;
    }
    h ^= h >> 13; h *= m; h ^= h >> 15;
    return h;
}
} // namespace

// Global-namespace symbol matching the SDK declaration (uint32 == unsigned int on x86-64 Linux):
//   uint32 MurmurHash2LowerCase(char const *pString, uint32 nSeed)   -> _Z20MurmurHash2LowerCasePKcj
unsigned int MurmurHash2LowerCase(char const* pString, unsigned int nSeed) {
    if (!pString) return s2_MurmurHash2("", 0, nSeed);
    size_t len = std::strlen(pString);
    char stackbuf[256];
    char* buf = (len < sizeof(stackbuf)) ? stackbuf : (char*)std::malloc(len + 1);
    if (!buf) return nSeed;  // OOM — degrade (won't match, but never crash)
    for (size_t i = 0; i < len; ++i) {
        char c = pString[i];
        buf[i] = (c >= 'A' && c <= 'Z') ? (char)(c + ('a' - 'A')) : c;
    }
    unsigned int h = s2_MurmurHash2(buf, (int)len, nSeed);
    if (buf != stackbuf) std::free(buf);
    return h;
}

// The length-taking overload — provided for completeness (same lowercase + core, honoring nLength):
//   uint32 MurmurHash2LowerCase(char const *pString, int nLength, uint32 nSeed) -> _Z20MurmurHash2LowerCasePKcij
unsigned int MurmurHash2LowerCase(char const* pString, int nLength, unsigned int nSeed) {
    if (!pString || nLength <= 0) return s2_MurmurHash2("", 0, nSeed);
    char stackbuf[256];
    char* buf = ((size_t)nLength < sizeof(stackbuf)) ? stackbuf : (char*)std::malloc((size_t)nLength + 1);
    if (!buf) return nSeed;
    for (int i = 0; i < nLength; ++i) {
        char c = pString[i];
        buf[i] = (c >= 'A' && c <= 'Z') ? (char)(c + ('a' - 'A')) : c;
    }
    unsigned int h = s2_MurmurHash2(buf, nLength, nSeed);
    if (buf != stackbuf) std::free(buf);
    return h;
}
