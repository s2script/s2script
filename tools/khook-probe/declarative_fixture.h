#pragma once
#include "controlled_evidence.h"
#include <string>

// Actual Linux x86_64 entry prologues, suitable for verified-image signature + prologue recipes.
inline constexpr const char* kS2ProbeVoidSignature = "0F 1F 84 00 53 32 56 34";
inline constexpr const char* kS2ProbeWideSignature = "0F 1F 84 00 53 32 57 34";
inline constexpr const char* kS2ProbeAcquireSignature = "0F 1F 84 00 53 32 41 34";
extern "C" void S2ProbeDeclarativeHudTarget(void*,int64_t,int64_t,int64_t);
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

// Acceptance-only main shim/core/JS bridge. No private-copy dispatch is installed
// on these targets. Early/deferred peers measure callback registration order.
bool S2ProbeBridgeInstallEarly(std::string& reason);
bool S2ProbeBridgePrepare(const std::string& run,const std::string& suite,const std::string& artifact,
                          const std::string& measured_main_path,uint64_t map_generation);
void S2ProbeBridgeWorld(uint64_t map_generation,uint64_t tick);
const std::vector<s2khook::MainBridgeObservation>& S2ProbeBridgeCollect();
const std::vector<s2khook::PrecacheTokenObservation>& S2ProbePrecacheCollect();
bool S2ProbeBridgeCanUnloadSync(const S2HookTerminalPermit& permit);
bool S2ProbeBridgeUnloadSync(const S2HookTerminalPermit& permit);

bool S2ProbeBridgeBind(const std::string& run,const std::string& suite,const std::string& artifact);

// Source-owned deployed gamedata and real engine peers (acceptance only).
void S2ProbeLiveInstallEarly(const std::string& probe_path);
std::string S2ProbeLiveGamedata();
const std::vector<s2khook::RealAcquireObservation>& S2ProbeRealAcquireCollect();
bool S2ProbeLiveProvenanceReady();
