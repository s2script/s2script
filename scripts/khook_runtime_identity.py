#!/usr/bin/env python3
"""Generate an unsigned receipt for one installed KHook acceptance runtime.

This command must run in the server's filesystem namespace. It measures the
paths reported by the running probe; it never maps container paths to a checkout
or derives build provenance from the controller's Git HEAD.
"""
from __future__ import annotations

import argparse
import contextlib
import hashlib
import importlib.util
import io
import json
import os
import re
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath
from typing import Any, Callable

import khook_acceptance as ka


BUILD_KEYS = {
    "schema", "kind", "source_revision", "s2script_commit",
    "fixture_revision", "fixture_token", "artifacts",
}
BUILD_ARTIFACT_PATHS = {
    "shim": "s2script/bin/linuxsteamrt64/s2script.so",
    "core": "s2script/bin/linuxsteamrt64/libs2script_core.so",
    "probe": "s2script/bin/linuxsteamrt64/s2_khook_probe.so",
    "fixture": "s2script/plugins/khook-acceptance.s2sp",
}
HOST_MODULE_PATHS = {
    "metamod": "bin/linuxsteamrt64/metamod.2.cs2.so",
    "metamod_loader": "bin/linuxsteamrt64/libserver.so",
}
MODULE_ROLES = ("probe", "shim", "core", "metamod", "metamod_loader")
HEX40 = re.compile(r"^[0-9a-f]{40}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
DEVICE = re.compile(r"^[0-9a-f]+:[0-9a-f]+$")
INODE = re.compile(r"^[1-9][0-9]*$")


class IdentityError(ValueError):
    pass


class IdentityPending(IdentityError):
    """The live runtime cannot yet supply a complete receipt witness."""


def _strict_json(text: str, label: str) -> Any:
    def pairs(items):
        value = {}
        for key, item in items:
            if key in value:
                raise IdentityError(f"{label}: duplicate JSON field {key}")
            value[key] = item
        return value

    try:
        return json.loads(text, object_pairs_hook=pairs)
    except IdentityError:
        raise
    except (TypeError, json.JSONDecodeError) as error:
        raise IdentityError(f"{label}: malformed JSON: {error}") from error


def _read_json(path: Path, label: str) -> tuple[Any, bytes]:
    try:
        raw = path.read_bytes()
    except OSError as error:
        raise IdentityError(f"{label}: cannot read {path}: {error}") from error
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise IdentityError(f"{label}: invalid UTF-8: {error}") from error
    return _strict_json(text, label), raw


def _require_hex(value: Any, pattern: re.Pattern[str], label: str) -> str:
    if not isinstance(value, str) or pattern.fullmatch(value) is None:
        raise IdentityError(f"{label} must be lowercase hexadecimal")
    return value


def _validate_build_manifest(doc: Any) -> dict:
    if not isinstance(doc, dict) or set(doc) != BUILD_KEYS:
        got = set(doc) if isinstance(doc, dict) else set()
        raise IdentityError(
            f"build manifest strict keys failed extra={sorted(got - BUILD_KEYS)} "
            f"missing={sorted(BUILD_KEYS - got)}"
        )
    if type(doc["schema"]) is not int or doc["schema"] != 1:
        raise IdentityError("build manifest schema must be 1")
    if doc["kind"] != "khook-runtime-build":
        raise IdentityError("build manifest kind must be khook-runtime-build")
    revisions = [
        _require_hex(doc[name], HEX40, f"build manifest {name}")
        for name in ("source_revision", "s2script_commit", "fixture_revision")
    ]
    if len(set(revisions)) != 1:
        raise IdentityError("build manifest revisions must identify the same clean source")
    _require_hex(doc["fixture_token"], HEX64, "build manifest fixture_token")
    artifacts = doc["artifacts"]
    if not isinstance(artifacts, dict) or set(artifacts) != set(BUILD_ARTIFACT_PATHS):
        raise IdentityError("build manifest artifacts must be exactly shim/core/probe/fixture")
    for role, expected_path in BUILD_ARTIFACT_PATHS.items():
        row = artifacts[role]
        if not isinstance(row, dict) or set(row) != {"path", "sha256"}:
            raise IdentityError(f"build manifest {role} must contain only path and sha256")
        if row["path"] != expected_path:
            raise IdentityError(f"build manifest {role} path must be {expected_path}")
        pure = PurePosixPath(row["path"])
        if pure.is_absolute() or ".." in pure.parts:
            raise IdentityError(f"build manifest {role} path escapes addons root")
        _require_hex(row["sha256"], HEX64, f"build manifest {role} sha256")
    return doc


def _extract_witness(text: str, kind: str) -> dict:
    if not isinstance(text, str):
        raise IdentityError(f"{kind} witness response must be text")
    found = []
    for number, line in enumerate(text.splitlines(), 1):
        if "{" not in line:
            continue
        candidate = line[line.index("{"):]
        value = _strict_json(candidate, f"{kind} witness line {number}")
        if isinstance(value, dict) and value.get("kind") == kind:
            found.append(value)
    if len(found) != 1:
        raise IdentityError(f"expected exactly one {kind} witness, got {len(found)}")
    return found[0]


def _validate_native_witness(doc: Any) -> dict:
    keys = {
        "schema", "kind", "result", "source_revision", "process_id",
        "probe_generation", "server_build", "map", "modules",
    }
    if not isinstance(doc, dict) or set(doc) != keys:
        raise IdentityError("native runtime witness has wrong fields")
    if type(doc["schema"]) is not int or doc["schema"] != 2 or doc["kind"] != "khook-runtime":
        raise IdentityError("native runtime witness has wrong schema or kind")
    result = doc["result"]
    if result not in ("ready", "pending"):
        raise IdentityError("native runtime witness has invalid result")
    revision = doc["source_revision"]
    if not isinstance(revision, str) or not (
        HEX40.fullmatch(revision) or (result == "pending" and revision == "unknown")
    ):
        raise IdentityError("native source_revision is malformed")
    if type(doc["process_id"]) is not int or doc["process_id"] <= 0:
        raise IdentityError("native process_id must be a positive integer")
    if not isinstance(doc["probe_generation"], str) or not doc["probe_generation"]:
        raise IdentityError("native probe_generation is missing")
    if type(doc["server_build"]) is not int or doc["server_build"] < 0:
        raise IdentityError("native server_build must be a nonnegative integer")
    if result == "ready" and doc["server_build"] == 0:
        raise IdentityError("ready native server_build must be positive")
    if not isinstance(doc["map"], str):
        raise IdentityError("native map must be a string")
    if result == "ready" and not doc["map"]:
        raise IdentityError("ready native map is missing")
    modules = doc["modules"]
    if not isinstance(modules, dict) or set(modules) != set(MODULE_ROLES):
        raise IdentityError("native modules must be exactly probe/shim/core/metamod/metamod_loader")
    for role in MODULE_ROLES:
        row = modules[role]
        if row is None and result == "pending":
            continue
        if not isinstance(row, dict) or set(row) != {"path", "device", "inode"}:
            raise IdentityError(f"native {role} module is missing or ambiguous")
        path = row["path"]
        if not isinstance(path, str) or not Path(path).is_absolute():
            raise IdentityError(f"native {role} path must be absolute")
        if not isinstance(row["device"], str) or DEVICE.fullmatch(row["device"]) is None:
            raise IdentityError(f"native {role} device is malformed")
        if not isinstance(row["inode"], str) or INODE.fullmatch(row["inode"]) is None:
            raise IdentityError(f"native {role} inode is malformed")
    if result == "pending":
        raise IdentityPending("native runtime witness is pending")
    return doc


def _validate_fixture_witness(doc: Any) -> dict:
    keys = {"schema", "kind", "result", "fixture_revision", "fixture_token", "generation"}
    if not isinstance(doc, dict) or set(doc) != keys:
        raise IdentityError("fixture runtime witness has wrong fields")
    if type(doc["schema"]) is not int or doc["schema"] != 1 or doc["kind"] != "khook-fixture-runtime":
        raise IdentityError("fixture runtime witness has wrong schema or kind")
    result = doc["result"]
    if result not in ("ready", "pending"):
        raise IdentityError("fixture runtime witness has invalid result")
    revision = doc["fixture_revision"]
    token = doc["fixture_token"]
    if not isinstance(revision, str) or not (
        HEX40.fullmatch(revision) or (result == "pending" and revision == "unknown")
    ):
        raise IdentityError("fixture revision is malformed")
    if not isinstance(token, str) or not (
        HEX64.fullmatch(token) or (result == "pending" and token == "unknown")
    ):
        raise IdentityError("fixture token is malformed")
    if type(doc["generation"]) is not int or doc["generation"] <= 0:
        raise IdentityError("fixture generation must be a positive integer")
    if result == "pending":
        raise IdentityPending("fixture runtime witness is pending")
    return doc


def _sample_runtime(send: Callable[[str], str]) -> tuple[dict, dict]:
    requests = (
        ("native", "s2_khook_probe runtime", "khook-runtime", _validate_native_witness),
        ("fixture", "s2_khook_accept runtime", "khook-fixture-runtime", _validate_fixture_witness),
    )
    ready = {}
    invalid = []
    pending = []
    for label, command, kind, validate in requests:
        try:
            text = send(command)
        except Exception as error:
            pending.append(f"{label} runtime witness unavailable: {error}")
            continue
        try:
            ready[label] = validate(_extract_witness(text, kind))
        except IdentityPending as error:
            pending.append(str(error))
        except IdentityError as error:
            invalid.append(str(error))
    if invalid:
        raise IdentityError("; ".join(invalid))
    if pending:
        raise IdentityPending("; ".join(pending))
    return ready["native"], ready["fixture"]


def _device(value: int) -> str:
    return f"{os.major(value):x}:{os.minor(value):x}"


def _stat_identity(info: os.stat_result) -> tuple[int, int, int, int]:
    return info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns


def _measure(path: Path, role: str, witness: dict | None = None) -> str:
    if not path.is_absolute():
        raise IdentityError(f"{role} path must be absolute")
    try:
        resolved = path.resolve(strict=True)
    except OSError as error:
        raise IdentityError(f"{role} installed file is unavailable: {error}") from error
    if str(resolved) != str(path):
        raise IdentityError(f"{role} installed path is not canonical: {path}")
    try:
        with resolved.open("rb") as handle:
            before = os.fstat(handle.fileno())
            if witness is not None:
                if witness["path"] != str(resolved):
                    raise IdentityError(f"{role} loaded path does not match installed path")
                if witness["device"] != _device(before.st_dev):
                    raise IdentityError(f"{role} installed device does not match loaded image")
                if witness["inode"] != str(before.st_ino):
                    raise IdentityError(f"{role} installed inode does not match loaded image")
            digest = hashlib.sha256()
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
            after = os.fstat(handle.fileno())
    except IdentityError:
        raise
    except OSError as error:
        raise IdentityError(f"{role} installed file cannot be measured: {error}") from error
    if _stat_identity(before) != _stat_identity(after):
        raise IdentityError(f"{role} installed file changed while hashing")
    return digest.hexdigest()


def verify_host_artifact(tree: Path, manifest: Path) -> None:
    verifier_path = Path(__file__).with_name("verify-metamod-artifact.py")
    spec = importlib.util.spec_from_file_location("khook_host_artifact_verifier", verifier_path)
    if spec is None or spec.loader is None:
        raise IdentityError(f"cannot load host artifact verifier at {verifier_path}")
    module = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(module)
    except Exception as error:
        raise IdentityError(f"cannot load host artifact verifier: {error}") from error
    errors = io.StringIO()
    try:
        with contextlib.redirect_stderr(errors):
            module.verify(tree, manifest)
    except SystemExit as error:
        detail = errors.getvalue().strip() or f"exit {error.code}"
        raise IdentityError(f"host artifact verification failed: {detail}") from error
    except Exception as error:
        raise IdentityError(f"host artifact verification failed: {error}") from error


def _load_run_id(run_dir: Path | None, explicit: str | None, server: str, revision: str) -> str:
    if run_dir is None:
        run_id = explicit or ka.new_run_id()
    else:
        doc, _ = _read_json(run_dir / "run.json", "pending run")
        if not isinstance(doc, dict):
            raise IdentityError("pending run must be an object")
        run_id = doc.get("run_id")
        if explicit is not None and explicit != run_id:
            raise IdentityError("wrong run_id for existing pending run")
        if doc.get("server") != server:
            raise IdentityError("wrong server for existing pending run")
        if doc.get("source_revision") != revision:
            raise IdentityError("wrong source revision for existing pending run")
        if doc.get("identity_bound") is True or isinstance(doc.get("runtime_identity"), dict):
            raise IdentityError("existing run already has a runtime identity")
    if not ka.valid_run_id(run_id):
        raise IdentityError("run_id must be a bounded command-safe token")
    return run_id


def _installed_path(addons_root: Path, relative: str, role: str) -> Path:
    if not addons_root.is_absolute():
        raise IdentityError("addons root must be absolute")
    try:
        root = addons_root.resolve(strict=True)
        path = (root / relative).resolve(strict=True)
    except OSError as error:
        raise IdentityError(f"{role} installed file is unavailable: {error}") from error
    try:
        path.relative_to(root)
    except ValueError as error:
        raise IdentityError(f"{role} installed path escapes addons root") from error
    return path


def generate_identity(
    *,
    addons_root: Path,
    build_manifest_path: Path,
    host_manifest_path: Path,
    expected_source_revision: str,
    server: str,
    port: int,
    run_dir: Path | None,
    run_id: str | None,
    output_path: Path,
    evidence_path: Path,
    rcon_send: Callable[[str], str] | None = None,
    now: Callable[[], datetime] | None = None,
    host_verify: Callable[[Path, Path], None] = verify_host_artifact,
) -> tuple[dict, bytes]:
    del output_path  # Publication paths never influence the receipt identity.
    revision = _require_hex(expected_source_revision, HEX40, "expected source revision")
    if server != f"127.0.0.1:{port}":
        raise IdentityError("server must match the local RCON endpoint 127.0.0.1:PORT")
    build_doc, build_raw = _read_json(build_manifest_path, "runtime build manifest")
    build = _validate_build_manifest(build_doc)
    if build["source_revision"] != revision:
        raise IdentityError("runtime build manifest has wrong source revision")
    selected_run = _load_run_id(run_dir, run_id, server, revision)
    send = rcon_send or (lambda command: ka.default_rcon_send(command, port=port))
    native_before, fixture_before = _sample_runtime(send)
    if native_before["source_revision"] != revision:
        raise IdentityError("native runtime has wrong source revision")
    if fixture_before["fixture_revision"] != build["fixture_revision"]:
        raise IdentityError("active fixture revision does not match runtime build manifest")
    if fixture_before["fixture_token"] != build["fixture_token"]:
        raise IdentityError("active fixture token does not match runtime build manifest")

    measured: dict[str, dict[str, str]] = {}
    for role, relative in BUILD_ARTIFACT_PATHS.items():
        path = _installed_path(addons_root, relative, role)
        witness = native_before["modules"].get(role) if role != "fixture" else None
        sha = _measure(path, role, witness)
        if sha != build["artifacts"][role]["sha256"]:
            raise IdentityError(f"{role} hash mismatch against runtime build manifest")
        measured[role] = {"path": str(path), "sha256": sha}

    host_doc, host_raw_before = _read_json(host_manifest_path, "installed host manifest")
    host_tree = host_manifest_path.parent.resolve(strict=True)
    if not isinstance(host_doc, dict) or not isinstance(host_doc.get("artifacts"), list):
        raise IdentityError("installed host manifest has malformed artifacts")
    host_hashes = {}
    for item in host_doc["artifacts"]:
        if isinstance(item, dict) and set(item) == {"path", "sha256"}:
            host_hashes[item["path"]] = item["sha256"]
    host_measured = {}
    for role, relative in HOST_MODULE_PATHS.items():
        path = _installed_path(host_tree, relative, role)
        sha = _measure(path, role, native_before["modules"][role])
        if host_hashes.get(relative) != sha:
            raise IdentityError(f"{role} hash does not match installed host manifest")
        host_measured[role] = sha
    host_verify(host_tree, host_manifest_path)
    _, host_raw_after = _read_json(host_manifest_path, "installed host manifest")
    if host_raw_before != host_raw_after:
        raise IdentityError("installed host manifest changed during verification")

    native_after, fixture_after = _sample_runtime(send)
    if native_after != native_before:
        raise IdentityError("native runtime changed during identity generation")
    if fixture_after != fixture_before:
        raise IdentityError("fixture runtime changed during identity generation")
    for role, relative in BUILD_ARTIFACT_PATHS.items():
        path = _installed_path(addons_root, relative, role)
        witness = native_after["modules"].get(role) if role != "fixture" else None
        if _measure(path, role, witness) != measured[role]["sha256"]:
            raise IdentityError(f"{role} changed after verification")
    for role, relative in HOST_MODULE_PATHS.items():
        path = _installed_path(host_tree, relative, role)
        if _measure(path, role, native_after["modules"][role]) != host_measured[role]:
            raise IdentityError(f"{role} changed after verification")
    _, build_raw_after = _read_json(build_manifest_path, "runtime build manifest")
    _, host_raw_final = _read_json(host_manifest_path, "installed host manifest")
    if build_raw_after != build_raw:
        raise IdentityError("runtime build manifest changed during verification")
    if host_raw_final != host_raw_before:
        raise IdentityError("installed host manifest changed after verification")

    checked_at = (now or (lambda: datetime.now(timezone.utc)))()
    if checked_at.tzinfo is None:
        raise IdentityError("verification timestamp must include a timezone")
    verified_at = checked_at.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    evidence_doc = {
        "schema": 1,
        "kind": "khook-runtime-verification-evidence",
        "run_id": selected_run,
        "server": server,
        "verified_at": verified_at,
        "expected_source_revision": revision,
        "build_manifest": {"path": str(build_manifest_path.resolve()), "sha256": hashlib.sha256(build_raw).hexdigest()},
        "host_manifest": {"path": str(host_manifest_path.resolve()), "sha256": hashlib.sha256(host_raw_before).hexdigest()},
        "artifacts": measured,
        "native_before": native_before,
        "fixture_before": fixture_before,
        "native_after": native_after,
        "fixture_after": fixture_after,
    }
    evidence = (json.dumps(evidence_doc, indent=2, sort_keys=True) + "\n").encode("utf-8")
    build_hash_input = {role: measured[role]["sha256"] for role in ("core", "shim")}
    receipt = {
        "schema": 1,
        "kind": "khook-runtime-identity",
        "run_id": selected_run,
        "source_revision": revision,
        "s2script_commit": build["s2script_commit"],
        "s2script_build_hash": ka.runtime_identity_digest(build_hash_input),
        "host_manifest_digest": hashlib.sha256(host_raw_before).hexdigest(),
        "fixture_revision": build["fixture_revision"],
        "server": server,
        "server_build": str(native_before["server_build"]),
        "initial_map": native_before["map"],
        "verified_at": verified_at,
        "evidence_path": str(evidence_path),
        "evidence_sha256": hashlib.sha256(evidence).hexdigest(),
        "artifacts": measured,
    }
    receipt["artifact_identity"] = ka.runtime_identity_digest(receipt)
    errors = ka.validate_runtime_identity(receipt, {
        "run_id": selected_run, "source_revision": revision, "server": server,
    })
    if errors:
        raise IdentityError("generated receipt failed controller validation: " + "; ".join(errors))
    return receipt, evidence


def _publish_no_replace(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temp_path = Path(temporary)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.link(temp_path, path)
    except FileExistsError as error:
        raise IdentityError(f"output already exists: {path}") from error
    finally:
        temp_path.unlink(missing_ok=True)


def generate_and_publish(**kwargs) -> dict:
    output_path = Path(kwargs["output_path"])
    evidence_path = Path(kwargs["evidence_path"])
    if output_path == evidence_path:
        raise IdentityError("receipt and evidence paths must differ")
    for path in (output_path, evidence_path):
        if path.exists():
            raise IdentityError(f"output already exists: {path}")
    receipt, evidence = generate_identity(**kwargs)
    receipt_bytes = (json.dumps(receipt, indent=2, sort_keys=True) + "\n").encode("utf-8")
    published_evidence = False
    try:
        _publish_no_replace(evidence_path, evidence)
        published_evidence = True
        _publish_no_replace(output_path, receipt_bytes)
    except Exception:
        if published_evidence:
            evidence_path.unlink(missing_ok=True)
        raise
    return receipt


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate a KHook receipt from the installed, running runtime (run inside the server filesystem namespace)."
    )
    parser.add_argument("--addons-root", required=True, type=Path)
    parser.add_argument("--build-manifest", required=True, type=Path)
    parser.add_argument("--host-manifest", required=True, type=Path)
    parser.add_argument("--expected-source-revision", required=True)
    parser.add_argument("--server", default="127.0.0.1:27015")
    parser.add_argument("--port", type=int, default=27015)
    parser.add_argument("--run-dir", type=Path, help="existing pending controller run whose run_id must be adopted")
    parser.add_argument("--run-id", help="new run id; omit to generate one")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    try:
        receipt = generate_and_publish(
            addons_root=args.addons_root,
            build_manifest_path=args.build_manifest,
            host_manifest_path=args.host_manifest,
            expected_source_revision=args.expected_source_revision,
            server=args.server,
            port=args.port,
            run_dir=args.run_dir,
            run_id=args.run_id,
            output_path=args.output,
            evidence_path=args.evidence,
        )
    except IdentityPending as error:
        print(f"PENDING: {error}", file=sys.stderr)
        return 2
    except IdentityError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(f"wrote installed-runtime receipt: {args.output}")
    print(f"run_id: {receipt['run_id']}")
    if args.run_dir:
        print(f"next: bash scripts/test-khook-live.sh A --collect --run-dir {args.run_dir} --identity {args.output}")
    else:
        print(f"next: bash scripts/test-khook-live.sh A --prepare --run-dir RUN_DIR --identity {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
