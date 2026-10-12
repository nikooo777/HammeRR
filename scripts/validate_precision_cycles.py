import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail
from validate_texture_chain import uv


def thin_fixture(path, entity=False):
    planes = [
        [[3928, 5888, 57.0981], [4396, 5888, 57.0981], [4396, 5784, -0.0516684]],
        [[3928, 5792, -0.0516685], [4396, 5792, -0.0516685], [4396, 5888, 52.9497]],
        [[3928, 5784, -0.0516684], [4396, 5784, -0.0516684], [4396, 5792, -0.0516685]],
        [[3928, 5888, 57.0981], [3928, 5784, -0.0516684], [3928, 5792, -0.0516685]],
        [[4396, 5888, 57.0981], [3928, 5888, 57.0981], [3928, 5888, 52.9497]],
        [[4396, 5888, 52.9497], [4396, 5792, -0.0516685], [4396, 5784, -0.0516684]],
    ]

    def normalized(vector):
        length = math.sqrt(sum(value * value for value in vector))
        return [value / length for value in vector]

    def cross(a, b):
        return [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]

    sides = []
    for index, points in enumerate(planes):
        normal = normalized(
            cross(
                [a - b for a, b in zip(points[0], points[1])],
                [a - b for a, b in zip(points[2], points[1])],
            )
        )
        candidate = [1, 0, 0] if abs(normal[0]) < 0.9 else [0, 1, 0]
        dot = sum(a * b for a, b in zip(normal, candidate))
        u = normalized([a - b * dot for a, b in zip(candidate, normal)])
        v = cross(normal, u)
        plane = " ".join(
            "(" + " ".join(str(value) for value in point) + ")" for point in points
        )
        axis_u = " ".join(str(value) for value in [*u, 13.375])
        axis_v = " ".join(str(value) for value in [*v, -7.125])
        sides.append(
            f'''side {{ "id" "{index + 10}" "plane" "{plane}" "material" "brick/test" "uaxis" "[{axis_u}] 0.25" "vaxis" "[{axis_v}] 0.25" "rotation" "0" "lightmapscale" "8" "smoothing_groups" "5" "vendor_side" "preserve" }}'''
        )
    brush = (
        'solid { "id" "2" '
        + " ".join(sides)
        + ' vendor_data { "resource" "test/brush" } }'
    )
    text = (
        'world { "id" "1" "classname" "worldspawn" ' + ("" if entity else brush) + " }"
    )
    if entity:
        text += (
            ' entity { "id" "3" "classname" "func_detail" "targetname" "precision_fixture" '
            + brush
            + " }"
        )
    path.write_text(text)


def validate(client, output, cycles):
    checks = []
    worst_geometry = 0
    worst_texture = 0

    def check(condition, label):
        assert condition, label
        checks.append(label)

    for label in ["fractional-world", "large-entity", "thin-world", "thin-entity"]:
        client.call("new_map", {"discard_changes": True})
        if label.startswith("thin"):
            path = output / f"{label}-source.vmf"
            thin_fixture(path, entity=label.endswith("entity"))
            client.call("open_map", {"path": str(path)})
            solid = 2
            owner = "entity:3" if label.endswith("entity") else "solid:2"
        else:
            origin = (
                [8192.125, -4096.25, 1024.375]
                if label.startswith("large")
                else [0.125, -0.25, 0.375]
            )
            owner = client.call(
                "create_block",
                {
                    "mins": origin,
                    "maxs": [a + b for a, b in zip(origin, [64.125, 31.75, 16.5])],
                    "material": "brick/test",
                },
            )["id"]
            solid = int(owner.split(":")[1])
            if label.endswith("entity"):
                owner = client.call(
                    "tie_to_entity", {"ids": [owner], "classname": "func_detail"}
                )["id"]
        report = client.call("get_brush_components", {"solids": [solid]})
        shape = report["brushes"][0]
        original_detail = detail(client, [owner])[0]
        original = (
            original_detail["brushes"][0]
            if owner.startswith("entity:")
            else original_detail
        )
        vertices = {
            tuple(vertex["sides"]): vertex["position"] for vertex in shape["vertices"]
        }
        faces = {face["side"]: face for face in original["faces"]}
        components = [
            {"solid": solid, "sides": [face["side"]]} for face in shape["faces"]
        ]
        for cycle in range(cycles):
            displacement = [
                [0.125, -0.25, 0.375],
                [-0.375, 0.125, -0.25],
                [0.25, 0.375, -0.125],
            ][cycle % 3]
            for sign in [1, -1]:
                snapshot = client.call("get_brush_components", {"solids": [solid]})[
                    "snapshot"
                ]
                selected = client.call(
                    "select_brush_components",
                    {"components": components, "snapshot": snapshot},
                )
                source = detail(client, [owner])
                history = len(client.call("get_map_info")["undo"])
                draft = client.call(
                    "preview_component_move",
                    {
                        "delta": [sign * value for value in displacement],
                        "snap": False,
                        "texture_lock": True,
                        "snapshot": selected["snapshot"],
                    },
                )
                assert detail(client, [owner]) == source
                client.call(
                    "commit_component_edit", {"token": draft["preview"]["token"]}
                )
                changed = detail(client, [owner])
                assert len(client.call("get_map_info")["undo"]) == history + 1
                client.call("undo")
                assert detail(client, [owner]) == source
                client.call("redo")
                assert detail(client, [owner]) == changed
                path = output / f"{label}-cycle.vmf"
                client.call("save_map", {"path": str(path)})
                client.call("open_map", {"path": str(path)})
            after_detail = detail(client, [owner])[0]
            after = (
                after_detail["brushes"][0]
                if owner.startswith("entity:")
                else after_detail
            )
            report = client.call("get_brush_components", {"solids": [solid]})
            shape = report["brushes"][0]
            assert len(shape["vertices"]) == len(vertices)
            for vertex in shape["vertices"]:
                point = vertex["position"]
                before = vertices[tuple(vertex["sides"])]
                worst_geometry = max(
                    worst_geometry, *(abs(a - b) for a, b in zip(point, before))
                )
                for side in vertex["sides"]:
                    old = faces[side]
                    new = next(face for face in after["faces"] if face["side"] == side)
                    worst_texture = max(
                        worst_texture,
                        *(abs(a - b) for a, b in zip(uv(old, before), uv(new, point))),
                    )
            assert worst_geometry < 1e-5 and worst_texture < 1e-4
            assert all(
                all(
                    face[field] == faces[face["side"]][field]
                    for field in ["material", "lightmap_scale", "smoothing_groups"]
                )
                for face in after["faces"]
            )
            assert all(
                abs(a - b) < 1e-8
                for face in after["faces"]
                for old, new in zip(faces[face["side"]]["plane"], face["plane"])
                for a, b in zip(old, new)
            )
            if owner.startswith("entity:"):
                assert after_detail["properties"] == original_detail["properties"]
            check(
                True,
                f"{label}: cycle {cycle + 1} preserves identities, exact undo, saved planes, ownership and attached UVs",
            )
        if label.startswith("thin"):
            saved = (output / f"{label}-cycle.vmf").read_text()
            check(
                saved.count('"vendor_side"') == 6
                and "vendor_data" in saved
                and '"resource" "test/brush"' in saved,
                f"{label}: unknown side and brush metadata survives every save",
            )
    return {
        "passed": len(checks),
        "cycles_per_fixture": cycles,
        "geometry_drift": worst_geometry,
        "texture_drift": worst_texture,
        "checks": checks,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-precision-cycles")
    )
    parser.add_argument("--cycles", type=int, default=16)
    args = parser.parse_args()
    if not 1 <= args.cycles <= 128:
        parser.error("cycles must be between 1 and 128")
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        result = validate(client, args.output, args.cycles)
        (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))
    finally:
        client.close()


if __name__ == "__main__":
    main()
