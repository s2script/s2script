use super::prepare_selection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);
struct TestDir(PathBuf);
impl TestDir {
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> TestDir {
    let path = std::env::temp_dir().join(format!(
        "s2-game-package-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    TestDir(path)
}
fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn record(id: &str, owner: &str, game: &str) -> Value {
    json!({"id":id,"match":{"engine":"source2","game":game},"gamedataOwner":owner,
      "bootstrap":{"path":format!("game-packages/{owner}/index.js"),"sha256":hash(b"boot")},
      "gamedata":{"path":format!("game-packages/{owner}/gamedata.json"),"sha256":hash(b"data")}})
}
fn manifest(packages: Vec<Value>) -> Value {
    json!({"schemaVersion":1,"packages":packages})
}
fn complete_fixture(packages: Vec<Value>) -> TestDir {
    let root = fixture();
    for package in &packages {
        for (kind, bytes) in [
            ("bootstrap", b"boot".as_slice()),
            ("gamedata", b"data".as_slice()),
        ] {
            let path = package[kind]["path"].as_str().unwrap();
            write(&root.path().join(path), bytes);
        }
    }
    write(
        &root.path().join("game-packages.json"),
        manifest(packages).to_string().as_bytes(),
    );
    root
}
fn select(root: &TestDir, game: &str) -> Result<super::PreparedSelection, super::PackageError> {
    prepare_selection(root.path(), "source2", game, "linuxsteamrt64")
}
fn error_for(packages: Vec<Value>) -> String {
    let root = fixture();
    write(
        &root.path().join("game-packages.json"),
        manifest(packages).to_string().as_bytes(),
    );
    select(&root, "csgo").unwrap_err().code().to_owned()
}

#[test]
fn selects_verified_owned_bytes_and_provenance() {
    let root = complete_fixture(vec![record("@fixture/a", "a", "csgo")]);
    let selected = select(&root, "csgo").unwrap();
    assert_eq!(selected.id, "@fixture/a");
    assert_eq!(selected.gamedata_owner, "a");
    assert_eq!(selected.bootstrap_bytes, b"boot");
    assert_eq!(selected.gamedata_bytes, b"data");
    assert_eq!(selected.bootstrap_sha256, hash(b"boot"));
    assert_eq!(selected.gamedata_sha256, hash(b"data"));
    assert_eq!(selected.provenance.engine, "source2");
    assert_eq!(selected.provenance.game, "csgo");
    assert_eq!(selected.provenance.platform, "linuxsteamrt64");
    std::fs::remove_file(root.path().join("game-packages/a/index.js")).unwrap();
    assert_eq!(selected.bootstrap_bytes, b"boot");
}

#[test]
fn matches_detected_game_token_without_an_unpublished_name_grammar() {
    let root = complete_fixture(vec![record("@fixture/a", "a", "fixture_mod")]);
    assert_eq!(select(&root, "fixture_mod").unwrap().id, "@fixture/a");
}

#[test]
fn missing_manifest_and_zero_match_are_named_missing() {
    assert_eq!(select(&fixture(), "csgo").unwrap_err().code(), "missing");
    let root = complete_fixture(vec![record("@fixture/a", "a", "csgo")]);
    assert_eq!(select(&root, "other").unwrap_err().code(), "missing");
}

#[test]
fn names_every_ambiguous_candidate_in_sorted_order() {
    let root = complete_fixture(vec![
        record("@fixture/b", "b", "csgo"),
        record("@fixture/a", "a", "csgo"),
    ]);
    let err = select(&root, "csgo").unwrap_err();
    assert_eq!(err.code(), "ambiguous");
    assert_eq!(err.candidates(), &["@fixture/a", "@fixture/b"]);
}

#[test]
fn rejects_invalid_schema_and_exact_field_sets() {
    let root = fixture();
    for value in [
        json!({"schemaVersion":2,"packages":[]}),
        json!({"schemaVersion":1,"packages":[],"extra":0}),
        json!({"schemaVersion":1,"packages":[{"id":"@fixture/a"}]}),
        manifest(vec![{
            let mut p = record("@fixture/a", "a", "csgo");
            p["match"]["platform"] = json!("linux");
            p
        }]),
    ] {
        write(
            &root.path().join("game-packages.json"),
            value.to_string().as_bytes(),
        );
        assert_eq!(
            select(&root, "csgo").unwrap_err().code(),
            "invalid-manifest"
        );
    }
}

#[test]
fn rejects_duplicate_ids_owners_and_artifact_paths() {
    assert_eq!(
        error_for(vec![
            record("@fixture/a", "a", "csgo"),
            record("@fixture/a", "b", "other")
        ]),
        "invalid-manifest"
    );
    assert_eq!(
        error_for(vec![
            record("@fixture/a", "a", "csgo"),
            record("@fixture/b", "a", "other")
        ]),
        "invalid-manifest"
    );
    let mut b = record("@fixture/b", "b", "other");
    b["bootstrap"]["path"] = json!("game-packages/a/index.js");
    assert_eq!(
        error_for(vec![record("@fixture/a", "a", "csgo"), b]),
        "invalid-manifest"
    );
}

#[test]
fn validates_every_record_metadata_before_selection() {
    let mut other = record("@fixture/b", "b", "other");
    other["gamedata"]["sha256"] = json!("A".repeat(64));
    assert_eq!(
        error_for(vec![record("@fixture/a", "a", "csgo"), other]),
        "invalid-manifest"
    );
    for bad in ["0".repeat(63), "G".repeat(64), "A".repeat(64)] {
        let mut p = record("@fixture/a", "a", "csgo");
        p["bootstrap"]["sha256"] = json!(bad);
        assert_eq!(error_for(vec![p]), "invalid-manifest");
    }
    assert_eq!(
        error_for(vec![record("@fixture/under_score", "a", "csgo")]),
        "invalid-manifest"
    );
    assert_eq!(
        error_for(vec![record("@fixture/a", "under_score", "csgo")]),
        "invalid-manifest"
    );
}

#[test]
fn rejects_parent_absolute_and_non_normal_paths_before_artifact_io() {
    for bad in [
        "../outside",
        "/tmp/outside",
        "game-packages/../outside",
        "game-packages//a",
        "game-packages/./a",
        "game-packages\\a",
    ] {
        let mut p = record("@fixture/a", "a", "csgo");
        p["bootstrap"]["path"] = json!(bad);
        assert_eq!(error_for(vec![p]), "invalid-path", "{bad}");
    }
}

#[test]
fn rejects_directory_and_escaping_symlink_artifacts() {
    let root = complete_fixture(vec![record("@fixture/a", "a", "csgo")]);
    let artifact = root.path().join("game-packages/a/index.js");
    std::fs::remove_file(&artifact).unwrap();
    std::fs::create_dir(&artifact).unwrap();
    assert_eq!(select(&root, "csgo").unwrap_err().code(), "invalid-path");
    #[cfg(unix)]
    {
        std::fs::remove_dir(&artifact).unwrap();
        write(&root.path().join("outside.js"), b"boot");
        std::os::unix::fs::symlink(root.path().join("outside.js"), &artifact).unwrap();
        assert_eq!(select(&root, "csgo").unwrap_err().code(), "invalid-path");
    }
}

#[cfg(unix)]
#[test]
fn rejects_duplicate_resolved_selected_artifact_paths() {
    let root = complete_fixture(vec![record("@fixture/a", "a", "csgo")]);
    let alias = root.path().join("game-packages/a/gamedata.json");
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(root.path().join("game-packages/a/index.js"), alias).unwrap();
    assert_eq!(
        select(&root, "csgo").unwrap_err().code(),
        "invalid-manifest"
    );
}

#[test]
fn unrelated_missing_artifacts_do_not_block_selection() {
    let root = complete_fixture(vec![
        record("@fixture/a", "a", "csgo"),
        record("@fixture/b", "b", "other"),
    ]);
    std::fs::remove_dir_all(root.path().join("game-packages/b")).unwrap();
    assert_eq!(select(&root, "csgo").unwrap().id, "@fixture/a");
}

#[test]
fn rejects_non_utf8_bootstrap_and_tampered_bytes() {
    let root = complete_fixture(vec![record("@fixture/a", "a", "csgo")]);
    write(&root.path().join("game-packages/a/index.js"), b"tampered");
    assert_eq!(select(&root, "csgo").unwrap_err().code(), "hash-mismatch");
    write(&root.path().join("game-packages/a/index.js"), &[0xff]);
    assert_eq!(
        select(&root, "csgo").unwrap_err().code(),
        "invalid-bootstrap"
    );
}
