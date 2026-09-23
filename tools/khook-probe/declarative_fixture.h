#pragma once
#include "controlled_evidence.h"
#include <string>

// Actual Linux x86_64 entry prologues, suitable for verified-image signature + prologue recipes.
inline constexpr const char* kS2ProbeVoidSignature = "0F 1F 84 00 53 32 56 34";
inline constexpr const char* kS2ProbeWideSignature = "0F 1F 84 00 53 32 57 34";
inline constexpr const char* kS2ProbeAcquireSignature = "0F 1F 84 00 53 32 41 34";
extern "C" void S2ProbeDeclarativeVoidTarget(void* self);
extern "C" void S2ProbeDeclarativeWideTarget(void* self, float value, int32_t integer, int64_t a, int64_t b);
extern "C" int32_t S2ProbeDeclarativeAcquireTarget(void* self, int64_t a, int32_t method, int64_t b);

// Production engine_hooks.cpp is compiled into this separate test plugin. These snapshots prove
// native C++ delivery only when invoked against the stock provider; S1-6 owns JS bridge evidence.
// Reset only fixture observations. It neither removes native hooks nor erases production state.
bool S2ProbeDeclarativeInstall(std::string& reason);
void S2ProbeDeclarativeReset();
void S2ProbeDeclarativeInvoke();
s2khook::DeclarativeSnapshot S2ProbeDeclarativeCollect();
