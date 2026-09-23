#!/usr/bin/env python3
"""Judge regressions for scripts/khook_acceptance.py (stdlib unittest)."""
from __future__ import annotations

import copy
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import khook_acceptance as ka  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
LIVE_SH = ROOT / "scripts" / "test-khook-live.sh"

IDENTITY = {
    "run_id": "khook-a-test-0001",
    "source_revision": ka.current_source_revision(),
}


def receipt_digest(value):
    payload = {key: item for key, item in value.items() if key != "artifact_identity"}
    return hashlib.sha256(json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def runtime_receipt(run_id=None):
    artifacts = {
        name: {"path": "/installed/" + name, "sha256": digit * 64}
        for name, digit in (("shim", "1"), ("core", "2"), ("probe", "3"), ("fixture", "4"))
    }
    build = {name: artifacts[name]["sha256"] for name in ("core", "shim")}
    receipt = {
        "schema": 1,
        "kind": "khook-runtime-identity",
        "run_id": run_id or IDENTITY["run_id"],
        "source_revision": IDENTITY["source_revision"],
        "s2script_commit": IDENTITY["source_revision"],
        "s2script_build_hash": receipt_digest(build),
        "host_manifest_digest": "5" * 64,
        "fixture_revision": IDENTITY["source_revision"],
        "server": "127.0.0.1:27015",
        "server_build": "12345-test-fixture",
        "initial_map": "de_dust2",
        "verified_at": "2026-09-15T00:00:00Z",
        "evidence_path": "captures/operator-runtime-verification.txt",
        "evidence_sha256": "6" * 64,
        "artifacts": artifacts,
    }
    receipt["artifact_identity"] = receipt_digest(receipt)
    return receipt


IDENTITY["runtime_identity"] = runtime_receipt()


def _req(case: str):
    return [s for s in ka.required_subchecks() if s.case == case]


def _human_actors(case: str):
    if case == "voice_recall":
        return [
            {"slot": 0, "identity": "speaker-0"},
            {"slot": 1, "identity": "allowed-1"},
            {"slot": 2, "identity": "denied-2"},
        ]
    return [
        {"slot": 0, "identity": "client-a"},
        {"slot": 1, "identity": "client-b"},
    ]


def make_record(case, subcheck, producer, result="pass", **overrides):
    rec = {
        "schema": ka.SCHEMA,
        "suite": "A",
        "run_id": IDENTITY["run_id"],
        "source_revision": IDENTITY["source_revision"],
        "artifact_identity": IDENTITY["runtime_identity"]["artifact_identity"],
        "case": case,
        "subcheck": subcheck,
        "producer": producer,
        "result": result,
        "expected": {"ok": True},
        "actual": {"ok": True} if result == "pass" else {"ok": False},
        "evidence": "unittest-fixture" if result != "pending" else "",
    }
    if producer == "human":
        rec["actors"] = _human_actors(case)
        rec["capture_path"] = "captures/unittest.txt" if result != "pending" else ""
        rec["timestamp"] = "2026-09-14T00:00:00Z" if result != "pending" else ""
    rec.update(overrides)
    return rec


def all_records(result="pass", producers=None, skip=None, overlay=None):
    skip = skip or set()
    overlay = overlay or {}
    out = []
    for sc in ka.required_subchecks():
        key = (sc.producer, sc.case, sc.subcheck)
        if key in skip:
            continue
        if producers is not None and sc.producer not in producers:
            continue
        rec = make_record(sc.case, sc.subcheck, sc.producer, result=result)
        if key in overlay:
            rec.update(overlay[key])
        out.append(rec)
    return out


def jsonl(recs):
    return "".join(json.dumps(r, sort_keys=True) + "\n" for r in recs)


def judge(recs, observations=None, identity=None):
    return ka.judge_records(
        recs,
        identity=identity or IDENTITY,
        observations=observations,
        suite="A",
    )


def write_run(tmp: Path, recs, identity=None):
    identity = dict(identity or IDENTITY)
    identity.setdefault("schema", ka.SCHEMA)
    identity.setdefault("suite", "A")
    identity.setdefault("identity_bound", bool(identity.get("runtime_identity")))
    tmp.mkdir(parents=True, exist_ok=True)
    (tmp / "run.json").write_text(json.dumps(identity, indent=2) + "\n")
    (tmp / "records.jsonl").write_text(jsonl(recs))
    return tmp


class RegistryTests(unittest.TestCase):
    def test_twelve_case_names_frozen(self):
        self.assertEqual(
            list(ka.SUITE_A_CASES),
            [
                "new_capsule_registration",
                "shared_capsule_registration",
                "peer_actions_both_orders",
                "one_normal_invocation",
                "frame_client_command_hooks",
                "fire_event_no_suppression",
                "fire_event_handled_recipient_mask",
                "voice_recall",
                "sdkhooks_one_of_two_entities",
                "sdkhooks_phase_removal",
                "entity_slot_reuse_map_teardown",
                "check_transmit",
            ],
        )

    def test_b_and_c_have_separate_registries(self):
        for suite in ("B", "C"):
            self.assertTrue(ka.required_subchecks(suite))
            r = ka.judge_records([], identity=IDENTITY, suite=suite)
            self.assertEqual(r.exit_code, 1)
            self.assertIn("missing case records", " ".join(r.messages))

    def test_frame_requires_native_and_js_and_controls(self):
        names = {(s.producer, s.subcheck) for s in _req("frame_client_command_hooks")}
        self.assertIn(("native", "native_gameframe_observed"), names)
        self.assertIn(("js", "js_gameframe_delivery"), names)
        self.assertIn(("native", "native_command_continue_original"), names)
        self.assertIn(("js", "js_command_continue_delivery"), names)
        self.assertIn(("native", "native_command_handled_skipped"), names)
        self.assertIn(("js", "js_command_handled_delivery"), names)
        self.assertIn(("native", "native_client_identity_join"), names)
        self.assertIn(("js", "js_client_identity_join"), names)
        self.assertIn(("native", "native_control_missing_js_hook"), names)
        self.assertIn(("native", "native_control_flipped_decision"), names)
        self.assertIn(("native", "native_control_omitted_acceptance_plugin"), names)

    def test_fire_event_native_owns_original_counts(self):
        fe = _req("fire_event_no_suppression")
        self.assertTrue(any(s.producer == "native" and s.subcheck == "native_original_once" for s in fe))
        self.assertTrue(any(s.producer == "native" and s.subcheck == "native_listener_delivery" for s in fe))
        self.assertFalse(any(s.producer == "js" and "original" in s.subcheck for s in fe))

    def test_human_assisted_human_cannot_replace_native_js(self):
        for case in (
            "voice_recall",
            "check_transmit",
            "fire_event_handled_recipient_mask",
        ):
            producers = {s.producer for s in _req(case)}
            self.assertIn("native", producers)
            self.assertIn("js", producers)
            self.assertIn("human", producers)

    def test_stateful_cases_have_named_subchecks(self):
        for case in (
            "sdkhooks_one_of_two_entities",
            "sdkhooks_phase_removal",
            "entity_slot_reuse_map_teardown",
        ):
            self.assertGreaterEqual(len(_req(case)), 2)

    def test_reload_registry_requires_script_evidence_and_rejects_native_hot_unload(self):
        names = {(s.producer, s.subcheck) for s in _req("entity_slot_reuse_map_teardown")}
        self.assertIn(("native", "script_hot_reload"), names)
        self.assertNotIn(("native", "native_unload_reload"), names)
        old = make_record("entity_slot_reuse_map_teardown", "native_unload_reload", "native")
        self.assertEqual(judge(all_records() + [old]).exit_code, 1)

    def test_script_reload_requires_both_observed_producers(self):
        names = {"script_hot_reload", "js_fresh_subscription_after_reload"}
        rows = all_records()
        complete = [copy.deepcopy(r) for r in rows if r["subcheck"] in names]
        for r in rows:
            if r["subcheck"] in names:
                r.update(result="pending", actual={}, evidence="await actual script reload")
        self.assertEqual(judge(rows).exit_code, 2)
        self.assertEqual(judge(rows + complete[:1]).exit_code, 2)
        self.assertEqual(judge(rows + complete).exit_code, 0)
        self.assertEqual(judge(rows + complete + rows).exit_code, 0)

    def test_script_reload_command_builders_keep_one_safe_run_argument(self):
        self.assertEqual(ka.probe_cmd("reload-arm", "run_1"), "s2_khook_probe reload-arm run_1")
        self.assertEqual(ka.accept_cmd("reload-arm", "run_1"), "s2_khook_accept reload-arm run_1")
        self.assertEqual(ka.accept_cmd("resume", "run_1", "a" * 64), "s2_khook_accept resume run_1 " + "a" * 64)
        for command in (ka.probe_cmd, ka.accept_cmd):
            with self.assertRaises(ValueError):
                command("reload-arm", "run unsafe")

    def test_human_schema_example_is_pending_and_matches_constant(self):
        example = ka.EXAMPLE_HUMAN_OBSERVATIONS
        self.assertEqual(example["schema"], ka.SCHEMA)
        self.assertEqual(example["run_id"], ka.EXAMPLE_IDENTITY["run_id"])
        self.assertEqual(example["source_revision"], ka.EXAMPLE_IDENTITY["source_revision"])
        self.assertTrue(example["observations"])
        for obs in example["observations"]:
            self.assertEqual(obs["result"], "pending")
            self.assertEqual(obs.get("evidence", ""), "")
        parsed = json.loads(json.dumps(example))
        self.assertEqual(parsed, example)


class JudgeTests(unittest.TestCase):
    def test_valid_all_pass(self):
        r = judge(all_records("pass"))
        self.assertEqual(r.exit_code, 0, r.messages)
        self.assertEqual(r.status, "pass")
        self.assertEqual(len(r.case_status), 12)
        self.assertTrue(all(v == "pass" for v in r.case_status.values()))

    def test_required_pending(self):
        recs = all_records("pass")
        for rec in recs:
            if rec["subcheck"] == "human_voice_allowed_hears":
                rec["result"] = "pending"
                rec["evidence"] = ""
                rec["actual"] = {}
                rec["capture_path"] = ""
                rec["timestamp"] = ""
        r = judge(recs)
        self.assertEqual(r.exit_code, 2, r.messages)
        self.assertEqual(r.status, "pending")
        self.assertEqual(r.case_status["voice_recall"], "pending")

    def test_actual_mismatch(self):
        recs = all_records("pass")
        for rec in recs:
            if rec["subcheck"] == "native_one_pre_post_orig":
                rec["actual"] = {"ok": False, "pre": 0}
                rec["expected"] = {"ok": True, "pre": 1}
        r = judge(recs)
        self.assertEqual(r.exit_code, 1, r.messages)
        self.assertEqual(r.case_status["one_normal_invocation"], "fail")

    def test_missing_record(self):
        recs = [rec for rec in all_records("pass") if rec["case"] != "check_transmit"]
        r = judge(recs)
        self.assertEqual(r.exit_code, 1, r.messages)
        self.assertTrue(any("missing" in m.lower() and "check_transmit" in m for m in r.messages))

    def test_duplicate_native_js_human_subcheck(self):
        recs = all_records("pass")
        recs.append(make_record("one_normal_invocation", "native_one_pre_post_orig", "native", result="fail"))
        r = judge(recs)
        self.assertEqual(r.exit_code, 1, r.messages)
        self.assertTrue(any("duplicate" in m.lower() for m in r.messages))

        recs = all_records("pass")
        recs.append(make_record("voice_recall", "human_voice_allowed_hears", "human", result="fail"))
        r = judge(recs)
        self.assertEqual(r.exit_code, 1, r.messages)
        self.assertTrue(any("duplicate" in m.lower() for m in r.messages))

        recs = all_records("pass")
        recs.append(
            make_record(
                "sdkhooks_one_of_two_entities",
                "js_hook_a_delivered",
                "js",
                result="fail",
            )
        )
        r = judge(recs)
        self.assertEqual(r.exit_code, 1, r.messages)

    def test_wrong_run(self):
        recs = all_records("pass")
        recs[0]["run_id"] = "other-run"
        r = judge(recs)
        self.assertEqual(r.exit_code, 1, r.messages)
        self.assertTrue(any("run" in m.lower() for m in r.messages))

    def test_wrong_source_revision(self):
        recs = all_records("pass")
        recs[-1]["source_revision"] = "0" * 40
        r = judge(recs)
        self.assertEqual(r.exit_code, 1, r.messages)
        self.assertTrue(any("revision" in m.lower() for m in r.messages))

        historical = all_records("pass")
        r = ka.judge_records(
            historical,
            identity={"run_id": IDENTITY["run_id"], "source_revision": "newrev" + "a" * 28},
            suite="A",
        )
        self.assertEqual(r.exit_code, 1)
        self.assertTrue(any("revision" in m.lower() for m in r.messages))

    def test_unsupported_schema_or_case(self):
        recs = all_records("pass")
        recs[0]["schema"] = 99
        r = judge(recs)
        self.assertEqual(r.exit_code, 1, r.messages)

        recs = all_records("pass")
        recs.append(make_record("not_a_case", "native_x", "native"))
        r = judge(recs)
        self.assertEqual(r.exit_code, 1, r.messages)
        self.assertTrue(any("unsupported" in m.lower() or "unknown" in m.lower() for m in r.messages))

    def test_malformed_json(self):
        r = ka.judge_text("{not json\n", identity=IDENTITY, suite="A")
        self.assertEqual(r.exit_code, 1, r.messages)
        self.assertTrue(any("malformed" in m.lower() for m in r.messages))

    def test_contradictory_result_legacy_pass_fields(self):
        recs = all_records("pass")
        recs[0]["pass"] = False
        recs[0]["result"] = "pass"
        r = judge(recs)
        self.assertEqual(r.exit_code, 1, r.messages)
        self.assertTrue(any("contradict" in m.lower() for m in r.messages))

    def test_forged_top_level_pass_missing_required_evidence_does_not_pass(self):
        forged = []
        for case in ka.SUITE_A_CASES:
            forged.append(
                {
                    "schema": ka.SCHEMA,
                    "suite": "A",
                    "run_id": IDENTITY["run_id"],
                    "source_revision": IDENTITY["source_revision"],
                    "case": case,
                    "subcheck": "native_one_pre_post_orig"
                    if case == "one_normal_invocation"
                    else _req(case)[0].subcheck,
                    "producer": _req(case)[0].producer,
                    "result": "pass",
                    "expected": {"ok": True},
                    "actual": {"ok": True},
                    "evidence": "forged-top-level",
                }
            )
        r = judge(forged)
        self.assertNotEqual(r.exit_code, 0, r.messages)
        self.assertNotEqual(r.status, "pass")

    def test_later_js_record_cannot_overwrite_native_failure(self):
        recs = all_records("pass")
        for rec in recs:
            if rec["producer"] == "native" and rec["case"] == "frame_client_command_hooks":
                rec["result"] = "fail"
                rec["actual"] = {"ok": False}
                rec["evidence"] = "native-fail"
        r = judge(recs)
        self.assertEqual(r.exit_code, 1, r.messages)
        self.assertEqual(r.case_status["frame_client_command_hooks"], "fail")
        js_pass = [
            rec
            for rec in recs
            if rec["producer"] == "js" and rec["case"] == "frame_client_command_hooks"
        ]
        self.assertTrue(js_pass)
        self.assertTrue(all(j["result"] == "pass" for j in js_pass))

    def test_valid_observations_complete_pending_to_pass(self):
        auto = all_records("pass", producers=("native", "js"))
        r = judge(auto)
        self.assertEqual(r.exit_code, 2, r.messages)
        self.assertEqual(r.status, "pending")
        human = copy.deepcopy(ka.EXAMPLE_HUMAN_OBSERVATIONS)
        human["source_revision"] = IDENTITY["source_revision"]
        # Example is pending and must not pass on its own.
        r2 = judge(auto, observations=human)
        self.assertEqual(r2.exit_code, 2)
        passing_human = {
            "schema": ka.SCHEMA,
            "run_id": IDENTITY["run_id"],
            "source_revision": IDENTITY["source_revision"],
            "artifact_identity": IDENTITY["runtime_identity"]["artifact_identity"],
            "observations": [],
        }
        for sc in ka.required_subchecks():
            if sc.producer != "human":
                continue
            passing_human["observations"].append(
                {
                    "case": sc.case,
                    "subcheck": sc.subcheck,
                    "result": "pass",
                    "expected": {"ok": True},
                    "actual": {"ok": True},
                    "evidence": "unittest-fixture",
                    "actors": _human_actors(sc.case),
                    "capture_path": "captures/unittest.txt",
                    "timestamp": "2026-09-14T00:00:00Z",
                }
            )
        r3 = judge(auto, observations=passing_human)
        self.assertEqual(r3.exit_code, 0, r3.messages)
        self.assertEqual(r3.status, "pass")

    def test_observations_wrong_run_absent_actors_or_native_fail_cannot_pass(self):
        auto = all_records("pass", producers=("native", "js"))
        human_ok = {
            "schema": ka.SCHEMA,
            "run_id": "other-run",
            "source_revision": IDENTITY["source_revision"],
            "observations": [
                {
                    "case": "voice_recall",
                    "subcheck": "human_voice_allowed_hears",
                    "result": "pass",
                    "expected": {"ok": True},
                    "actual": {"ok": True},
                    "evidence": "unittest-fixture",
                    "actors": _human_actors("voice_recall"),
                    "capture_path": "captures/unittest.txt",
                    "timestamp": "2026-09-14T00:00:00Z",
                }
            ],
        }
        r = judge(auto, observations=human_ok)
        self.assertNotEqual(r.exit_code, 0)
        self.assertNotEqual(r.status, "pass")

        human_no_actors = {
            "schema": ka.SCHEMA,
            "run_id": IDENTITY["run_id"],
            "source_revision": IDENTITY["source_revision"],
            "observations": [
                {
                    "case": sc.case,
                    "subcheck": sc.subcheck,
                    "result": "pass",
                    "expected": {"ok": True},
                    "actual": {"ok": True},
                    "evidence": "unittest-fixture",
                    "actors": [],
                    "capture_path": "captures/unittest.txt",
                    "timestamp": "2026-09-14T00:00:00Z",
                }
                for sc in ka.required_subchecks()
                if sc.producer == "human"
            ],
        }
        r = judge(auto, observations=human_no_actors)
        self.assertNotEqual(r.exit_code, 0)
        self.assertNotEqual(r.status, "pass")

        auto_fail = all_records("pass")
        for rec in auto_fail:
            if rec["subcheck"] == "native_voice_listen_bits":
                rec["result"] = "fail"
                rec["actual"] = {"ok": False}
                rec["evidence"] = "native-fail"
        r = judge(auto_fail)
        self.assertEqual(r.exit_code, 1)
        self.assertEqual(r.case_status["voice_recall"], "fail")

    def test_from_file_and_live_use_same_parser_judge(self):
        identity = copy.deepcopy(IDENTITY)
        recs = all_records("pass")
        for rec in recs:
            rec["source_revision"] = identity["source_revision"]
        text = jsonl(recs)
        parsed, errs = ka.parse_records(text)
        self.assertEqual(errs, [])
        file_result = ka.judge_records(parsed, identity=identity, suite="A")
        with tempfile.TemporaryDirectory() as td:
            run_dir = write_run(Path(td) / "run-001", recs, identity=identity)
            live = ka.judge_run_dir(run_dir, observations=None)
        self.assertEqual(file_result.exit_code, live.exit_code)
        self.assertEqual(file_result.case_status, live.case_status)
        self.assertIs(ka.from_file_parse, ka.parse_records)
        self.assertIs(ka.from_file_judge, ka.judge_records)
        self.assertIs(ka.live_parse, ka.parse_records)
        self.assertIs(ka.live_judge, ka.judge_records)

        with tempfile.TemporaryDirectory() as td:
            path = Path(td) / "records.jsonl"
            path.write_text(text)
            receipt_path = Path(td) / "identity.json"
            receipt_path.write_text(json.dumps(identity["runtime_identity"]))
            proc = subprocess.run(
                ["bash", str(LIVE_SH), "A", "--from-file", str(path), "--identity", str(receipt_path)],
                cwd=str(ROOT),
                capture_output=True,
                text=True,
            )
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)


class OrchestrationTests(unittest.TestCase):
    def test_prepare_unreachable_creates_run_without_invented_native(self):
        with tempfile.TemporaryDirectory() as td:
            run_dir = Path(td) / "run-001"
            seen = []

            def rcon(cmd):
                seen.append(cmd)
                raise ka.RconUnreachable("rcon_unreachable")

            r = ka.prepare_run(
                run_dir,
                suite="A",
                identity_fields={
                    "source_revision": IDENTITY["source_revision"],
                    "s2script_commit": IDENTITY["source_revision"],
                },
                rcon_send=rcon,
            )
            self.assertEqual(r.exit_code, 2, r.messages)
            self.assertTrue(any("rcon_unreachable" in m for m in r.messages))
            meta = json.loads((run_dir / "run.json").read_text())
            self.assertEqual(meta["source_revision"], IDENTITY["source_revision"])
            self.assertTrue(meta["run_id"])
            records_path = run_dir / "records.jsonl"
            if records_path.exists():
                self.assertEqual(records_path.read_text().strip(), "")
            self.assertTrue(any("prepare" in c for c in seen))

    def test_collect_does_not_resend_prepare(self):
        with tempfile.TemporaryDirectory() as td:
            run_dir = Path(td) / "run-001"
            seen = []

            def rcon(cmd):
                seen.append(cmd)
                raise ka.RconUnreachable("rcon_unreachable")

            ka.prepare_run(
                run_dir,
                suite="A",
                identity_fields={"source_revision": IDENTITY["source_revision"]},
                rcon_send=rcon,
            )
            seen.clear()
            r = ka.collect_run(run_dir, rcon_send=rcon)
            self.assertEqual(r.exit_code, 2)
            self.assertTrue(seen)
            self.assertFalse(any("prepare" in c for c in seen))
            self.assertTrue(any("collect" in c for c in seen))

    def test_judge_is_readonly(self):
        with tempfile.TemporaryDirectory() as td:
            recs = all_records("pass")
            run_dir = write_run(Path(td) / "run-001", recs)
            before = (run_dir / "records.jsonl").read_text()
            meta_before = (run_dir / "run.json").read_text()
            called = []
            ka.judge_run_dir(run_dir, rcon_send=lambda cmd: called.append(cmd))
            self.assertEqual((run_dir / "records.jsonl").read_text(), before)
            self.assertEqual((run_dir / "run.json").read_text(), meta_before)
            self.assertEqual(called, [])

    def test_need_clients_printed_and_pending(self):
        recs = all_records("pass", producers=("native", "js"))
        r = judge(recs)
        self.assertEqual(r.exit_code, 2)
        joined = "\n".join(r.messages)
        self.assertIn("NEED_CLIENTS:", joined)
        self.assertIn("voice_recall", joined)
        self.assertIn("check_transmit", joined)
        self.assertIn("fire_event_handled_recipient_mask", joined)

    def test_pass_requires_expected_actual_and_evidence(self):
        recs = all_records("pass")
        recs[0]["evidence"] = ""
        r = judge(recs)
        self.assertEqual(r.exit_code, 1, r.messages)
        recs = all_records("pass")
        recs[0].pop("expected")
        r = judge(recs)
        self.assertEqual(r.exit_code, 1, r.messages)

    def test_example_record_matches_executable_fixture(self):
        example = ka.EXAMPLE_RECORD
        parsed, errs = ka.parse_records(json.dumps(example))
        self.assertEqual(errs, [])
        self.assertEqual(parsed[0]["subcheck"], example["subcheck"])
        self.assertEqual(parsed[0]["result"], "pending")


class ProtocolTests(unittest.TestCase):
    def test_run_ids_are_bounded_command_tokens_before_side_effects(self):
        for bad in ("../escape", "a/b", "two words", "semi;quit", "line\nquit", "x" * 65, "", 123):
            with self.subTest(run_id=bad), tempfile.TemporaryDirectory() as td:
                run = Path(td) / "not-created"
                commands = []
                result = ka.prepare_run(run, identity_fields={"run_id": bad},
                                        rcon_send=lambda cmd: commands.append(cmd) or "")
                self.assertEqual(result.exit_code, 1, result.messages)
                self.assertIn("run_id", " ".join(result.messages))
                self.assertFalse(run.exists())
                self.assertEqual(commands, [])
                for builder in (ka.probe_cmd, ka.accept_cmd):
                    with self.assertRaises(ValueError):
                        builder("collect", bad)

    def test_collect_rejects_unsafe_stored_run_before_rcon(self):
        with tempfile.TemporaryDirectory() as td:
            run = write_run(Path(td), [], identity={"run_id": "../escape", "source_revision": IDENTITY["source_revision"]})
            commands = []
            result = ka.collect_run(run, rcon_send=lambda cmd: commands.append(cmd) or "")
            self.assertEqual(result.exit_code, 1)
            self.assertIn("run_id", " ".join(result.messages))
            self.assertEqual(commands, [])

    def test_command_protocol_strings(self):
        rid = "khook-a-test-0001"
        self.assertEqual(ka.probe_cmd("prepare", rid), "s2_khook_probe prepare khook-a-test-0001")
        self.assertEqual(ka.probe_cmd("collect", rid), "s2_khook_probe collect khook-a-test-0001")
        self.assertEqual(ka.probe_cmd("report", rid), "s2_khook_probe report khook-a-test-0001")
        self.assertEqual(ka.accept_cmd("prepare", rid), "s2_khook_accept prepare khook-a-test-0001")
        self.assertEqual(ka.accept_cmd("collect", rid), "s2_khook_accept collect khook-a-test-0001")
        self.assertEqual(ka.accept_cmd("report", rid), "s2_khook_accept report khook-a-test-0001")
        self.assertEqual(ka.accept_cmd("teardown", rid), "s2_khook_accept teardown khook-a-test-0001")


class HistoryRegressionTests(unittest.TestCase):
    def collect_output(self, run_dir, output):
        return ka.collect_run(
            run_dir,
            rcon_send=lambda command: output if "probe collect" in command else "",
            interval_s=0,
            max_attempts=1,
        )

    def test_persisted_failure_cannot_be_replaced_by_later_pass(self):
        failed = all_records()
        failed[0].update(result="fail", actual={"ok": False})
        with tempfile.TemporaryDirectory() as td:
            run = write_run(Path(td), failed)
            self.assertEqual(ka.judge_run_dir(run).exit_code, 1)
            result = self.collect_output(run, jsonl(all_records()))
            self.assertEqual(result.exit_code, 1, result.messages)
            self.assertEqual(ka.judge_run_dir(run).exit_code, 1)

    def test_duplicate_failure_then_pass_has_same_live_and_offline_verdict(self):
        failed = dict(all_records()[0], result="fail", actual={"ok": False})
        output = jsonl([failed] + all_records())
        offline = ka.judge_text(output, identity=IDENTITY)
        with tempfile.TemporaryDirectory() as td:
            run = write_run(Path(td), [])
            live = self.collect_output(run, output)
            self.assertEqual(offline.exit_code, 1)
            self.assertEqual(live.exit_code, offline.exit_code, live.messages)
            self.assertEqual(ka.judge_run_dir(run).exit_code, 1)

    def test_malformed_input_remains_invalid_after_later_valid_collect(self):
        bad_output = "{broken JSON\n" + jsonl(all_records())
        with tempfile.TemporaryDirectory() as td:
            run = write_run(Path(td), [])
            first = self.collect_output(run, bad_output)
            self.assertEqual(first.exit_code, 1, first.messages)
            second = self.collect_output(run, jsonl(all_records()))
            self.assertEqual(second.exit_code, 1, second.messages)
            stored = ka.judge_run_dir(run)
            self.assertTrue(any("malformed" in message for message in stored.messages))
            offline = ka.judge_text((run / "records.jsonl").read_text(), identity=IDENTITY)
            self.assertEqual(offline.exit_code, stored.exit_code)

    def test_repeated_commands_keep_every_raw_capture(self):
        with tempfile.TemporaryDirectory() as td:
            run = write_run(Path(td), all_records())
            self.collect_output(run, "first response\n")
            original = {path: path.read_bytes() for path in (run / "raw").iterdir()}
            self.collect_output(run, "second response\n")
            captures = list((run / "raw").iterdir())
            self.assertEqual(len(captures), 8)
            for path, contents in original.items():
                self.assertEqual(path.read_bytes(), contents)
            self.assertTrue(any(path.read_text() == "first response\n" for path in captures))
            self.assertTrue(any(path.read_text() == "second response\n" for path in captures))

    def test_identical_report_replays_are_idempotent(self):
        result = judge(all_records() + all_records())
        self.assertEqual(result.exit_code, 0, result.messages)

    def test_pending_progresses_to_observed_without_losing_pass_on_later_pending(self):
        history = all_records("pending") + all_records() + all_records("pending")
        result = judge(history)
        self.assertEqual(result.exit_code, 0, result.messages)
        with tempfile.TemporaryDirectory() as td:
            run = write_run(Path(td), all_records("pending"))
            self.assertEqual(self.collect_output(run, jsonl(all_records())).exit_code, 0)
            self.assertEqual(self.collect_output(run, jsonl(all_records("pending"))).exit_code, 0)

    def test_different_terminal_observations_are_contradictory(self):
        recs = all_records()
        recs.append(dict(recs[0], expected={"count": 2}, actual={"count": 2}))
        result = judge(recs)
        self.assertEqual(result.exit_code, 1, result.messages)

    def test_duplicate_json_status_keys_cannot_hide_failure(self):
        record = json.dumps(all_records()[0]).replace('"result": "pass"', '"result": "fail", "result": "pass"')
        output = record + "\n" + jsonl(all_records()[1:])
        result = ka.judge_text(output, identity=IDENTITY)
        self.assertEqual(result.exit_code, 1, result.messages)
        with tempfile.TemporaryDirectory() as td:
            run = write_run(Path(td), [])
            self.assertEqual(self.collect_output(run, output).exit_code, 1)

    def test_malformed_schema_types_are_invalid(self):
        records = all_records()
        records[0]["schema"] = True
        result = judge(records)
        self.assertEqual(result.exit_code, 1, result.messages)


class HumanValidationRegressionTests(unittest.TestCase):
    def human_file(self):
        return {
            "schema": 1,
            **IDENTITY,
            "artifact_identity": IDENTITY["runtime_identity"]["artifact_identity"],
            "observations": [rec for rec in all_records() if rec["producer"] == "human"],
        }

    def test_missing_and_null_human_outcomes_are_invalid(self):
        auto = all_records(producers=("native", "js"))
        for value in (None, {}, [], ""):
            with self.subTest(value=value):
                human = self.human_file()
                human["observations"][0].update(expected=value, actual=value)
                result = judge(auto, observations=human)
                self.assertEqual(result.exit_code, 1, result.messages)
        human = self.human_file()
        for name in ("expected", "actual"):
            human["observations"][0].pop(name)
        self.assertEqual(judge(auto, observations=human).exit_code, 1)

    def test_human_normalization_preserves_contradictory_status(self):
        human = self.human_file()
        human["observations"][0]["pass"] = False
        result = judge(all_records(producers=("native", "js")), observations=human)
        self.assertEqual(result.exit_code, 1, result.messages)
        self.assertTrue(any("contradict" in message for message in result.messages))


class ArtifactIdentityRegressionTests(unittest.TestCase):
    def test_cli_judge_validates_supplied_identity_without_raw_records(self):
        for reason in ("rcon_unreachable", "incomplete_records"):
            with self.subTest(reason=reason), tempfile.TemporaryDirectory() as td:
                run = write_run(Path(td) / "run", [], identity=IDENTITY)
                meta = json.loads((run / "run.json").read_text())
                meta["reason"] = reason
                (run / "run.json").write_text(json.dumps(meta))
                (run / "records.jsonl").unlink()
                receipt_path = Path(td) / "bad-identity.json"
                receipt_path.write_text("{malformed")
                proc = subprocess.run([sys.executable, str(ROOT / "scripts/khook_acceptance.py"),
                    "A", "--judge", "--run-dir", str(run), "--identity", str(receipt_path)],
                    capture_output=True, text=True)
                self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
                self.assertIn("malformed runtime identity", proc.stdout)

    def test_collect_can_bind_existing_pending_run_without_preparing_again(self):
        receipt = runtime_receipt()
        unbound = {key: value for key, value in IDENTITY.items() if key != "runtime_identity"}
        pending = all_records("pending")
        for record in pending:
            record.pop("artifact_identity")
        with tempfile.TemporaryDirectory() as td:
            run = write_run(Path(td), pending, identity=unbound)
            commands = []

            def send(command):
                commands.append(command)
                return jsonl(all_records()) if "probe collect" in command else ""

            result = ka.collect_run(run, identity_receipt=receipt, rcon_send=send, max_attempts=1)
            self.assertEqual(result.exit_code, 0, result.messages)
            self.assertFalse(any("prepare" in command for command in commands))
            self.assertEqual(sum(" bind " in command for command in commands), 2)
            self.assertTrue(all(command.endswith(receipt["artifact_identity"]) for command in commands if " bind " in command))
            commands.clear()
            replay = ka.collect_run(run, identity_receipt=receipt, rcon_send=send, max_attempts=1)
            self.assertEqual(replay.exit_code, 0, replay.messages)
            self.assertFalse(any(" bind " in command for command in commands))
            self.assertEqual(json.loads((run / "run.json").read_text())["run_id"], IDENTITY["run_id"])

    def test_changed_artifact_receipt_remains_invalid_after_retry(self):
        changed = runtime_receipt()
        changed["artifacts"]["probe"]["sha256"] = "9" * 64
        changed["artifact_identity"] = receipt_digest(changed)
        with tempfile.TemporaryDirectory() as td:
            run = write_run(Path(td), all_records())
            before = (run / "run.json").read_bytes()
            commands = []
            rejected = ka.collect_run(run, identity_receipt=changed, rcon_send=lambda command: commands.append(command) or "")
            self.assertEqual(rejected.exit_code, 1, rejected.messages)
            self.assertEqual(commands, [])
            self.assertEqual((run / "run.json").read_bytes(), before)
            retry = ka.collect_run(run, identity_receipt=runtime_receipt(), rcon_send=lambda command: jsonl(all_records()), max_attempts=1)
            self.assertEqual(retry.exit_code, 1, retry.messages)
            offline = ka.judge_from_file(run / "records.jsonl", identity_receipt=runtime_receipt())
            self.assertEqual(offline.exit_code, 1)

    def test_source_revision_mismatch_has_same_offline_and_run_verdict(self):
        identity = copy.deepcopy(IDENTITY)
        identity["source_revision"] = "0" * 40
        receipt = identity["runtime_identity"]
        for field in ("source_revision", "s2script_commit", "fixture_revision"):
            receipt[field] = "0" * 40
        receipt["artifact_identity"] = receipt_digest(receipt)
        records = all_records()
        for record in records:
            record["source_revision"] = "0" * 40
            record["artifact_identity"] = receipt["artifact_identity"]
        with tempfile.TemporaryDirectory() as td:
            run = write_run(Path(td), records, identity=identity)
            live = ka.judge_run_dir(run)
            offline = ka.judge_from_file(run / "records.jsonl", identity_receipt=receipt)
            self.assertEqual(live.exit_code, 1, live.messages)
            self.assertEqual(offline.exit_code, 1, offline.messages)

    def test_prepare_adopts_operator_receipt_and_sends_its_binding(self):
        receipt = runtime_receipt()
        with tempfile.TemporaryDirectory() as td:
            commands = []
            ka.prepare_run(
                Path(td),
                identity_fields={"runtime_identity": receipt},
                rcon_send=lambda command: commands.append(command) or "prepared",
            )
            meta = json.loads((Path(td) / "run.json").read_text())
            self.assertEqual(meta["run_id"], receipt["run_id"])
            self.assertEqual(meta.get("runtime_identity"), receipt)
            self.assertEqual(meta["s2script_build_hash"], receipt["s2script_build_hash"])
            self.assertTrue(all(command.endswith(receipt["artifact_identity"]) for command in commands))

    def test_cli_can_supply_identity_for_offline_and_readonly_judge(self):
        with tempfile.TemporaryDirectory() as td:
            run = write_run(Path(td) / "run", all_records(), identity={
                "run_id": IDENTITY["run_id"], "source_revision": IDENTITY["source_revision"],
            })
            receipt_path = Path(td) / "identity.json"
            receipt_path.write_text(json.dumps(runtime_receipt()))
            before = (run / "run.json").read_bytes()
            for mode in (["--judge", "--run-dir", str(run)], ["--from-file", str(run / "records.jsonl")]):
                proc = subprocess.run(
                    [sys.executable, str(ROOT / "scripts/khook_acceptance.py"), "A", *mode,
                     "--identity", str(receipt_path)], capture_output=True, text=True,
                )
                self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            self.assertEqual((run / "run.json").read_bytes(), before)

    def test_prepare_existing_directory_does_not_reset_run(self):
        with tempfile.TemporaryDirectory() as td:
            run = write_run(Path(td), all_records())
            before = (run / "run.json").read_bytes()
            commands = []
            result = ka.prepare_run(run, rcon_send=lambda command: commands.append(command) or "")
            self.assertEqual(result.exit_code, 1, result.messages)
            self.assertEqual((run / "run.json").read_bytes(), before)
            self.assertEqual(commands, [])

    def test_complete_records_without_installed_identity_stay_pending(self):
        identity = {key: value for key, value in IDENTITY.items() if key != "runtime_identity"}
        result = judge(all_records(), identity=identity)
        self.assertEqual(result.exit_code, 2, result.messages)
        self.assertTrue(any("identity" in message for message in result.messages))

    def test_missing_record_binding_stays_pending(self):
        records = all_records()
        records[0].pop("artifact_identity")
        result = judge(records)
        self.assertEqual(result.exit_code, 2, result.messages)

    def test_wrong_record_artifact_is_invalid(self):
        records = all_records()
        records[0]["artifact_identity"] = "9" * 64
        result = judge(records)
        self.assertEqual(result.exit_code, 1, result.messages)

    def test_missing_binary_hashes_and_wrong_build_receipt_are_invalid(self):
        for mutate in (
            lambda value: value["artifacts"].pop("core"),
            lambda value: value.update(s2script_build_hash="9" * 64),
            lambda value: value.update(run_id="another-run"),
            lambda value: value.update(source_revision="0" * 40),
        ):
            identity = copy.deepcopy(IDENTITY)
            receipt = identity["runtime_identity"]
            mutate(receipt)
            receipt["artifact_identity"] = receipt_digest(receipt)
            result = judge(all_records(), identity=identity)
            self.assertEqual(result.exit_code, 1, result.messages)

    def test_changed_receipt_bytes_do_not_match_digest(self):
        identity = copy.deepcopy(IDENTITY)
        identity["runtime_identity"]["artifacts"]["probe"]["sha256"] = "9" * 64
        result = judge(all_records(), identity=identity)
        self.assertEqual(result.exit_code, 1, result.messages)

    def test_human_observations_must_bind_same_artifact(self):
        human = HumanValidationRegressionTests().human_file()
        human["artifact_identity"] = "9" * 64
        result = judge(all_records(producers=("native", "js")), observations=human)
        self.assertEqual(result.exit_code, 1, result.messages)


class SuiteBoundaryTests(unittest.TestCase):
    def test_suite_suffix_preserves_a_commands(self):
        for command in (ka.probe_cmd, ka.accept_cmd):
            self.assertTrue(command("prepare", "run").endswith("prepare run"))
            self.assertTrue(command("prepare", "run", "a" * 64, suite="B").endswith("run " + "a" * 64 + " B"))
            self.assertTrue(command("collect", "run", suite="C").endswith("run C"))
            with self.assertRaises(ValueError): command("collect", "run", suite="D")
            with self.assertRaises(ValueError): command("prepare", "run", "bad hash")

    def test_cli_suite_mismatch_refuses_before_rcon(self):
        with tempfile.TemporaryDirectory() as tmp:
            run = write_run(Path(tmp), [], dict(IDENTITY, suite="B"))
            calls = []
            result = ka.collect_run(run, suite="C", rcon_send=lambda cmd: calls.append(cmd))
            self.assertEqual(result.exit_code, 1)
            self.assertIn("suite mismatch", " ".join(result.messages))
            self.assertEqual(calls, [])

    def test_synthetic_bc_cannot_pass_live_judge(self):
        for suite in ("B", "C"):
            records = ka.synthetic_fixture("all-pass", IDENTITY, suite=suite)
            self.assertEqual({r["evidence_class"] for r in records}, {"synthetic"})
            for rec in records: rec["artifact_identity"] = IDENTITY["runtime_identity"]["artifact_identity"]
            result = ka.judge_records(records, identity=IDENTITY, suite=suite)
            self.assertEqual(result.exit_code, 1)
            self.assertIn("synthetic", " ".join(result.messages))

    def test_terminal_process_status_is_diagnostic(self):
        for suite in ("B", "C"):
            self.assertFalse(any("terminal" in s.subcheck for s in ka.required_subchecks(suite)))
            records = ka.synthetic_fixture("pending", IDENTITY, suite=suite)
            diagnostic = dict(records[0], kind="diagnostic", case="process_terminal", subcheck="native_process_exit",
                              producer="native", result="fail", expected={"exit": 0}, actual={"exit": 139},
                              evidence="known shutdown-only139; user disposition non-blocking")
            result = ka.judge_records(records + [diagnostic], identity=IDENTITY, suite=suite)
            self.assertEqual(result.exit_code, 2, result.messages)
            self.assertIn("DIAGNOSTIC", " ".join(result.messages))


class IntegrationEvidenceTests(unittest.TestCase):
    def gamedata_fixture(self):
        native_file = dict(path="/installed/gamedata/cs2/master.gamedata.jsonc", size=0,
                           fingerprint_algorithm="fnv1a64-diagnostic", fingerprint=ka._fnv1a64(b""))
        measured = dict(path=native_file["path"], size=0, fingerprint=native_file["fingerprint"], sha256=hashlib.sha256(b"").hexdigest())
        binding = IDENTITY["runtime_identity"]["artifact_identity"]
        native = dict(run_id=IDENTITY["run_id"], artifact_identity=binding, prepared=True, unchanged=True, files=[native_file])
        phase = dict(run_id=IDENTITY["run_id"], artifact_identity=binding, status="pass", files=[measured])
        return native, dict(before=copy.deepcopy(phase), after=copy.deepcopy(phase))

    def records(self, suite):
        # Deliberately fabricated test input exercises judge validation only.
        # These are never emitted by --emit-fixture as observed/live records.
        records = ka.synthetic_fixture("all-pass", IDENTITY, suite=suite)
        for rec in records:
            rule = ka.SUITES[suite].rules[(rec["case"], rec["subcheck"], rec["producer"])]
            rec.update(artifact_identity=IDENTITY["runtime_identity"]["artifact_identity"], evidence_class="observed",
                       group=rule.group, provenance=rule.provenance, callback_owner=rule.callback_owner, target=rule.target)
            rec["observations"] = [dict(scenario_id="fixture", sequence=1, generation=1, invocation="fixture-1",
                callbacks=1, peer_order="peer-first", facts={"pre": 1}, stimulus="engine", route="main-virtual-precache",
                frame_token=1, map_generation=1, receiver="r1", vtable="v1", manifest="m1")]
            if rec["subcheck"] in ka.GAMEDATA_SUBCHECKS:
                rec["gamedata"] = self.gamedata_fixture()[0]
            if rec["subcheck"] == "native_main_bypass_absent_then_next_delivered":
                rec["observations"][0]["facts"].update(bypass_original=1, bypass_peer_pre=1, bypass_peer_post=1,
                    bypass_js=0, next_original=1, next_peer_pre=1, next_peer_post=1, next_js=1)
            if "peer_orders" in rec["subcheck"] or "both_orders" in rec["subcheck"] or suite == "C":
                rec["observations"].append(dict(rec["observations"][0], sequence=2, invocation="fixture-2",
                    peer_order="s2script-first", frame_token=2, map_generation=2, manifest="m2"))
        return records

    def judge(self, records, suite):
        identity = dict(IDENTITY, gamedata_capture=self.gamedata_fixture()[1])
        return ka.judge_records(records, identity=identity, suite=suite)

    def test_populated_parser_examples(self):
        for suite in ("B", "C"):
            result = self.judge(self.records(suite), suite)
            self.assertEqual(result.exit_code, 0, result.messages)

    def test_noop_or_duplicate_bypass_cannot_pass_from_direct_call_alone(self):
        for field, value in (("bypass_original", 0), ("bypass_peer_pre", 0), ("bypass_peer_post", 0),
                             ("bypass_js", 1), ("next_original", 2), ("next_js", 0)):
            records = self.records("B")
            row = next(r for r in records if r["subcheck"] == "native_main_bypass_absent_then_next_delivered")
            row["observations"][0]["facts"][field] = value
            self.assertEqual(self.judge(records, "B").exit_code, 1, field)

    def test_deployed_gamedata_requires_independent_matching_hashes(self):
        records = self.records("B")
        self.assertEqual(ka.judge_records(records, identity=IDENTITY, suite="B").exit_code, 2)
        for mutation in ("hash", "native_changed", "duplicate"):
            native, capture = self.gamedata_fixture()
            identity = dict(IDENTITY, gamedata_capture=capture)
            rows = copy.deepcopy(records)
            if mutation == "hash": capture["after"]["files"][0]["sha256"] = "f" * 64
            elif mutation == "native_changed": next(r for r in rows if r["subcheck"] in ka.GAMEDATA_SUBCHECKS)["gamedata"]["unchanged"] = False
            else: capture["before"]["files"].append(copy.deepcopy(capture["before"]["files"][0]))
            self.assertEqual(ka.judge_records(rows, identity=identity, suite="B").exit_code, 1)

    def test_gamedata_hash_capture_reads_installed_bytes_and_rejects_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "gamedata/cs2/master.gamedata.jsonc"
            path.parent.mkdir(parents=True)
            data = b'{"files": []}'
            path.write_bytes(data)
            native, _ = self.gamedata_fixture()
            native.update(addon_root=str(root), files=[dict(path=str(path), size=len(data),
                fingerprint_algorithm="fnv1a64-diagnostic", fingerprint=ka._fnv1a64(data))])
            mapped_identity = dict(IDENTITY, gamedata_root=str(root / "gamedata"))
            observed = ka.capture_gamedata_inputs(native, mapped_identity)
            self.assertEqual(observed["status"], "pass")
            self.assertEqual(observed["files"][0]["sha256"], hashlib.sha256(data).hexdigest())
            path.write_bytes(b"changed")
            self.assertEqual(ka.capture_gamedata_inputs(native, mapped_identity)["status"], "fail")
            path.unlink()
            self.assertEqual(ka.capture_gamedata_inputs(native, mapped_identity)["status"], "pending")

    def test_missing_half_case_and_pending_callback(self):
        for suite in ("B", "C"):
            records = self.records(suite)
            self.assertEqual(self.judge([r for r in records if r["producer"] != "js"], suite).exit_code, 2)
            self.assertEqual(self.judge([r for r in records if r["case"] != records[0]["case"]], suite).exit_code, 1)
            records[0]["result"] = "pending"
            records[0]["actual"] = {}
            records[0]["observations"] = []
            self.assertEqual(self.judge(records, suite).exit_code, 2)

    def test_envelope_provenance_and_owner_rejections(self):
        mutations = [dict(suite="A"), dict(run_id="stale"), dict(source_revision="f"*40),
            dict(artifact_identity="f"*64), dict(producer="js"), dict(evidence_class="synthetic"),
            dict(provenance="invented"), dict(callback_owner="other"), dict(group="fabricated-group"),
            dict(observations=[])]
        for suite in ("B", "C"):
            for mutation in mutations:
                records = self.records(suite)
                records[0].update(mutation)
                self.assertEqual(self.judge(records, suite).exit_code, 1, (suite, mutation))
            records = self.records(suite)
            records[0]["artifact_identity"] = ""
            self.assertEqual(self.judge(records, suite).exit_code, 2)

    def test_producer_cannot_invent_success_contract(self):
        for suite in ("B", "C"):
            records = self.records(suite)
            records[0].update(expected={"ok": True}, actual={"ok": True})
            self.assertEqual(self.judge(records, suite).exit_code, 1)

    def test_main_bridge_never_joins_private_copy(self):
        for name in ("scenario_id", "sequence", "generation", "invocation", "peer_order"):
            records = self.records("B")
            row = next(r for r in records if r["subcheck"] == "js_acquire_outbound_pre_vote")
            row["observations"][0][name] = 9 if name in ("sequence", "generation") else "s2script-first" if name == "peer_order" else "unrelated"
            result = self.judge(records, "B")
            self.assertEqual(result.exit_code, 1, result.messages)
        records = self.records("B")
        row = next(r for r in records if r["subcheck"] == "native_main_acquire_outbound_pre_vote")
        row.update(group="controlled-mechanics", provenance="controlled-stock-provider")
        self.assertEqual(self.judge(records, "B").exit_code, 1)

    def test_real_precache_requires_main_frame_two_maps(self):
        for field, value in (("route", "session-manifest"), ("callbacks", 0), ("stimulus", "selftest"), ("frame_token", 0)):
            records = self.records("C")
            row = next(r for r in records if r["subcheck"] == "js_precache_resource_each_generation")
            row["observations"][0][field] = value
            self.assertEqual(self.judge(records, "C").exit_code, 1)
        records = self.records("C")
        for row in records:
            if row["case"] == "precache_map_transition":
                for observation in row["observations"]: observation["map_generation"] = 1
        self.assertEqual(self.judge(records, "C").exit_code, 1)

    def test_fail_cannot_be_erased_and_report_is_idempotent(self):
        for suite in ("B", "C"):
            records = self.records(suite)
            self.assertEqual(self.judge(records + copy.deepcopy(records), suite).exit_code, 0)
            first = copy.deepcopy(records[0])
            first.update(result="fail", actual={"failed": True})
            self.assertEqual(self.judge([first] + records, suite).exit_code, 1)


class SniperResourceTests(unittest.TestCase):
    def capture(self, **settings):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            docker = root / "docker"
            docker.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$CAPTURE"\n')
            docker.chmod(0o755)
            env = {key: value for key, value in os.environ.items()
                   if key not in ("S2_BUILD_JOBS", "CARGO_BUILD_JOBS", "S2_BUILD_CPUS", "S2_BUILD_MEMORY", "S2_BUILD_IMAGE")}
            env.update(PATH=str(root) + os.pathsep + env["PATH"], CAPTURE=str(root / "args"), **settings)
            proc = subprocess.run(["bash", str(ROOT / "scripts/test-khook-sniper-build.sh")], env=env, text=True, capture_output=True)
            args = (root / "args").read_text().splitlines() if (root / "args").exists() else []
            return proc, args

    def test_limits_and_pinned_image_forward_as_arguments(self):
        image = "rust@sha256:" + "a" * 64
        proc, args = self.capture(S2_BUILD_JOBS="2", CARGO_BUILD_JOBS="2", S2_BUILD_CPUS="2", S2_BUILD_MEMORY="8g", S2_BUILD_IMAGE=image)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        for pair in (("--cpus", "2"), ("--memory", "8g"), ("-e", "S2_BUILD_JOBS=2"), ("-e", "CARGO_BUILD_JOBS=2")):
            self.assertTrue(any(args[i:i+2] == list(pair) for i in range(len(args)-1)), pair)
        self.assertIn(image, args)
        self.assertNotIn("rust:bullseye", args)

    def test_invalid_jobs_fail_before_docker_or_package_work(self):
        for name in ("S2_BUILD_JOBS", "CARGO_BUILD_JOBS"):
            for value in ("0", "-1", "2; false", "two", "1.5"):
                proc, args = self.capture(**{name: value})
                self.assertNotEqual(proc.returncode, 0, (name, value))
                self.assertIn(name, proc.stderr)
                self.assertEqual(args, [])
                env = dict(os.environ, **{name: value})
                proc = subprocess.run(["bash", str(ROOT / "scripts/build-sniper.sh")], env=env, text=True, capture_output=True)
                self.assertNotEqual(proc.returncode, 0)
                self.assertIn(name, proc.stderr)


if __name__ == "__main__":
    unittest.main()
