#!/usr/bin/env python3
"""Exercise source preparation inside a parent Git checkout, without a compiler."""
import os
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import tarfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "third_party/metamod-source"
SPEC = importlib.util.spec_from_file_location("metamod_verifier", ROOT / "scripts/verify-metamod-artifact.py")
VERIFIER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFIER)
RELEASE_URL = "https://mms.alliedmods.net/mmsdrop/2.0/mmsource-test-linux.tar.gz"
GITHUB_RELEASE_URL = "https://github.com/alliedmodders/metamod-source/releases/download/2.0.0.1466/mmsource-2.0.0-git1466-linux.tar.gz"
DEFAULT_RELEASE_URL = "https://github.com/alliedmodders/metamod-source/releases/download/2.0.0.1469/mmsource-2.0.0-git1469-linux.tar.gz"


def manifest(provenance):
    return {"schema": 2, "plapi": 18, "target": "linux-x86_64", "glibc_max": "2.31",
            "provenance": provenance, "artifacts": [{"path": "unused.so", "sha256": "0" * 64}]}


class StockManifestTests(unittest.TestCase):
    def load(self, value):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "manifest.json"
            path.write_text(json.dumps(value))
            return VERIFIER.load_manifest(path)

    def test_unmodified_source_manifest_needs_no_patch_digest(self):
        value = manifest({"kind": "unmodified-source", "metamod_commit": VERIFIER.EXPECTED_METAMOD,
                          "khook_commit": VERIFIER.EXPECTED_KHOOK})
        try:
            actual = self.load(value)
        except SystemExit:
            self.fail("unmodified-source schema 2 manifest was rejected")
        self.assertEqual(actual, value)

    def test_official_release_manifest_needs_no_private_source_pin(self):
        value = manifest({"kind": "official-release", "url": RELEASE_URL, "archive_sha256": "a" * 64})
        try:
            actual = self.load(value)
        except SystemExit:
            self.fail("official release schema 2 manifest was rejected")
        self.assertEqual(actual, value)

    def test_official_github_release_asset_is_accepted(self):
        value = manifest({"kind": "official-release", "url": GITHUB_RELEASE_URL, "archive_sha256": "a" * 64})
        try:
            actual = self.load(value)
        except SystemExit:
            self.fail("official AlliedModders GitHub release asset was rejected")
        self.assertEqual(actual, value)

    def test_obsolete_patch_manifest_is_rejected(self):
        value = manifest({"kind": "unmodified-source", "metamod_commit": VERIFIER.EXPECTED_METAMOD,
                          "khook_commit": VERIFIER.EXPECTED_KHOOK})
        value["schema"] = 1
        value["patchset_sha256"] = "a" * 64
        with self.assertRaises(SystemExit):
            self.load(value)

    def test_official_origin_cannot_be_a_lookalike_or_insecure_url(self):
        for url in (RELEASE_URL.replace("https:", "http:"),
                    RELEASE_URL.replace("mms.alliedmods.net", "mms.alliedmods.net.example.org"),
                    GITHUB_RELEASE_URL.replace("github.com", "github.com.example.org"),
                    GITHUB_RELEASE_URL.replace("alliedmodders/", "alliedmodders-mirror/"),
                    GITHUB_RELEASE_URL.replace("metamod-source/", "metamod-source-mirror/"),
                    "https://github.com/alliedmodders/metamod-source/releases/latest"):
            with self.subTest(url=url), self.assertRaises(SystemExit):
                self.load(manifest({"kind": "official-release", "url": url, "archive_sha256": "a" * 64}))


class OfficialArchiveTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="metamod-release-test-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.archive = self.root / "release.tar.gz"
        self.tree = self.root / "tree"
        self.receipt = self.root / "manifest.json"

    def archive_with(self, name, content=b"fixture", link=None):
        with tarfile.open(self.archive, "w:gz") as archive:
            member = tarfile.TarInfo(name)
            if link is not None:
                member.type = tarfile.SYMTYPE
                member.linkname = link
                archive.addfile(member)
            else:
                member.size = len(content)
                archive.addfile(member, io.BytesIO(content))

    def prepare(self, digest=None):
        self.receipt.write_text('{"stale":true}\n')
        return subprocess.run(["python3", str(ROOT / "scripts/verify-metamod-artifact.py"),
                               "--prepare-release", str(self.archive), "--release-url", RELEASE_URL,
                               "--release-plapi", "18",
                               "--archive-sha256", digest or hashlib.sha256(self.archive.read_bytes()).hexdigest(),
                               "--tree", str(self.tree), "--manifest", str(self.receipt)],
                              capture_output=True, text=True)

    def test_wrong_archive_checksum_rejects_before_extraction_and_invalidates_receipt(self):
        self.archive_with("addons/metamod/metaplugins.ini")
        result = self.prepare("0" * 64)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("archive_hash_mismatch", result.stderr)
        self.assertFalse(self.tree.exists())
        self.assertFalse(self.receipt.exists())

    def test_archive_path_escape_is_rejected(self):
        self.archive_with("addons/metamod/../../../outside")
        result = self.prepare()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unsafe_archive", result.stderr)
        self.assertFalse((self.root / "outside").exists())
        self.assertFalse(self.receipt.exists())

    def test_archive_symlink_is_rejected(self):
        self.archive_with("addons/metamod/bin", link="../../../outside")
        result = self.prepare()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unsafe_archive", result.stderr)
        self.assertFalse(self.receipt.exists())

    def test_checksum_verified_archive_extracts_upstream_bytes(self):
        self.archive_with("addons/metamod/metaplugins.ini", b"unchanged upstream bytes\n")
        digest = hashlib.sha256(self.archive.read_bytes()).hexdigest()
        VERIFIER.extract_official_archive(self.archive, digest, RELEASE_URL, self.tree)
        self.assertEqual((self.tree / "metaplugins.ini").read_bytes(), b"unchanged upstream bytes\n")
        self.assertFalse(self.receipt.exists(), "extraction alone cannot claim a verified runtime artifact")

    def test_release_preparation_requires_explicit_plapi_confirmation(self):
        self.archive_with("addons/metamod/metaplugins.ini")
        self.receipt.write_text('{"stale":true}\n')
        with self.assertRaises(SystemExit):
            VERIFIER.prepare_release(self.archive, hashlib.sha256(self.archive.read_bytes()).hexdigest(),
                                     RELEASE_URL, None, self.tree, self.receipt)
        self.assertFalse(self.receipt.exists())
        self.assertFalse(self.tree.exists())

    def test_archive_duplicate_member_is_rejected(self):
        with tarfile.open(self.archive, "w:gz") as archive:
            for payload in (b"first", b"second"):
                member = tarfile.TarInfo("addons/metamod/metaplugins.ini")
                member.size = len(payload)
                archive.addfile(member, io.BytesIO(payload))
        result = self.prepare()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unsafe_archive", result.stderr)
        self.assertFalse(self.receipt.exists())


class InstallerDefaultTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="metamod-default-test-")
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        (self.repo / "scripts").mkdir()
        (self.repo / "scripts/verify-metamod-artifact.py").symlink_to(ROOT / "scripts/verify-metamod-artifact.py")
        self.log = self.repo / "curl.json"
        self.curl = self.repo / "curl"
        self.curl.write_text("#!/usr/bin/env python3\nimport json,os,sys\nfrom pathlib import Path\n"
                             "Path(os.environ['S2_CURL_LOG']).write_text(json.dumps(sys.argv[1:]))\nsys.exit(19)\n")
        self.curl.chmod(0o755)

    def resolve(self, **overrides):
        env = {key: value for key, value in os.environ.items() if not key.startswith("S2_METAMOD_")}
        env.update(S2_SCRIPT_REPO=str(self.repo), S2_METAMOD_CURL=str(self.curl), S2_CURL_LOG=str(self.log))
        env.update(overrides)
        return subprocess.run(["bash", "-c", 'source "$1"; s2_metamod_resolve_source', "bash",
                               str(ROOT / "scripts/cloud/install.sh")], env=env, capture_output=True, text=True)

    def test_fresh_setup_selects_exact_stock_asset_without_source_build(self):
        result = self.resolve()
        self.assertNotEqual(result.returncode, 0, "test transport intentionally refuses network")
        self.assertTrue(self.log.exists(), result.stdout + result.stderr)
        self.assertIn(DEFAULT_RELEASE_URL, json.loads(self.log.read_text()))

    def test_override_does_not_inherit_default_plapi_confirmation(self):
        result = self.resolve(S2_METAMOD_RELEASE_URL=GITHUB_RELEASE_URL, S2_METAMOD_RELEASE_SHA256="a" * 64)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing_release_identity", result.stderr)
        self.assertFalse(self.log.exists(), "unconfirmed override must not download the default or override")

    def test_complete_override_selects_operator_asset(self):
        result = self.resolve(S2_METAMOD_RELEASE_URL=GITHUB_RELEASE_URL,
                              S2_METAMOD_RELEASE_SHA256="a" * 64, S2_METAMOD_RELEASE_PLAPI="18")
        self.assertNotEqual(result.returncode, 0, "test transport intentionally refuses network")
        self.assertTrue(self.log.exists(), result.stdout + result.stderr)
        self.assertIn(GITHUB_RELEASE_URL, json.loads(self.log.read_text()))
        self.assertNotIn(DEFAULT_RELEASE_URL, json.loads(self.log.read_text()))


class PreparationTests(unittest.TestCase):
    def setUp(self):
        (ROOT / "build").mkdir(exist_ok=True)
        self.tmp = tempfile.TemporaryDirectory(prefix="metamod-prepare-test-", dir=ROOT / "build")
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name)
        (self.repo / "third_party").mkdir()
        (self.repo / "third_party/metamod-source").symlink_to(SOURCE, target_is_directory=True)
        self.output = self.repo / "build/metamod-pinned"

    def run_build(self):
        return subprocess.run(["bash", str(ROOT / "scripts/build-metamod-pinned.sh"), "--prepare-only"],
                              env={**os.environ, "S2_REPO": str(self.repo)}, text=True, capture_output=True)

    def test_prepared_source_is_unmodified_without_a_patch_directory(self):
        before = subprocess.check_output(["git", "-C", str(SOURCE), "status", "--porcelain"])
        result = self.run_build()
        prepared = self.output / "source"
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        # Every source byte, including nested dependencies, must remain upstream.
        with tempfile.TemporaryDirectory(prefix="metamod-expected-") as temp:
            expected = Path(temp) / "source"
            shutil.copytree(SOURCE, expected, ignore=shutil.ignore_patterns(".git"))
            expected_files = {p.relative_to(expected): p.read_bytes() for p in expected.rglob("*") if p.is_file()}
            prepared_files = {p.relative_to(prepared): p.read_bytes() for p in prepared.rglob("*")
                              if p.is_file() and ".git" not in p.relative_to(prepared).parts}
            self.assertEqual(expected_files.keys(), prepared_files.keys())
            for path, content in expected_files.items():
                self.assertEqual(content, prepared_files[path], f"upstream source changed: {path}")
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
