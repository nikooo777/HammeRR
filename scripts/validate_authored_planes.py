import argparse
import json
import struct
from pathlib import Path

from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail


def records(data, lump, layout, expected_version=0):
    offset, length, version, compression = struct.unpack_from(
        "<4i", data, 8 + lump * 16
    )
    assert version == expected_version and compression == 0
    assert 0 <= offset <= offset + length <= len(data)
    assert length % struct.calcsize(layout) == 0
    return list(struct.iter_unpack(layout, data[offset : offset + length]))


def dot(normal, point):
    return sum(a * b for a, b in zip(normal, point, strict=True))


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    for label, transform in [
        ("near-axis", {"rotation": [0, 0, 0.1]}),
        (
            "reflected",
            {"rotation": [-0.1, 0, 0], "reflection": [True, False, False]},
        ),
        (
            "sheared",
            {"shear": {"axis": "x", "reference": "y", "factor": -0.001}},
        ),
    ]:
        client.call("new_map", {"discard_changes": True})
        for mins, maxs in [
            ([-272, -272, -16], [272, 272, 0]),
            ([-272, -272, 256], [272, 272, 272]),
            ([-272, -272, 0], [-256, 272, 256]),
            ([256, -272, 0], [272, 272, 256]),
            ([-256, -272, 0], [256, -256, 256]),
            ([-256, 256, 0], [256, 272, 256]),
        ]:
            client.call(
                "create_block",
                {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"},
            )
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [-64, 0, 8]},
        )
        block = client.call(
            "create_block",
            {
                "mins": [0, 0, 64],
                "maxs": [64, 64, 128],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]
        solid = int(block.split(":")[1])
        owner = client.call(
            "tie_to_entity", {"ids": [block], "classname": "func_brush"}
        )["id"]
        client.call("transform_objects", {"ids": [owner], **transform})
        vmf = output / f"{label}.vmf"
        client.call("save_map", {"path": str(vmf)})
        client.call("open_map", {"path": str(vmf)})
        saved = vmf.read_bytes()
        before = detail(client, [owner])
        history = client.call("get_map_info")["undo"]
        shape = client.call("get_brush_components", {"solids": [solid]})["brushes"][0]
        points = [vertex["position"] for vertex in shape["vertices"]]
        by_side = {
            face["side"]: [points[index] for index in face["vertices"]]
            for face in shape["faces"]
        }
        bsp = output / f"{label}.bsp"
        if label == "sheared":
            client.call("start_compile", {"path": str(bsp)})
            job = wait_job(client)
            check(
                job["status"] == "completed",
                f"{label}: background compilation completes",
            )
            compiled = job["result"]
        else:
            compiled = client.call("compile_map", {"path": str(bsp)})
        check(
            compiled["success"] and compiled["leak"] is None,
            f"{label}: sealed map compiles through MCP",
        )
        check(
            detail(client, [owner]) == before
            and client.call("get_map_info")["undo"] == history
            and vmf.read_bytes() == saved,
            f"{label}: compilation preserves map, materials, projections and history",
        )
        data = bsp.read_bytes()
        check(data[:8] == b"VBSP\x14\x00\x00\x00", f"{label}: native BSP v20 published")
        planes = records(data, 1, "<4fi")
        brushes = records(data, 18, "<3i")
        sides = records(data, 19, "<HhhBB")
        check(len(brushes) == 7, f"{label}: room and brush submodel retained")
        first, count, _ = brushes[-1]
        authored = [side for side in sides[first : first + count] if side[3] == 0]
        source = before[0]["brushes"][0]["faces"]
        check(len(authored) == len(source) == 6, f"{label}: all authored sides retained")
        for native, face in zip(authored, source, strict=True):
            plane_index = native[0]
            normal = planes[plane_index][:3]
            distance = planes[plane_index][3]
            check(
                max(
                    abs(dot(normal, point) - distance)
                    for point in by_side[face["side"]]
                )
                <= 0.01,
                f"{label}: side {face['side']} fits the whole authored face",
            )
            check(
                abs(max(dot(normal, point) for point in points) - distance) <= 0.01,
                f"{label}: side {face['side']} supports the complete brush",
            )
            twin = planes[plane_index ^ 1]
            check(
                all(
                    a == -b
                    for a, b in zip(planes[plane_index][:4], twin[:4], strict=True)
                )
                and planes[plane_index][4] == twin[4],
                f"{label}: side {face['side']} retains its opposite plane",
            )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-authored-planes-mcp"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output)
        result = {"passed": len(checks), "checks": checks}
        (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"passed": len(checks)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
