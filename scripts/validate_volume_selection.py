import argparse
import json
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail
from validate_material_replacement import preserved_map


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def volume(mins, maxs, **options):
        return client.call("select_volume", {"mins": mins, "maxs": maxs, **options})

    with preserved_map(client):
        client.call("set_view", {"tool_textures": True, "surface": "textured"})
        ids = []
        for x in [0, 128, 256]:
            ids.append(
                client.call(
                    "create_block",
                    {
                        "mins": [x, 0, 0],
                        "maxs": [x + 64, 64, 64],
                        "material": "brick/brickwall001a",
                    },
                )["id"]
            )
        client.call("set_object_visibility", {"action": "hide", "ids": [ids[2]]})
        client.call("select", {"ids": [ids[1]]})
        original = detail(client, ids)
        history = client.call("get_map_info")["undo"]
        client.call(
            "set_camera", {"position": [250, -240, 240], "look_at": [90, 30, 30]}
        )
        initial_camera = client.call("get_map_info")["camera"]
        render_camera = {
            "position": [250, -240, 240],
            "look_at": [90, 30, 30],
            "projection": "perspective",
        }
        before_png = client.render(output / "before.png", camera=render_camera)
        report = volume([-8, -8, -8], [72, 72, 72], preview=True)
        draft = report["preview"]
        token = draft["token"]
        check(
            draft["objects"] == ids[:1] and draft["object_count"] == 1,
            "Preview finds the expected object",
        )
        check(
            client.call("get_map_info")["selection"] == [ids[1]],
            "Preview preserves committed selection",
        )
        check(
            detail(client, ids) == original
            and client.call("get_map_info")["undo"] == history,
            "Preview preserves map and history",
        )
        check(
            client.render(output / "volume-preview.png", camera=render_camera)
            != before_png,
            "Volume guide and candidate highlights render",
        )
        for projection in ["top", "front", "side", "perspective"]:
            client.call("set_camera", {"projection": projection})
            client.call("frame_volume_selection", {"token": token})
            check(
                client.call("get_volume_selection")["preview"]["token"] == token,
                f"Preview survives {projection} camera and framing",
            )
            client.render(output / f"volume-{projection}.png")
        client.call(
            "set_camera",
            {
                k: initial_camera[k]
                for k in [
                    "position",
                    "projection",
                    "fov",
                    "yaw",
                    "pitch",
                    "ortho_height",
                ]
            },
        )
        client.call("cancel_volume_selection", {"token": token})
        check(
            client.call("get_map_info")["selection"] == [ids[1]],
            "Cancel preserves selection",
        )
        check(
            client.render(output / "cancelled.png", camera=render_camera) == before_png,
            "Cancel restores the original render",
        )
        draft = volume([-8, -8, -8], [72, 72, 72], preview=True)["preview"]
        client.call("commit_volume_selection", {"token": token}, error=True)
        checks.append("Superseded token rejected")
        client.call("commit_volume_selection", {"token": draft["token"]})
        check(
            client.call("get_map_info")["selection"] == ids[:1],
            "Commit applies the reviewed selection",
        )
        for mode, bounds, expected in [
            ("add", ([120, -8, -8], [200, 72, 72]), ids[:2]),
            ("remove", ([-8, -8, -8], [72, 72, 72]), [ids[1]]),
            ("toggle", ([120, -8, -8], [200, 72, 72]), []),
        ]:
            volume(*bounds, mode=mode)
            check(
                client.call("get_map_info")["selection"] == expected,
                f"Shared {mode} rule",
            )
        report = volume([-1, -1, -1], [321, 65, 65], preview=True)
        check(report["preview"]["object_count"] == 2, "Hidden object excluded")
        page = client.call("get_volume_selection", {"offset": 1, "limit": 1})
        check(page["preview"]["objects"] == [ids[1]], "Candidate pagination")
        token = report["preview"]["token"]
        for bounds in [
            ([0, 0, 0], [0, 64, 64]),
            ([1, 1, 1], [0, 0, 0]),
            ([0, 0, 0], [100000, 1, 1]),
        ]:
            client.call(
                "select_volume",
                {"mins": bounds[0], "maxs": bounds[1], "preview": True},
                error=True,
            )
            check(
                client.call("get_volume_selection")["preview"]["token"] == token,
                "Invalid bounds preserve existing preview",
            )
        volume([16, 16, 16], [32, 32, 32])
        check(
            client.call("get_map_info")["selection"] == ids[:1],
            "Volume fully inside a brush intersects its solid geometry",
        )
        volume([16, 16, 16], [32, 32, 32], rule="contained")
        check(
            not client.call("get_map_info")["selection"],
            "Containment excludes a surrounding brush",
        )
        report = volume(
            [-1, -1, 63],
            [65, 65, 65],
            selection_mode="faces",
            rule="contained",
            preview=True,
        )
        check(
            report["preview"]["face_count"] == 1,
            "Face containment selects only the top face",
        )
        check(
            client.call("get_face_selection")["mode"] == "objects",
            "Preview leaves selection mode unchanged",
        )
        client.call("commit_volume_selection", {"token": report["preview"]["token"]})
        check(
            client.call("get_face_selection")["mode"] == "faces",
            "Face commit changes selection mode",
        )
        check(
            len(client.call("get_face_selection")["faces"]) == 1,
            "Face commit selects exactly one face",
        )
        check(
            detail(client, ids) == original
            and client.call("get_map_info")["undo"] == history,
            "Selection workflows add no document edits",
        )
        report = volume([-1, -1, -1], [65, 65, 65], preview=True)
        client.call("set_view", {"tool_textures": False})
        check(
            client.call("get_volume_selection")["preview"] is None,
            "Visibility rule changes invalidate candidates",
        )
        client.call(
            "commit_volume_selection", {"token": report["preview"]["token"]}, error=True
        )
        report = volume([-1, -1, -1], [65, 65, 65], preview=True)
        client.call("select_faces", {"faces": []})
        check(
            client.call("get_volume_selection")["preview"] is None,
            "Selection changes invalidate candidates",
        )
        client.call("save_map", {"path": str(output / "volume-selection.vmf")})
    checks.append("Original map, selection and camera restored")
    result = {
        "transport": "http" if client.url else "stdio",
        "checks": len(checks),
        "passed": checks,
    }
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k != "passed"}))


def main():
    parser = argparse.ArgumentParser(
        description="Validate volume selection through MCP; HTTP requires a clean idle map with empty history."
    )
    parser.add_argument("--url")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
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
