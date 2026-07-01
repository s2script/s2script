//! s2script engine-generic core. Embeds V8 and exposes a tiny C ABI.
//! MUST NOT depend on any game package (enforced by scripts/check-core-boundary.sh).

mod async_rt;
pub mod plugin;
pub(crate) mod entity;
mod ffi;
mod loader;
mod multiplexer;
mod schema;
mod v8host;
