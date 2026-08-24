#pragma once

#include <cstdint>

namespace s2abi {

// x86-64 architectural GPR number -> flattened integer argument slot, or -1 when that register does
// not carry an incoming argument for this shape. Kept as data because SysV assigns GP arguments from
// one independent sequence while Microsoft x64 assigns paired GP/XMM registers by author position.
struct RegisterMap {
    int8_t slotOfGpr[16] = {
        -1, -1, -1, -1, -1, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1,
    };
};

}  // namespace s2abi
