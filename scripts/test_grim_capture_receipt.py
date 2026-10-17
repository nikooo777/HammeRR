import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

from grim_capture_receipt import (
    HEIGHT,
    MAX_LINE_BYTES,
    MAX_TRACE_BYTES,
    MAX_TRACE_LINES,
    STACK_SHA256,
    STRIDE,
    WIDTH,
    capture_receipt,
    digest,
    main,
    read_bounded,
    unique_object,
)

TRACE_LINES = [
    ' -> wl_display#1.get_registry(new id wl_registry#2)',
    'wl_registry#2.global(10, "wl_shm", 2)',
    ' -> wl_registry#2.bind(10, "wl_shm", 1, new id [unknown]#4)',
    'wl_registry#2.global(11, "wl_output", 4)',
    ' -> wl_registry#2.bind(11, "wl_output", 4, new id [unknown]#5)',
    'wl_registry#2.global(12, "zwlr_screencopy_manager_v1", 3)',
    ' -> wl_registry#2.bind(12, "zwlr_screencopy_manager_v1", 1, new id [unknown]#6)',
    'wl_shm#4.format(0)',
    'wl_shm#4.format(1)',
    'wl_output#5.geometry(0, 0, 0, 0, 0, "headless", "headless", 0)',
    'wl_output#5.mode(1, 1280, 720, 60000)',
    'wl_output#5.scale(1)',
    'wl_output#5.name("HEADLESS-1")',
    'wl_output#5.description("Headless output 1")',
    'wl_output#5.done()',
    ' -> wl_display#1.sync(new id wl_callback#7)',
    'wl_callback#7.done(99)',
    'wl_display#1.delete_id(7)',
    ' -> zwlr_screencopy_manager_v1#6.capture_output(new id zwlr_screencopy_frame_v1#7, 0, wl_output#5)',
    'zwlr_screencopy_frame_v1#7.buffer(1, 1280, 720, 5120)',
    ' -> wl_shm#4.create_pool(new id wl_shm_pool#8, fd 6, 3686400)',
    ' -> wl_shm_pool#8.create_buffer(new id wl_buffer#9, 0, 1280, 720, 5120, 1)',
    ' -> wl_shm_pool#8.destroy()',
    ' -> zwlr_screencopy_frame_v1#7.copy(wl_buffer#9)',
    'zwlr_screencopy_frame_v1#7.flags(0)',
    'zwlr_screencopy_frame_v1#7.ready(0, 167026, 123456789)',
    'discarded wl_buffer#9.release()',
    ' -> zwlr_screencopy_frame_v1#7.destroy()',
    ' -> wl_buffer#9.destroy()',
    ' -> wl_output#5.release()',
    ' -> zwlr_screencopy_manager_v1#6.destroy()',
]


def trace(lines=None):
    return "".join(f"[03:59:59.123456] {line}\n" for line in (TRACE_LINES if lines is None else lines)).encode()


def metadata(stderr, image, start=167026123456788, returned=167026123456790):
    return {
        "schema": 1,
        "phase": "on",
        "ordinal": 1,
        "native_pid": 1234,
        "grim_pid": 5678,
        "returncode": 0,
        "monotonic_start_ns": start,
        "monotonic_return_ns": returned,
        "argv": ["/usr/bin/grim", "-t", "ppm", "-o", "HEADLESS-1", "--", "/tmp/hammerr-four-ppm/on-1.ppm"],
        "environment": {"WAYLAND_DEBUG": "client", "WAYLAND_DISPLAY": "wayland-0", "XDG_RUNTIME_DIR": "/tmp/hammerr-four-ppm/wayland"},
        "image": {"path": "/tmp/hammerr-four-ppm/on-1.ppm", "bytes": len(image), "sha256": digest(image)},
        "stderr": {"path": "/tmp/hammerr-four-ppm/on-1.stderr", "bytes": len(stderr), "sha256": digest(stderr)},
        "capture_stack_sha256": dict(STACK_SHA256),
    }


class GrimCaptureReceipt(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.image = f"P6\n{WIDTH} {HEIGHT}\n255\n".encode() + bytes([17, 128, 239]) * (WIDTH * HEIGHT)

    def verify(self, data, image=None):
        image = self.image if image is None else image
        return capture_receipt(metadata(data, image), data, image)

    def test_installed_wayland_formatter_bytes_and_direction_spacing(self):
        data = trace()
        lines = data.split(b"\n")
        self.assertEqual(lines[0], b'[03:59:59.123456]  -> wl_display#1.get_registry(new id wl_registry#2)')
        self.assertEqual(lines[1], b'[03:59:59.123456] wl_registry#2.global(10, "wl_shm", 2)')
        self.assertEqual(lines[26], b'[03:59:59.123456] discarded wl_buffer#9.release()')
        queued = data.replace(b'] ', b'] {Capture Queue} ')
        self.assertEqual(queued.split(b"\n")[0], b'[03:59:59.123456] {Capture Queue}  -> wl_display#1.get_registry(new id wl_registry#2)')
        self.assertTrue(self.verify(queued)["protocol"]["events"][0]["request"])
        for invalid in (data.replace(b']  -> ', b'] -> '), queued.replace(b'}  -> ', b'} -> ')):
            with self.subTest(prefix=invalid[:90]), self.assertRaises(ValueError):
                self.verify(invalid)

    def test_receipt_preserves_provenance_and_separate_clock_domains(self):
        data = trace()
        receipt = self.verify(data)
        protocol = receipt["protocol"]
        self.assertTrue(receipt["diagnostic_only"])
        self.assertTrue(receipt["protocol_receipt_valid"])
        self.assertFalse(receipt["same_native_present_verified"])
        self.assertIsNone(receipt["native_frame_id"])
        self.assertIsNone(receipt["native_present_id"])
        self.assertIsNone(receipt["compositor_commit_seq"])
        self.assertEqual(protocol["negotiated_version"], 1)
        self.assertEqual(protocol["compositor_commit_monotonic_ns"], 167026123456789)
        self.assertEqual(protocol["ready"], {"tv_sec_hi": 0, "tv_sec_lo": 167026, "tv_nsec": 123456789})
        self.assertEqual(protocol["output"]["name"], "HEADLESS-1")
        self.assertEqual(protocol["buffer"]["format_name"], "xrgb8888")
        self.assertEqual(protocol["buffer"]["stride"], STRIDE)
        ready = [event for event in protocol["events"] if event["method"] == "ready"][0]
        self.assertEqual(ready["debug_realtime_prefix"], "03:59:59.123456")
        self.assertEqual(ready["debug_prefix_clock"], "CLOCK_REALTIME")
        self.assertEqual(receipt["supplied_provenance"]["grim_pid"], 5678)
        self.assertEqual(receipt["supplied_provenance"]["stderr"]["sha256"], digest(data))

    def test_both_mapped_formats_and_optional_ignored_release(self):
        for format_id in (0, 1):
            for include_release in (False, True):
                with self.subTest(format_id=format_id, release=include_release):
                    lines = [line.replace('buffer(1,', f'buffer({format_id},')
                             .replace('5120, 1)', f'5120, {format_id})') for line in TRACE_LINES
                             if include_release or '.release()' not in line or 'wl_buffer#' not in line]
                    receipt = self.verify(trace(lines))
                    self.assertEqual(receipt["protocol"]["buffer"]["format"], format_id)

    def test_large_seconds_and_midnight_prefix_are_not_float_or_clock_joins(self):
        data = trace().replace(b'ready(0, 167026, 123456789)', b'ready(1, 7, 999999999)')
        data = data.replace(b'[03:59:59.123456]', b'[23:59:59.999999]', 24)
        data = data.replace(b'[03:59:59.123456]', b'[00:00:00.000001]')
        timestamp = ((1 << 32) | 7) * 1_000_000_000 + 999999999
        result = capture_receipt(metadata(data, self.image, timestamp - 1, timestamp + 1), data, self.image)
        self.assertEqual(result["protocol"]["compositor_commit_monotonic_ns"], timestamp)
        self.assertIs(type(result["protocol"]["compositor_commit_monotonic_ns"]), int)
        queued = trace().replace(b'] ', b'] {Capture Queue} ')
        self.assertEqual(self.verify(queued)["protocol"]["events"][0]["queue"], "Capture Queue")

    def test_protocol_rejections_are_not_hidden_by_updated_hashes(self):
        replacements = [
            ('bind(12, "zwlr_screencopy_manager_v1", 1,', 'bind(12, "zwlr_screencopy_manager_v1", 3,'),
            ('wl_output#5.name("HEADLESS-1")', 'wl_output#5.name("HEADLESS-2")'),
            ('wl_output#5.scale(1)', 'wl_output#5.scale(2)'),
            ('"headless", "headless", 0)', '"headless", "headless", 2)'),
            ('mode(1, 1280, 720,', 'mode(1, 720, 1280,'),
            ('0, wl_output#5)', '1, wl_output#5)'),
            ('0, wl_output#5)', '0, wl_output#99)'),
            ('frame_v1#7.buffer(1,', 'frame_v1#7.buffer(875713112,'),
            ('frame_v1#7.buffer(1, 1280, 720, 5120)', 'frame_v1#7.buffer(1, 1280, 720, 5124)'),
            ('5120, 1)', '5120, 0)'),
            ('copy(wl_buffer#9)', 'copy(wl_buffer#8)'),
            ('new id wl_buffer#9', 'new id wl_buffer#7'),
            ('frame_v1#7.flags(0)', 'frame_v1#7.flags(1)'),
            ('ready(0, 167026, 123456789)', 'ready(0, 167026, 1000000000)'),
            ('ready(0, 167026, 123456789)', 'ready(4294967296, 167026, 1)'),
            ('ready(0, 167026, 123456789)', 'ready(-1, 167026, 1)'),
            ('ready(0, 167026, 123456789)', 'ready(0, 167026, 1, 2)'),
            ('frame_v1#7.ready(', 'frame_v1#99.ready('),
            ('zwlr_screencopy_frame_v1#7.ready(', 'discarded zwlr_screencopy_frame_v1#7.ready('),
            ('frame_v1#7.ready(0, 167026, 123456789)', 'frame_v1#7.failed()'),
            ('wl_shm#4.format(1)', 'wl_shm#4.format(+1)'),
            ('bind(11, "wl_output", 4,', 'bind(99, "wl_output", 4,'),
            ('[03:59:59.123456]', '[24:00:00.000000]'),
            ('[03:59:59.123456]', '[167026123.456]'),
            ('wl_buffer#9.release()', 'wl_buffer#10.release()'),
            ('frame_v1#7.destroy()', 'frame_v1#7.destroy(1)'),
            ('wl_output#5.release()', 'wl_output#5.release(1)'),
        ]
        for before, after in replacements:
            with self.subTest(before=before, after=after):
                data = trace().replace(before.encode(), after.encode())
                with self.assertRaises(ValueError):
                    self.verify(data)

    def test_missing_duplicate_second_output_and_reordered_operations_reject(self):
        for index in (0, 4, 6, 12, 18, 19, 20, 21, 22, 23, 24, 25, 27, 28, 29, 30):
            with self.subTest(missing=index), self.assertRaises(ValueError):
                self.verify(trace(TRACE_LINES[:index] + TRACE_LINES[index + 1:]))
            with self.subTest(duplicate=index), self.assertRaises(ValueError):
                self.verify(trace(TRACE_LINES[:index] + [TRACE_LINES[index]] + TRACE_LINES[index:]))
        additions = [
            'wl_registry#2.global(20, "wl_output", 4)',
            ' -> zwlr_screencopy_manager_v1#6.capture_output_region(new id zwlr_screencopy_frame_v1#20, 0, wl_output#5, 0, 0, 1280, 720)',
            'zwlr_screencopy_frame_v1#7.buffer_done()',
            ' -> ext_image_copy_capture_manager_v1#20.create_session(new id ext_image_copy_capture_session_v1#21, ext_image_capture_source_v1#22, 0)',
            'wl_display#1.error(wl_output#5, 1, "failed")',
        ]
        for addition in additions:
            with self.subTest(addition=addition), self.assertRaises(ValueError):
                self.verify(trace([*TRACE_LINES, addition]))
        for left, right in ((18, 19), (19, 23), (23, 25), (24, 25), (27, 28)):
            lines = list(TRACE_LINES)
            lines[left], lines[right] = lines[right], lines[left]
            with self.subTest(reordered=(left, right)), self.assertRaises(ValueError):
                self.verify(trace(lines))

    def test_supplied_provenance_hashes_clock_bracket_and_command_reject(self):
        data = trace()
        changes = [
            {"schema": True}, {"schema": 2}, {"phase": "other"}, {"ordinal": 0}, {"ordinal": 5},
            {"ordinal": True}, {"native_pid": 5678}, {"grim_pid": 0}, {"returncode": 1},
            {"returncode": False}, {"monotonic_start_ns": 167026123456790},
            {"monotonic_return_ns": 167026123456788},
            {"monotonic_start_ns": 14_399_123_456_000, "monotonic_return_ns": 14_399_123_456_001},
            {"capture_stack_sha256": {}},
            {"environment": {"WAYLAND_DEBUG": "1"}},
        ]
        for change in changes:
            with self.subTest(change=change), self.assertRaises(ValueError):
                capture_receipt({**metadata(data, self.image), **change}, data, self.image)
        for key in ("image", "stderr"):
            for field, value in (("sha256", "0" * 64), ("bytes", 1), ("path", "relative")):
                invalid = metadata(data, self.image)
                invalid[key][field] = value
                with self.subTest(key=key, field=field), self.assertRaises(ValueError):
                    capture_receipt(invalid, data, self.image)
        for index, value in ((0, "grim"), (2, "png"), (4, "HEADLESS-2"), (5, "-c"), (6, "/tmp/another.ppm")):
            invalid = metadata(data, self.image)
            invalid["argv"][index] = value
            with self.subTest(argv=index), self.assertRaises(ValueError):
                capture_receipt(invalid, data, self.image)

    def test_input_bounds_truncation_and_p6_framing_reject(self):
        for data in (b"", trace()[:-1], trace().replace(b'\n', b'\r\n'), trace() + b"noise\n", b"x" * (MAX_TRACE_BYTES + 1),
                     b"x" * (MAX_LINE_BYTES + 1) + b"\n", b"\xff\n", trace().replace(b'[', b'\x1b[31m[', 1),
                     (b'[00:00:00.000000] wl_callback#3.done(0)\n' * (MAX_TRACE_LINES + 1))):
            with self.subTest(data_prefix=data[:30]), self.assertRaises(ValueError):
                self.verify(data)
        for image in (self.image[:-1], self.image + b"x", self.image.replace(b'255\n', b'256\n', 1),
                      self.image.replace(b'P6\n', b'P6\n#comment\n', 1)):
            with self.subTest(image_prefix=image[:30]), self.assertRaises(ValueError):
                self.verify(trace(), image)
        with self.assertRaises(ValueError):
            json.loads('{"schema":1,"schema":1}', object_pairs_hook=unique_object)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input"
            path.write_bytes(b"12345")
            with self.assertRaises(ValueError):
                read_bounded(path, 4)

    def test_offline_cli_keeps_inputs_and_validates_supplied_paths(self):
        data = trace()
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            stderr, image, manifest = directory / "capture.stderr", directory / "capture.ppm", directory / "metadata.json"
            stderr.write_bytes(data)
            image.write_bytes(self.image)
            supplied = metadata(data, self.image)
            supplied["stderr"]["path"] = str(stderr)
            supplied["image"]["path"] = str(image)
            supplied["argv"][-1] = str(image)
            manifest.write_text(json.dumps(supplied))
            original = {path: path.read_bytes() for path in (stderr, image, manifest)}
            argv = ["grim_capture_receipt.py", "--metadata", str(manifest), "--stderr", str(stderr), "--ppm", str(image)]
            with patch("sys.argv", argv), redirect_stdout(io.StringIO()) as output:
                main()
            self.assertTrue(json.loads(output.getvalue())["protocol_receipt_valid"])
            self.assertEqual(original, {path: path.read_bytes() for path in original})
            wrong = directory / "wrong.stderr"
            wrong.write_bytes(data)
            with patch("sys.argv", [*argv[:4], str(wrong), *argv[5:]]), self.assertRaises(ValueError):
                main()


if __name__ == "__main__":
    unittest.main()
