//! s2script engine-generic core. Embeds V8 and exposes a tiny C ABI.
//! MUST NOT depend on any game package (enforced by scripts/check-core-boundary.sh).

mod ffi;
mod multiplexer;
mod v8host;
