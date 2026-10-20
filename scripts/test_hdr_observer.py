import json
import tempfile
import unittest
from pathlib import Path

from hdr_observer import read_probe, verify_hardware


class HdrObserverProtocol(unittest.TestCase):
    def records(self, level):
        return [
            {"event": "start", "pid": 1234},
            {"event": "hardware_factory", "present": True, "code": 0, "version": "MaterialSystemHardwareConfig012"},
            *[{"event": "sample", "dx_support_level": 95, "hardware_hdr_type": 2,
               "hdr_type": 2 if level == 2 else 0, "hdr_enabled": level == 2} for _ in range(8)],
            {"event": "complete"},
        ]

    def test_complete_receipt_requires_acquisition_identity_and_stable_channel(self):
        for level in (0, 2):
            rows = self.records(level)
            result = verify_hardware(rows, level)
            self.assertEqual(result["native_pid"], 1234)
            self.assertEqual(len(result["samples"]), 8)
            for index, patch in [(0, {"event": "other"}), (1, {"version": "wrong"}),
                                 (2, {"hdr_enabled": level != 2}), (3, {"dx_support_level": 80}),
                                 (4, {"hardware_hdr_type": 0})]:
                with self.subTest(level=level, index=index, patch=patch):
                    invalid = [{**row, **patch} if i == index else row for i, row in enumerate(rows)]
                    with self.assertRaises(RuntimeError):
                        verify_hardware(invalid, level)
            with self.assertRaises(RuntimeError):
                verify_hardware([rows[0], *rows], level)
        rows = self.records(2)
        rows[3] = {**rows[3], "hdr_type": 1, "hdr_enabled": True}
        with self.assertRaisesRegex(RuntimeError, "Native HDR state changed during sampling"):
            verify_hardware(rows, 2)

    def test_incomplete_stream_remains_pending_and_failures_reject_immediately(self):
        self.assertIsNone(verify_hardware([], 2))
        self.assertIsNone(verify_hardware(self.records(2)[:-1], 2))
        for event in ("invalid_identity", "acquisition_failed", "thread_failed", "ready_timeout"):
            with self.subTest(event=event), self.assertRaises(RuntimeError):
                verify_hardware([{"event": event}], 2)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "hardware.jsonl"
            self.assertEqual(read_probe(path), [])
            rows = self.records(2)
            path.write_text("\n".join(json.dumps(row) for row in rows))
            self.assertIsNone(verify_hardware(read_probe(path), 2))
            with path.open("a") as stream:
                stream.write("\n")
            self.assertIsNotNone(verify_hardware(read_probe(path), 2))


if __name__ == "__main__":
    unittest.main()
