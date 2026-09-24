//! Preparation of a deployed game package, before any runtime registration or bootstrap.
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PackageError {
    Missing,
    Ambiguous(Vec<String>),
    InvalidManifest,
    InvalidPath,
    InvalidBootstrap,
    HashMismatch,
}

impl PackageError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Ambiguous(_) => "ambiguous",
            Self::InvalidManifest => "invalid-manifest",
            Self::InvalidPath => "invalid-path",
            Self::InvalidBootstrap => "invalid-bootstrap",
            Self::HashMismatch => "hash-mismatch",
        }
    }

    pub(crate) fn candidates(&self) -> &[String] {
        match self {
            Self::Ambiguous(ids) => ids,
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Provenance {
    pub engine: String,
    pub game: String,
    pub platform: String,
    pub manifest_path: PathBuf,
    pub bootstrap_path: PathBuf,
    pub gamedata_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedSelection {
    pub id: String,
    pub gamedata_owner: String,
    pub bootstrap_bytes: Vec<u8>,
    pub gamedata_bytes: Vec<u8>,
    pub bootstrap_sha256: String,
    pub gamedata_sha256: String,
    pub provenance: Provenance,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Manifest {
    schema_version: u32,
    packages: Vec<Package>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Package {
    id: String,
    #[serde(rename = "match")]
    matcher: Match,
    gamedata_owner: String,
    bootstrap: Artifact,
    gamedata: Artifact,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Match {
    engine: String,
    game: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    path: String,
    sha256: String,
}

fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn valid_id(value: &str) -> bool {
    let Some(rest) = value.strip_prefix('@') else {
        return false;
    };
    let Some((scope, name)) = rest.split_once('/') else {
        return false;
    };
    valid_token(scope) && valid_token(name)
}

fn digest(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        fn nibble(c: u8) -> Option<u8> {
            match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                _ => None,
            }
        }
        bytes[index] = nibble(chunk[0])? * 16 + nibble(chunk[1])?;
    }
    Some(bytes)
}

fn path_parts(value: &str) -> Result<(), PackageError> {
    // The deployed format uses forward-slash, addon-relative names with no normalization step.
    if value.starts_with('/') || value.contains('\\') || value.contains(':') {
        return Err(PackageError::InvalidPath);
    }
    let parts: Vec<_> = value.split('/').collect();
    if parts.len() < 3
        || parts[0] != "game-packages"
        || parts
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err(PackageError::InvalidPath);
    }
    Ok(())
}

fn artifact_path(
    root: &Path,
    package_root: &Path,
    artifact: &Artifact,
) -> Result<PathBuf, PackageError> {
    let canonical = root
        .join(&artifact.path)
        .canonicalize()
        .map_err(|_| PackageError::InvalidPath)?;
    if !canonical.starts_with(package_root) || canonical == package_root {
        return Err(PackageError::InvalidPath);
    }
    if !canonical
        .metadata()
        .map_err(|_| PackageError::InvalidPath)?
        .is_file()
    {
        return Err(PackageError::InvalidPath);
    }
    Ok(canonical)
}

fn verify(bytes: &[u8], expected: &str) -> Result<(), PackageError> {
    let expected = digest(expected).ok_or(PackageError::InvalidManifest)?;
    if Sha256::digest(bytes).as_slice() != expected {
        return Err(PackageError::HashMismatch);
    }
    Ok(())
}

pub(crate) fn prepare_selection(
    addon_root: &Path,
    engine: &str,
    game: &str,
    platform: &str,
) -> Result<PreparedSelection, PackageError> {
    let manifest_path = addon_root.join("game-packages.json");
    let manifest_bytes = match fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(PackageError::Missing)
        }
        Err(_) => return Err(PackageError::InvalidManifest),
    };
    let manifest: Manifest =
        serde_json::from_slice(&manifest_bytes).map_err(|_| PackageError::InvalidManifest)?;
    if manifest.schema_version != 1 {
        return Err(PackageError::InvalidManifest);
    }

    let mut ids = HashSet::new();
    let mut owners = HashSet::new();
    let mut paths = HashSet::new();
    for package in &manifest.packages {
        if !valid_id(&package.id)
            || !valid_token(&package.gamedata_owner)
            || package.matcher.engine.is_empty()
            || package.matcher.game.is_empty()
            || !ids.insert(&package.id)
            || !owners.insert(&package.gamedata_owner)
        {
            return Err(PackageError::InvalidManifest);
        }
        for artifact in [&package.bootstrap, &package.gamedata] {
            path_parts(&artifact.path)?;
            if digest(&artifact.sha256).is_none() || !paths.insert(&artifact.path) {
                return Err(PackageError::InvalidManifest);
            }
        }
    }

    let mut matches: Vec<_> = manifest
        .packages
        .iter()
        .filter(|p| p.matcher.engine == engine && p.matcher.game == game)
        .collect();
    if matches.is_empty() {
        return Err(PackageError::Missing);
    }
    if matches.len() > 1 {
        let mut names: Vec<_> = matches.iter().map(|p| p.id.clone()).collect();
        names.sort();
        return Err(PackageError::Ambiguous(names));
    }
    let package = matches.pop().unwrap();
    let root = addon_root
        .canonicalize()
        .map_err(|_| PackageError::InvalidPath)?;
    let package_root = root
        .join("game-packages")
        .canonicalize()
        .map_err(|_| PackageError::InvalidPath)?;
    if !package_root.starts_with(&root) || package_root == root || !package_root.is_dir() {
        return Err(PackageError::InvalidPath);
    }
    let bootstrap_path = artifact_path(&root, &package_root, &package.bootstrap)?;
    let gamedata_path = artifact_path(&root, &package_root, &package.gamedata)?;
    if bootstrap_path == gamedata_path {
        return Err(PackageError::InvalidManifest);
    }
    let bootstrap_bytes = fs::read(&bootstrap_path).map_err(|_| PackageError::InvalidPath)?;
    let gamedata_bytes = fs::read(&gamedata_path).map_err(|_| PackageError::InvalidPath)?;
    std::str::from_utf8(&bootstrap_bytes).map_err(|_| PackageError::InvalidBootstrap)?;
    verify(&bootstrap_bytes, &package.bootstrap.sha256)?;
    verify(&gamedata_bytes, &package.gamedata.sha256)?;
    Ok(PreparedSelection {
        id: package.id.clone(),
        gamedata_owner: package.gamedata_owner.clone(),
        bootstrap_bytes,
        gamedata_bytes,
        bootstrap_sha256: package.bootstrap.sha256.clone(),
        gamedata_sha256: package.gamedata.sha256.clone(),
        provenance: Provenance {
            engine: engine.to_owned(),
            game: game.to_owned(),
            platform: platform.to_owned(),
            manifest_path: root.join("game-packages.json"),
            bootstrap_path,
            gamedata_path,
        },
    })
}
