#!/usr/bin/env python3
"""Exercise source preparation inside a parent Git checkout, without a compiler."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "third_party/metamod-source"


class PreparationTests(unittest.TestCase):
    def setUp(self):
        (ROOT / "build").mkdir(exist_ok=True)
        self.tmp = tempfile.TemporaryDirectory(prefix="metamod-prepare-test-", dir=ROOT / "build")
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        (self.repo / "third_party").mkdir()
        (self.repo / "third_party/metamod-source").symlink_to(SOURCE, target_is_directory=True)
        (self.repo / "patches").symlink_to(ROOT / "patches", target_is_directory=True)
        self.output = self.repo / "build/metamod-pinned"

    def run_build(self):
        return subprocess.run(["bash", str(ROOT / "scripts/build-metamod-pinned.sh"), "--prepare-only"],
                              env={**os.environ, "S2_REPO": str(self.repo)}, text=True, capture_output=True)

    def test_prepared_source_contains_applied_patch_not_parent_checkout(self):
        before = subprocess.check_output(["git", "-C", str(SOURCE), "status", "--porcelain"])
        result = self.run_build()
        prepared = self.output / "source"
        # Independent git-apply outside every repository is the expected filesystem effect.
        with tempfile.TemporaryDirectory(prefix="metamod-expected-") as temp:
            expected = Path(temp) / "source"
            shutil.copytree(SOURCE, expected, ignore=shutil.ignore_patterns(".git"))
            series = ROOT / "patches/metamod-source"
            for entry in (series / "series").read_text().splitlines():
                entry = entry.strip()
                if entry and not entry.startswith("#"):
                    subprocess.run(["git", "apply", str(series / entry)], cwd=expected, check=True)
            expected_files = {p.relative_to(expected): p.read_bytes() for p in expected.rglob("*") if p.is_file()}
            prepared_files = {p.relative_to(prepared): p.read_bytes() for p in prepared.rglob("*")
                              if p.is_file() and ".git" not in p.relative_to(prepared).parts}
            self.assertEqual(expected_files.keys(), prepared_files.keys())
            for path, content in expected_files.items():
                self.assertEqual(content, prepared_files[path], f"patch not applied to {path}")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(before, subprocess.check_output(["git", "-C", str(SOURCE), "status", "--porcelain"]))
        self.assertFalse((self.output / "metamod-build.json").exists())

    def test_failed_rebuild_invalidates_previous_success_manifest(self):
        self.output.mkdir(parents=True)
        manifest = self.output / "metamod-build.json"
        manifest.write_text('{"stale":true}\n')
        (self.repo / "third_party/metamod-source").unlink()
        result = self.run_build()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(manifest.exists(), "failed rebuild left a usable success manifest")


if __name__ == "__main__":
    unittest.main()
