#!/usr/bin/env python3
"""Build one source-bound KHook runtime bundle and immutable manifest."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import secrets
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Callable

TOKEN_RE = re.compile(r"^[0-9a-f]{64}$")
REVISION_RE = re.compile(r"^[0-9a-f]{40}$")
MANIFEST_NAME = "khook-runtime-build.json"

NATIVE_SOURCES = {
    "shim": Path("build/shim/s2script.so"),
    "core": Path("target/release/libs2script_core.so"),
    "probe": Path("build/khook-probe/s2_khook_probe.so"),
}
ARTIFACT_PATHS = {
    "shim": Path("s2script/bin/linuxsteamrt64/s2script.so"),
    "core": Path("s2script/bin/linuxsteamrt64/libs2script_core.so"),
    "probe": Path("s2script/bin/linuxsteamrt64/s2_khook_probe.so"),
    "fixture": Path("s2script/plugins/khook-acceptance.s2sp"),
}


class BuildError(RuntimeError):
    pass


NativeBuilder = Callable[[Path], None]
FixtureBuilder = Callable[[Path, Path], None]


def _git(root: Path, *args: str) -> str:
    try:
        return subprocess.check_output(
            ["git", *args], cwd=root, text=True, stderr=subprocess.STDOUT
        ).strip()
    except subprocess.CalledProcessError as exc:
        raise BuildError(f"git {' '.join(args)} failed: {exc.output.strip()}") from exc


def _source_state(root: Path) -> tuple[str, str]:
    revision = _git(root, "rev-parse", "HEAD")
    if not REVISION_RE.fullmatch(revision):
        raise BuildError(f"source revision is not a full 40-hex commit: {revision!r}")
    status = _git(root, "status", "--porcelain=v1", "--untracked-files=all")
    return revision, status


def _require_clean_source(root: Path) -> str:
    revision, status = _source_state(root)
    if status:
        raise BuildError(f"source tree is dirty; commit or remove these changes first:\n{status}")
    return revision


def _require_same_source(root: Path, revision: str) -> None:
    current, status = _source_state(root)
    if current != revision or status:
        detail = status or f"HEAD moved from {revision} to {current}"
        raise BuildError(f"source tree changed during build; refusing a mixed bundle:\n{detail}")


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _default_native_builder(root: Path) -> None:
    subprocess.run(["bash", "scripts/test-khook-sniper-build.sh"], cwd=root, check=True)


def _default_fixture_builder(root: Path, fixture_stage: Path) -> None:
    subprocess.run(
        [
            "node", str(root / "packages/sdk/dist/cli.js"), "build", str(fixture_stage),
            "--packages-dir", str(root / "packages"),
        ],
        cwd=root,
        check=True,
    )


def _check_prerequisites(root: Path) -> None:
    missing = [name for name in ("git", "bash", "node") if shutil.which(name) is None]
    cli = root / "packages/sdk/dist/cli.js"
    if not cli.is_file():
        missing.append("packages/sdk/dist/cli.js (run: npm run build -w @s2script/sdk)")
    if not (root / "node_modules").is_dir():
        missing.append("node_modules (run: npm install)")
    if missing:
        raise BuildError("missing runtime-build prerequisites: " + "; ".join(missing))


def _safe_output_root(root: Path, requested: Path) -> Path:
    expected = root / "build/khook-runtime"
    requested = Path(os.path.abspath(requested))
    if requested.name != "khook-runtime" or requested.parent.resolve() != root / "build":
        raise BuildError(f"runtime-build output must be {expected}")
    if requested.is_symlink():
        raise BuildError(f"unsafe runtime-build output is a symlink: {requested}")
    return expected


def _require_ignored_output(root: Path) -> None:
    ignored = subprocess.run(
        ["git", "check-ignore", "-q", "--", "build/khook-runtime"], cwd=root
    )
    if ignored.returncode != 0:
        raise BuildError("runtime-build output is not ignored by git: build/khook-runtime")


def _copy_tracked_fixture(root: Path, fixture_stage: Path) -> None:
    prefix = "examples/khook-acceptance/"
    files = _git(root, "ls-files", "--", prefix).splitlines()
    if not files:
        raise BuildError("tracked KHook acceptance fixture is missing")
    for rel_text in files:
        rel = Path(rel_text)
        if not rel_text.startswith(prefix):
            raise BuildError(f"unexpected fixture source path: {rel_text}")
        source = root / rel
        target = fixture_stage / rel.relative_to(prefix)
        if not source.is_file():
            raise BuildError(f"tracked fixture source is not a regular file: {rel_text}")
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)


def _write_embedded_identity(path: Path, revision: str, token: str) -> None:
    path.write_text(
        "// Generated only in the ignored runtime-build stage.\n"
        f'export const KHOOK_FIXTURE_REVISION = "{revision}";\n'
        f'export const KHOOK_FIXTURE_TOKEN = "{token}";\n',
        encoding="utf-8",
    )


def build_runtime(
    root: Path,
    output_root: Path,
    *,
    fixture_token: str | None = None,
    native_builder: NativeBuilder | None = None,
    fixture_builder: FixtureBuilder | None = None,
) -> Path:
    root = root.resolve()
    output_root = _safe_output_root(root, output_root)
    # Invalidate the only success receipt before any fallible prerequisite or
    # source check. Partial files without this manifest cannot be accepted.
    (output_root / MANIFEST_NAME).unlink(missing_ok=True)
    token = fixture_token or secrets.token_hex(32)
    if not TOKEN_RE.fullmatch(token):
        raise BuildError("fixture token must be exactly 64 lowercase hexadecimal characters")
    if native_builder is None or fixture_builder is None:
        _check_prerequisites(root)
    _require_ignored_output(root)
    revision = _require_clean_source(root)

    # A successful receipt can only name files produced during this invocation.
    shutil.rmtree(output_root, ignore_errors=True)
    for rel in NATIVE_SOURCES.values():
        (root / rel).unlink(missing_ok=True)

    native = native_builder or _default_native_builder
    fixture = fixture_builder or _default_fixture_builder
    native(root)
    _require_same_source(root, revision)
    for name, rel in NATIVE_SOURCES.items():
        if not (root / rel).is_file():
            raise BuildError(f"fresh {name} artifact missing after native build: {rel.as_posix()}")

    fixture_stage = output_root / "fixture-src"
    _copy_tracked_fixture(root, fixture_stage)
    _write_embedded_identity(fixture_stage / "src/build_identity.ts", revision, token)
    fixture(root, fixture_stage)
    _require_same_source(root, revision)

    built_fixtures = sorted((fixture_stage / "dist").glob("*.s2sp"))
    if len(built_fixtures) != 1:
        raise BuildError(
            f"fresh fixture build produced {len(built_fixtures)} .s2sp archives; expected exactly one"
        )

    addons = output_root / "addons"
    source_paths = {**{name: root / rel for name, rel in NATIVE_SOURCES.items()}, "fixture": built_fixtures[0]}
    artifacts: dict[str, dict[str, str]] = {}
    for name in ("shim", "core", "probe", "fixture"):
        source = source_paths[name]
        target_rel = ARTIFACT_PATHS[name]
        target = addons / target_rel
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        artifacts[name] = {"path": target_rel.as_posix(), "sha256": _sha256(target)}

    _require_same_source(root, revision)
    manifest = {
        "schema": 1,
        "kind": "khook-runtime-build",
        "source_revision": revision,
        "s2script_commit": revision,
        "fixture_revision": revision,
        "fixture_token": token,
        "artifacts": artifacts,
    }
    manifest_path = output_root / MANIFEST_NAME
    temp_path = output_root / f"{MANIFEST_NAME}.tmp"
    temp_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    os.replace(temp_path, manifest_path)
    return manifest_path


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--fixture-token",
        help="explicit 64-hex token for reproducible tests; omitted builds use a random token",
    )
    args = parser.parse_args(argv)
    root = Path(__file__).resolve().parents[1]
    try:
        manifest = build_runtime(
            root, root / "build/khook-runtime", fixture_token=args.fixture_token
        )
    except (BuildError, subprocess.CalledProcessError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    print(manifest)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
