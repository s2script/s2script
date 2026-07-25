// The pure console-command tokenizer. No SDK types by design — see ccommand_tokenize.h.
#include "ccommand_tokenize.h"

#include <cstring>

namespace {
inline bool is_space(char c) {
    return c == ' ' || c == '\t' || c == '\r' || c == '\n' || c == '\f' || c == '\v';
}
}  // namespace

int s2_tokenize_command(const char* src, int len,
                        char* argvBuf, int argvCap,
                        char** args, int maxArgs,
                        int* argv0Size) {
    if (argv0Size) *argv0Size = 0;
    if (!src || !argvBuf || !args || len <= 0) return 0;

    int argc = 0;
    int used = 0;
    int i = 0;

    while (i < len) {
        while (i < len && is_space(src[i])) ++i;
        if (i >= len) break;
        if (argc >= maxArgs) break;

        const bool quoted = src[i] == '"';
        if (quoted) ++i;
        const int tokStart = i;
        while (i < len && (quoted ? src[i] != '"' : !is_space(src[i]))) ++i;
        const int tokLen = i - tokStart;
        const int tokEnd = i;                     // the closing quote / delimiter
        if (quoted && i < len) ++i;               // step over the closing quote

        if (used + tokLen + 1 > argvCap) break;   // out of buffer: stop, never overrun
        char* dst = argvBuf + used;
        std::memcpy(dst, src + tokStart, tokLen);
        dst[tokLen] = '\0';
        used += tokLen + 1;

        args[argc++] = dst;

        // After argv0, ArgS() must point into the ORIGINAL string, past argv0 and the whitespace
        // separating it from the rest — so inner quoting/spacing is preserved verbatim.
        if (argc == 1) {
            int rest = quoted ? tokEnd + 1 : tokEnd;
            while (rest < len && is_space(src[rest])) ++rest;
            if (argv0Size) *argv0Size = (rest < len) ? rest : 0;   // 0 ⇒ ArgS() is ""
        }
    }
    return argc;
}
