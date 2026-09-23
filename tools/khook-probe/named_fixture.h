#pragma once

#include "controlled_evidence.h"
#include "khook_binding.h"
#include <string>

bool S2ProbeNamedInstall(std::string& reason);
void S2ProbeNamedReset();
void S2ProbeNamedInvoke();
s2khook::NamedSnapshot S2ProbeNamedCollect();
bool S2ProbeNamedCanUnloadSync(const S2HookTerminalPermit& permit);
bool S2ProbeNamedUnloadSync(const S2HookTerminalPermit& permit);
bool S2ProbeNamedRemovalComplete();
