#!/usr/bin/env python3
"""Regression tests for the KHook runtime bundle builder."""
from __future__ import annotations

import hashlib
import importlib.util
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "build_khook_runtime", ROOT / "scripts" / "build-khook-runtime.py"
)
builder = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(builder)

TOKEN = "a" * 64
ARTIFACT_SOURCES = {
    "shim": Path("build/shim/s2script.so"),
    "core": Path("target/release/libs2script_core.so"),
    "probe": Path("build/khook-probe/s2_khook_probe.so"),
}
EXPECTED_PATHS = {
    "shim": "s2script/bin/linuxsteamrt64/s2script.so",
    "core": "s2script/bin/linuxsteamrt64/libs2script_core.so",
    "probe": "s2script/bin/linuxsteamrt64/s2_khook_probe.so",
    "fixture": "s2script/plugins/khook-acceptance.s2sp",
}


def run(*args: str, cwd: Path) -> str:
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def init_repo(path: Path) -> str:
    fixture = path / "examples/khook-acceptance"
    (fixture / "src").mkdir(parents=True)
    (fixture / "package.json").write_text('{"name":"@example/khook-acceptance"}\n')
    (fixture / "tsconfig.json").write_text("{}\n")
    (fixture / "src/plugin.ts").write_text("export function OnPluginStart() {}\n")
    (fixture / "src/build_identity.ts").write_text(
        'export const KHOOK_FIXTURE_REVISION = "unknown";\n'
        'export const KHOOK_FIXTURE_TOKEN = "unknown";\n'
    )
    (path / ".gitignore").write_text("/build\n/target\n")
    run("git", "init", "-q", cwd=path)
    run("git", "config", "user.email", "test@example.invalid", cwd=path)
    run("git", "config", "user.name", "Runtime Builder Test", cwd=path)
    run("git", "add", ".", cwd=path)
    run("git", "commit", "-qm", "fixture", cwd=path)
    return run("git", "rev-parse", "HEAD", cwd=path)


def write_native(root: Path, *, omit: str | None = None) -> None:
    for name, rel in ARTIFACT_SOURCES.items():
        if name == omit:
            continue
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes((name + "-fresh").encode())


def write_fixture(_root: Path, fixture_stage: Path) -> None:
    identity = (fixture_stage / "src/build_identity.ts").read_text()
    if TOKEN not in identity:
        raise AssertionError("fixture builder did not receive the requested embedded token")
    out = fixture_stage / "dist"
    out.mkdir()
    (out / "_example_khook-acceptance.s2sp").write_bytes(b"fixture-fresh")


class RuntimeBuildTests(unittest.TestCase):
    def test_builds_one_fresh_bundle_and_exact_manifest(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            revision = init_repo(root)
            output = root / "build/khook-runtime"
            output.mkdir(parents=True)
            (output / "stale").write_text("must disappear")
            for rel in ARTIFACT_SOURCES.values():
                path = root / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"stale")

            manifest_path = builder.build_runtime(
                root, output, fixture_token=TOKEN,
                native_builder=lambda repo: write_native(repo),
                fixture_builder=write_fixture,
            )

            self.assertFalse((output / "stale").exists())
            manifest = json.loads(manifest_path.read_text())
            self.assertEqual(set(manifest), {
                "schema", "kind", "source_revision", "s2script_commit",
                "fixture_revision", "fixture_token", "artifacts",
            })
            self.assertEqual(manifest["schema"], 1)
            self.assertEqual(manifest["kind"], "khook-runtime-build")
            self.assertEqual(manifest["source_revision"], revision)
            self.assertEqual(manifest["s2script_commit"], revision)
            self.assertEqual(manifest["fixture_revision"], revision)
            self.assertEqual(manifest["fixture_token"], TOKEN)
            self.assertEqual(set(manifest["artifacts"]), set(EXPECTED_PATHS))
            for name, rel in EXPECTED_PATHS.items():
                item = manifest["artifacts"][name]
                self.assertEqual(item["path"], rel)
                data = (output / "addons" / rel).read_bytes()
                self.assertEqual(item["sha256"], hashlib.sha256(data).hexdigest())
            embedded = (output / "fixture-src/src/build_identity.ts").read_text()
            self.assertIn(revision, embedded)
            self.assertIn(TOKEN, embedded)

    def test_dirty_tracked_or_untracked_source_is_rejected_before_build(self):
        for dirty in ("tracked", "untracked"):
            with self.subTest(dirty=dirty), tempfile.TemporaryDirectory() as td:
                root = Path(td)
                init_repo(root)
                if dirty == "tracked":
                    (root / "examples/khook-acceptance/src/plugin.ts").write_text("dirty\n")
                else:
                    (root / "unexpected.txt").write_text("dirty\n")
                output = root / "build/khook-runtime"
                output.mkdir(parents=True)
                manifest = output / "khook-runtime-build.json"
                manifest.write_text("stale success\n")
                called = []
                with self.assertRaisesRegex(builder.BuildError, "source tree is dirty"):
                    builder.build_runtime(
                        root, output, fixture_token=TOKEN,
                        native_builder=lambda _repo: called.append(True),
                        fixture_builder=write_fixture,
                    )
                self.assertEqual(called, [])
                self.assertFalse(manifest.exists())

    def test_missing_prerequisites_remove_an_existing_success_manifest(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            init_repo(root)
            output = root / "build/khook-runtime"
            output.mkdir(parents=True)
            manifest = output / "khook-runtime-build.json"
            manifest.write_text("stale success\n")
            with self.assertRaisesRegex(builder.BuildError, "missing runtime-build prerequisites"):
                builder.build_runtime(root, output, fixture_token=TOKEN)
            self.assertFalse(manifest.exists())

    def test_output_stage_must_be_the_real_ignored_build_location(self):
        with tempfile.TemporaryDirectory() as td, tempfile.TemporaryDirectory() as outside_td:
            root = Path(td)
            init_repo(root)
            outside = Path(outside_td)
            sentinel = outside / "keep"
            sentinel.write_text("preserve\n")
            (root / "build").mkdir()
            (root / "build/khook-runtime").symlink_to(outside, target_is_directory=True)
            with self.assertRaisesRegex(builder.BuildError, "unsafe runtime-build output"):
                builder.build_runtime(
                    root, root / "build/khook-runtime", fixture_token=TOKEN,
                    native_builder=lambda repo: write_native(repo), fixture_builder=write_fixture,
                )
            self.assertEqual(sentinel.read_text(), "preserve\n")

            with self.assertRaisesRegex(builder.BuildError, "must be"):
                builder.build_runtime(
                    root, root / "somewhere-else", fixture_token=TOKEN,
                    native_builder=lambda repo: write_native(repo), fixture_builder=write_fixture,
                )

    def test_missing_fresh_artifact_cannot_reuse_a_stale_binary(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            init_repo(root)
            stale = root / ARTIFACT_SOURCES["core"]
            stale.parent.mkdir(parents=True)
            stale.write_bytes(b"stale-core")
            with self.assertRaisesRegex(builder.BuildError, "fresh core artifact missing"):
                builder.build_runtime(
                    root, root / "build/khook-runtime", fixture_token=TOKEN,
                    native_builder=lambda repo: write_native(repo, omit="core"),
                    fixture_builder=write_fixture,
                )
            self.assertFalse((root / "build/khook-runtime/khook-runtime-build.json").exists())

    def test_source_change_during_build_refuses_a_mixed_bundle(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            init_repo(root)
            fixture_called = []

            def mutating_native(repo: Path) -> None:
                write_native(repo)
                (repo / "examples/khook-acceptance/src/plugin.ts").write_text("changed\n")

            with self.assertRaisesRegex(builder.BuildError, "source tree changed during build"):
                builder.build_runtime(
                    root, root / "build/khook-runtime", fixture_token=TOKEN,
                    native_builder=mutating_native,
                    fixture_builder=lambda *_args: fixture_called.append(True),
                )
            self.assertEqual(fixture_called, [])

    def test_failed_builder_leaves_no_success_manifest(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            init_repo(root)
            output = root / "build/khook-runtime"
            output.mkdir(parents=True)
            (output / "khook-runtime-build.json").write_text("stale success\n")

            def fail(_repo: Path) -> None:
                raise subprocess.CalledProcessError(9, ["native-build"])

            with self.assertRaises(subprocess.CalledProcessError):
                builder.build_runtime(
                    root, output, fixture_token=TOKEN,
                    native_builder=fail, fixture_builder=write_fixture,
                )
            self.assertFalse((output / "khook-runtime-build.json").exists())

    def test_fixture_token_must_be_exact_lowercase_sha256_shape(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            init_repo(root)
            for token in ("a" * 63, "A" * 64, "g" * 64):
                with self.subTest(token=token), self.assertRaisesRegex(builder.BuildError, "fixture token"):
                    builder.build_runtime(
                        root, root / "build/khook-runtime", fixture_token=token,
                        native_builder=lambda repo: write_native(repo),
                        fixture_builder=write_fixture,
                    )


if __name__ == "__main__":
    unittest.main()
