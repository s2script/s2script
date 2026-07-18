//! Crash spool: the on-disk handoff between capture (any context) and upload (next boot /
//! periodic sweep). Bounded both directions; every failure is a silent skip (fail-off).
use std::path::{Path, PathBuf};

pub const MAX_SPOOL: usize = 50;
pub const MAX_SENT: usize = 50;

#[derive(Debug, PartialEq)]
pub enum SpoolItem {
    /// A rendered envelope (<uuid>.json) — js/panic kinds.
    Envelope(PathBuf),
    /// A Breakpad pair: <stem>.dmp + <stem>.dmp.s2meta — native kind.
    Native { meta: PathBuf, dump: PathBuf },
}

/// Write one rendered envelope as <uuid>.json. None (skip) when the dir is missing/unwritable
/// or already holds MAX_SPOOL pending incidents (bounded disk).
pub fn write_incident(dir: &Path, envelope_json: &str) -> Option<PathBuf> {
    if scan(dir).len() >= MAX_SPOOL { return None; }
    let path = dir.join(format!("{}.json", uuid::Uuid::new_v4()));
    std::fs::write(&path, envelope_json).ok()?;
    Some(path)
}

/// Enumerate pending incidents: every *.json, plus every *.dmp that has a *.dmp.s2meta sidecar.
/// (A .dmp without a sidecar is still uploaded — breadcrumbless; a sidecar without a .dmp is
/// treated as an orphan and reported envelope-only by the uploader.)
pub fn scan(dir: &Path) -> Vec<SpoolItem> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for e in entries.flatten() {
        let p = e.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if name.ends_with(".json") {
            out.push(SpoolItem::Envelope(p));
        } else if name.ends_with(".dmp") {
            let meta = PathBuf::from(format!("{}.s2meta", p.display()));
            out.push(SpoolItem::Native { meta, dump: p });
        }
    }
    out.sort_by_key(|i| match i {
        SpoolItem::Envelope(p) => p.clone(),
        SpoolItem::Native { dump, .. } => dump.clone(),
    });
    out
}

/// Move uploaded files into <dir>/sent/, pruning sent/ down to MAX_SENT (oldest first by mtime).
pub fn mark_sent(dir: &Path, files: &[PathBuf]) {
    let sent = dir.join("sent");
    let _ = std::fs::create_dir_all(&sent);
    for f in files {
        if let Some(name) = f.file_name() {
            let _ = std::fs::rename(f, sent.join(name));
        }
    }
    // Prune sent/ (oldest first).
    let Ok(entries) = std::fs::read_dir(&sent) else { return };
    let mut all: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let t = e.metadata().ok()?.modified().ok()?;
            Some((t, p))
        })
        .collect();
    if all.len() <= MAX_SENT { return; }
    all.sort_by_key(|(t, _)| *t);
    for (_, p) in all.iter().take(all.len() - MAX_SENT) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("s2crash-spool-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn write_scan_mark_sent_roundtrip() {
        let d = tmpdir("rt");
        let p = write_incident(&d, r#"{"schema_version":1}"#).expect("write");
        assert!(p.exists());
        // A native pair is discovered as one item.
        std::fs::write(d.join("aaaa.dmp"), b"MDMP").unwrap();
        std::fs::write(d.join("aaaa.dmp.s2meta"), b"meta").unwrap();
        let items = scan(&d);
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|i| matches!(i, SpoolItem::Native { .. })));
        mark_sent(&d, &[p.clone(), d.join("aaaa.dmp"), d.join("aaaa.dmp.s2meta")]);
        assert!(scan(&d).is_empty());
        assert!(d.join("sent").join(p.file_name().unwrap()).exists());
    }

    #[test]
    fn spool_is_bounded_at_max() {
        let d = tmpdir("cap");
        for _ in 0..MAX_SPOOL {
            assert!(write_incident(&d, "{}").is_some());
        }
        assert!(write_incident(&d, "{}").is_none(), "51st incident must be dropped");
    }
}
