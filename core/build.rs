use std::path::Path;
use std::process::Command;

fn main() {
    // Keep libs2script_core.so resident for the process lifetime so the V8
    // platform survives a Metamod `meta unload` / `meta load` cycle (see ARCHITECTURE §2.1 / spec §5).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,-z,nodelete");
    }
    framework_version();
}

/// Stamp the framework version (`S2SCRIPT_VERSION`) the same way the shim's CMake does: from
/// `scripts/framework-version.sh`, which takes `VERSION`, a `v*` tag build, or `git describe`.
/// The crate's own Cargo version stays 0.0.0 and is never what the runtime reports.
fn framework_version() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let root = Path::new(&manifest).join("..");
    for var in ["VERSION", "GITHUB_REF_TYPE", "GITHUB_REF_NAME"] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    println!("cargo:rerun-if-changed={}", root.join("scripts/framework-version.sh").display());
    // A commit or a new tag changes a development version: watch HEAD, the ref it points at, and tags.
    for spec in ["HEAD", "packed-refs", "refs/tags"] {
        if let Some(path) = git_path(&root, spec) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(head_ref) = git(&root, &["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git_path(&root, &head_ref) {
            println!("cargo:rerun-if-changed={path}");
        }
    }

    let resolved = Command::new("bash")
        .arg("scripts/framework-version.sh")
        .current_dir(&root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let version = resolved.unwrap_or_else(|| {
        println!(
            "cargo:warning=cannot resolve the framework version (no Git history and no VERSION); \
             reporting 0.0.0-unknown"
        );
        "0.0.0-unknown".to_string()
    });
    println!("cargo:rustc-env=S2SCRIPT_VERSION={version}");
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(root).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn git_path(root: &Path, spec: &str) -> Option<String> {
    // `--git-path` answers relative to `root` unless the Git dir is elsewhere (bullseye's Git 2.30
    // has no `--path-format`).
    let path = root.join(git(root, &["rev-parse", "--git-path", spec])?);
    path.exists().then(|| path.display().to_string())
}
