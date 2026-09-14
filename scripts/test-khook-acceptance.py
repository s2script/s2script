#!/usr/bin/env python3
"""Judge regressions for scripts/khook_acceptance.py (stdlib unittest)."""
from __future__ import annotations

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
    "source_revision": "d66d7bd45721b3be2a44da358f33b6cef1b5d594",
}


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

    def test_b_and_c_explicitly_unavailable(self):
        self.assertEqual(ka.UNAVAILABLE_SUITES["B"], "not authored")
        self.assertEqual(ka.UNAVAILABLE_SUITES["C"], "not authored")
        r = ka.judge_records([], identity=IDENTITY, suite="B")
        self.assertEqual(r.exit_code, 1)
        self.assertIn("not authored", " ".join(r.messages).lower())

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
        recs.append(make_record("one_normal_invocation", "native_one_pre_post_orig", "native"))
        r = judge(recs)
        self.assertEqual(r.exit_code, 1, r.messages)
        self.assertTrue(any("duplicate" in m.lower() for m in r.messages))

        recs = all_records("pass")
        recs.append(make_record("voice_recall", "human_voice_allowed_hears", "human"))
        r = judge(recs)
        self.assertEqual(r.exit_code, 1, r.messages)
        self.assertTrue(any("duplicate" in m.lower() for m in r.messages))

        recs = all_records("pass")
        recs.append(
            make_record(
                "sdkhooks_one_of_two_entities",
                "js_hook_a_delivered",
                "js",
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
        human = ka.EXAMPLE_HUMAN_OBSERVATIONS
        # Example is pending and must not pass on its own.
        r2 = judge(auto, observations=human)
        self.assertEqual(r2.exit_code, 2)
        passing_human = {
            "schema": ka.SCHEMA,
            "run_id": IDENTITY["run_id"],
            "source_revision": IDENTITY["source_revision"],
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
        identity = {
            "run_id": IDENTITY["run_id"],
            "source_revision": ka.current_source_revision(),
        }
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

        with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as fh:
            fh.write(text)
            path = fh.name
        try:
            proc = subprocess.run(
                ["bash", str(LIVE_SH), "A", "--from-file", path],
                cwd=str(ROOT),
                capture_output=True,
                text=True,
            )
        finally:
            os.unlink(path)
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
    def test_command_protocol_strings(self):
        rid = "khook-a-test-0001"
        self.assertEqual(ka.probe_cmd("prepare", rid), "s2_khook_probe prepare khook-a-test-0001")
        self.assertEqual(ka.probe_cmd("collect", rid), "s2_khook_probe collect khook-a-test-0001")
        self.assertEqual(ka.probe_cmd("report", rid), "s2_khook_probe report khook-a-test-0001")
        self.assertEqual(ka.accept_cmd("prepare", rid), "s2_khook_accept prepare khook-a-test-0001")
        self.assertEqual(ka.accept_cmd("collect", rid), "s2_khook_accept collect khook-a-test-0001")
        self.assertEqual(ka.accept_cmd("report", rid), "s2_khook_accept report khook-a-test-0001")
        self.assertEqual(ka.accept_cmd("teardown", rid), "s2_khook_accept teardown khook-a-test-0001")


if __name__ == "__main__":
    unittest.main()
