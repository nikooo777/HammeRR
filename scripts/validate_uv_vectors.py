import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_material_replacement import preserved_map
from validate_texture_chain import uv


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    with preserved_map(client):
        schemas = {
            tool["name"]: tool["inputSchema"]
            for tool in client.rpc("tools/list", {})["tools"]
        }
        check(
            all(key in schemas["edit_faces"]["properties"] for key in ["u_axis", "v_axis"]),
            "Direct vectors use the existing edit_faces schema",
        )
        ids = [
            client.call(
                "create_block",
                {
                    "mins": [x, 0, 16],
                    "maxs": [x + 128, 64, 80],
                    "material": "dev/dev_measuregeneric01",
                },
            )["id"]
            for x in [0, 256]
        ]
        brushes = detail(client, ids)
        faces = [
            face_ref(brush, next(face for face in brush["faces"] if face["normal"] == [0, 0, 1]))
            for brush in brushes
        ]
        for face, u_x in zip(faces, [1, 2], strict=True):
            client.call(
                "edit_faces",
                {
                    "faces": [face],
                    "u_axis": [u_x, 0, 0],
                    "scale_u": -0.5,
                    "scale_v": 0.25,
                    "shift_u": 13.125,
                    "shift_v": -27.5,
                    "lightmap_scale": 8,
                    "smoothing_groups": [1, 3, 32],
                },
            )
        owner = client.call("tie_to_entity", {"ids": [ids[1]], "classname": "func_brush"})["id"]
        owned = detail(client, [owner])[0]
        check(bool(owned), "Entity-owned brush target retained")
        client.call("select_faces", {"faces": faces})
        initial = client.call("get_face_selection")
        check([face["u_axis"] for face in initial["faces"]] == [[1, 0, 0], [2, 0, 0]], "Mixed vector values are inspectable")
        baseline = client.call("get_face_selection")["faces"]
        history = client.call("get_map_info")["undo"]
        client.call("set_camera", {"position": [450, -360, 360], "look_at": [160, 32, 48]})
        pixels = client.render(output / "before.png")
        patch = {"u_axis": [None, 0.5, 0.125], "v_axis": [0.25, -1.5, 0.375], "preview": True}
        draft = client.call("edit_faces", patch)
        token = draft["state"]["preview_token"]
        check(client.call("get_face_selection")["faces"][0]["u_axis"] == [1, 0, 0], "Preview preserves committed vectors")
        check(client.call("get_map_info")["undo"] == history, "Preview preserves history")
        previews = draft["state"]["preview_faces"]
        check([face["u_axis"] for face in previews] == [[1, 0.5, 0.125], [2, 0.5, 0.125]], "Null components retain each selected face's value")
        check(uv(previews[0], [16, 24, 80]) == [-62.875, -35.5], "Known texel coordinates use preserved scale and offset")
        review = client.render(output / "uv-vectors-preview.png")
        check(review != pixels, "Direct skew preview changes rendered texture")
        for invalid in [
            {"u_axis": [0, 0, 0]},
            {"u_axis": [0, 0, 1]},
            {"u_axis": [1, 0, 0], "v_axis": [2, 0, 0]},
            {"u_axis": [1, 0, 0], "rotation": 0},
            {"v_axis": [0, -1, 0], "alignment": "face"},
            {"u_axis": [1, 0]},
            {"faces": [faces[0], {**faces[1], "side": 4294967295}], "u_axis": [2, 0.5, 0]},
        ]:
            client.call("edit_faces", {**invalid, "preview": True}, error=True)
            check(client.call("get_face_selection")["preview_token"] == token, "Invalid vector draft retains existing preview")
        client.call("cancel_face_preview", {"token": token})
        check(client.call("get_face_selection")["faces"] == baseline, "Cancel preserves all committed metadata")
        check(client.render(output / "cancel.png") == pixels, "Cancel restores identical pixels")
        draft = client.call("edit_faces", patch)
        client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})
        after = client.call("get_face_selection")["faces"]
        for old, changed in zip(baseline, after, strict=True):
            for key in ["material", "scale_u", "scale_v", "shift_u", "shift_v", "rotation", "lightmap_scale", "smoothing_mask"]:
                check(old[key] == changed[key], f"Direct vectors preserve {key}")
        check(client.call("get_map_info")["undo"][1:] == history[:9], "One Undo entry for mixed ownership")
        client.call("undo")
        check(client.call("get_face_selection")["faces"] == baseline, "Undo restores exact face metadata")
        client.call("redo")
        check(client.call("get_face_selection")["faces"] == after, "Redo restores direct vectors")
        saved = output / "uv-vectors.vmf"
        client.call("save_map", {"path": str(saved)})
        client.call("open_map", {"path": str(saved)})
        client.call("select_faces", {"faces": faces})
        check(client.call("get_face_selection")["faces"] == after, "VMF save/reopen preserves authored vectors")
    checks.append("Original map and view restored")
    result = {"transport": "http" if client.url else "stdio", "checks": len(checks), "passed": checks}
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key: value for key, value in result.items() if key != "passed"}))


def main():
    parser = argparse.ArgumentParser(description="Validate direct UV vectors through MCP; HTTP requires a clean idle map with empty history.")
    parser.add_argument("--url")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--gameinfo", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    client = Client(args.url, args.binary.resolve(), output, args.gameinfo)
    try:
        validate(client, output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
