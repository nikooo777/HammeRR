import argparse
import hashlib
import json
import re
from dataclasses import dataclass
from pathlib import Path

SCHEMA = 1
WIDTH = 1280
HEIGHT = 720
STRIDE = WIDTH * 4
MAX_METADATA_BYTES = 64 * 1024
MAX_TRACE_BYTES = 1024 * 1024
MAX_TRACE_LINES = 8192
MAX_LINE_BYTES = 4096
MAX_IMAGE_BYTES = 3 * 1024 * 1024
UINT32_MAX = (1 << 32) - 1
UINT64_MAX = (1 << 64) - 1
FORMATS = {0: "argb8888", 1: "xrgb8888"}
STACK_SHA256 = {
    "grim": "d1c823ea8e341f179a3c864eb063653f1c01f017e2d427429ee134186fe034bf",
    "cage": "d4a28d22fc33f8e1333f5ea382e6116743540ef605c005f70c3078891edf8e64",
    "wlroots": "7edca9a7d65855addda68e29ba5db94d57c803eb5fa7d0f86db3136b90ca9837",
    "wayland_client": "39c83ae7b73d22f18b4f81da14749c4535c304b7e62910a2f1aae682f279cc9d",
}
MANAGER = "zwlr_screencopy_manager_v1"
FRAME = "zwlr_screencopy_frame_v1"
UINT = r"(0|[1-9][0-9]{0,9})"
OBJECT_ID = r"([1-9][0-9]{0,9})"
EVENT = re.compile(
    r"\[([0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{6})\] "
    r"(?:\{([^{}\r\n]{1,64})\} )?(discarded )?( -> )?"
    r"([a-z][a-z0-9_]{0,79})#" + OBJECT_ID
    + r"\.([a-z][a-z0-9_]{0,79})\(([^\r\n]*)\)"
)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value, maximum=UINT64_MAX):
    require(type(value) is int and 0 <= value <= maximum, "Invalid integer")
    return value


def numbers(pattern, text):
    match = re.fullmatch(pattern, text)
    require(match is not None, f"Malformed protocol arguments: {text[:160]}")
    return match.groups()


def u32(value):
    require(type(value) is str and re.fullmatch(UINT, value) is not None, "Malformed uint32")
    return integer(int(value), UINT32_MAX)


def digest(data):
    return hashlib.sha256(data).hexdigest()


@dataclass(frozen=True)
class Event:
    line: int
    realtime: str
    queue: str | None
    discarded: bool
    request: bool
    interface: str
    object_id: int
    method: str
    arguments: str

    def receipt(self):
        return {
            "line": self.line,
            "debug_realtime_prefix": self.realtime,
            "debug_prefix_clock": "CLOCK_REALTIME",
            "queue": self.queue,
            "discarded": self.discarded,
            "request": self.request,
            "interface": self.interface,
            "object_id": self.object_id,
            "method": self.method,
            "arguments": self.arguments,
        }


def read_events(data):
    require(type(data) is bytes and 0 < len(data) <= MAX_TRACE_BYTES, "Invalid trace size")
    require(data.endswith(b"\n"), "Incomplete trace line")
    text = data.decode("utf-8", errors="strict")
    lines = text.split("\n")[:-1]
    require(0 < len(lines) <= MAX_TRACE_LINES, "Invalid trace line count")
    events = []
    for index, line in enumerate(lines, 1):
        require(0 < len(line.encode()) <= MAX_LINE_BYTES, "Invalid trace line size")
        match = EVENT.fullmatch(line)
        require(match is not None, f"Unsupported or malformed trace line {index}")
        time, queue, discarded, request, interface, object_id, method, arguments = match.groups()
        hour, minute, second = map(int, (time[:2], time[3:5], time[6:8]))
        require(hour < 24 and minute < 60 and second < 60, "Invalid realtime prefix")
        require(not any(ord(char) < 32 or ord(char) == 127 for char in line), "Control byte in trace")
        events.append(Event(index, time, queue, bool(discarded), bool(request), interface, u32(object_id), method, arguments))
    return events


def one(events, interface, method, request, object_id=None):
    matches = [event for event in events if event.interface == interface
               and event.method == method and event.request is request
               and (object_id is None or event.object_id == object_id)]
    require(len(matches) == 1, f"Expected one {interface}.{method}, found {len(matches)}")
    return matches[0]


def ordered(*events):
    lines = [event.line for event in events]
    require(all(left < right for left, right in zip(lines, lines[1:])), "Protocol events out of order")


def parse_trace(data, output_name):
    events = read_events(data)
    registry_event = one(events, "wl_display", "get_registry", True, 1)
    registry_id, = numbers("new id wl_registry#" + OBJECT_ID, registry_event.arguments)
    registry_id = u32(registry_id)
    globals_by_name = {}
    bindings = {}
    bound_ids = {1, registry_id}
    evidence = [registry_event]
    for event in events:
        if event.interface == "wl_display":
            require(event.object_id == 1, "Unbound display object")
        if event.interface == "wl_display" and event.method == "error":
            raise ValueError("Wayland display error")
        if event.interface.startswith("ext_") and "capture" in event.interface:
            raise ValueError("Unsupported ext capture path")
        if event.interface != "wl_registry":
            continue
        require(event.object_id == registry_id, "Multiple registries")
        require((event.method, event.request) in {("global", False), ("bind", True)}, "Unexpected registry operation")
        if event.method == "global" and not event.request:
            name, interface, version = numbers(UINT + r', "([a-z][a-z0-9_]+)", ' + UINT, event.arguments)
            name, version = u32(name), u32(version)
            require(name not in globals_by_name, "Repeated registry global")
            globals_by_name[name] = (interface, version, event)
        elif event.method == "bind" and event.request:
            name, interface, version, object_id = numbers(
                UINT + r', "([a-z][a-z0-9_]+)", ' + UINT + r", new id \[unknown\]#" + OBJECT_ID,
                event.arguments,
            )
            name, version, object_id = u32(name), u32(version), u32(object_id)
            require(name in globals_by_name, "Binding an unadvertised global")
            advertised_interface, advertised_version, advertised_event = globals_by_name[name]
            require(advertised_interface == interface and 0 < version <= advertised_version, "Global binding mismatch")
            require(object_id not in bound_ids, "Reused bound object")
            bound_ids.add(object_id)
            require(interface not in bindings, "Multiple interface bindings")
            bindings[interface] = (object_id, version, name, event)
            ordered(advertised_event, event)
            if interface in {MANAGER, "wl_output", "wl_shm"}:
                evidence.extend([advertised_event, event])
            require(not (interface.startswith("ext_") and "capture" in interface), "Unsupported ext capture binding")
    require(sum(value[0] == "wl_output" for value in globals_by_name.values()) == 1, "Expected one advertised output")
    require(all(interface in bindings for interface in (MANAGER, "wl_output", "wl_shm")), "Missing capture interface")
    manager_id, version, manager_global, manager_bind = bindings[MANAGER]
    output_id, output_version, output_global, output_bind = bindings["wl_output"]
    shm_id, shm_version, _, shm_bind = bindings["wl_shm"]
    require(version == 1, "Unsupported negotiated screencopy version; reviewed Grim binds version 1")
    require(output_version == 4 and shm_version == 1, "Unsupported output or SHM version")
    name_event = one(events, "wl_output", "name", False, output_id)
    require(name_event.arguments == json.dumps(output_name), "Output name mismatch")
    scale_event = one(events, "wl_output", "scale", False, output_id)
    require(scale_event.arguments == "1", "Unsupported output scale")
    geometry_event = one(events, "wl_output", "geometry", False, output_id)
    geometry = numbers(r'(-?[0-9]+), (-?[0-9]+), ([0-9]+), ([0-9]+), ([0-9]+), "[^"\r\n]*", "[^"\r\n]*", ([0-9]+)', geometry_event.arguments)
    for value in geometry[2:]:
        u32(value)
    require(int(geometry[0]) == int(geometry[1]) == int(geometry[-1]) == 0, "Unsupported output geometry or transform")
    mode_event = one(events, "wl_output", "mode", False, output_id)
    flags, width, height, refresh = map(u32, numbers(UINT + ", " + UINT + ", " + UINT + ", " + UINT, mode_event.arguments))
    require(flags in (1, 3) and (width, height) == (WIDTH, HEIGHT), "Unsupported output mode")
    capture = one(events, MANAGER, "capture_output", True)
    require(capture.object_id == manager_id, "Capture uses an unbound manager")
    frame_id, cursor, capture_output = map(u32, numbers("new id " + FRAME + "#" + OBJECT_ID + ", " + UINT + ", wl_output#" + OBJECT_ID, capture.arguments))
    require(cursor == 0 and capture_output == output_id, "Capture output or cursor mismatch")
    require(frame_id not in bound_ids, "Reused frame object")
    buffer_event = one(events, FRAME, "buffer", False)
    format_id, width, height, stride = map(u32, numbers(UINT + ", " + UINT + ", " + UINT + ", " + UINT, buffer_event.arguments))
    require(format_id in FORMATS and (width, height, stride) == (WIDTH, HEIGHT, STRIDE), "Unsupported capture buffer")
    announced_formats = [u32(event.arguments) for event in events if event.interface == "wl_shm"
                         and event.object_id == shm_id and event.method == "format" and not event.request]
    require(format_id in announced_formats, "Capture format was not advertised by SHM")
    pool = one(events, "wl_shm", "create_pool", True)
    require(pool.object_id == shm_id, "Pool uses an unbound SHM interface")
    pool_id, fd, pool_bytes = map(u32, numbers("new id wl_shm_pool#" + OBJECT_ID + ", fd " + UINT + ", " + UINT, pool.arguments))
    require(fd <= (1 << 31) - 1 and pool_bytes == STRIDE * HEIGHT, "Invalid SHM pool")
    create_buffer = one(events, "wl_shm_pool", "create_buffer", True)
    require(create_buffer.object_id == pool_id, "Buffer uses another pool")
    buffer_id, offset, buffer_width, buffer_height, buffer_stride, buffer_format = map(u32, numbers(
        "new id wl_buffer#" + OBJECT_ID + ", " + ", ".join([UINT] * 5), create_buffer.arguments))
    require((offset, buffer_width, buffer_height, buffer_stride, buffer_format) == (0, WIDTH, HEIGHT, STRIDE, format_id), "SHM buffer mismatch")
    require(len(bound_ids | {frame_id, pool_id, buffer_id}) == len(bound_ids) + 3, "Reused capture object")
    copy = one(events, FRAME, "copy", True)
    require(copy.arguments == f"wl_buffer#{buffer_id}", "Copy uses another buffer")
    frame_flags = one(events, FRAME, "flags", False)
    require(frame_flags.arguments == "0", "Unsupported frame flags")
    ready = one(events, FRAME, "ready", False)
    hi, lo, nsec = map(u32, numbers(UINT + ", " + UINT + ", " + UINT, ready.arguments))
    require(nsec < 1_000_000_000, "Invalid ready nanoseconds")
    frame_destroy = one(events, FRAME, "destroy", True)
    pool_destroy = one(events, "wl_shm_pool", "destroy", True, pool_id)
    buffer_destroy = one(events, "wl_buffer", "destroy", True, buffer_id)
    manager_destroy = one(events, MANAGER, "destroy", True, manager_id)
    output_release = one(events, "wl_output", "release", True, output_id)
    require(all(event.arguments == "" for event in (frame_destroy, pool_destroy, buffer_destroy, manager_destroy, output_release)), "Malformed teardown arguments")
    releases = [event for event in events if event.interface == "wl_buffer" and event.method == "release"]
    require(len(releases) <= 1, "Repeated buffer release")
    for event in events:
        require(not event.discarded or event.interface == "wl_buffer" and event.method == "release", "Discarded capture event")
        if event.interface == FRAME:
            require(event.object_id == frame_id and (event.method, event.request) in {
                ("buffer", False), ("flags", False), ("ready", False), ("copy", True), ("destroy", True),
            }, "Unexpected frame event or request")
        if event.interface == MANAGER:
            require(event.object_id == manager_id and event.request and event.method in {"capture_output", "destroy"}, "Unexpected capture manager request")
        if event.interface == "wl_output":
            require(event.object_id == output_id, "Unbound output object")
            require((event.method, event.request) in {("geometry", False), ("mode", False), ("scale", False), ("name", False), ("description", False), ("done", False), ("release", True)}, "Unexpected output operation")
            if event.method == "done":
                require(event.arguments == "", "Malformed output done event")
        if event.interface == "wl_shm":
            require(event.object_id == shm_id and (event.method, event.request) in {("format", False), ("create_pool", True)}, "Unexpected SHM operation")
        if event.interface == "wl_shm_pool":
            require(event.object_id == pool_id and event.request and event.method in {"create_buffer", "destroy"}, "Unexpected pool object or request")
        if event.interface == "wl_buffer":
            require(event.object_id == buffer_id and (event.method, event.request) in {("destroy", True), ("release", False)} and event.arguments == "", "Unexpected buffer event or request")
    ordered(registry_event, manager_bind, capture, buffer_event, pool, create_buffer, pool_destroy, copy, frame_flags, ready, frame_destroy, buffer_destroy)
    for event in (name_event, scale_event, geometry_event, mode_event):
        ordered(output_bind, event, capture)
    ordered(shm_bind, pool)
    ordered(buffer_destroy, output_release, manager_destroy)
    for event in releases:
        ordered(ready, event, frame_destroy)
    evidence.extend([name_event, scale_event, geometry_event, mode_event, capture, buffer_event,
                     pool, create_buffer, pool_destroy, copy, frame_flags, ready, frame_destroy,
                     buffer_destroy, output_release, manager_destroy, *releases])
    return {
        "protocol_interface": MANAGER,
        "negotiated_version": version,
        "manager_global": manager_global,
        "manager_object_id": manager_id,
        "output": {"global": output_global, "object_id": output_id, "name": output_name, "version": output_version, "scale": 1, "transform": 0, "refresh_millihz": refresh},
        "frame_object_id": frame_id,
        "buffer": {"object_id": buffer_id, "pool_object_id": pool_id, "format": format_id, "format_name": FORMATS[format_id], "width": WIDTH, "height": HEIGHT, "stride": STRIDE, "flags": 0},
        "ready": {"tv_sec_hi": hi, "tv_sec_lo": lo, "tv_nsec": nsec},
        "compositor_commit_monotonic_ns": ((hi << 32) | lo) * 1_000_000_000 + nsec,
        "compositor_timestamp_clock": "CLOCK_MONOTONIC",
        "events": [event.receipt() for event in sorted(evidence, key=event_line)],
    }


def event_line(event):
    return event.line


def file_record(record, data, maximum):
    require(type(record) is dict and set(record) == {"path", "bytes", "sha256"}, "Invalid file provenance")
    require(type(record["path"]) is str and Path(record["path"]).is_absolute()
            and not any(ord(char) < 32 for char in record["path"]), "Expected absolute artifact path")
    require(type(data) is bytes and 0 < len(data) <= maximum, "Invalid artifact size")
    require(integer(record["bytes"], maximum) == len(data) and record["sha256"] == digest(data), "Artifact length or hash mismatch")


def capture_receipt(metadata, stderr, ppm):
    fields = {"schema", "phase", "ordinal", "native_pid", "grim_pid", "returncode", "monotonic_start_ns", "monotonic_return_ns", "argv", "environment", "image", "stderr", "capture_stack_sha256"}
    require(type(metadata) is dict and set(metadata) == fields, "Invalid capture metadata fields")
    require(type(metadata["schema"]) is int and metadata["schema"] == SCHEMA, "Unsupported metadata schema")
    require(metadata["phase"] in ("on", "off") and integer(metadata["ordinal"], 4) >= 1, "Invalid phase or ordinal")
    require(0 < integer(metadata["native_pid"], (1 << 31) - 1) != integer(metadata["grim_pid"], (1 << 31) - 1) > 0, "Invalid supplied process identities")
    require(type(metadata["returncode"]) is int and metadata["returncode"] == 0, "Capture did not exit successfully")
    start, returned = integer(metadata["monotonic_start_ns"]), integer(metadata["monotonic_return_ns"])
    require(start < returned, "Invalid parent monotonic bracket")
    require(metadata["capture_stack_sha256"] == STACK_SHA256, "Capture stack differs from reviewed installation")
    environment = metadata["environment"]
    require(type(environment) is dict and set(environment) == {"WAYLAND_DEBUG", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"}, "Invalid capture environment provenance")
    require(environment["WAYLAND_DEBUG"] == "client" and type(environment["WAYLAND_DISPLAY"]) is str
            and re.fullmatch(r"wayland-[0-9]+", environment["WAYLAND_DISPLAY"]) is not None, "Unsupported client logging or display")
    require(type(environment["XDG_RUNTIME_DIR"]) is str and Path(environment["XDG_RUNTIME_DIR"]).is_absolute()
            and not any(ord(char) < 32 for char in environment["XDG_RUNTIME_DIR"]), "Expected absolute private runtime directory")
    file_record(metadata["stderr"], stderr, MAX_TRACE_BYTES)
    file_record(metadata["image"], ppm, MAX_IMAGE_BYTES)
    require(metadata["stderr"]["path"] != metadata["image"]["path"], "Artifacts share a path")
    argv = metadata["argv"]
    require(type(argv) is list and len(argv) == 7 and all(type(item) is str for item in argv), "Invalid capture command")
    require(argv[:4] == ["/usr/bin/grim", "-t", "ppm", "-o"] and argv[6] == metadata["image"]["path"], "Capture command mismatch")
    require(argv[5] == "--" and re.fullmatch(r"HEADLESS-[1-9][0-9]*", argv[4]) is not None, "Expected one explicit headless output")
    header = f"P6\n{WIDTH} {HEIGHT}\n255\n".encode()
    require(ppm.startswith(header) and len(ppm) == len(header) + WIDTH * HEIGHT * 3, "Invalid fixed P6 image")
    protocol = parse_trace(stderr, argv[4])
    require(start <= protocol["compositor_commit_monotonic_ns"] <= returned, "Compositor commit outside supplied monotonic bracket")
    return {
        "schema": SCHEMA,
        "diagnostic_only": True,
        "protocol_receipt_valid": True,
        "supplied_provenance": metadata,
        "protocol": protocol,
        "same_native_present_verified": False,
        "native_frame_id": None,
        "native_present_id": None,
        "compositor_commit_seq": None,
    }


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "Duplicate metadata key")
        result[key] = value
    return result


def read_bounded(path, maximum):
    with path.open("rb") as stream:
        data = stream.read(maximum + 1)
    require(0 < len(data) <= maximum, "Invalid input file size")
    return data


def main():
    parser = argparse.ArgumentParser(description="Validate one retained diagnostic Grim capture without launching a client")
    parser.add_argument("--metadata", type=Path, required=True)
    parser.add_argument("--stderr", type=Path, required=True)
    parser.add_argument("--ppm", type=Path, required=True)
    args = parser.parse_args()
    metadata = json.loads(read_bounded(args.metadata, MAX_METADATA_BYTES), object_pairs_hook=unique_object)
    receipt = capture_receipt(metadata, read_bounded(args.stderr, MAX_TRACE_BYTES), read_bounded(args.ppm, MAX_IMAGE_BYTES))
    require(Path(metadata["stderr"]["path"]).resolve(strict=True) == args.stderr.resolve(strict=True), "Stderr path mismatch")
    require(Path(metadata["image"]["path"]).resolve(strict=True) == args.ppm.resolve(strict=True), "Image path mismatch")
    print(json.dumps(receipt, indent=2))


if __name__ == "__main__":
    main()
