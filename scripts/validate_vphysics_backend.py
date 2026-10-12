#!/usr/bin/env python3
"""Probe the verified CS:S Linux64 collision ABI in an isolated child."""

import argparse
import ctypes
import hashlib
import json
import math
import os
import resource
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

LIBRARY_SHA256 = "2676cdc02be4b4188f7121c201d5f5a681f9c1c7c924fe7f9159cefd5a718277"
BUILD_ID = "ab4c2d0c4c56edf61af5a7501907d1b6aba51324"
MEMORY_LIMIT = 24 * 1024**3
MAX_COLLISION_BYTES = 16 * 1024**2
GUARD_BYTES = 64
POINTER = ctypes.c_void_p
FLOAT = ctypes.c_float
INTEGER = ctypes.c_int


class Vector(ctypes.Structure):
    _fields_ = [("x", FLOAT), ("y", FLOAT), ("z", FLOAT)]

    def values(self):
        return [self.x, self.y, self.z]


VECTOR_POINTER = ctypes.POINTER(Vector)
SLOTS = {
    "from_vertices": (2, 0x23030, POINTER, [ctypes.POINTER(VECTOR_POINTER), INTEGER]),
    "from_planes": (3, 0x20D00, POINTER, [ctypes.POINTER(FLOAT), INTEGER, FLOAT]),
    "convex_volume": (4, 0x21B00, FLOAT, [POINTER]),
    "convex_area": (5, 0x223C0, FLOAT, [POINTER]),
    "set_game_data": (6, 0x20AA0, None, [POINTER, ctypes.c_uint]),
    "convex_free": (7, 0x20C90, None, [POINTER]),
    "bbox_convex": (8, 0x230D0, POINTER, [VECTOR_POINTER, VECTOR_POINTER]),
    "convert": (15, 0x20980, POINTER, [ctypes.POINTER(POINTER), INTEGER]),
    "destroy": (17, 0x227B0, None, [POINTER]),
    "size": (18, 0x21CE0, INTEGER, [POINTER]),
    "write": (19, 0x20A90, INTEGER, [POINTER, POINTER, ctypes.c_bool]),
    "read": (20, 0x23A00, POINTER, [POINTER, INTEGER, INTEGER]),
    "volume": (21, 0x23F90, FLOAT, [POINTER]),
    "area": (22, 0x24050, FLOAT, [POINTER]),
    "aabb": (
        24,
        0x21A70,
        None,
        [VECTOR_POINTER, VECTOR_POINTER, POINTER, VECTOR_POINTER, VECTOR_POINTER],
    ),
    "index": (29, 0x21CB0, INTEGER, [POINTER]),
    "load": (37, 0x23A10, None, [POINTER, INTEGER, POINTER, INTEGER, ctypes.c_bool]),
    "unload": (38, 0x22660, None, [POINTER]),
    "query": (43, 0x24180, POINTER, [POINTER]),
    "destroy_query": (44, 0x22860, None, [POINTER]),
    "thread_create": (45, 0x20AB0, POINTER, []),
    "thread_destroy": (46, 0x20AC0, None, [POINTER]),
    "virtual_mesh": (48, 0x24550, ctypes.c_bool, []),
}
QUERY_SLOTS = {
    "count": (2, 0x20B00, INTEGER, []),
    "triangles": (3, 0x20B10, INTEGER, [INTEGER]),
    "game_data": (4, 0x20B30, ctypes.c_uint, [INTEGER]),
    "vertices": (5, 0x20B50, None, [INTEGER, INTEGER, VECTOR_POINTER]),
}


class VCollide(ctypes.Structure):
    _fields_ = [
        ("count_and_packed", ctypes.c_ushort),
        ("description_size", ctypes.c_ushort),
        ("solids", ctypes.POINTER(POINTER)),
        ("keyvalues", POINTER),
    ]


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def near(actual, expected, label):
    require(
        math.isfinite(actual)
        and math.isclose(actual, expected, rel_tol=2e-5, abs_tol=0.01),
        f"{label}: {actual} != {expected}",
    )


class Collision:
    def __init__(self, directory):
        library_path = directory / "vphysics.so"
        digest = hashlib.sha256(library_path.read_bytes()).hexdigest()
        require(
            digest == LIBRARY_SHA256,
            "Unverified vphysics build: inspect its ABI before updating this probe",
        )
        self.tier0 = ctypes.CDLL(
            str(directory / "libtier0.so"), mode=ctypes.RTLD_GLOBAL
        )
        self.library = ctypes.CDLL(str(library_path))
        factory = self.library.CreateInterface
        factory.argtypes = [ctypes.c_char_p, ctypes.POINTER(INTEGER)]
        factory.restype = POINTER
        status = INTEGER(-1)
        self.pointer = factory(b"VPhysicsCollision007", ctypes.byref(status))
        require(self.pointer and status.value == 0, "VPhysicsCollision007 unavailable")
        for name in (b"VPhysicsCollision006", b"VPhysicsCollision008"):
            require(
                not factory(name, ctypes.byref(status)) and status.value == 1,
                "Unexpected interface registration",
            )
        self.base = next(
            int(line.split("-")[0], 16)
            for line in Path("/proc/self/maps").read_text().splitlines()
            if line.endswith(str(library_path)) and line.split()[2] == "00000000"
        )
        self.functions = self.bind(self.pointer, SLOTS)

    def bind(self, pointer, slots):
        vtable = POINTER.from_address(pointer).value
        functions = {}
        for name, (slot, offset, result, arguments) in slots.items():
            address = POINTER.from_address(vtable + slot * ctypes.sizeof(POINTER)).value
            require(address == self.base + offset, f"Unexpected method at slot {slot}")
            functions[name] = ctypes.CFUNCTYPE(result, POINTER, *arguments)(address)
        return functions

    def call(self, name, *arguments):
        return self.functions[name](self.pointer, *arguments)

    def serialize(self, collide):
        size = self.call("size", collide)
        require(28 <= size <= MAX_COLLISION_BYTES, f"Invalid collision size {size}")
        guarded = ctypes.create_string_buffer(b"\xa5" * (size + 2 * GUARD_BYTES))
        written = self.call(
            "write", ctypes.addressof(guarded) + GUARD_BYTES, collide, False
        )
        require(written == size, "Serialization size mismatch")
        require(
            guarded.raw[:GUARD_BYTES] == b"\xa5" * GUARD_BYTES,
            "Leading serialization guard changed",
        )
        require(
            guarded.raw[GUARD_BYTES + size : GUARD_BYTES * 2 + size]
            == b"\xa5" * GUARD_BYTES,
            "Trailing serialization guard changed",
        )
        data = guarded.raw[GUARD_BYTES : GUARD_BYTES + size]
        magic, version, kind, body_size = struct.unpack_from("<IHHI", data)
        require(
            (magic, version, kind) == (0x59485056, 0x100, 0), "Unexpected VPHY header"
        )
        require(body_size == size - 28, "VPHY payload size mismatch")
        return data


def box_roundtrip(physics, low, high, index):
    mins, maxs = Vector(*low), Vector(*high)
    sides = [b - a for a, b in zip(low, high)]
    volume = math.prod(sides)
    area = 2 * (sides[0] * sides[1] + sides[0] * sides[2] + sides[1] * sides[2])
    convex = physics.call("bbox_convex", ctypes.byref(mins), ctypes.byref(maxs))
    require(convex, "Convex construction failed")
    collide = restored = None
    try:
        near(physics.call("convex_volume", convex), volume, "Convex volume")
        near(physics.call("convex_area", convex), area, "Convex area")
        convexes = (POINTER * 1)(convex)
        collide = physics.call("convert", convexes, 1)
        convex = None
        require(collide, "Collision conversion failed")
        data = physics.serialize(collide)
        require(physics.serialize(collide) == data, "Repeated serialization differs")
        buffer = ctypes.create_string_buffer(data)
        restored = physics.call("read", buffer, len(data), index)
        require(restored, "Collision deserialization failed")
        require(physics.call("index", restored) == index, "Deserialization index lost")
        for body in (collide, restored):
            near(physics.call("volume", body), volume, "Collision volume")
            near(physics.call("area", body), area, "Collision area")
            out_min, out_max = Vector(), Vector()
            origin, angles = Vector(128, -256, 64), Vector()
            physics.call(
                "aabb",
                ctypes.byref(out_min),
                ctypes.byref(out_max),
                body,
                ctypes.byref(origin),
                ctypes.byref(angles),
            )
            for axis, (actual, expected) in enumerate(
                zip(
                    out_min.values() + out_max.values(),
                    [a + b for a, b in zip(low + high, origin.values() * 2)],
                )
            ):
                near(actual, expected, f"Translated AABB {axis}")
            angles.y = 90
            physics.call(
                "aabb",
                ctypes.byref(out_min),
                ctypes.byref(out_max),
                body,
                ctypes.byref(origin),
                ctypes.byref(angles),
            )
            rotated = [-high[1], low[0], low[2], -low[1], high[0], high[2]]
            for axis, (actual, expected) in enumerate(
                zip(
                    out_min.values() + out_max.values(),
                    [a + b for a, b in zip(rotated, origin.values() * 2)],
                )
            ):
                near(actual, expected, f"Rotated AABB {axis}")
        return {
            "mins": low,
            "maxs": high,
            "volume": volume,
            "area": area,
            "bytes": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
            "header_hex": data[:28].hex(),
        }
    finally:
        if restored:
            physics.call("destroy", restored)
        if collide:
            physics.call("destroy", collide)
        if convex:
            physics.call("convex_free", convex)


def query_shapes(physics, collide, expected_tags):
    query = physics.call("query", collide)
    require(query, "Query construction failed")
    try:
        calls = physics.bind(query, QUERY_SLOTS)
        count = calls["count"](query)
        require(count == len(expected_tags), "Convex count changed")
        tags = []
        triangles = 0
        for convex in range(count):
            tag = calls["game_data"](query, convex)
            tags.append(tag)
            require(tag in expected_tags, "Unexpected convex game data")
            offset = 32 if tag == 2 else 0
            corners = set()
            count_triangles = calls["triangles"](query, convex)
            require(count_triangles == 12, "Box triangle count changed")
            triangles += count_triangles
            for triangle in range(count_triangles):
                points = (Vector * 3)()
                calls["vertices"](query, convex, triangle, points)
                require(
                    all(
                        math.isfinite(value)
                        for point in points
                        for value in point.values()
                    ),
                    "Nonfinite query vertex",
                )
                for point in points:
                    for value, low in zip(point.values(), [offset, 0, 0]):
                        require(
                            math.isclose(value, low, abs_tol=0.001)
                            or math.isclose(value, low + 16, abs_tol=0.001),
                            "Query vertex left its box corners",
                        )
                    corners.add(tuple(round(value, 3) for value in point.values()))
            require(len(corners) == 8, "Query lost a box corner")
        require(sorted(tags) == sorted(expected_tags), "Per-convex game data lost")
        return {"convexes": count, "triangles": triangles, "game_data": sorted(tags)}
    finally:
        physics.call("destroy_query", query)


def compound_roundtrip(physics):
    convexes = []
    collide = restored = None
    container = VCollide()
    loaded = False
    try:
        for offset, tag in ((0, 1), (32, 2)):
            low, high = Vector(offset, 0, 0), Vector(offset + 16, 16, 16)
            convex = physics.call("bbox_convex", ctypes.byref(low), ctypes.byref(high))
            require(convex, "Compound convex construction failed")
            convexes.append(convex)
            physics.call("set_game_data", convex, tag)
        inputs = (POINTER * len(convexes))(*convexes)
        collide = physics.call("convert", inputs, len(convexes))
        convexes.clear()
        require(collide, "Compound conversion failed")
        near(physics.call("volume", collide), 8192, "Compound volume")
        near(physics.call("area", collide), 3072, "Compound area")
        data = physics.serialize(collide)
        query_shapes(physics, collide, [1, 2])
        buffer = ctypes.create_string_buffer(data)
        restored = physics.call("read", buffer, len(data), 3)
        require(restored, "Compound deserialization failed")
        result = query_shapes(physics, restored, [1, 2])
        keyvalues = b'staticsolid { "index" "0" "contents" "1" }\n\0'
        payload = struct.pack("<i", len(data)) + data + keyvalues
        packed = ctypes.create_string_buffer(payload)
        require(ctypes.sizeof(VCollide) == 24, "Unexpected vcollide_t layout")
        physics.call("load", ctypes.byref(container), 1, packed, len(payload), False)
        loaded = True
        require(
            container.count_and_packed == 1 and container.solids, "VCollide load failed"
        )
        require(
            ctypes.string_at(container.keyvalues, len(keyvalues)) == keyvalues,
            "VCollide key data changed",
        )
        require(
            physics.call("index", container.solids[0]) == 0,
            "VCollide solid index changed",
        )
        query_shapes(physics, container.solids[0], [1, 2])
        result.update(
            {
                "bytes": len(data),
                "vcollide_payload_bytes": len(payload),
                "keyvalues": keyvalues[:-1].decode(),
            }
        )
        return result
    finally:
        if loaded:
            physics.call("unload", ctypes.byref(container))
        if restored:
            physics.call("destroy", restored)
        if collide:
            physics.call("destroy", collide)
        for convex in convexes:
            physics.call("convex_free", convex)


def construction_methods(physics):
    low, high = [-32, -16, 8], [64, 48, 40]
    plane_values = (
        (1, 0, 0, high[0]),
        (-1, 0, 0, -low[0]),
        (0, 1, 0, high[1]),
        (0, -1, 0, -low[1]),
        (0, 0, 1, high[2]),
        (0, 0, -1, -low[2]),
    )
    planes = (FLOAT * (len(plane_values) * 4))(
        *(value for plane in plane_values for value in plane)
    )
    convex = physics.call("from_planes", planes, 6, 0.25)
    require(convex, "Plane convex construction failed")
    try:
        near(physics.call("convex_volume", convex), 196608, "Plane box volume")
        near(physics.call("convex_area", convex), 22528, "Plane box area")
    finally:
        physics.call("convex_free", convex)
    points = [Vector(0, 0, 0), Vector(32, 0, 0), Vector(0, 48, 0), Vector(0, 0, 64)]
    vertices = (VECTOR_POINTER * len(points))(
        *(ctypes.pointer(point) for point in points)
    )
    convex = physics.call("from_vertices", vertices, len(points))
    require(convex, "Vertex convex construction failed")
    try:
        near(physics.call("convex_volume", convex), 16384, "Tetrahedron volume")
        area = 3328 + math.sqrt(3072**2 + 2048**2 + 1536**2) / 2
        near(physics.call("convex_area", convex), area, "Tetrahedron area")
        return {
            "plane_box_volume": 196608,
            "vertex_tetrahedron_volume": 16384,
            "vertex_tetrahedron_area": area,
        }
    finally:
        physics.call("convex_free", convex)


def worker(directory, output):
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    resource.setrlimit(resource.RLIMIT_AS, (MEMORY_LIMIT, MEMORY_LIMIT))
    physics = Collision(directory)
    context = physics.call("thread_create")
    require(context == physics.pointer, "Thread context behavior changed")
    physics.call("thread_destroy", context)
    cases = [
        ([-32, -16, 8], [64, 48, 40]),
        ([-1, -2, -3], [1, 2, 3]),
        ([128, 256, -64], [160, 320, 0]),
    ]
    results = [
        box_roundtrip(physics, low, high, index + 17)
        for index, (low, high) in enumerate(cases)
    ]
    constructors = construction_methods(physics)
    compound = compound_roundtrip(physics)
    for index in range(128):
        box_roundtrip(physics, *cases[index % len(cases)], index)
    result = {
        "interface": "VPhysicsCollision007",
        "build_id": BUILD_ID,
        "library_sha256": LIBRARY_SHA256,
        "cases": results,
        "constructors": constructors,
        "compound": compound,
        "repeat_cycles": 128,
        "thread_context_is_singleton": True,
        "supports_virtual_mesh": physics.call("virtual_mesh"),
        "max_rss_kib": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
    }
    output.write_text(json.dumps(result, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    parser.add_argument("--worker-output", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    directory = (args.game_dir / "bin/linux64").resolve()
    if args.worker_output:
        worker(directory, args.worker_output)
        return
    require(
        sys.platform == "linux" and ctypes.sizeof(POINTER) == 8,
        "This probe requires Linux64",
    )
    env = os.environ.copy()
    env["LD_LIBRARY_PATH"] = str(directory)
    with tempfile.TemporaryDirectory(prefix="hammerr-vphysics-") as temporary:
        output = Path(temporary) / "result.json"
        result = subprocess.run(
            [
                sys.executable,
                "-B",
                str(Path(__file__).resolve()),
                "--game-dir",
                str(args.game_dir.resolve()),
                "--worker-output",
                str(output),
            ],
            check=False,
            env=env,
            capture_output=True,
            text=True,
            timeout=20,
        )
        require(
            result.returncode == 0,
            f"VPhysics probe failed ({result.returncode}): {result.stdout}{result.stderr}",
        )
        print(output.read_text(), end="")


if __name__ == "__main__":
    main()
