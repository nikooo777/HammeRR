import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_material_replacement import preserved_map


def axis(value):
    return [float(part) for part in value.replace("[", "").replace("]", "").split()]


def uv(face, point):
    values = []
    for key in ["uaxis", "vaxis"]:
        vector = axis(face[key])
        values.append(
            sum(a * b for a, b in zip(vector[:3], point, strict=True)) / vector[4]
            + vector[3]
        )
    return values


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    with preserved_map(client):
        client.call("set_view", {"tool_textures": True, "surface": "textured"})
        ids = []
        for start, end in [(-128, 0), (0, 128)]:
            ids.append(
                client.call(
                    "create_block",
                    {
                        "mins": [start, -64, 0],
                        "maxs": [end, 64, 16],
                        "material": "tile/tilefloor001a",
                    },
                )["id"]
            )
        brushes = detail(client, ids)
        refs = [
            {
                "solid": brush["solid"],
                "side": next(
                    face["side"]
                    for face in brush["faces"]
                    if face["normal"] == [0, 0, 1]
                ),
            }
            for brush in brushes
        ]
        wall = {
            "solid": brushes[0]["solid"],
            "side": next(
                face["side"]
                for face in brushes[0]["faces"]
                if face["normal"] == [0, 1, 0]
            ),
        }
        client.call("select_faces", {"faces": refs})
        client.call(
            "edit_faces",
            {
                "faces": refs[:1],
                "scale_u": -0.25,
                "shift_u": 13,
                "lightmap_scale": 8,
                "smoothing_groups": [1, 32],
            },
        )
        original = detail(client, ids)
        history = client.call("get_map_info")["undo"]
        client.call("set_camera", {"position": [220, -240, 250], "look_at": [0, 0, 16]})
        before_png = client.render(output / "before.png")
        draft = client.call(
            "project_face_group", {"repeats": [2, 0.5], "preview": True}
        )
        check(draft["faces_changed"] == 2, "Both faces share the preview")
        check(
            detail(client, ids) == original
            and client.call("get_map_info")["undo"] == history,
            "Preview preserves map and history",
        )
        check(
            client.render(output / "group-fit.png") != before_png,
            "Grouped projection changes the rendered texture",
        )
        token = draft["state"]["preview_token"]
        for arguments in [
            {"repeats": [0, 1]},
            {"repeats": [1e-310, 1]},
            {"material": "missing/material"},
            {"faces": [refs[0], wall]},
            {"reference": wall},
        ]:
            client.call(
                "project_face_group", {**arguments, "preview": True}, error=True
            )
            check(
                client.call("get_face_selection")["preview_token"] == token,
                "Invalid mapping preserves previous draft",
            )
        client.call("cancel_face_preview", {"token": token})
        check(
            client.render(output / "cancelled.png") == before_png,
            "Cancel restores exact render",
        )
        draft = client.call(
            "project_face_group", {"repeats": [2, 0.5], "preview": True}
        )
        client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})
        mapped = client.call("get_face_selection")["faces"]
        check(
            mapped[0]["uaxis"] == mapped[1]["uaxis"]
            and mapped[0]["vaxis"] == mapped[1]["vaxis"],
            "Common projection across faces",
        )
        check(mapped[0]["scale_u"] < 0, "Reference mirroring preserved")
        for y in [-64, 0, 64]:
            check(
                uv(mapped[0], [0, y, 16]) == uv(mapped[1], [0, y, 16]),
                "Continuous seam texel coordinates",
            )
        texture = client.call("preview_material", {"material": "tile/tilefloor001a"})
        values = [uv(mapped[0], [x, y, 16]) for x in [-128, 128] for y in [-64, 64]]
        for index, extent in enumerate([texture["width"] * 2, texture["height"] * 0.5]):
            check(
                abs(min(value[index] for value in values)) < 1e-6
                and abs(max(value[index] for value in values) - extent) < 1e-6,
                "Tile count uses combined bounds and texture dimensions",
            )
        check(
            len(client.call("get_map_info")["undo"]) == len(history) + 1,
            "Grouped edit has one undo step",
        )
        committed = detail(client, ids)
        client.call("undo")
        check(detail(client, ids) == original, "Undo restores original projections")
        client.call("redo")
        check(detail(client, ids) == committed, "Redo restores grouped mapping")
        client.call("undo")
        draft = client.call(
            "project_face_group", {"projection": "view", "preview": True}
        )
        client.render(output / "view-projection.png")
        client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})
        mapped = client.call("get_face_selection")["faces"]
        camera = client.call("get_map_info")["camera"]
        yaw = math.radians(camera["yaw"])
        right = [math.sin(yaw), -math.cos(yaw), 0]
        check(
            sum(a * b for a, b in zip(axis(mapped[0]["uaxis"])[:3], right, strict=True))
            > 0.99999,
            "View projection follows camera right axis",
        )
        for previous, current in zip(original, detail(client, ids), strict=True):
            for old, new in zip(previous["faces"], current["faces"], strict=True):
                check(
                    all(
                        old[key] == new[key]
                        for key in [
                            "plane",
                            "normal",
                            "material",
                            "lightmap_scale",
                            "smoothing_groups",
                            "displacement",
                        ]
                    ),
                    "Geometry and face lighting preserved",
                )
        client.call(
            "set_camera",
            {"projection": "front", "center": [0, 0, 16], "ortho_height": 512},
        )
        before_invalid = detail(client, ids)
        client.call("project_face_group", {"projection": "view"}, error=True)
        check(detail(client, ids) == before_invalid, "Edge-on view rejected atomically")
        client.call(
            "edit_faces", {"faces": refs[:1], "material": "dev/dev_measurewall01a"}
        )
        client.call("project_face_group", {}, error=True)
        checks.append("Mixed materials require explicit shared material")
        client.call("project_face_group", {"material": "tile/tilefloor001a"})
        committed = detail(client, ids)
        client.call("save_map", {"path": str(output / "grouped-textures.vmf")})
        client.call("open_map", {"path": str(output / "grouped-textures.vmf")})
        check(detail(client, ids) == committed, "VMF round trip preserves mapping")
    checks.append("Original map and view restored")
    result = {
        "transport": "http" if client.url else "stdio",
        "checks": len(checks),
        "passed": checks,
    }
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k != "passed"}))


def main():
    parser = argparse.ArgumentParser(
        description="Validate grouped fitting and view projection over MCP; HTTP requires a clean idle map with empty history."
    )
    parser.add_argument("--url")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    client = Client(args.url, args.binary.resolve(), output)
    try:
        validate(client, output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
