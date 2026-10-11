import argparse
import json
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail, face_ref
from validate_material_replacement import preserved_map


def uv(face, point):
    result = []
    for key in ["uaxis", "vaxis"]:
        values = [
            float(value)
            for value in face[key].replace("[", "").replace("]", "").split()
        ]
        result.append(
            sum(a * b for a, b in zip(values[:3], point, strict=True)) / values[4]
            + values[3]
        )
    return result


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    with preserved_map(client):
        tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
        check("propagate_face_texture" in tools, "Connected wrapping discoverable")
        ids = [
            client.call(
                "create_block",
                {
                    "mins": [x, 0, 0],
                    "maxs": [x + 128, 128, 128],
                    "material": "brick/brickwall001a",
                },
            )["id"]
            for x in [0, 256]
        ]
        brush, other = detail(client, ids)
        by_normal = {
            tuple(face["normal"]): face_ref(brush, face) for face in brush["faces"]
        }
        source, front, right = [
            by_normal[normal] for normal in [(-1, 0, 0), (0, -1, 0), (1, 0, 0)]
        ]
        targets = [front, right]
        all_faces = [face_ref(brush, face) for face in brush["faces"]]
        disconnected = face_ref(other, other["faces"][0])
        client.call(
            "edit_faces",
            {
                "faces": [source],
                "material": "dev/dev_measuregeneric01",
                "shift_u": 13.5,
                "shift_v": -21,
                "rotation": 17,
                "scale_u": 0.5,
                "scale_v": -0.25,
            },
        )
        client.call(
            "edit_faces",
            {"faces": targets, "lightmap_scale": 8, "smoothing_groups": [3, 9]},
        )
        client.call("select_faces", {"faces": targets})
        client.call(
            "set_camera",
            {
                "projection": "perspective",
                "position": [300, -300, 230],
                "look_at": [64, 64, 64],
            },
        )
        client.call("propagate_face_texture", {"preview": True}, error=True)
        checks.append("Sample required")
        client.call("sample_face_texture", {"face": source})
        original = detail(client, ids)
        selected = [
            {key: face[key] for key in ["solid", "side"]}
            for face in client.call("get_face_selection")["faces"]
        ]
        history = client.call("get_map_info")["undo"]
        baseline = client.render(output / "before.png")
        client.call(
            "paint_face_texture",
            {"faces": targets, "mode": "wrap", "preview": True},
            error=True,
        )
        checks.append("Direct wrapping cannot reach the opposite face")
        draft = client.call("propagate_face_texture", {"preview": True})
        check(
            draft["faces_changed"] == 2
            and draft["links"]
            == [{"parent": source, "face": front}, {"parent": front, "face": right}],
            "Propagation follows the intermediate face",
        )
        check(
            draft["max_targets"] == 256 and not draft["source_in_targets"],
            "Shared limit and source handling reported",
        )
        token = draft["state"]["preview_token"]
        check(
            detail(client, ids) == original
            and client.call("get_map_info")["undo"] == history,
            "Preview leaves map and history unchanged",
        )
        check(
            client.render(output / "preview.png") != baseline,
            "Shared wrapped preview renders",
        )
        before = client.call("get_face_selection")
        for faces in [
            [right],
            [front, disconnected],
            all_faces,
            [source],
            [{"solid": 2, "side": 10000 + i} for i in range(257)],
        ]:
            client.call(
                "propagate_face_texture", {"faces": faces, "preview": True}, error=True
            )
            check(
                client.call("get_face_selection") == before,
                "Invalid topology or budget preserves existing preview",
            )
        client.call("cancel_face_preview", {"token": token})
        check(
            client.render(output / "cancel.png") == baseline,
            "Cancel restores identical pixels",
        )
        draft = client.call(
            "propagate_face_texture", {"faces": [right, source, front], "preview": True}
        )
        check(
            draft["source_in_targets"] and draft["faces_changed"] == 2,
            "Source in target list is preserved",
        )
        current = draft["state"]["preview_token"]
        client.call("commit_face_preview", {"token": token}, error=True)
        check(
            client.call("get_face_selection")["preview_token"] == current,
            "Stale apply preserves newer preview",
        )
        client.call("commit_face_preview", {"token": current})
        after = detail(client, ids)
        changed_faces = {face["side"]: face for face in after[0]["faces"]}
        old_faces = {face["side"]: face for face in original[0]["faces"]}
        target_sides = {target["side"] for target in targets}
        mutable = {
            "material",
            "uaxis",
            "vaxis",
            "rotation",
            "shift_u",
            "shift_v",
            "scale_u",
            "scale_v",
        }
        for side, new in changed_faces.items():
            old = old_faces[side]
            if side in target_sides:
                check(
                    new["material"] == "dev/dev_measuregeneric01",
                    "Sampled material propagated",
                )
                check(
                    {k: v for k, v in new.items() if k not in mutable}
                    == {k: v for k, v in old.items() if k not in mutable},
                    "Geometry, lighting and smoothing metadata preserved",
                )
            else:
                assert new == old, "Unselected face changed"
        check(after[1] == original[1], "Disconnected brush untouched")
        for first, second, x in [(source, front, 0), (front, right, 128)]:
            for z in [0, 64, 128]:
                a = uv(changed_faces[first["side"]], [x, 0, z])
                b = uv(changed_faces[second["side"]], [x, 0, z])
                check(
                    max(abs(a - b) for a, b in zip(a, b, strict=True)) < 0.001,
                    "Shared-edge texel coordinates agree",
                )
        check(
            [
                {key: face[key] for key in ["solid", "side"]}
                for face in client.call("get_face_selection")["faces"]
            ]
            == selected
            and len(client.call("get_map_info")["undo"]) == len(history) + 1,
            "One undo step with unchanged selection",
        )
        client.render(output / "applied.png")
        client.call("propagate_face_texture", {"faces": targets})
        check(
            len(client.call("get_map_info")["undo"]) == len(history) + 1,
            "Repeated propagation adds no edit",
        )
        client.call("undo")
        check(detail(client, ids) == original, "Undo restores every face")
        client.call("redo")
        check(detail(client, ids) == after, "Redo restores wrapped projections")
        client.call("undo")
        draft = client.call(
            "propagate_face_texture", {"faces": targets, "preview": True}
        )
        client.call("sample_face_texture", {"face": source})
        check(
            not client.call("get_face_selection")["preview"],
            "Resampling clears the old face preview",
        )
        client.call(
            "commit_face_preview",
            {"token": draft["state"]["preview_token"]},
            error=True,
        )
        client.call("propagate_face_texture", {"faces": targets})
        saved = output / "connected-wrap.vmf"
        client.call("save_map", {"path": str(saved)})
        client.call("open_map", {"path": str(saved)})
        check(
            detail(client, ids) == after, "VMF save and reopen preserve wrapped faces"
        )
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
        description="Validate connected texture wrapping through MCP; HTTP requires a clean idle map with empty history."
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
