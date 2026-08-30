//! s2script engine-generic core. Embeds V8 and exposes a tiny C ABI.
//! MUST NOT depend on any game package (enforced by scripts/check-core-boundary.sh).

// These two lints are DENIED because together they are the fingerprint of a silent, shippable bug
// that nothing else in the gate suite catches.
//
// A `match k { KIND_I32 => .., KIND_F32 => .. }` is a constant comparison only while those constants
// are IN SCOPE. Move the arms to another module and leave the constants behind and each `KIND_*`
// silently degrades into an irrefutable BINDING pattern: the first arm swallows every value, the rest
// are dead, and every field read returns the wrong type. It compiles. `cargo build` succeeds. All 542
// tests pass. CI is green. That was measured, not assumed — see the core-stabilization entity slice,
// where exactly this happened while moving the entity natives out of `v8host.rs`.
//
// `unreachable_patterns` is the reliable signal: once the first arm swallows everything, arms 2..N
// are provably dead, so any such match trips it. Denying it turns the one warning that existed (lost
// in a 100k-line build log) into a build failure. `non_snake_case` also fires here — on the accidental
// bindings — but it is NOT denied: it false-positives on deliberately emphatic test names like
// `..._published_by_a_DIFFERENT_producer`, and a gate that fires on correct code is a gate someone
// eventually deletes.
//
// Denied at the CRATE ROOT rather than via RUSTFLAGS in ci-native.sh so it also fires in a local
// `cargo build`, and so no workflow path filter can route around it.
#![deny(unreachable_patterns)]

mod acquire;
mod admin;
mod async_rt;
mod bans;
pub mod config;
mod cookies;
mod db;
pub mod interfaces;
pub mod plugin;
pub(crate) mod entity;
pub(crate) mod liveness;
pub(crate) mod entity_live;
pub(crate) mod fold;
mod channels;
mod client;
mod commands;
mod events;
mod ffi;
mod gamedata_calls;
mod gamedata_hooks;
mod http;
mod jobs;
mod loader;
mod multiplexer;
mod nest;
mod dispatch;
mod net;
pub mod owner_stores;
pub mod process_singletons;
pub(crate) mod crash;
mod schema;
mod schema_catalog;
mod sdkhooks;
mod sqldb;
mod usermsg;
mod v8host;
mod ws;
