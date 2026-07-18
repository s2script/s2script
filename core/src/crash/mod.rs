//! Crash-reporter capture client (engine-generic). Sub-project 1 of the crash-reporter spec.
//! No V8 types cross into this module; no game names ever appear here.
pub mod breadcrumb;
pub mod config;
pub mod envelope;
pub mod panic_hook;
pub mod spool;

use std::path::PathBuf;
use std::sync::Mutex;

static SPOOL_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Record the crash-spool directory (pushed by the shim with the identity block). Empty → None.
pub fn set_spool_dir(path: &str) {
    let mut g = match SPOOL_DIR.lock() { Ok(g) => g, Err(p) => p.into_inner() };
    *g = if path.is_empty() { None } else { Some(PathBuf::from(path)) };
}

pub fn spool_dir() -> Option<PathBuf> {
    match SPOOL_DIR.lock() { Ok(g) => g.clone(), Err(p) => p.into_inner().clone() }
}
