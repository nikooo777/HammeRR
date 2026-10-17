import copy
import unittest

from contact_runtime_observer import (
    trigger_state_summary,
    verify_contact_trigger_state,
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


if __name__ == "__main__":
    unittest.main()
