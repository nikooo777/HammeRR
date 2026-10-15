import argparse
import itertools
import json
import math
import struct
from pathlib import Path
from unittest.mock import patch

from validate_authored_planes import records
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_static_props import static_records
from validate_terrain_lod import build
from validate_visibility import ROOM

MINS = [-64, -2, -1]
MAXS = [64, 2, 1]


def assets(output):
    game = output / "game"
    (game / "models").mkdir(parents=True)
    materials = game / "materials/audit"
    materials.mkdir(parents=True)
    for name, text in [
        ("solid", "LightmappedGeneric {}"),
        ("hint", "UnlitGeneric { %compilehint 1 }"),
        ("skip", "UnlitGeneric { %compileskip 1 }"),
    ]:
        (materials / f"{name}.vmt").write_text(text)
    data = bytearray(240)
    data[:4] = b"IDST"
    for offset, value in [(4, 48), (76, 240), (152, 16), (224, 1)]:
        struct.pack_into("<i", data, offset, value)
    for offset, value in [(104, MINS), (116, MAXS), (128, MINS), (140, MAXS)]:
        struct.pack_into("<3f", data, offset, *value)
    (game / "models/box.mdl").write_bytes(data)
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text(
        'GameInfo { FileSystem { SearchPaths { Game "|gameinfo_path|." } } }\n'
    )
    return gameinfo


def transformed(point, angles, origin):
    pitch, yaw, roll = map(math.radians, angles)
    x, y, z = point
    y, z = (
        y * math.cos(roll) - z * math.sin(roll),
        y * math.sin(roll) + z * math.cos(roll),
    )
    x, z = (
        x * math.cos(pitch) + z * math.sin(pitch),
        -x * math.sin(pitch) + z * math.cos(pitch),
    )
    x, y = x * math.cos(yaw) - y * math.sin(yaw), x * math.sin(yaw) + y * math.cos(yaw)
    return [x + origin[0], y + origin[1], z + origin[2]]


def dot(a, b):
    return sum(x * y for x, y in zip(a, b, strict=True))


def membership(data, prop):
    nodes = records(data, 5, "<3i6h2H2h")
    planes = records(data, 1, "<4fi")
    leaves = records(data, 10, "<ihh6h4Hhh", expected_version=1)
    head = records(data, 14, "<9f3i")[0][9]
    origin, angles = prop[:3], prop[3:6]
    corners = [
        transformed(point, angles, origin)
        for point in itertools.product(*zip(MINS, MAXS, strict=True))
    ]
    low = [min(point[axis] for point in corners) - 0.01 for axis in range(3)]
    high = [max(point[axis] for point in corners) + 0.01 for axis in range(3)]
    center = [(a + b) / 2 for a, b in zip(low, high, strict=True)]
    radius = [(b - a) / 2 for a, b in zip(low, high, strict=True)]
    pending, broad = [head], set()
    while pending:
        index = pending.pop()
        if index < 0:
            index = -index - 1
            if leaves[index][0] & 1 == 0:
                broad.add(index)
            continue
        node = nodes[index]
        plane = planes[node[0]]
        distance = dot(plane[:3], center) - plane[3]
        extent = dot([abs(value) for value in plane[:3]], radius)
        if distance + extent >= 0:
            pending.append(node[1])
        if distance - extent <= 0:
            pending.append(node[2])
    sampled = set()
    for x, y, z in itertools.product(range(17), range(5), range(3)):
        point = transformed([-64 + x * 8, -2 + y, -1 + z], angles, origin)
        index = head
        while index >= 0:
            node = nodes[index]
            plane = planes[node[0]]
            index = node[1 if dot(plane[:3], point) - plane[3] >= 0 else 2]
        sampled.add(-index - 1)
    return broad, sampled


def validate(client, output):
    for low, high in ROOM:
        client.call(
            "create_block", {"mins": low, "maxs": high, "material": "audit/solid"}
        )
    for low, high, normal in [
        ([20, -256, 0], [24, 256, 256], [-1, 0, 0]),
        ([-256, -20, 0], [256, -16, 256], [0, -1, 0]),
    ]:
        brush = client.call(
            "create_block", {"mins": low, "maxs": high, "material": "audit/skip"}
        )["id"]
        shape = detail(client, [brush])[0]
        side = next(row for row in shape["faces"] if row["normal"] == normal)
        client.call("select_faces", {"faces": [face_ref(shape, side)]})
        client.call("edit_faces", {"material": "audit/hint"})
    client.call(
        "create_entity",
        {"classname": "info_player_terrorist", "origin": [-160, -160, 8]},
    )
    prop = client.call(
        "create_entity",
        {
            "classname": "prop_static",
            "origin": [0.125, -0.25, 64],
            "properties": {"model": "models/box.mdl", "solid": "2"},
        },
    )["id"]
    client.call("set_compile_settings", {"lighting": "fullbright", "physics": "none"})
    report, unrotated = [], None
    for name, angles in [
        ("unrotated", "0 0 0"),
        ("yaw", "0 45 0"),
        ("tilted", "25 45 15"),
        ("full-turn", "0 360 0"),
    ]:
        client.call("set_entity_properties", {"id": prop, "set": {"angles": angles}})
        source = output / f"{name}.vmf"
        client.call("save_map", {"path": str(source)})
        before = detail(client, [prop])
        history = client.call("get_map_info")["undo"]
        camera = client.call("get_camera")
        saved = source.read_bytes()
        path, stats = build(client, output, name)
        data = path.read_bytes()
        native = static_records(data)
        assert len(native["props"]) == stats["static_props"] == 1
        record = native["props"][0]
        emitted = set(native["leaves"][record[7] : record[7] + record[8]])
        broad, sampled = membership(data, record)
        assert sampled <= emitted <= broad
        assert stats["static_prop_leaf_refs"] == len(emitted)
        assert (
            detail(client, [prop]) == before
            and client.call("get_map_info")["undo"] == history
        )
        assert client.call("get_camera") == camera and source.read_bytes() == saved
        if name == "yaw":
            assert len(emitted) < len(broad)
        if name == "unrotated":
            assert emitted == broad
            unrotated = native
        if name == "full-turn":
            assert native == unrotated
        client.call("open_map", {"path": str(source)})
        reopened, _ = build(client, output, f"{name}-reopened")
        assert reopened.read_bytes() == data
        report.append(
            {
                "case": name,
                "aabb_leaves": len(broad),
                "box_leaves": len(emitted),
                "sampled_leaves": len(sampled),
            }
        )
    (output / "results.json").write_text(
        json.dumps({"cases": report, "desktop_input": False}, indent=2) + "\n"
    )
    return report


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-static-prop-leaves")
    )
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    gameinfo = assets(args.output)
    with patch.dict(
        "os.environ",
        {
            "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"),
            "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
        },
    ):
        client = Client(None, args.binary.resolve(), args.output, gameinfo=gameinfo)
    try:
        report = validate(client, args.output)
        print(json.dumps({"cases": len(report), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
