//! Engine-generic async file IO (`@s2script/sdk/files`): binary/text reads and writes inside a
//! per-plugin sandbox, executed on the shared tokio runtime's blocking pool so disk latency (and
//! optional gzip work) never lands on the game thread. The main thread only copies the payload
//! (a memcpy out of a TypedArray) and later materialises the result.
//!
//! Root: `<data dir>/files/<plugin id>/`. Paths are relative, `/`-separated, and every segment is
//! `[A-Za-z0-9._-]` with no `.`/`..` segment, so a plugin can never address a file outside its own
//! root. Writes are atomic (temp file + rename) unless appending. Sizes are capped by the async
//! policy's `input_item_bytes` (one payload, both directions, after decompression) and charged to
//! the job lease like every other async producer (docs/ASYNC_LIMITS.md).
//!
//! Like `http.rs`, the engine half holds no V8 handles; promise settlement lives in `v8host`
//! (`resolve_files`) because it needs the resolver map and the owning plugin's context.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

/// Deepest relative path accepted (segments).
const MAX_DEPTH: usize = 8;
/// Longest relative path accepted (bytes).
const MAX_PATH: usize = 256;
/// Most names one `list` returns.
const MAX_LIST: usize = 4096;

pub(crate) enum FileOp {
    Read { path: PathBuf, gzip: bool, text: bool },
    Write { path: PathBuf, data: Vec<u8>, gzip: bool, append: bool },
    Remove { path: PathBuf },
    List { dir: PathBuf },
}

pub(crate) enum FileOutput {
    /// `None` = the file does not exist.
    Bytes(Option<Vec<u8>>),
    Text(Option<String>),
    Done,
    Removed(bool),
    Names(Vec<String>),
}

pub(crate) struct FileCompletion {
    pub id: u64,
    pub result: Result<FileOutput, String>,
    pub lease: crate::async_limits::JobLease,
    queue: crate::async_limits::QueueTicket,
}

struct Channel {
    tx: Sender<FileCompletion>,
    rx: Mutex<Receiver<FileCompletion>>,
}
static CHANNEL: OnceLock<Channel> = OnceLock::new();

fn channel_get() -> &'static Channel {
    CHANNEL.get_or_init(|| {
        let (tx, rx) = channel();
        Channel { tx, rx: Mutex::new(rx) }
    })
}

/// Validate a plugin-supplied relative path and join it under `root`. `allow_empty` admits `""`
/// (the root itself, for `list`).
pub(crate) fn sandbox_path(root: &Path, rel: &str, allow_empty: bool) -> Result<PathBuf, String> {
    if rel.is_empty() {
        return if allow_empty { Ok(root.to_path_buf()) } else { Err("InvalidPath: empty path".into()) };
    }
    if rel.len() > MAX_PATH {
        return Err("InvalidPath: path too long".into());
    }
    let mut out = root.to_path_buf();
    let mut depth = 0;
    for seg in rel.split('/') {
        depth += 1;
        let ok = !seg.is_empty()
            && seg != "."
            && seg != ".."
            && seg.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-');
        if !ok || depth > MAX_DEPTH {
            return Err(format!("InvalidPath: {rel:?}"));
        }
        out.push(seg);
    }
    Ok(out)
}

/// The per-plugin root under the s2script data directory. The plugin id is reduced to the same
/// segment alphabet so `@scope/name` becomes one directory, never a nested or escaping path.
pub(crate) fn plugin_root(data_dir: &Path, plugin_id: &str) -> PathBuf {
    let seg: String = plugin_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '_' })
        .collect();
    let seg = if seg.is_empty() || seg.chars().all(|c| c == '.') { "_".to_owned() } else { seg };
    data_dir.join("files").join(seg)
}

/// Run one operation synchronously (on a blocking-pool thread). `limit` caps any payload.
pub(crate) fn run(op: FileOp, limit: usize, lease: &mut crate::async_limits::JobLease) -> Result<FileOutput, String> {
    match op {
        FileOp::Read { path, gzip, text } => {
            let file = match std::fs::File::open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(if text { FileOutput::Text(None) } else { FileOutput::Bytes(None) })
                }
                Err(e) => return Err(format!("read failed: {e}")),
            };
            if !gzip {
                if let Ok(meta) = file.metadata() {
                    if meta.len() > limit as u64 {
                        return Err("AsyncPayloadTooLarge".into());
                    }
                }
            }
            let reader: Box<dyn Read> = if gzip {
                Box::new(flate2::read::GzDecoder::new(std::io::BufReader::new(file)))
            } else {
                Box::new(file)
            };
            // Read at most limit+1 bytes: one more than the cap proves the payload is too large
            // without ever buffering an unbounded (or decompression-bomb) stream.
            let mut buf = Vec::new();
            let mut take = reader.take(limit as u64 + 1);
            let mut chunk = [0u8; 64 * 1024];
            loop {
                let n = take.read(&mut chunk).map_err(|e| format!("read failed: {e}"))?;
                if n == 0 {
                    break;
                }
                if buf.len() + n > limit {
                    return Err("AsyncPayloadTooLarge".into());
                }
                lease.grow(n).map_err(|e| e.to_string())?;
                buf.extend_from_slice(&chunk[..n]);
            }
            if text {
                Ok(FileOutput::Text(Some(match String::from_utf8(buf) {
                    Ok(s) => s,
                    Err(e) => crate::async_limits::utf8_lossy_owned(e.as_bytes()),
                })))
            } else {
                Ok(FileOutput::Bytes(Some(buf)))
            }
        }
        FileOp::Write { path, data, gzip, append } => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("write failed: {e}"))?;
            }
            let encoded = if gzip {
                let mut enc = flate2::write::GzEncoder::new(Vec::with_capacity(data.len() / 2), flate2::Compression::fast());
                enc.write_all(&data).map_err(|e| format!("write failed: {e}"))?;
                enc.finish().map_err(|e| format!("write failed: {e}"))?
            } else {
                data
            };
            if append {
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .map_err(|e| format!("write failed: {e}"))?;
                f.write_all(&encoded).map_err(|e| format!("write failed: {e}"))?;
            } else {
                // Atomic replace: a crash or a concurrent reader never observes a torn file. Each
                // write gets its own temp name, so overlapping writes to one path cannot share (and
                // splice) a temp file; the last rename wins whole.
                static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let mut tmp = path.clone().into_os_string();
                tmp.push(format!(".{}-{seq}.s2tmp", std::process::id()));
                let tmp = PathBuf::from(tmp);
                {
                    let mut f = std::fs::File::create(&tmp).map_err(|e| format!("write failed: {e}"))?;
                    f.write_all(&encoded).map_err(|e| format!("write failed: {e}"))?;
                }
                std::fs::rename(&tmp, &path).map_err(|e| {
                    let _ = std::fs::remove_file(&tmp);
                    format!("write failed: {e}")
                })?;
            }
            Ok(FileOutput::Done)
        }
        FileOp::Remove { path } => match std::fs::remove_file(&path) {
            Ok(()) => Ok(FileOutput::Removed(true)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FileOutput::Removed(false)),
            Err(e) => Err(format!("remove failed: {e}")),
        },
        FileOp::List { dir } => {
            let rd = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(FileOutput::Names(Vec::new())),
                Err(e) => return Err(format!("list failed: {e}")),
            };
            let mut names = Vec::new();
            for entry in rd.flatten() {
                let Ok(name) = entry.file_name().into_string() else { continue };
                if name.ends_with(".s2tmp") {
                    continue;
                }
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let name = if is_dir { format!("{name}/") } else { name };
                lease.grow(name.len() + 32).map_err(|e| e.to_string())?;
                names.push(name);
                if names.len() >= MAX_LIST {
                    break;
                }
            }
            names.sort();
            Ok(FileOutput::Names(names))
        }
    }
}

/// Submit `op` on the shared runtime's blocking pool. The lease travels with the work and comes
/// back in the completion, exactly like `http::fetch_reserved`.
pub(crate) fn submit_reserved(id: u64, op: FileOp, lease: crate::async_limits::JobLease) -> Result<(), String> {
    let tx = channel_get().tx.clone();
    let limit = lease.policy.input_item_bytes;
    let spawned = crate::http::try_spawn(async move {
        let cancel = lease.cancel.clone();
        let mut work = tokio::task::spawn_blocking(move || {
            let mut lease = lease;
            let r = run(op, limit, &mut lease);
            (r, lease)
        });
        let (result, mut lease) = tokio::select! {
            biased;
            _ = cancel.wait() => match work.await {
                // The blocking work cannot be interrupted; it ran to completion, but the owner is
                // gone, so its output is dropped and only the lease is retired.
                Ok((_, lease)) => (Err("AsyncCancelled".to_owned()), lease),
                Err(_) => return,
            },
            r = &mut work => match r {
                Ok(pair) => pair,
                Err(_) => return,
            },
        };
        let result = result.map_err(crate::async_limits::diagnostic);
        if result.is_err() {
            lease.discard_result();
        }
        let _guard = crate::async_limits::delivery_guard(&lease.cancel);
        if _guard.is_some() {
            let _ = tx.send(FileCompletion { id, result, lease, queue: crate::async_limits::QueueTicket::new(5) });
        }
    });
    if spawned { Ok(()) } else { Err("files not available".into()) }
}

pub(crate) fn try_recv_completed() -> Option<FileCompletion> {
    let mut c = CHANNEL.get()?.rx.lock().ok()?.try_recv().ok()?;
    c.queue.dequeue();
    Some(c)
}

// ---------------------------------------------------------------------------
// V8 adapter — `__s2_files_op(kind, path, data?, flags) -> Promise`.
// kind: 0 read, 1 write, 2 remove, 3 list. flags: bit0 gzip, bit1 append, bit2 text (read).
// ---------------------------------------------------------------------------

use crate::v8host::set_native;

fn s2_files_op(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let resolver = v8::PromiseResolver::new(scope).unwrap();
        let promise = resolver.get_promise(scope);
        let result = (|| -> Result<(), String> {
            let kind = args.get(0).int32_value(scope).unwrap_or(-1);
            let flags = args.get(3).int32_value(scope).unwrap_or(0);
            let (gzip, append, text) = (flags & 1 != 0, flags & 2 != 0, flags & 4 != 0);
            let data_val = args.get(2);
            let data_len = if kind == 1 {
                if let Ok(s) = v8::Local::<v8::String>::try_from(data_val) {
                    s.utf8_length(scope)
                } else if let Ok(v) = v8::Local::<v8::ArrayBufferView>::try_from(data_val) {
                    v.byte_length()
                } else {
                    return Err("TypeError: data must be a string or a Uint8Array".into());
                }
            } else {
                0
            };
            // Admission before any copy: the payload is charged as the job's input.
            let mut lease = crate::jobs::reserve(scope, 0).map_err(|e| e.to_string())?;
            let rel = crate::jobs::copy_string(scope, args.get(1), &mut lease).map_err(|e| e.to_string())?;
            lease.input_grow(data_len).map_err(|e| e.to_string())?;
            let owner = crate::v8host::current_plugin(scope).unwrap_or_default();
            let data_dir = crate::v8host::files_data_dir().ok_or("files not available")?;
            let root = plugin_root(Path::new(&data_dir), &owner);
            let op = match kind {
                0 => FileOp::Read { path: sandbox_path(&root, &rel, false)?, gzip, text },
                1 => {
                    let path = sandbox_path(&root, &rel, false)?;
                    let data = if let Ok(s) = v8::Local::<v8::String>::try_from(data_val) {
                        s.to_rust_string_lossy(scope).into_bytes()
                    } else {
                        let view = v8::Local::<v8::ArrayBufferView>::try_from(data_val).map_err(|_| "TypeError: data")?;
                        let mut buf = vec![0u8; data_len];
                        let n = view.copy_contents(&mut buf);
                        buf.truncate(n);
                        buf
                    };
                    if append && gzip {
                        return Err("TypeError: append and gzip cannot be combined".into());
                    }
                    FileOp::Write { path, data, gzip, append }
                }
                2 => FileOp::Remove { path: sandbox_path(&root, &rel, false)? },
                3 => FileOp::List { dir: sandbox_path(&root, &rel, true)? },
                _ => return Err("TypeError: unknown file operation".into()),
            };
            crate::jobs::check_live(&lease)?;
            let id = crate::jobs::next_id();
            let cancel = lease.cancel.clone();
            submit_reserved(id, op, lease)?;
            crate::jobs::commit_reserved(scope, id, resolver, cancel);
            Ok(())
        })();
        if let Err(e) = result {
            crate::jobs::reject(scope, resolver, &e);
        }
        rv.set(promise.into());
    }));
}

/// Build the JS value a completed operation resolves to.
pub(crate) fn output_to_js<'s>(scope: &mut v8::PinScope<'s, '_>, out: FileOutput) -> v8::Local<'s, v8::Value> {
    match out {
        FileOutput::Bytes(None) | FileOutput::Text(None) => v8::null(scope).into(),
        FileOutput::Bytes(Some(bytes)) => {
            let len = bytes.len();
            let store = v8::ArrayBuffer::new_backing_store_from_vec(bytes).make_shared();
            let ab = v8::ArrayBuffer::with_backing_store(scope, &store);
            match v8::Uint8Array::new(scope, ab, 0, len) {
                Some(u) => u.into(),
                None => v8::null(scope).into(),
            }
        }
        FileOutput::Text(Some(s)) => v8::String::new(scope, &s)
            .map(|s| s.into())
            .unwrap_or_else(|| v8::null(scope).into()),
        FileOutput::Done => v8::undefined(scope).into(),
        FileOutput::Removed(b) => v8::Boolean::new(scope, b).into(),
        FileOutput::Names(names) => {
            let items: Vec<v8::Local<v8::Value>> = names
                .iter()
                .filter_map(|n| v8::String::new(scope, n).map(|s| s.into()))
                .collect();
            v8::Array::new_with_elements(scope, &items).into()
        }
    }
}

pub(crate) fn install_natives(scope: &mut v8::PinScope, global_obj: v8::Local<v8::Object>) {
    set_native(scope, global_obj, "__s2_files_op", s2_files_op);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease() -> crate::async_limits::JobLease {
        crate::async_limits::domain().job(None, 0).unwrap()
    }

    #[test]
    fn sandbox_rejects_escapes_and_accepts_nested_names() {
        let root = Path::new("/srv/data/files/p");
        assert!(sandbox_path(root, "../x", false).is_err());
        assert!(sandbox_path(root, "a/../../x", false).is_err());
        assert!(sandbox_path(root, "/etc/passwd", false).is_err());
        assert!(sandbox_path(root, "a//b", false).is_err());
        assert!(sandbox_path(root, "a/./b", false).is_err());
        assert!(sandbox_path(root, "a\\b", false).is_err());
        assert!(sandbox_path(root, "", false).is_err());
        assert_eq!(sandbox_path(root, "", true).unwrap(), root);
        assert!(sandbox_path(root, &"a/".repeat(9).trim_end_matches('/').to_string(), false).is_err());
        assert_eq!(
            sandbox_path(root, "replays/surf_x/0_wr.bin", false).unwrap(),
            root.join("replays").join("surf_x").join("0_wr.bin")
        );
    }

    #[test]
    fn plugin_root_flattens_scoped_ids() {
        let d = Path::new("/srv/data");
        assert_eq!(plugin_root(d, "@s2s/surftimer"), d.join("files").join("_s2s_surftimer"));
        assert_eq!(plugin_root(d, ".."), d.join("files").join("_"));
        assert_eq!(plugin_root(d, ""), d.join("files").join("_"));
    }

    #[test]
    fn write_read_gzip_append_list_remove_roundtrip() {
        let dir = std::env::temp_dir().join(format!("s2files-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("a").join("b.bin");
        let payload: Vec<u8> = (0..100_000u32).map(|i| (i % 7) as u8).collect();
        let mut l = lease();
        assert!(matches!(run(FileOp::Write { path: p.clone(), data: payload.clone(), gzip: true, append: false }, 1 << 20, &mut l), Ok(FileOutput::Done)));
        assert!(std::fs::metadata(&p).unwrap().len() < payload.len() as u64 / 4, "gzip should shrink a repetitive payload");
        match run(FileOp::Read { path: p.clone(), gzip: true, text: false }, 1 << 20, &mut l) {
            Ok(FileOutput::Bytes(Some(b))) => assert_eq!(b, payload),
            _ => panic!("gzip roundtrip"),
        }
        // A decompressed payload over the cap is refused, never buffered whole.
        assert!(run(FileOp::Read { path: p.clone(), gzip: true, text: false }, 1000, &mut l).is_err());
        let t = dir.join("log.txt");
        for line in ["one\n", "two\n"] {
            run(FileOp::Write { path: t.clone(), data: line.as_bytes().to_vec(), gzip: false, append: true }, 1 << 20, &mut l).unwrap();
        }
        match run(FileOp::Read { path: t.clone(), gzip: false, text: true }, 1 << 20, &mut l) {
            Ok(FileOutput::Text(Some(s))) => assert_eq!(s, "one\ntwo\n"),
            _ => panic!("append text"),
        }
        assert!(matches!(run(FileOp::Read { path: dir.join("missing"), gzip: false, text: false }, 10, &mut l), Ok(FileOutput::Bytes(None))));
        match run(FileOp::List { dir: dir.clone() }, 10, &mut l) {
            Ok(FileOutput::Names(n)) => assert_eq!(n, vec!["a/".to_owned(), "log.txt".to_owned()]),
            _ => panic!("list"),
        }
        assert!(matches!(run(FileOp::Remove { path: t.clone() }, 10, &mut l), Ok(FileOutput::Removed(true))));
        assert!(matches!(run(FileOp::Remove { path: t }, 10, &mut l), Ok(FileOutput::Removed(false))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn overlapping_replacing_writes_never_splice() {
        let dir = std::env::temp_dir().join(format!("s2files-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("same.bin");
        let writers: Vec<_> = (0..8u8)
            .map(|k| {
                let p = p.clone();
                std::thread::spawn(move || {
                    let mut l = lease();
                    for _ in 0..20 {
                        run(FileOp::Write { path: p.clone(), data: vec![k; 64 * 1024], gzip: false, append: false }, 1 << 20, &mut l).unwrap();
                    }
                })
            })
            .collect();
        for w in writers { w.join().unwrap(); }
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(bytes.len(), 64 * 1024);
        assert!(bytes.iter().all(|&b| b == bytes[0]), "the final file is exactly one writer's payload");
        let leftovers = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".s2tmp")).count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
