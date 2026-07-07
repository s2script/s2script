//! s2script engine-generic core. Embeds V8 and exposes a tiny C ABI.
//! MUST NOT depend on any game package (enforced by scripts/check-core-boundary.sh).

mod async_rt;
pub mod config;
mod cookies;
mod db;
pub mod interfaces;
pub mod plugin;
pub(crate) mod entity;
mod event_mux;
mod ffi;
mod http;
mod loader;
mod multiplexer;
mod schema;
mod schema_catalog;
mod v8host;
