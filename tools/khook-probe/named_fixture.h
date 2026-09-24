#pragma once

#include "controlled_evidence.h"
#include "khook_binding.h"
#include <cstdint>
#include <string>

bool S2ProbeNamedInstall(std::string& reason);
void S2ProbeNamedReset(std::uint64_t map_generation);
void S2ProbeNamedInvoke();
s2khook::NamedSnapshot S2ProbeNamedCollect();
bool S2ProbeNamedCanUnloadSync(const S2HookTerminalPermit& permit);
bool S2ProbeNamedUnloadSync(const S2HookTerminalPermit& permit);
bool S2ProbeNamedRemovalComplete();

// One-way early-peer -> async completion -> late-peer registration phases.
void S2ProbeNamedStartOrders(const std::string& run);
void S2ProbeNamedAdvanceOrders();
s2khook::NamedOrderSnapshot S2ProbeNamedCollectOrders();
