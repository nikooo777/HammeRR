import argparse
import hashlib
import json
from contextlib import contextmanager
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail


@contextmanager
def preserved_map(client):
    original = client.call("get_map_info")
    assert (
        not original["unsaved_changes"]
        and not original["undo"]
        and not original["redo"]
    ), "A clean saved map and empty history are required"
    assert not any(
        [
            original["face_selection"]["preview"],
            original["primitive"]["preview"],
            original["clipping"]["enabled"],
            original["clipping"]["preview"],
            original["transform"]["drag"],
            original["transform"]["preview"],
            original["area_selection"]["preview"],
            original.get("volume_selection", {}).get("preview"),
            original.get("paint_selection", {}).get("preview"),
            original.get("pointfile", {}).get("source"),
            original.get("hollow", {}).get("preview"),
            original.get("components", {}).get("preview"),
            original.get("components", {}).get("selection"),
            original.get("components", {}).get("picking"),
            original["block_preview"],
            original["spatial"]["draw_block"],
            original["connection_draft"],
            original["rename_draft"],
            original["property_draft"],
            original["context"]["draft"],
            original["context"]["focused_region"],
            original["map_checks"]["report"],
            original["compiling"],
            original["launching"],
            original["game_running"],
            client.call("get_material_search"),
            client.call("get_surface_sample")["sample"],
        ]
    ), "Idle session without previews or reports required"
    original_hash = (
        hashlib.sha256(Path(original["path"]).read_bytes()).hexdigest()
        if original["path"]
        else None
    )
    try:
        client.call("new_map")
        yield original
    finally:
        if original["path"]:
            client.call("open_map", {"path": original["path"], "discard_changes": True})
        else:
            client.call("new_map", {"discard_changes": True})
        if original["face_selection"]["mode"] == "faces":
            client.call(
                "select_faces",
                {
                    "faces": [
                        {k: face[k] for k in ["solid", "side"]}
                        for face in original["face_selection"]["faces"]
                    ]
                },
            )
        else:
            client.call("set_selection_mode", {"mode": "objects"})
            client.call("select", {"ids": original["selection"]})
        camera = original["camera"]
        client.call(
            "set_camera",
            {
                k: camera[k]
                for k in [
                    "position",
                    "projection",
                    "fov",
                    "ortho_height",
                    "yaw",
                    "pitch",
                ]
                if k in camera
                and (camera["projection"] == "perspective" or k not in ["yaw", "pitch"])
            },
        )
        client.call("set_view", original["view"])
        transform = original["transform"]["settings"]
        client.call(
            "set_transform_tool",
            {
                **transform,
                "pivot_mode": "custom"
                if transform["pivot"] is not None
                else "selection",
            },
        )
        client.call(
            "set_area_selection_options",
            {key: original["area_selection"][key] for key in ["mode", "rule"]},
        )
        if "paint_selection" in original:
            client.call(
                "set_paint_tool",
                {
                    key: original["paint_selection"][key]
                    for key in ["enabled", "radius", "operation"]
                    if key in original["paint_selection"]
                },
            )
        restored = client.call("get_map_info")
        component_options = original.get("components", {}).get("move_options")
        if component_options:
            state = client.call("get_brush_components")
            restored_options = client.call(
                "set_component_move_options",
                {**component_options, "snapshot": state["snapshot"]},
            )["move_options"]
            assert restored_options == component_options
        clip_options = original.get("clipping", {}).get("handle_options")
        if clip_options:
            state = client.call("get_clip_preview")
            restored_options = client.call(
                "set_clip_handle_options",
                {**clip_options, "snapshot": state["snapshot"]},
            )["handle_options"]
            assert restored_options == clip_options
        for key in [
            "path",
            "unsaved_changes",
            "world_brushes",
            "point_entities",
            "brush_entities",
            "camera",
            "view",
            "selection",
            "undo",
            "redo",
            "face_selection",
            "transform",
            "area_selection",
        ]:
            assert restored[key] == original[key], f"Restore {key}"
        if "paint_selection" in original:
            assert restored["paint_selection"] == original["paint_selection"]
        if original_hash:
            assert (
                hashlib.sha256(Path(original["path"]).read_bytes()).hexdigest()
                == original_hash
            )


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    with preserved_map(client):
        client.call("set_view", {"tool_textures": True, "surface": "textured"})
        ids = []
        for index in range(3):
            block = client.call(
                "create_block",
                {
                    "mins": [index * 96, 0, 0],
                    "maxs": [index * 96 + 64, 64, 64],
                    "material": "dev/dev_measuregeneric01",
                },
            )
            ids.append(block["id"])
        client.call("set_object_visibility", {"action": "hide", "ids": [ids[2]]})
        original = detail(client, ids)
        face = {"solid": original[0]["solid"], "side": original[0]["faces"][0]["side"]}
        client.call(
            "edit_faces",
            {
                "faces": [face],
                "shift_u": 13,
                "lightmap_scale": 8,
                "smoothing_groups": [1, 32],
            },
        )
        original = detail(client, ids)
        history = client.call("get_map_info")["undo"]
        query = {"material": "MATERIALS\\DEV/DEV_MEASUREGENERIC01.vmt"}
        report = client.call("find_material_faces", query)
        check(
            report["total"] == 12 and report["hidden_excluded"] == 6,
            "Hidden faces excluded by default",
        )
        page = client.call("get_material_search", {"offset": 10, "limit": 2})
        check(
            len(page["faces"]) == 2 and page["total"] == 12,
            "Pagination retains full count",
        )
        client.call("select_material_faces", {"token": report["token"]})
        check(
            len(client.call("get_face_selection")["faces"]) == 12,
            "Select all result pages",
        )
        client.call(
            "set_camera", {"position": [210, -270, 220], "look_at": [80, 32, 32]}
        )
        before_png = client.render(output / "before.png")
        draft = client.call(
            "replace_materials",
            {
                "token": report["token"],
                "material": "dev/dev_measurewall01a",
                "preview": True,
            },
        )
        check(draft["faces_changed"] == 12, "Preview affected count")
        check(
            detail(client, ids) == original
            and client.call("get_map_info")["undo"] == history,
            "Preview preserves map and undo",
        )
        check(
            client.render(output / "preview.png") != before_png,
            "Replacement preview changes rendered materials",
        )
        old_preview = draft["state"]["preview_token"]
        client.call("cancel_face_preview", {"token": old_preview})
        check(
            client.render(output / "cancelled.png") == before_png,
            "Cancel restores identical render",
        )
        draft = client.call(
            "replace_materials",
            {
                "token": report["token"],
                "material": "dev/dev_measurewall01a",
                "preview": True,
            },
        )
        client.call("commit_face_preview", {"token": old_preview}, error=True)
        checks.append("Replaced preview token rejected")
        client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})
        changed = detail(client, ids)
        check(changed[2] == original[2], "Hidden brush untouched")
        for previous, current in zip(original[:2], changed[:2], strict=True):
            for old_face, new_face in zip(
                previous["faces"], current["faces"], strict=True
            ):
                check(
                    {k: v for k, v in old_face.items() if k != "material"}
                    == {k: v for k, v in new_face.items() if k != "material"},
                    "Preserved face UV and lighting metadata",
                )
        check(
            len(client.call("get_map_info")["undo"]) == len(history) + 1,
            "Single undo step",
        )
        client.call(
            "replace_materials",
            {"token": report["token"], "material": "tools/toolsnodraw"},
            error=True,
        )
        checks.append("Stale report rejected")
        client.call("undo")
        check(detail(client, ids) == original, "Undo restores all faces")
        client.call("redo")
        check(detail(client, ids) == changed, "Redo reapplies replacement")
        client.call("undo")
        report = client.call("find_material_faces", {**query, "include_hidden": True})
        check(
            report["total"] == 18 and report["hidden_matches"] == 6,
            "Explicit hidden inclusion",
        )
        client.call(
            "replace_materials",
            {"token": report["token"], "material": "dev/dev_measurewall01a"},
        )
        check(
            client.call("find_material_faces", {"material": "dev/dev_measurewall01a"})[
                "hidden_excluded"
            ]
            == 6,
            "Replacement preserves hidden state",
        )
        client.call("undo")
        client.call("select", {"ids": [ids[0]]})
        report = client.call(
            "find_material_faces", {**query, "scope": "selected_objects"}
        )
        check(report["total"] == 6, "Selected object scope")
        client.call("select", {"ids": [ids[1]]})
        client.call(
            "replace_materials",
            {"token": report["token"], "material": "tools/toolsnodraw"},
            error=True,
        )
        checks.append("Changed selection rejected")
        client.call("select_faces", {"faces": [face]})
        report = client.call(
            "find_material_faces", {**query, "scope": "selected_faces"}
        )
        check(report["total"] == 1, "Selected face scope")
        client.call(
            "replace_materials",
            {"token": report["token"], "material": "dev/dev_measurewall01a"},
        )
        committed = detail(client, ids)
        client.call("save_map", {"path": str(output / "replacement.vmf")})
        client.call("open_map", {"path": str(output / "replacement.vmf")})
        check(detail(client, ids) == committed, "VMF save and reopen preserve results")
    checks.append("Original map, view, selection and file restored")
    result = {
        "transport": "http" if client.url else "stdio",
        "checks": len(checks),
        "passed": checks,
    }
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k != "passed"}))


def main():
    parser = argparse.ArgumentParser(
        description="Validate material replacement through MCP; HTTP requires a clean saved map and empty undo history."
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
