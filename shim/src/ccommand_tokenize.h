// The pure tokenizer behind CCommand::Tokenize — deliberately free of every SDK type so it can be
// unit-tested standalone (shim/src/ccommand_selftest.cpp) without dragging in the tier1 cascade
// (CUtlString / CUtlVector / g_pMemAlloc / V_tier0_strlen). ccommand_shim.cpp is a thin adapter
// that owns the CCommand buffers and calls this.
#pragma once

/// Split `src` (length `len`) into console-command tokens.
///
///  * whitespace separates tokens; a double-quoted run is ONE token with the quotes stripped
///  * `argvBuf` receives each token nul-terminated, back to back; `args` receives pointers into it
///  * `*argv0Size` is the offset in `src` where the text AFTER argv0 begins (0 when there is none) —
///    this is what `CCommand::ArgS()` indexes, so quoting and inner spacing survive verbatim
///  * stops cleanly at `maxArgs` or `argvCap` rather than overrunning either
///
/// Returns the number of tokens written (0 when `src` is empty or all whitespace).
int s2_tokenize_command(const char* src, int len,
                        char* argvBuf, int argvCap,
                        char** args, int maxArgs,
                        int* argv0Size);
