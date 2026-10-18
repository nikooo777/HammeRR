import copy
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from contact_runtime_observer import (
    DIRECT_WAKE_MODE,
    DIRECT_WAKE_OPTION,
    trigger_state_summary,
    verify_contact_trigger_state,
    verify_direct_wake,
    verify_direct_wake_option,
    verify_intervention_records,
    verify_trigger_state_option,
)


class ContactTriggerStateProtocol(unittest.TestCase):
    def test_legacy_rows_require_no_trigger_state_fields(self):
        verify_trigger_state_option({}, {"schema": 1}, False)
        verify_contact_trigger_state({"source": "0x123", "other": "0x456"}, False)
        for field in ("trigger_state_schema", "source_is_trigger", "other_is_trigger"):
            with self.subTest(field=field), self.assertRaises(AssertionError):
                verify_contact_trigger_state({field: False}, False)
        for receipt, start in (
            ({"contact_trigger_state": False}, {}),
            ({}, {"contact_trigger_state": False}),
            ({}, {"contact_trigger_state_schema": 1}),
        ):
            with self.subTest(receipt=receipt, start=start), self.assertRaises(AssertionError):
                verify_trigger_state_option(receipt, start, False)

    def test_requested_option_requires_matching_receipt_and_native_start(self):
        receipt = {"contact_trigger_state": {"schema": 1, "observation_only": True}}
        start = {"contact_trigger_state": True, "contact_trigger_state_schema": 1}
        verify_trigger_state_option(receipt, start, True)
        for invalid in (None, False, True, {}, {"schema": 1},
                        {"schema": 1, "observation_only": False},
                        {"schema": True, "observation_only": True},
                        {"schema": 1.0, "observation_only": True},
                        {"schema": 1, "observation_only": 1}):
            with self.subTest(receipt=invalid), self.assertRaises((AssertionError, KeyError)):
                verify_trigger_state_option({"contact_trigger_state": invalid}, start, True)
        for field in start:
            missing = copy.deepcopy(start)
            del missing[field]
            with self.subTest(missing=field), self.assertRaises(KeyError):
                verify_trigger_state_option(receipt, missing, True)
            invalid_values = (None, False, 0, 1, "true", 1.0)
            if field == "contact_trigger_state_schema":
                invalid_values = (None, False, True, 0, "1", 1.0)
            for invalid in invalid_values:
                with self.subTest(field=field, value=invalid), self.assertRaises(AssertionError):
                    verify_trigger_state_option(receipt, {**start, field: invalid}, True)
        with self.assertRaises(KeyError):
            verify_trigger_state_option({}, start, True)

    def test_pair_states_remain_strict_booleans_and_report_observations(self):
        rows = [
            {"trigger_state_schema": 1, "source_is_trigger": source, "other_is_trigger": other}
            for source in (False, True) for other in (False, True)
        ]
        for row in rows:
            verify_contact_trigger_state(row, True)
        self.assertEqual(trigger_state_summary(rows), {
            "contact_rows": 4, "nontrigger_pair_rows": 1, "trigger_pair_rows": 3,
        })
        for field in rows[0]:
            missing = dict(rows[0])
            del missing[field]
            with self.subTest(missing=field), self.assertRaises(KeyError):
                verify_contact_trigger_state(missing, True)
            values = (None, 0, 1, "false", [])
            if field == "trigger_state_schema":
                values = (None, False, True, 0, 1.0, "1")
            for invalid in values:
                with self.subTest(field=field, value=invalid), self.assertRaises(AssertionError):
                    verify_contact_trigger_state({**rows[0], field: invalid}, True)


class DirectWakeProtocol(unittest.TestCase):
    def capture(self, activation_offset=135):
        samples, records = [], []
        by_sample, grouped = {}, {}
        times = [5.0, 12.0] + [12.105 + index * 0.015 for index in range(335)]
        call_id, end_id = 136, len(times) - 1
        activation_id = None if activation_offset is None else 2 + activation_offset
        for index, time in enumerate(times):
            sample = {
                "event": "sample", "sample": index, "frame": 1 if index == 0 else 7 + index,
                "simulation_time": time, "awake": index == 0 or (activation_id is not None and index >= activation_id),
                "velocity": [0.0, 0.0, 0.0], "angular_velocity": [0.0, 0.0, 0.0],
                "object": "0x10", "owner": "0x20", "collide": "0x30", "crate_index": 44,
                "contacts": 1,
            }
            contact = {"event": "contact", "sample": index, "name": "blocked_door_side_brace"}
            by_sample[index] = [contact]
            records.extend([contact, sample])
            samples.append(sample)
            if index == 2:
                records.append({"event": "wake_arm", "sample": index, "frame": sample["frame"],
                                "last_solid_time": times[1], "first_disabled_time": time,
                                "last_active_brace_contact_time": times[0]})
            if index == call_id:
                identity = {field: sample[field] for field in ("sample", "frame", "simulation_time", "object", "owner", "collide")}
                records.append({"event": "wake_call", "stage": "enter", **identity,
                                "slot": 25, "target_elf": 0x37210, "crate_index": 44,
                                "contacts": 1, "brace_contacts": 1, "asleep": True,
                                "settled_since": times[2], "snapshot_destroyed": True, "in_simulation": False})
                records.append({"event": "wake_call", "stage": "return", **identity})
                records.append({"event": "wake_contact", "stage": "after", "sample": index,
                                "name": "blocked_door_side_brace"})
                records.append({"event": "wake_after", **identity, "awake": False,
                                "velocity": [0.0, 0.0, 0.0], "angular_velocity": [0.0, 0.0, 0.0],
                                "contacts": 1, "brace_contacts": 1, "snapshot_destroyed": True, "in_simulation": False})
            if index == activation_id:
                records.append({"event": "wake_activation", **{field: sample[field] for field in ("sample", "frame", "simulation_time")}})
            if index == end_id:
                records.append({"event": "wake_window_end", **{field: sample[field] for field in ("sample", "frame", "simulation_time")}})
        for record in records:
            grouped.setdefault(record["event"], []).append(record)
        return records, grouped, samples, by_sample, samples[2], samples[1], by_sample[0], [0.0, 0.1], 12.0, 16.1

    def test_matching_direct_call_and_processed_activation_admit_the_control(self):
        report = verify_direct_wake(*self.capture())
        self.assertTrue(report["admitted"])
        self.assertEqual(report["direct_calls"], 1)
        self.assertEqual(report["before_brace_contacts"], 1)
        self.assertEqual(report["after_brace_contacts"], 1)
        self.assertFalse(report["awake_after_call"])
        self.assertTrue(report["next_frame_awake"])
        self.assertEqual(report["dense_observation_frames"], 335)

    def test_absent_or_late_activation_remains_explicitly_unadmitted(self):
        for offset in (None, 334):
            with self.subTest(offset=offset):
                report = verify_direct_wake(*self.capture(offset))
                self.assertFalse(report["admitted"])
                self.assertEqual(report["direct_calls"], 1)

        immediate_only = self.capture(None)
        immediate_only[1]["wake_after"][0]["awake"] = True
        self.assertFalse(verify_direct_wake(*immediate_only)["admitted"])

    def test_corrupt_call_timing_identity_order_and_dense_observations_reject(self):
        for change in ("duplicate_call", "wrong_slot", "wrong_target", "different_owner", "missing_activation",
                       "missing_window", "frame_gap", "time_gap", "unsettled", "call_order", "unknown_event"):
            data = self.capture()
            records, grouped, samples = data[:3]
            enter = grouped["wake_call"][0]
            with self.subTest(change=change):
                if change == "duplicate_call":
                    grouped["wake_call"].append(dict(enter))
                elif change == "wrong_slot":
                    enter["slot"] = 28
                elif change == "wrong_target":
                    enter["target_elf"] = 0x38100
                elif change == "different_owner":
                    enter["owner"] = "0x999"
                elif change == "missing_activation":
                    grouped.pop("wake_activation")
                elif change == "missing_window":
                    grouped.pop("wake_window_end")
                elif change == "frame_gap":
                    samples[enter["sample"] + 2]["frame"] += 1
                elif change == "time_gap":
                    samples[enter["sample"] + 2]["simulation_time"] += 0.1
                elif change == "unsettled":
                    samples[enter["sample"] - 1]["velocity"][0] = 1.0
                elif change == "call_order":
                    records.remove(enter)
                    records.append(enter)
                elif change == "unknown_event":
                    grouped["wake_fallback"] = []
                with self.assertRaises((AssertionError, KeyError)):
                    verify_direct_wake(*data)

    def test_option_schema_and_event_families_cannot_relabel_old_modes(self):
        receipt = {"schema": 2, "direct_wake_control": dict(DIRECT_WAKE_OPTION)}
        start = {"schema": 2, "intervention": "single-wake", "direct_wake_schema": 1}
        verify_direct_wake_option(receipt, start, True)
        for field, value in (("slot", 28), ("target_elf", 0x38100), ("schema", True), ("observation_only", True)):
            bad = copy.deepcopy(receipt)
            bad["direct_wake_control"][field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                verify_direct_wake_option(bad, start, True)
        for mode in ("kill", "disable", "disable-wake", "disable-refresh"):
            verify_direct_wake_option({"schema": 1}, {"schema": 1}, False)
            with self.subTest(mode=mode), self.assertRaises(AssertionError):
                verify_intervention_records({"wake_call": []}, mode)
        with self.assertRaises(AssertionError):
            verify_intervention_records({"refresh_call": []}, DIRECT_WAKE_MODE)
        with self.assertRaises(AssertionError):
            verify_direct_wake_option(receipt, start, False)

    def test_prepare_overrides_inherited_interventions_and_keeps_receipts_distinct(self):
        import contact_runtime_observer as contact
        from contact_runtime_observer import prepare
        from validate_rotating_removal_recovery import fixture_identity, observer
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            root = parent / "installed"
            root.mkdir()
            (root / "cstrike_linux64").write_bytes(b"not executed")
            library = parent / "probe.so"
            library.write_bytes(b"parser fixture only")
            modules = {}
            for name in ("engine.so", "server.so", "vphysics.so"):
                (root / name).write_bytes(name.encode())
                modules[name] = (name, hashlib.sha256(name.encode()).hexdigest(), "fixture")
            identities = set()
            for mode in ("kill", "disable", "disable-wake", "disable-refresh", DIRECT_WAKE_MODE):
                output = parent / mode
                game = output / "cstrike"
                (game / "cfg").mkdir(parents=True)
                (game / "cfg/hammerr_private_launch.cfg").write_text(
                    "sv_lan 1\necho HAMMERR_PRIVATE_PREMAP_READY\nmap hammerr_runtime\n")
                command = [str(root / "cstrike_linux64"), "-default", "-insecure", "-game", str(game)]
                inherited = {"HAMMERR_CONTACT_INTERVENTION": "single-wake", "HAMMERR_CONTACT_INITIAL_PROFILE": "1", "HAMMERR_CONTACT_TRIGGER_STATE": "1"}
                with patch.object(contact, "MODULES", modules):
                    actual_command, environment = prepare(library, mode, root, game, output, command, inherited)
                self.assertEqual(actual_command, command)
                self.assertEqual(environment["HAMMERR_CONTACT_INTERVENTION"],
                                 "single-wake" if mode == DIRECT_WAKE_MODE else "single-recheck" if mode == "disable-refresh" else "none")
                self.assertEqual(environment["HAMMERR_CONTACT_INITIAL_PROFILE"], "0")
                self.assertEqual(environment["HAMMERR_CONTACT_TRIGGER_STATE"], "0")
                receipt = json.loads((output / "contact-observer-launch.json").read_text())
                self.assertEqual(receipt["schema"], 2 if mode == DIRECT_WAKE_MODE else 1)
                self.assertEqual(receipt["release_mode"], mode)
                identities.add(fixture_identity(mode))
            self.assertEqual(len(identities), 5)
            self.assertEqual(observer(DIRECT_WAKE_MODE), observer("disable"))
            self.assertNotEqual(observer(DIRECT_WAKE_MODE), observer("disable-wake"))


if __name__ == "__main__":
    unittest.main()
