#!/usr/bin/env python3
"""Verify a Metamod tree against an independently supplied build manifest.

Accepts stock official-release provenance or an optional unmodified-source build.
Release preparation checks an operator-supplied archive checksum before extracting
and measuring its contents. It does not claim that upstream signs that checksum.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path, PurePosixPath
from typing import Any
from urllib.parse import urlsplit

EXPECTED_SCHEMA = 2
EXPECTED_PLAPI = 18
EXPECTED_METAMOD = "fa6f80e4662e5b96cc2e97722d812f374581dfd8"
EXPECTED_KHOOK = "40d233d160b5bf60cc3e732939142b222fbd8ece"
EXPECTED_TARGET = "linux-x86_64"
EXPECTED_GLIBC_MAX = "2.31"
REQUIRED_LOADER = (
    "bin/linuxsteamrt64/metamod.2.cs2.so",
    "bin/linuxsteamrt64/libserver.so",
)
REQUIRED_KEYS = (
    "schema",
    "plapi",
    "provenance",
    "target",
    "glibc_max",
    "artifacts",
)
HEX64 = re.compile(r"^[0-9a-f]{64}$")
GLIBC_RE = re.compile(rb"GLIBC_(\d+)\.(\d+(?:\.\d+)?)")
GLIBC_TEXT_RE = re.compile(r"GLIBC_(\d+)\.(\d+(?:\.\d+)?)")
BOUNDED_READ = 8 * 1024 * 1024
ELF_MAGIC = b"\x7fELF"
ELFCLASS64 = 2
ELFDATA2LSB = 1
ET_DYN = 3
EM_X86_64 = 62
ELF64_HEADER_SIZE = 64

def fail(reason: str, detail: str) -> None:
    sys.stderr.write(f"error: {reason}: {detail}\n")
    raise SystemExit(1)


def parse_version(value: str) -> tuple[int, ...]:
    try:
        parts = tuple(int(p) for p in value.split("."))
    except ValueError:
        fail("malformed_manifest", f"invalid version {value!r}")
    if not parts:
        fail("malformed_manifest", f"invalid version {value!r}")
    return parts


def version_le(a: tuple[int, ...], b: tuple[int, ...]) -> bool:
    n = max(len(a), len(b))
    a = a + (0,) * (n - len(a))
    b = b + (0,) * (n - len(b))
    return a <= b


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def official_release_url(value: Any) -> str:
    if not isinstance(value, str):
        fail("invalid_release_url", "official release URL must be a string")
    parsed = urlsplit(value)
    component = r"[A-Za-z0-9._+-]+"
    asset = rf"mmsource-{component}-linux\.tar\.gz"
    official_path = (
        parsed.netloc == "mms.alliedmods.net"
        and re.fullmatch(rf"/mmsdrop/{component}/{asset}", parsed.path)
    ) or (
        parsed.netloc == "github.com"
        and re.fullmatch(rf"/alliedmodders/metamod-source/releases/download/{component}/{asset}", parsed.path)
    )
    if (parsed.scheme != "https" or not official_path
            or parsed.query or parsed.fragment or ".." in PurePosixPath(parsed.path).parts):
        fail("invalid_release_url", "use an explicit AlliedModders GitHub release asset or legacy mmsdrop Linux archive URL")
    return value


def validate_provenance(value: Any) -> None:
    if not isinstance(value, dict):
        fail("malformed_manifest", "provenance must be an object")
    if value.get("kind") == "unmodified-source":
        if set(value) != {"kind", "metamod_commit", "khook_commit"}:
            fail("malformed_manifest", "invalid unmodified-source provenance fields")
        for key, expected in (("metamod_commit", EXPECTED_METAMOD), ("khook_commit", EXPECTED_KHOOK)):
            if value[key] != expected:
                fail("unexpected_" + key, f"{value[key]!r} != {expected}")
    elif value.get("kind") == "official-release":
        if set(value) != {"kind", "url", "archive_sha256"}:
            fail("malformed_manifest", "invalid official-release provenance fields")
        official_release_url(value["url"])
        if not isinstance(value["archive_sha256"], str) or not HEX64.fullmatch(value["archive_sha256"]):
            fail("malformed_manifest", "archive_sha256 must be 64 lowercase hex characters")
    else:
        fail("unexpected_provenance", "expected official-release or unmodified-source")


def load_manifest(path: Path) -> dict[str, Any]:
    if not path.is_file():
        fail("missing_manifest", f"{path} does not exist")
    raw = path.read_bytes()
    if not raw.strip():
        fail("empty_manifest", f"{path} is empty")
    try:
        doc = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail("malformed_manifest", str(exc))
    if not isinstance(doc, dict):
        fail("malformed_manifest", "manifest root must be an object")
    extra = set(doc) - set(REQUIRED_KEYS)
    missing = set(REQUIRED_KEYS) - set(doc)
    if extra or missing:
        fail(
            "malformed_manifest",
            f"strict keys failed extra={sorted(extra)} missing={sorted(missing)}",
        )
    if type(doc["schema"]) is not int or doc["schema"] != EXPECTED_SCHEMA:
        fail("unexpected_schema", f"{doc['schema']!r} != {EXPECTED_SCHEMA}")
    if not isinstance(doc["plapi"], int) or isinstance(doc["plapi"], bool):
        fail("malformed_manifest", "plapi must be an integer")
    if doc["plapi"] != EXPECTED_PLAPI:
        fail("unexpected_plapi", f"{doc['plapi']!r} != {EXPECTED_PLAPI}")
    for key, expected, reason in (
        ("target", EXPECTED_TARGET, "unexpected_target"),
        ("glibc_max", EXPECTED_GLIBC_MAX, "unexpected_glibc_max"),
    ):
        value = doc[key]
        if not isinstance(value, str):
            fail("malformed_manifest", f"{key} must be a string")
        if value != expected:
            fail(reason, f"{value} != {expected}")
    validate_provenance(doc["provenance"])
    artifacts = doc["artifacts"]
    if not isinstance(artifacts, list) or not artifacts:
        fail("malformed_manifest", "artifacts must be a nonempty list")
    return doc


def resolve_in_tree(tree: Path, rel: str) -> Path:
    if not isinstance(rel, str) or not rel or rel.startswith("/"):
        fail("path_escape", f"artifact path must be a relative path: {rel!r}")
    p = Path(rel)
    if p.is_absolute() or ".." in p.parts:
        fail("path_escape", f"artifact path escapes the tree: {rel}")
    tree_r = tree.resolve()
    full = (tree / rel).resolve()
    try:
        full.relative_to(tree_r)
    except ValueError:
        fail("path_escape", f"artifact path escapes the tree: {rel}")
    return full


def glibc_versions_from_bytes(data: bytes) -> set[tuple[int, ...]]:
    found: set[tuple[int, ...]] = set()
    for match in GLIBC_RE.finditer(data):
        found.add(parse_version(f"{match.group(1).decode()}.{match.group(2).decode()}"))
    return found


def glibc_versions_from_readelf(path: Path) -> set[tuple[int, ...]]:
    found: set[tuple[int, ...]] = set()
    for args in (["-W", "-s", str(path)], ["-W", "-V", str(path)]):
        proc = subprocess.run(
            ["readelf", *args],
            capture_output=True,
            text=True,
        )
        text = (proc.stdout or "") + (proc.stderr or "")
        for match in GLIBC_TEXT_RE.finditer(text):
            found.add(parse_version(f"{match.group(1)}.{match.group(2)}"))
    return found


def check_elf(path: Path, rel: str) -> None:
    size = path.stat().st_size
    if size == 0:
        fail("empty_artifact", f"{rel} is zero bytes")
    header = path.read_bytes()[: max(ELF64_HEADER_SIZE, 16)]
    if len(header) < 16 or not header.startswith(ELF_MAGIC):
        fail("not_elf", f"{rel} is not an ELF object")
    elfclass = header[4]
    data = header[5]
    if elfclass != ELFCLASS64:
        fail("wrong_architecture", f"{rel} is not ELF64")
    if data != ELFDATA2LSB:
        fail("wrong_architecture", f"{rel} is not ELF little-endian")
    if size < ELF64_HEADER_SIZE or len(header) < ELF64_HEADER_SIZE:
        fail("truncated_elf", f"{rel} is shorter than an ELF64 header")
    e_type, e_machine = struct.unpack_from("<HH", header, 16)
    if e_machine != EM_X86_64:
        fail("wrong_architecture", f"{rel} e_machine={e_machine} is not x86_64")
    if e_type != ET_DYN:
        fail("not_shared_object", f"{rel} e_type={e_type} is not ET_DYN")

    if shutil.which("readelf") is None:
        fail("readelf_unavailable", "readelf not found (need binutils to validate ELF)")

    proc = subprocess.run(["readelf", "-h", str(path)], capture_output=True, text=True)
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout or "readelf -h failed").strip()
        if size < ELF64_HEADER_SIZE or "Failed to read" in detail or "truncated" in detail.lower():
            fail("truncated_elf", f"{rel}: {detail}")
        fail("malformed_elf", f"{rel}: {detail}")
    text = proc.stdout
    if "ELF64" not in text:
        fail("wrong_architecture", f"{rel} readelf did not report ELF64")
    if "little endian" not in text.lower() and "2's complement, little endian" not in text:
        fail("wrong_architecture", f"{rel} readelf did not report little endian")
    if "X86-64" not in text and "x86-64" not in text and "X86_64" not in text:
        fail("wrong_architecture", f"{rel} readelf did not report x86_64")
    if "DYN" not in text and "Shared object" not in text:
        fail("not_shared_object", f"{rel} readelf did not report a shared object")

    bounded = path.read_bytes()[:BOUNDED_READ]
    versions = glibc_versions_from_bytes(bounded) | glibc_versions_from_readelf(path)
    limit = parse_version(EXPECTED_GLIBC_MAX)
    for ver in versions:
        if not version_le(ver, limit):
            pretty = ".".join(str(x) for x in ver)
            fail("glibc_too_new", f"{rel} requires GLIBC_{pretty} > {EXPECTED_GLIBC_MAX}")


def verify(tree: Path, manifest_path: Path) -> None:
    if not tree.is_dir():
        fail("missing_tree", f"{tree} is not a directory")
    doc = load_manifest(manifest_path)

    listed: list[str] = []
    seen: set[str] = set()
    for item in doc["artifacts"]:
        if not isinstance(item, dict):
            fail("malformed_manifest", "artifact entry must be an object")
        extra = set(item) - {"path", "sha256"}
        if extra or "path" not in item or "sha256" not in item:
            fail("malformed_manifest", f"artifact entry must have only path and sha256: {item!r}")
        rel = item["path"]
        digest = item["sha256"]
        if not isinstance(rel, str) or not isinstance(digest, str):
            fail("malformed_manifest", "artifact path and sha256 must be strings")
        if not HEX64.fullmatch(digest):
            fail("malformed_manifest", f"artifact sha256 is not 64 lowercase hex: {digest}")
        if rel in seen:
            fail("malformed_manifest", f"duplicate artifact path {rel}")
        seen.add(rel)
        listed.append(rel)
        full = resolve_in_tree(tree, rel)
        if not full.is_file():
            fail("artifact_missing", f"{rel} is not a file in the candidate tree")
        if full.stat().st_size == 0:
            fail("empty_artifact", f"{rel} is zero bytes")
        got = sha256_file(full)
        if got != digest:
            fail("hash_mismatch", f"{rel} sha256={got} != manifest {digest}")
        if rel.endswith(".so"):
            check_elf(full, rel)

    missing_required = [rel for rel in REQUIRED_LOADER if rel not in seen]
    if missing_required:
        fail(
            "required_loader_missing",
            "manifest artifacts omit required loader files: " + ", ".join(missing_required),
        )


def extract_official_archive(archive_path: Path, expected_sha: str, url: str, destination: Path) -> None:
    """Extract regular files from a checksum-verified stock release into a fresh directory."""
    official_release_url(url)
    if not HEX64.fullmatch(expected_sha):
        fail("invalid_archive_hash", "expected archive SHA256 must be supplied independently")
    if not archive_path.is_file():
        fail("missing_archive", str(archive_path))
    if sha256_file(archive_path) != expected_sha:
        fail("archive_hash_mismatch", "official archive differs from the supplied expected SHA256")
    if destination.exists():
        fail("existing_extract_tree", "archive extraction needs a fresh destination")
    try:
        with tarfile.open(archive_path, "r:gz") as archive:
            selected = []
            seen = set()
            for member in archive.getmembers():
                path = PurePosixPath(member.name)
                if path.is_absolute() or ".." in path.parts or not (member.isfile() or member.isdir()):
                    fail("unsafe_archive", f"only regular files and directories with safe paths are accepted: {member.name}")
                if path in seen:
                    fail("unsafe_archive", f"duplicate archive path: {member.name}")
                seen.add(path)
                if path.parts[:2] == ("addons", "metamod") and len(path.parts) > 2 and member.isfile():
                    selected.append((member, Path(*path.parts[2:])))
            if not selected:
                fail("missing_archive_tree", "archive has no addons/metamod files")
            destination.mkdir(parents=True)
            for member, relative in selected:
                output = destination / relative
                output.parent.mkdir(parents=True, exist_ok=True)
                with archive.extractfile(member) as source, output.open("xb") as target:
                    shutil.copyfileobj(source, target)
                output.chmod(member.mode & 0o755)
    except (tarfile.TarError, OSError) as exc:
        fail("unsafe_archive", str(exc))


def prepare_release(archive: Path, expected_sha: str, url: str, plapi: int | None,
                    tree: Path, manifest_path: Path) -> None:
    # A failed attempt must invalidate a previous success, even before extraction.
    manifest_path.unlink(missing_ok=True)
    if plapi != EXPECTED_PLAPI:
        fail("unconfirmed_release_plapi", "operator must confirm --release-plapi 18 for the selected release")
    tree.parent.mkdir(parents=True, exist_ok=True)
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="metamod-release-", dir=tree.parent) as directory:
        candidate = Path(directory) / "tree"
        extract_official_archive(archive, expected_sha, url, candidate)
        items = []
        for relative in (*REQUIRED_LOADER, "metaplugins.ini", "README.txt"):
            path = candidate / relative
            if path.is_file():
                items.append({"path": relative, "sha256": sha256_file(path)})
        doc = {"schema": EXPECTED_SCHEMA, "plapi": plapi, "target": EXPECTED_TARGET,
               "glibc_max": EXPECTED_GLIBC_MAX, "artifacts": items,
               "provenance": {"kind": "official-release", "url": url, "archive_sha256": expected_sha}}
        candidate_manifest = Path(directory) / "manifest.json"
        candidate_manifest.write_text(json.dumps(doc, indent=2) + "\n")
        verify(candidate, candidate_manifest)
        if tree.exists():
            shutil.rmtree(tree)
        shutil.move(str(candidate), str(tree))
        shutil.copyfile(candidate_manifest, manifest_path)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Verify a Metamod artifact tree against an independent build manifest.")
    parser.add_argument("--tree", required=True, help="candidate Metamod tree")
    parser.add_argument("--manifest", required=True, help="independent metamod-build.json (never invented from the tree)")
    parser.add_argument("--prepare-release", help="prepare stock release archive using independently supplied checksum")
    parser.add_argument("--archive-sha256", default="", help="operator-supplied expected archive SHA256")
    parser.add_argument("--release-url", default="", help="explicit official release archive URL")
    parser.add_argument("--release-plapi", type=int, help="operator-confirmed plugin API version of the release")
    args = parser.parse_args(argv)
    if args.prepare_release:
        prepare_release(Path(args.prepare_release), args.archive_sha256, args.release_url,
                        args.release_plapi, Path(args.tree), Path(args.manifest))
    else:
        verify(Path(args.tree), Path(args.manifest))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv[1:]))
    except BrokenPipeError:
        raise SystemExit(1)
