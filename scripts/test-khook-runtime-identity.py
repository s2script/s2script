#!/usr/bin/env python3
"""Regression tests for the installed KHook runtime identity generator."""
from __future__ import annotations

import copy
import hashlib
import json
import os
import stat
import sys
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import khook_runtime_identity as kri  # noqa: E402


REV = "1" * 40
TOKEN = "2" * 64
SERVER = "127.0.0.1:27015"
FIXED_TIME = datetime(2026, 9, 15, 12, 0, tzinfo=timezone.utc)
ARTIFACT_PATHS = {
    "shim": "s2script/bin/linuxsteamrt64/s2script.so",
    "core": "s2script/bin/linuxsteamrt64/libs2script_core.so",
    "probe": "s2script/bin/linuxsteamrt64/s2_khook_probe.so",
    "fixture": "s2script/plugins/khook-acceptance.s2sp",
}
HOST_PATHS = {
    "metamod": "bin/linuxsteamrt64/metamod.2.cs2.so",
    "metamod_loader": "bin/linuxsteamrt64/libserver.so",
}


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def device(st_dev: int) -> str:
    return f"{os.major(st_dev):x}:{os.minor(st_dev):x}"


def elf_fixture(label: bytes) -> bytes:
    data = bytearray(64)
    data[:4] = b"\x7fELF"
    data[4] = 2
    data[5] = 1
    data[16:18] = (3).to_bytes(2, "little")
    data[18:20] = (62).to_bytes(2, "little")
    return bytes(data) + label


class RuntimeTree:
    def __init__(self, root: Path):
        self.root = root
        self.addons = root / "addons"
        self.build_manifest = root / "khook-runtime-build.json"
        self.host_manifest = self.addons / "metamod" / ".s2script-metamod-build.json"
        self.output = root / "run" / "runtime-identity.json"
        self.evidence = root / "run" / "captures" / "runtime-identity.txt"
        self.run_dir = root / "run"
        for role, rel in ARTIFACT_PATHS.items():
            path = self.addons / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes((role + "-installed").encode())
        for role, rel in HOST_PATHS.items():
            path = self.addons / "metamod" / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(elf_fixture(role.encode()))
        artifacts = {
            role: {"path": rel, "sha256": digest(self.addons / rel)}
            for role, rel in ARTIFACT_PATHS.items()
        }
        self.build = {
            "schema": 1,
            "kind": "khook-runtime-build",
            "source_revision": REV,
            "s2script_commit": REV,
            "fixture_revision": REV,
            "fixture_token": TOKEN,
            "artifacts": artifacts,
        }
        self.build_manifest.write_text(json.dumps(self.build))
        host_artifacts = [
            {"path": rel, "sha256": digest(self.addons / "metamod" / rel)}
            for rel in HOST_PATHS.values()
        ]
        self.host = {
            "schema": 2,
            "plapi": 18,
            "provenance": {
                "kind": "unmodified-source",
                "metamod_commit": "fa6f80e4662e5b96cc2e97722d812f374581dfd8",
                "khook_commit": "40d233d160b5bf60cc3e732939142b222fbd8ece",
            },
            "target": "linux-x86_64",
            "glibc_max": "2.31",
            "artifacts": host_artifacts,
        }
        self.host_manifest.write_text(json.dumps(self.host))
        self.native = self.native_sample()
        self.fixture = {
            "schema": 1,
            "kind": "khook-fixture-runtime",
            "result": "ready",
            "fixture_revision": REV,
            "fixture_token": TOKEN,
            "generation": 7,
        }

    def module(self, path: Path) -> dict:
        resolved = path.resolve()
        info = resolved.stat()
        return {"path": str(resolved), "device": device(info.st_dev), "inode": str(info.st_ino)}

    def native_sample(self) -> dict:
        modules = {
            role: self.module(self.addons / rel)
            for role, rel in ARTIFACT_PATHS.items() if role != "fixture"
        }
        modules.update({
            role: self.module(self.addons / "metamod" / rel)
            for role, rel in HOST_PATHS.items()
        })
        return {
            "schema": 2,
            "kind": "khook-runtime",
            "result": "ready",
            "source_revision": REV,
            "process_id": 321,
            "probe_generation": "gen-abc",
            "server_build": 2000123,
            "map": "de_dust2",
            "modules": modules,
        }

    def sender(self, *, second_native=None, second_fixture=None):
        responses = iter((
            self.native,
            self.fixture,
            second_native or self.native,
            second_fixture or self.fixture,
        ))

        def send(command: str) -> str:
            value = next(responses)
            return "RCON connected.\n" + json.dumps(value) + "\n"

        return send

    def args(self, **overrides):
        values = dict(
            addons_root=self.addons,
            build_manifest_path=self.build_manifest,
            host_manifest_path=self.host_manifest,
            expected_source_revision=REV,
            server=SERVER,
            port=27015,
            run_dir=None,
            run_id="khook-a-unit",
            output_path=self.output,
            evidence_path=self.evidence,
            rcon_send=self.sender(),
            now=lambda: FIXED_TIME,
            host_verify=lambda _tree, _manifest: None,
        )
        values.update(overrides)
        return values


class RuntimeIdentityTests(unittest.TestCase):
    def test_generates_controller_valid_receipt_from_stable_installed_runtime(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            receipt, evidence = kri.generate_identity(**tree.args())
            self.assertEqual(receipt["run_id"], "khook-a-unit")
            self.assertEqual(receipt["source_revision"], REV)
            self.assertEqual(receipt["server_build"], "2000123")
            self.assertEqual(receipt["initial_map"], "de_dust2")
            self.assertEqual(receipt["artifacts"]["fixture"]["path"], str((tree.addons / ARTIFACT_PATHS["fixture"]).resolve()))
            self.assertEqual(receipt["artifacts"]["probe"]["sha256"], tree.build["artifacts"]["probe"]["sha256"])
            self.assertEqual(receipt["host_manifest_digest"], digest(tree.host_manifest))
            self.assertEqual(receipt["evidence_sha256"], hashlib.sha256(evidence).hexdigest())
            self.assertEqual(kri.ka.validate_runtime_identity(receipt, {
                "run_id": "khook-a-unit", "source_revision": REV, "server": SERVER,
            }), [])

    def test_production_host_verifier_rejects_manifest_hash_mismatch(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            fake_bin = Path(td) / "bin"
            fake_bin.mkdir()
            readelf = fake_bin / "readelf"
            readelf.write_text("#!/bin/sh\nprintf '%s\\n' 'Class: ELF64' \"Data: 2's complement, little endian\" 'Type: DYN (Shared object file)' 'Machine: Advanced Micro Devices X86-64'\n")
            readelf.chmod(readelf.stat().st_mode | stat.S_IXUSR)
            old_path = os.environ.get("PATH", "")
            os.environ["PATH"] = str(fake_bin) + os.pathsep + old_path
            try:
                kri.verify_host_artifact(tree.addons / "metamod", tree.host_manifest)
                tree.host["artifacts"][0]["sha256"] = "0" * 64
                tree.host_manifest.write_text(json.dumps(tree.host))
                with self.assertRaisesRegex(kri.IdentityError, "host artifact verification failed"):
                    kri.verify_host_artifact(tree.addons / "metamod", tree.host_manifest)
            finally:
                os.environ["PATH"] = old_path

    def test_rejects_duplicate_json_fields_from_runtime(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            duplicate = json.dumps(tree.native).replace('"schema": 2', '"schema": 2, "schema": 2', 1)
            with self.assertRaisesRegex(kri.IdentityError, "duplicate JSON field schema"):
                kri.generate_identity(**tree.args(rcon_send=lambda _command: duplicate))

    def test_rcon_failure_is_a_named_identity_error(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))

            def unavailable(_command: str) -> str:
                raise RuntimeError("socket closed")

            with self.assertRaisesRegex(kri.IdentityError, "runtime witness unavailable"):
                kri.generate_identity(**tree.args(rcon_send=unavailable))

    def test_rejects_pending_or_ambiguous_runtime_module(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            native = copy.deepcopy(tree.native)
            native["result"] = "pending"
            native["modules"]["core"] = None
            tree.native = native
            with self.assertRaisesRegex(kri.IdentityError, "native runtime witness is pending"):
                kri.generate_identity(**tree.args(rcon_send=tree.sender()))

    def test_pending_native_payload_is_structurally_validated_before_classification(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            base = copy.deepcopy(tree.native)
            base.update(result="pending", source_revision="unknown", server_build=0, map="")
            base["modules"]["core"] = None
            malformed = (
                ("modules", lambda value: value.update(modules="corrupt")),
                ("process", lambda value: value.update(process_id=-1)),
                ("generation", lambda value: value.update(probe_generation=3)),
                ("revision", lambda value: value.update(source_revision=3)),
                ("map", lambda value: value.update(map=3)),
            )
            for name, mutate in malformed:
                native = copy.deepcopy(base)
                mutate(native)
                responses = iter((native, tree.fixture))
                with self.subTest(name=name), self.assertRaises(kri.IdentityError) as caught:
                    kri._sample_runtime(lambda _command: json.dumps(next(responses)))
                self.assertNotIsInstance(caught.exception, kri.IdentityPending)
            responses = iter((base, tree.fixture))
            with self.assertRaises(kri.IdentityPending):
                kri._sample_runtime(lambda _command: json.dumps(next(responses)))

    def test_pending_fixture_payload_is_structurally_validated_before_classification(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            base = dict(tree.fixture, result="pending", fixture_revision="unknown", fixture_token="unknown")
            for field, value in (("fixture_revision", 3), ("fixture_token", 3), ("generation", 0)):
                fixture = dict(base, **{field: value})
                responses = iter((tree.native, fixture))
                with self.subTest(field=field), self.assertRaises(kri.IdentityError) as caught:
                    kri._sample_runtime(lambda _command: json.dumps(next(responses)))
                self.assertNotIsInstance(caught.exception, kri.IdentityPending)
            responses = iter((tree.native, base))
            with self.assertRaises(kri.IdentityPending):
                kri._sample_runtime(lambda _command: json.dumps(next(responses)))

    def test_invalid_peer_wins_over_pending_or_unavailable_witness(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            pending_native = copy.deepcopy(tree.native)
            pending_native.update(result="pending", source_revision="unknown", server_build=0, map="")
            pending_native["modules"]["core"] = None
            invalid_native = copy.deepcopy(pending_native)
            invalid_native["process_id"] = -1
            invalid_fixture = dict(
                tree.fixture, result="pending", fixture_revision="unknown",
                fixture_token="unknown", generation=0,
            )
            for name, sequence in (
                ("pending-first-invalid-second", (pending_native, invalid_fixture)),
                ("invalid-first-pending-second", (invalid_native, dict(tree.fixture, result="pending", fixture_revision="unknown", fixture_token="unknown"))),
                ("invalid-first-unavailable-second", (invalid_native, RuntimeError("socket closed"))),
                ("unavailable-first-invalid-second", (RuntimeError("socket closed"), invalid_fixture)),
            ):
                values = iter(sequence)

                def send(_command: str) -> str:
                    value = next(values)
                    if isinstance(value, Exception):
                        raise value
                    return json.dumps(value)

                with self.subTest(name=name), self.assertRaises(kri.IdentityError) as caught:
                    kri._sample_runtime(send)
                self.assertNotIsInstance(caught.exception, kri.IdentityPending)

    def test_malformed_pending_witness_publishes_no_files(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            invalid = copy.deepcopy(tree.native)
            invalid.update(result="pending", process_id=-1)
            tree.native = invalid
            with self.assertRaises(kri.IdentityError) as caught:
                kri.generate_and_publish(**tree.args(rcon_send=tree.sender()))
            self.assertNotIsInstance(caught.exception, kri.IdentityPending)
            self.assertFalse(tree.output.exists())
            self.assertFalse(tree.evidence.exists())

    def test_rejects_loaded_module_inode_that_differs_from_installed_file(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            native = copy.deepcopy(tree.native)
            native["modules"]["shim"]["inode"] = str(int(native["modules"]["shim"]["inode"]) + 1)
            tree.native = native
            with self.assertRaisesRegex(kri.IdentityError, "shim installed inode"):
                kri.generate_identity(**tree.args(rcon_send=tree.sender()))

    def test_rejects_wrong_installed_artifact_hash(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            (tree.addons / ARTIFACT_PATHS["fixture"]).write_bytes(b"wrong fixture")
            with self.assertRaisesRegex(kri.IdentityError, "fixture hash mismatch"):
                kri.generate_identity(**tree.args())

    def test_missing_installed_artifact_is_a_named_identity_error(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            (tree.addons / ARTIFACT_PATHS["core"]).unlink()
            with self.assertRaisesRegex(kri.IdentityError, "core installed file is unavailable"):
                kri.generate_identity(**tree.args())

    def test_rejects_stale_active_fixture_token(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            fixture = dict(tree.fixture, fixture_token="3" * 64)
            tree.fixture = fixture
            with self.assertRaisesRegex(kri.IdentityError, "fixture token"):
                kri.generate_identity(**tree.args(rcon_send=tree.sender()))

    def test_rejects_runtime_change_between_samples(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            native = copy.deepcopy(tree.native)
            native["process_id"] += 1
            with self.assertRaisesRegex(kri.IdentityError, "native runtime changed"):
                kri.generate_identity(**tree.args(rcon_send=tree.sender(second_native=native)))

    def test_rejects_fixture_replaced_after_hash_before_second_sample(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            ordinary = tree.sender()
            calls = 0

            def replace_between_samples(command: str) -> str:
                nonlocal calls
                calls += 1
                if calls == 3:
                    (tree.addons / ARTIFACT_PATHS["fixture"]).write_bytes(b"replaced after first hash")
                return ordinary(command)

            with self.assertRaisesRegex(kri.IdentityError, "fixture changed after verification"):
                kri.generate_identity(**tree.args(rcon_send=replace_between_samples))

    def test_adopts_pending_run_and_rejects_wrong_server_or_revision(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            tree.run_dir.mkdir()
            run = {"run_id": "khook-a-existing", "server": SERVER, "source_revision": REV, "identity_bound": False}
            (tree.run_dir / "run.json").write_text(json.dumps(run))
            receipt, _ = kri.generate_identity(**tree.args(run_dir=tree.run_dir, run_id=None))
            self.assertEqual(receipt["run_id"], "khook-a-existing")
            for field, value, message in (
                ("server", "127.0.0.1:27016", "wrong server"),
                ("source_revision", "4" * 40, "wrong source revision"),
            ):
                broken = dict(run, **{field: value})
                (tree.run_dir / "run.json").write_text(json.dumps(broken))
                with self.subTest(field=field), self.assertRaisesRegex(kri.IdentityError, message):
                    kri.generate_identity(**tree.args(run_dir=tree.run_dir, run_id=None))

    def test_failed_generation_has_no_output_side_effects(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            tree.build["source_revision"] = "5" * 40
            tree.build_manifest.write_text(json.dumps(tree.build))
            with self.assertRaises(kri.IdentityError):
                kri.generate_and_publish(**tree.args())
            self.assertFalse(tree.output.exists())
            self.assertFalse(tree.evidence.exists())
            self.assertFalse(tree.output.parent.exists())

    def test_publish_refuses_existing_target_without_overwriting_either_file(self):
        with tempfile.TemporaryDirectory() as td:
            tree = RuntimeTree(Path(td))
            tree.evidence.parent.mkdir(parents=True)
            tree.output.write_text("keep receipt")
            with self.assertRaisesRegex(kri.IdentityError, "already exists"):
                kri.generate_and_publish(**tree.args())
            self.assertEqual(tree.output.read_text(), "keep receipt")
            self.assertFalse(tree.evidence.exists())


if __name__ == "__main__":
    unittest.main()
