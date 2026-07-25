// Self-contained definitions of the CCommand members CS2 does not export.
//
// WHY THIS FILE EXISTS. `tier1/convar.h` declares CCommand's constructor, Tokenize, Reset and
// EnsureBuffers out-of-line, but NO shipped CS2 binary defines them:
//
//   $ nm -D --defined-only <every game .so> | grep -E 'CCommand::(CCommand|Tokenize|Reset)'
//   (nothing)
//
// A shared library links fine with undefined symbols — they resolve at dlopen — so using them
// compiles, links, passes `make shim`, and then kills the ENTIRE addon on a live server with
// `undefined symbol: _ZN8CCommand8TokenizeE10CUtlStringP14characterset_t`. That is not
// hypothetical; it happened. `scripts/check-shim-symbols.sh` now catches the class at build time.
//
// The fix is NOT reverse engineering. CCommand's data members are CUtlVectorFixedGrowable
// templates declared in the header, so the LAYOUT is already ours — the header is the definition.
// Only the out-of-line METHOD BODIES are missing, and hl2sdk vendors them at
// third_party/hl2sdk/tier1/convar.cpp. Compiling that file wholesale would drag in the whole
// ConVar system plus the CUtlString/UtlVectorMemory/tier0 cascade that `tier1_shims.cpp` exists to
// avoid (see its header comment). So, exactly as that file does for MurmurHash2LowerCase, we
// define here the few members we actually need. Defining them as member functions (rather than
// poking at the struct from outside) gives us private access and emits precisely the mangled
// symbols the linker is asking for.
//
// FIDELITY. Reset/EnsureBuffers/the default ctor are line-for-line Valve. The tokenizer is ours
// rather than a copy, because Valve's routes through CUtlBuffer::ParseToken (another unexported
// symbol) and CCommand::DefaultBreakSet → g_pCVar->GetCharacterSet(). It lives in
// ccommand_tokenize.h/.cpp, free of SDK types so it is unit-testable standalone, and reproduces the
// observable contract Valve's has, which is all the engine consumes:
//   * argv[0] is the command name; ArgC/ArgV/Arg index the tokens
//   * whitespace separates tokens; a double-quoted run is ONE token with the quotes stripped
//   * ArgS() is everything after argv0, taken from the ORIGINAL string (so inner quoting and
//     spacing survive verbatim) — that is what m_nArgv0Size indexes
//   * over-long input is refused (false) rather than truncated
// Re-check against hl2sdk/tier1/convar.cpp on any submodule bump.

#include "tier1/convar.h"
#include "tier1/utlstring.h"

#include <cstring>

#include "ccommand_tokenize.h"

void CCommand::EnsureBuffers() {
    m_ArgSBuffer.SetSize(MaxCommandLength());
    m_ArgvBuffer.SetSize(MaxCommandLength());
}

void CCommand::Reset() {
    m_nArgv0Size = 0;
    m_ArgSBuffer.Base()[0] = '\0';
    m_Args.RemoveAll();
}

CCommand::CCommand() {
    EnsureBuffers();
    Reset();
}

// The break-set argument is accepted for signature compatibility and ignored: the engine only ever
// tokenizes console command strings, for which the break set is whitespace-plus-quotes. Honouring a
// caller-supplied set would mean reimplementing characterset_t semantics for a path nothing uses.
bool CCommand::Tokenize(CUtlString pCommand, characterset_t* /*pBreakSet*/) {
    if (m_ArgSBuffer.Count() == 0) EnsureBuffers();
    Reset();

    if (pCommand.IsEmpty()) return false;

    const char* src = pCommand.Get();
    const int nLen = pCommand.Length();
    // Valve refuses rather than truncating; a silently-clipped command is worse than a rejected one.
    if (nLen >= m_ArgSBuffer.Count() - 1) return false;

    memmove(m_ArgSBuffer.Base(), src, nLen + 1);

    char* args[COMMAND_MAX_ARGC];
    int argv0Size = 0;
    const int n = s2_tokenize_command(m_ArgSBuffer.Base(), nLen,
                                      m_ArgvBuffer.Base(), m_ArgvBuffer.Count(),
                                      args, COMMAND_MAX_ARGC, &argv0Size);
    if (n <= 0) return false;

    for (int i = 0; i < n; ++i) m_Args.AddToTail(args[i]);
    m_nArgv0Size = argv0Size;
    return true;
}
