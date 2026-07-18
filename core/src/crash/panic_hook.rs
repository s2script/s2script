//! Rust panic → envelope(kind=panic) → spool. The existing catch_unwind in ffi.rs still keeps
//! the panic from crossing FFI (the process survives); this hook makes it REPORTED instead of
//! silently swallowed (spec §6.4). The hook chains the previous hook and must itself never panic.
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Once;

static INSTALL: Once = Once::new();
/// Per-boot cap so a per-frame panicking descriptor cannot fill the spool (Task 5 adds
/// signature-level dedup on top of this).
static REPORTED: AtomicU32 = AtomicU32::new(0);
const MAX_PANICS_PER_BOOT: u32 = 32;

pub fn install() {
    INSTALL.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Everything best-effort; a failure here must never obscure the panic itself.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| report(info)));
            prev(info);
        }));
    });
}

fn report(info: &std::panic::PanicHookInfo) {
    if REPORTED.fetch_add(1, Ordering::Relaxed) >= MAX_PANICS_PER_BOOT { return; }
    let Some(dir) = crate::crash::spool_dir() else { return }; // identity not pushed yet → fail-off
    let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "panic (non-string payload)".to_string()
    };
    let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
    let backtrace = std::backtrace::Backtrace::force_capture().to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let bc = crate::crash::breadcrumb::snapshot();
    let env = crate::crash::envelope::render(
        &bc,
        "panic",
        crate::crash::envelope::Detail::Panic {
            message: if loc.is_empty() { msg } else { format!("{} ({})", msg, loc) },
            backtrace,
        },
        Some(crate::crash::envelope::iso8601_utc(now)),
        "", // server_id is patched in by the uploader at upload time (D-1 / Task 3)
        &crate::crash::config::scrub(&crate::crash::config::load()),
    );
    if let Ok(json) = serde_json::to_string(&env) {
        let _ = crate::crash::spool::write_incident(&dir, &json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swallowed_panic_writes_a_panic_envelope_to_spool() {
        let d = std::env::temp_dir().join(format!("s2crash-panic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        crate::crash::set_spool_dir(d.to_str().unwrap());
        install();
        crate::crash::breadcrumb::set_identity("fp-p", "0", "sdk", "sch", false);
        // The ffi.rs pattern: catch_unwind swallows the panic — but the hook has already reported.
        let r = std::panic::catch_unwind(|| panic!("test-panic-boom"));
        assert!(r.is_err());
        let items = crate::crash::spool::scan(&d);
        assert_eq!(items.len(), 1);
        let crate::crash::spool::SpoolItem::Envelope(p) = &items[0] else { panic!("expected envelope") };
        let json = std::fs::read_to_string(p).unwrap();
        let env: crate::crash::envelope::Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(env.kind, "panic");
        assert_eq!(env.schema_version, 1);
        match env.detail {
            crate::crash::envelope::Detail::Panic { message, backtrace } => {
                assert!(message.contains("test-panic-boom"));
                assert!(!backtrace.is_empty());
            }
            other => panic!("wrong detail: {:?}", other),
        }
        crate::crash::set_spool_dir(""); // don't leak the dir into other tests
    }
}
