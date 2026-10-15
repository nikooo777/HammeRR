import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_material_replacement import preserved_map


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def state():
        return client.call("get_texture_stroke")

    def begin(mode="material", point=None, **options):
        return client.call(
            "begin_texture_stroke",
            {
                "pointer": {
                    "position": point or [325, 300],
                    "width": 800,
                    "height": 600,
                },
                "mode": mode,
                "radius": 8,
                **options,
            },
        )["preview"]

    def extend(token):
        return client.call(
            "update_texture_stroke",
            {
                "token": token,
                "points": [[500, 300], [675, 300]],
            },
        )["preview"]

    def selection():
        info = client.call("get_map_info")
        return {key: info[key] for key in ["selection", "face_selection"]}

    with preserved_map(client):
        tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
        check(
            all(
                name in tools
                for name in [
                    "begin_texture_stroke",
                    "update_texture_stroke",
                    "get_texture_stroke",
                    "commit_texture_stroke",
                    "cancel_texture_stroke",
                ]
            ),
            "Texture stroke tools discoverable",
        )
        ids = []
        for index, x in enumerate([-160, -48, 64, 176]):
            ids.append(
                client.call(
                    "create_block",
                    {
                        "mins": [x - 24, -24, 0],
                        "maxs": [x + 24, 24, 64],
                        "material": "props/woodcrate001a"
                        if index == 0
                        else "brick/brickwall001a",
                    },
                )["id"]
            )
        objects = detail(client, ids)
        tops = [
            face_ref(
                obj, next(face for face in obj["faces"] if face["normal"] == [0, 0, 1])
            )
            for obj in objects
        ]
        client.call(
            "edit_faces",
            {
                "faces": tops[:1],
                "shift_u": 13.5,
                "shift_v": -21,
                "scale_u": 0.5,
                "scale_v": -0.25,
                "rotation": 37,
            },
        )
        client.call(
            "edit_faces",
            {
                "faces": tops[1:3],
                "shift_u": -17,
                "lightmap_scale": 8,
                "smoothing_groups": [3, 9],
            },
        )
        client.call("set_object_visibility", {"action": "hide", "ids": ids[3:]})
        client.call("create_entity", {"classname": "light", "origin": [64, 0, 128]})
        client.call(
            "set_camera",
            {"projection": "top", "center": [0, 0, 32], "ortho_height": 384},
        )
        client.call("set_view", {"surface": "textured", "tool_textures": True})
        client.call("select_faces", {"faces": tops[:1]})
        client.call(
            "begin_texture_stroke",
            {"pointer": {"position": [325, 300], "width": 800, "height": 600}},
            error=True,
        )
        checks.append("Texture stroke requires a source sample")
        client.call("sample_face_texture", {"face": tops[0]})
        original = detail(client, ids)
        source = next(
            face for face in original[0]["faces"] if face["side"] == tops[0]["side"]
        )
        selected = selection()
        history = client.call("get_map_info")["undo"]
        image_args = {"width": 800, "height": 600}
        baseline = client.render(output / "before.png", **image_args)
        for mode in ["material", "projection"]:
            client.call(
                "set_paint_tool", {"operation": mode, "enabled": True, "radius": 8}
            )
            check(state()["operation"] == mode, f"{mode}: shared toolbar mode")
            token = begin(mode)["token"]
            draft = extend(token)
            check(
                draft["faces"] == tops[1:3] and draft["texture"]["faces_changed"] == 2,
                f"{mode}: visible target faces found, hidden brush and point helper excluded",
            )
            check(
                detail(client, ids) == original
                and selection() == selected
                and client.call("get_map_info")["undo"] == history,
                f"{mode}: preview preserves committed map, selection and history",
            )
            check(
                client.render(output / f"{mode}-preview.png", **image_args) != baseline,
                f"{mode}: draft renders",
            )
            check(
                client.call("get_texture_stroke", {"offset": 1, "limit": 1})["preview"][
                    "faces"
                ]
                == tops[2:3],
                f"{mode}: candidate pagination",
            )
            client.call(
                "update_texture_stroke",
                {"token": token, "points": [[325, 300], [500, 300], [675, 300]]},
            )
            check(
                state()["preview"]["texture"]["faces_changed"] == 2,
                f"{mode}: repeated passes do not repeat edits",
            )
            before = state()
            for name, args in [
                ("update_texture_stroke", {"token": token, "points": []}),
                ("update_texture_stroke", {"token": token, "points": [[0, 0]] * 257}),
                ("update_texture_stroke", {"token": "0", "points": [[500, 300]]}),
                ("commit_paint_selection", {"token": token}),
                ("update_paint_selection", {"token": token, "points": [[500, 300]]}),
                ("set_paint_tool", {"operation": "wrap"}),
            ]:
                client.call(name, args, error=True)
                check(state() == before, f"{mode}: invalid {name} preserves draft")
            client.call(
                "begin_texture_stroke",
                {
                    "pointer": {"position": [325, 300], "width": 800, "height": 600},
                    "mode": "projection",
                    "camera": {
                        "projection": "front",
                        "center": [0, 0, 32],
                        "ortho_height": 384,
                    },
                },
                error=True,
            )
            check(
                state() == before,
                f"{mode}: collapsed projection start preserves existing stroke",
            )
            replacement = begin(mode)["token"]
            client.call("cancel_texture_stroke", {"token": token}, error=True)
            client.call("commit_texture_stroke", {"token": token}, error=True)
            check(
                state()["preview"]["token"] == replacement,
                f"{mode}: stale actions preserve replacement",
            )
            client.call("cancel_texture_stroke", {"token": replacement})
            check(
                client.render(output / f"{mode}-cancel.png", **image_args) == baseline,
                f"{mode}: cancel restores pixels",
            )
            token = begin(mode)["token"]
            extend(token)
            changed = client.call("commit_texture_stroke", {"token": token})[
                "faces_changed"
            ]
            check(changed == 2, f"{mode}: commit reports two changed faces")
            after = detail(client, ids)
            allowed = {"material"}
            if mode == "projection":
                allowed.update(
                    [
                        "uaxis",
                        "vaxis",
                        "rotation",
                        "shift_u",
                        "shift_v",
                        "scale_u",
                        "scale_v",
                    ]
                )
            for old_obj, new_obj, top in zip(
                original[1:3], after[1:3], tops[1:3], strict=True
            ):
                for old, new in zip(old_obj["faces"], new_obj["faces"], strict=True):
                    if old["side"] == top["side"]:
                        check(
                            all(new[key] == source[key] for key in allowed),
                            f"{mode}: sampled values applied",
                        )
                        check(
                            {k: v for k, v in old.items() if k not in allowed}
                            == {k: v for k, v in new.items() if k not in allowed},
                            f"{mode}: geometry, lighting and unrelated metadata preserved",
                        )
                    else:
                        assert old == new, "Untouched face changed"
            check(
                after[0] == original[0] and after[3] == original[3],
                f"{mode}: source and hidden brush unchanged",
            )
            check(
                selection() == selected
                and len(client.call("get_map_info")["undo"]) == len(history) + 1,
                f"{mode}: one undo step with unchanged selection",
            )
            token = begin(mode)["token"]
            extend(token)
            check(
                client.call("commit_texture_stroke", {"token": token})["faces_changed"]
                == 0
                and len(client.call("get_map_info")["undo"]) == len(history) + 1,
                f"{mode}: unchanged stroke adds no undo step",
            )
            client.call("undo")
            check(
                detail(client, ids) == original, f"{mode}: undo restores original faces"
            )
            client.call("redo")
            check(detail(client, ids) == after, f"{mode}: redo restores applied faces")
            client.call("undo")
        token = begin()["token"]
        client.call("sample_face_texture", {"face": tops[0]})
        check(
            state()["preview"] is None,
            "Sampling a source invalidates an active texture stroke",
        )
        client.call("commit_texture_stroke", {"token": token}, error=True)
        token = begin(point=[40, 40])["token"]
        check(
            client.call("commit_texture_stroke", {"token": token})["faces_changed"] == 0
            and client.call("get_map_info")["undo"] == history,
            "Empty stroke adds no edit",
        )
        token = begin()["token"]
        client.call("set_camera", {"projection": "front"})
        check(state()["preview"] is None, "Camera changes invalidate texture strokes")
        client.call("save_map", {"path": str(output / "texture-strokes.vmf")})
    checks.append("Original map, selection, camera and brush settings restored")
    result = {
        "transport": "http" if client.url else "stdio",
        "tools": len(tools),
        "checks": len(checks),
        "passed": checks,
    }
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k != "passed"}))


def main():
    parser = argparse.ArgumentParser(
        description="Validate texture strokes through MCP; HTTP requires a clean idle map with empty history."
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
