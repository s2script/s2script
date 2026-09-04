---
"@s2script/cs2": patch
---

gamedata(cs2): re-derive CCSGameRules_TerminateRound for build 1.41.7.8/14178

The pinned signature stopped matching and the descriptor degraded as designed —
`call 'terminateRound' unavailable: signature did not match this build` — taking
`onTerminateRound` with it, since the hook targets the same signature.

The function did not move; its prologue was reordered. The reason capture went
from `mov r15d,esi` to `mov r12d,esi` and the `lea rsi` anchor from fn+0xb to
fn+0x10, so `string-xref.at` moves 11 -> 16.

Re-derived by the recipe already in the file: the single rip-relative xref to
"TerminateRound" (0x8f4d9b) lands at 0x13cd4a0, entry 0x13cd490, and the sole
xref to "TerminateRound: unknown round end ID %i" sits in the same body. The new
pattern is unique binary-wide, and the boot gate now arms both the call and the
hook.
