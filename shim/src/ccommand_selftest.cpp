// Self-test for the console-command tokenizer behind CCommand::Tokenize.
//
// That tokenizer is the one piece of the FakeClientCommand capability that is OURS rather than
// Valve's (ccommand_shim.cpp explains why). Hand-written parsing with no test is how
// `sm_ban "some guy" 60` silently becomes three arguments. It is deliberately free of SDK types, so
// this binary links nothing but the tokenizer — no CUtlString, no g_pMemAlloc, no stubs.
//
// Run by scripts/ci-native.sh; non-zero exit on any failure.

#include "ccommand_tokenize.h"

#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

namespace {

int g_failures = 0;

void check(bool ok, const std::string& what, const std::string& got, const std::string& want) {
    if (ok) return;
    ++g_failures;
    std::printf("  FAIL %-44s got=[%s] want=[%s]\n", what.c_str(), got.c_str(), want.c_str());
}

// Mirrors what CCommand::Tokenize does with the result, so assertions read in ArgS()/Arg() terms.
struct Parsed {
    std::vector<std::string> argv;
    std::string argS;
    int count = 0;
};

Parsed run(const std::string& input, int argvCap = 511, int maxArgs = 64) {
    std::vector<char> buf(argvCap > 0 ? argvCap : 1, '\0');
    std::vector<char*> args(maxArgs > 0 ? maxArgs : 1, nullptr);
    int argv0Size = 0;
    Parsed p;
    p.count = s2_tokenize_command(input.c_str(), (int)input.size(),
                                  buf.data(), argvCap, args.data(), maxArgs, &argv0Size);
    for (int i = 0; i < p.count; ++i) p.argv.emplace_back(args[i]);
    p.argS = argv0Size ? input.substr(argv0Size) : "";
    return p;
}

void expect(const std::string& input,
            const std::vector<std::string>& wantArgv,
            const std::string& wantArgS) {
    const Parsed p = run(input);
    const std::string label = "\"" + input + "\"";
    check(p.argv.size() == wantArgv.size(), label + " argc",
          std::to_string(p.argv.size()), std::to_string(wantArgv.size()));
    if (p.argv.size() != wantArgv.size()) return;
    for (size_t i = 0; i < wantArgv.size(); ++i) {
        check(p.argv[i] == wantArgv[i], label + " arg" + std::to_string(i), p.argv[i], wantArgv[i]);
    }
    check(p.argS == wantArgS, label + " ArgS", p.argS, wantArgS);
}

void expectNoTokens(const std::string& input, const std::string& why) {
    const Parsed p = run(input);
    check(p.count == 0, why, std::to_string(p.count), "0");
}

}  // namespace

int main() {
    std::printf("ccommand_selftest: s2_tokenize_command\n");

    // The ordinary case, and the reason ArgS() exists: a command plus its remainder.
    expect("sm_help",                {"sm_help"},                          "");
    expect("sm_kick 3",              {"sm_kick", "3"},                     "3");
    expect("sm_ban 3 60 being rude", {"sm_ban","3","60","being","rude"},   "3 60 being rude");

    // Quoting: a quoted run is ONE argument, quotes stripped. This is the case that silently
    // breaks admin commands if the tokenizer just splits on spaces.
    expect("sm_ban \"some guy\" 60", {"sm_ban", "some guy", "60"},         "\"some guy\" 60");
    expect("say \"hello world\"",    {"say", "hello world"},               "\"hello world\"");
    expect("sm_msg 2 \"\"",          {"sm_msg", "2", ""},                  "2 \"\"");

    // ArgS() comes from the ORIGINAL string, so inner spacing survives — a handler that
    // re-broadcasts ArgS() must not reflow the user's text.
    expect("say   hello    world",   {"say", "hello", "world"},            "hello    world");

    // Whitespace must never yield empty arguments.
    expect("  sm_help  ",            {"sm_help"},                          "");
    // ArgS() is a pointer INTO the original buffer, so trailing whitespace is part of it — that is
    // Valve's behaviour too (m_ArgSBuffer holds the command verbatim). Asserted rather than trimmed
    // so a future "tidy-up" that diverges from the engine gets caught.
    expect("\tsm_kick\t3\t",         {"sm_kick", "3"},                     "3\t");

    // A quoted argv0 is stripped like any other token, and ArgS() still points past it.
    expect("\"sm_help\" x",          {"sm_help", "x"},                     "x");

    // Nothing to dispatch.
    expectNoTokens("", "empty string yields no tokens");
    expectNoTokens("   ", "whitespace-only yields no tokens");

    // Bounded: stop cleanly at the argv ceiling rather than overrunning the caller's array.
    {
        std::string many;
        for (int i = 0; i < 200; ++i) { many += "a"; if (i != 199) many += " "; }
        const Parsed p = run(many, 511, 64);
        check(p.count == 64, "stops at maxArgs", std::to_string(p.count), "64");
    }
    // Bounded: stop cleanly when the token buffer fills.
    {
        const Parsed p = run("aaaa bbbb cccc", /*argvCap=*/6, /*maxArgs=*/64);
        check(p.count == 1, "stops when argvBuf is full", std::to_string(p.count), "1");
        check(!p.argv.empty() && p.argv[0] == "aaaa", "first token still intact",
              p.argv.empty() ? "<none>" : p.argv[0], "aaaa");
    }
    // Defensive: a null input must not crash or write.
    {
        char buf[8]; char* args[4]; int a0 = -1;
        check(s2_tokenize_command(nullptr, 5, buf, 8, args, 4, &a0) == 0, "null src -> 0", "?", "0");
        check(a0 == 0, "null src resets argv0Size", std::to_string(a0), "0");
    }

    if (g_failures) { std::printf("ccommand_selftest: %d FAILED\n", g_failures); return 1; }
    std::printf("ccommand_selftest: all passed\n");
    return 0;
}
