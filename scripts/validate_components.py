import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_material_replacement import preserved_map
from validate_texture_chain import uv


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def inspect(solids=None):
        return client.call("get_brush_components", {"solids": solids} if solids else {})

    def select(components, mode="replace"):
        return client.call(
            "select_brush_components",
            {"components": components, "mode": mode, "snapshot": inspect()["snapshot"]},
        )

    def preview(delta, snap=False, texture_lock=False):
        return client.call(
            "preview_component_move",
            {
                "delta": delta,
                "snap": snap,
                "texture_lock": texture_lock,
                "snapshot": inspect()["snapshot"],
            },
        )

    def top_edge(shape):
        top = max(vertex["position"][2] for vertex in shape["vertices"])
        return next(
            {"solid": shape["solid"], "sides": edge["sides"]}
            for edge in shape["edges"]
            if all(abs(point[2] - top) < 1e-5 for point in edge["positions"])
        )

    with preserved_map(client):
        tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
        check(
            {
                "get_brush_components",
                "select_brush_components",
                "preview_component_move",
                "commit_component_move",
                "cancel_component_move",
            }
            <= tools,
            "Component tools discoverable",
        )
        ids = [
            client.call(
                "create_block",
                {
                    "mins": [x, 0, 0],
                    "maxs": [x + 64, 64, 64],
                    "material": "dev/dev_measuregeneric01",
                },
            )["id"]
            for x in [0, 128]
        ]
        solid_ids = [int(id.split(":")[1]) for id in ids]
        shape = inspect(solid_ids)["brushes"][0]
        check(
            (len(shape["vertices"]), len(shape["edges"]), len(shape["faces"]))
            == (8, 12, 6),
            "Cube topology has eight vertices, twelve edges and six faces",
        )
        edge = top_edge(shape)
        face = {"solid": shape["solid"], "side": shape["faces"][0]["side"]}
        client.call(
            "edit_faces",
            {
                "faces": [face],
                "shift_u": 17.25,
                "lightmap_scale": 8,
                "smoothing_groups": [3, 9],
            },
        )
        selected = select([edge])
        check(selected["selection"] == [edge], "Stable incident-face selection")
        client.call(
            "set_camera",
            {
                "projection": "perspective",
                "position": [160.123456, -180.123456, 170.123456],
                "look_at": [32, 32, 32],
                "fov": 73.12345,
            },
        )
        original = detail(client, ids)
        history = client.call("get_map_info")["undo"]
        baseline = client.render(output / "before.png")
        camera = client.call("get_map_info")["camera"]
        client.call(
            "set_camera",
            {key: camera[key] for key in ["position", "yaw", "pitch", "fov"]},
        )
        check(
            client.render(output / "camera-restored.png") == baseline,
            "Exact camera restoration preserves pixels",
        )
        stamp = inspect()["snapshot"]
        draft = preview([0, 0, 16])
        token = draft["preview"]["token"]
        check(
            detail(client, ids) == original
            and client.call("get_map_info")["undo"] == history,
            "Preview leaves map and history untouched",
        )
        check(
            max(vertex["position"][2] for vertex in draft["brushes"][0]["vertices"])
            == 80,
            "Topology query shows moved preview vertices",
        )
        pixels = client.render(output / "preview.png")
        check(pixels != baseline, "Component preview renders in shared scene")
        client.call(
            "preview_component_move",
            {"delta": [0, 0, 32], "snapshot": stamp},
            error=True,
        )
        client.call(
            "select_brush_components", {"components": [], "snapshot": stamp}, error=True
        )
        checks.append("Stale snapshots reject mutations")
        for delta in [[0, 0, 0], [0, 0, -64], [0, 0, -96], [0, 0, 100000]]:
            client.call(
                "preview_component_move",
                {"delta": delta, "snapshot": inspect()["snapshot"]},
                error=True,
            )
            check(
                inspect()["preview"]["token"] == token,
                "Invalid movement preserves valid draft",
            )
        client.call("cancel_component_move", {"token": token})
        check(
            client.render(output / "cancelled.png") == baseline,
            "Cancel restores identical pixels",
        )
        draft = preview([0, 0, 19], snap=True)
        check(
            draft["preview"]["delta"] == [0, 0, 16],
            "Shared editor grid snaps the move offset",
        )
        current = draft["preview"]["token"]
        client.call("commit_component_move", {"token": token}, error=True)
        client.call("cancel_component_move", {"token": token}, error=True)
        check(
            inspect()["preview"]["token"] == current,
            "Stale tokens preserve newer preview",
        )
        client.call("commit_component_move", {"token": current})
        after = detail(client, ids)
        check(
            client.render(output / "applied.png") == pixels,
            "Committed geometry and markers match preview pixels",
        )
        check(
            len(client.call("get_map_info")["undo"]) == len(history) + 1,
            "Apply is one undo step",
        )
        check(after[1] == original[1], "Other brush unchanged")
        for old, new in zip(original[0]["faces"], after[0]["faces"], strict=True):
            keys = [
                "side",
                "material",
                "uaxis",
                "vaxis",
                "rotation",
                "lightmap_scale",
                "smoothing_groups",
            ]
            check(
                {key: old[key] for key in keys} == {key: new[key] for key in keys},
                "Face identity, texture and lighting settings preserved",
            )
        client.call("undo")
        check(detail(client, ids) == original, "Undo restores original brush")
        client.call("redo")
        check(detail(client, ids) == after, "Redo restores edited brush")
        client.call("undo")
        shape = inspect(solid_ids)["brushes"][0]
        top = next(
            face
            for face in shape["faces"]
            if all(
                abs(shape["vertices"][index]["position"][2] - 64) < 1e-5
                for index in face["vertices"]
            )
        )
        whole_face = {"solid": shape["solid"], "sides": [top["side"]]}
        select([whole_face])
        face_draft = preview([0, 0, 16])
        check(
            sum(
                abs(vertex["position"][2] - 80) < 1e-5
                for vertex in face_draft["brushes"][0]["vertices"]
            )
            == 4,
            "Whole-face selection moves all four corners",
        )
        check(
            next(
                face
                for face in face_draft["brushes"][0]["faces"]
                if face["side"] == top["side"]
            )["selected"],
            "Face geometry selection is inspectable",
        )
        client.call("commit_component_move", {"token": face_draft["preview"]["token"]})
        client.call("undo")
        check(detail(client, ids) == original, "Face movement is one reversible edit")
        select([whole_face])
        attached = preview([0, 0, 32], texture_lock=True)
        check(
            attached["texture_mode"] == "attached"
            and attached["preview"]["texture_lock"],
            "Attached texture mode is inspectable",
        )
        attached_pixels = client.render(output / "texture-attached-preview.png")
        client.call("commit_component_move", {"token": attached["preview"]["token"]})
        check(
            client.render(output / "texture-attached-applied.png") == attached_pixels,
            "Attached texture preview and apply have identical pixels",
        )
        attached_faces = detail(client, ids)
        moved_shape = inspect(solid_ids)["brushes"][0]
        for old in original[0]["faces"]:
            new = next(
                face
                for face in attached_faces[0]["faces"]
                if face["side"] == old["side"]
            )
            for vertex in shape["vertices"]:
                if old["side"] not in vertex["sides"]:
                    continue
                moved = next(
                    item
                    for item in moved_shape["vertices"]
                    if item["sides"] == vertex["sides"]
                )
                assert all(
                    abs(a - b) < 1e-4
                    for a, b in zip(
                        uv(old, vertex["position"]),
                        uv(new, moved["position"]),
                        strict=True,
                    )
                )
        checks.append("Every face corner keeps its original texture coordinates")
        check(
            any(
                old["uaxis"] != new["uaxis"] or old["vaxis"] != new["vaxis"]
                for old, new in zip(
                    original[0]["faces"], attached_faces[0]["faces"], strict=True
                )
            ),
            "Stretching changes texture projection when attachment is enabled",
        )
        client.call("undo")
        check(
            detail(client, ids) == original,
            "Undo restores attached texture projections",
        )
        client.call("redo")
        check(
            detail(client, ids) == attached_faces,
            "Redo restores attached texture projections",
        )
        client.call("undo")
        select([edge])
        world = preview([0, 0, 16])
        rejected = client.call(
            "preview_component_move",
            {
                "delta": [0, 0, 16],
                "texture_lock": True,
                "snapshot": inspect()["snapshot"],
            },
            error=True,
        )
        check(
            "every corner" in json.dumps(rejected),
            "Unsupported corner attachment gives an actionable explanation",
        )
        check(
            inspect()["preview"]["token"] == world["preview"]["token"]
            and inspect()["texture_mode"] == "world",
            "Rejected texture attachment preserves valid world-anchored preview",
        )
        client.call("cancel_component_move", {"token": world["preview"]["token"]})
        vertex = {"solid": shape["solid"], "sides": shape["vertices"][0]["sides"]}
        select([vertex])
        result = client.call(
            "preview_component_move",
            {"delta": [4, 4, 4], "snapshot": inspect()["snapshot"]},
            error=True,
        )
        check(
            "nonplanar" in json.dumps(result),
            "Nonplanar corner explains why the move cannot apply",
        )
        select([edge])
        invalid = {"solid": shape["solid"], "sides": [9998, 9999]}
        client.call(
            "select_brush_components",
            {"components": [invalid], "snapshot": inspect()["snapshot"]},
            error=True,
        )
        check(
            inspect()["selection"] == [edge],
            "Invalid component leaves selection unchanged",
        )
        select([edge], "toggle")
        check(inspect()["selection"] == [], "Toggle clears component selection")
        entity = client.call(
            "tie_to_entity", {"ids": [ids[1]], "classname": "func_detail"}
        )["id"]
        client.call("select", {"ids": [ids[0], entity]})
        shapes = inspect()["brushes"]
        selected = select([top_edge(shape) for shape in shapes])
        before_batch = detail(client, [ids[0], entity])
        draft = preview([0, 0, 8])
        check(
            draft["preview"]["brushes"] == 2,
            "One preview spans world and entity brushes",
        )
        client.call("commit_component_move", {"token": draft["preview"]["token"]})
        batch = detail(client, [ids[0], entity])
        check(
            batch != before_batch and batch[1]["id"] == entity,
            "Component movement preserves entity ownership",
        )
        client.call("undo")
        check(
            detail(client, [ids[0], entity]) == before_batch,
            "One undo restores the entire multibrush move",
        )
        client.call("redo")
        saved = output / "components.vmf"
        client.call("save_map", {"path": str(saved)})
        client.call("open_map", {"path": str(saved)})
        check(
            detail(client, [ids[0], entity]) == batch,
            "Edited geometry and metadata survive VMF save/reopen",
        )
        client.call("select", {"ids": [ids[0], entity]})
        shape = inspect()["brushes"][0]
        select([top_edge(shape)])
        draft = preview([0, 0, 8])
        client.call("move_objects", {"ids": [ids[0]], "offset": [0, 8, 0]})
        client.call(
            "commit_component_move", {"token": draft["preview"]["token"]}, error=True
        )
        check(
            inspect()["preview"] is None and inspect()["selection"] == [],
            "External source edit invalidates component state",
        )
        client.call(
            "create_primitive",
            {
                "shape": "cone",
                "segments": 3,
                "mins": [-160, -160, 0],
                "maxs": [-96, -96, 96],
                "material": "dev/dev_measuregeneric01",
            },
        )
        tetra = inspect()["brushes"][0]
        apex = max(tetra["vertices"], key=lambda vertex: vertex["position"][2])
        select([{"solid": tetra["solid"], "sides": apex["sides"]}])
        draft = preview([8, 4, 8], texture_lock=True)
        check(
            len(draft["brushes"][0]["vertices"]) == 4,
            "A single tetrahedron vertex can move while retaining topology",
        )
        client.call("commit_component_move", {"token": draft["preview"]["token"]})
        moved_apex = next(
            vertex
            for vertex in inspect()["brushes"][0]["vertices"]
            if vertex["sides"] == apex["sides"]
        )
        check(
            all(
                abs(a + d - b) < 1e-6
                for a, d, b in zip(
                    apex["position"], [8, 4, 8], moved_apex["position"], strict=True
                )
            ),
            "Single-vertex offset is precise",
        )
        tetra_after = detail(client, [f"solid:{tetra['solid']}"])
        attached_path = output / "texture-attachment.vmf"
        client.call("save_map", {"path": str(attached_path)})
        client.call("open_map", {"path": str(attached_path)})
        check(
            detail(client, [f"solid:{tetra['solid']}"]) == tetra_after,
            "Attached triangle textures survive VMF save/reopen",
        )
        for mins, maxs in [
            ([-256, -256, -16], [256, 256, 0]),
            ([-256, -256, 240], [256, 256, 256]),
            ([-256, -256, 0], [-240, 256, 240]),
            ([240, -256, 0], [256, 256, 240]),
            ([-240, -256, 0], [240, -240, 240]),
            ([-240, 240, 0], [240, 256, 240]),
        ]:
            client.call("create_block", {"mins": mins, "maxs": maxs})
        client.call(
            "create_entity",
            {"classname": "info_player_counterterrorist", "origin": [-128, 0, 16]},
        )
        compiled = client.call("compile_map", {"path": str(output / "components.bsp")})
        check(
            compiled["success"] and Path(compiled["bsp"]).is_file(),
            "Sealed fixture with edited world and entity brushes compiles",
        )
        (output / "compile.json").write_text(json.dumps(compiled, indent=2) + "\n")
    checks.append("Original session restored")
    result = {
        "transport": "http" if client.url else "stdio",
        "tools": len(tools),
        "checks": len(checks),
        "passed": checks,
    }
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key: value for key, value in result.items() if key != "passed"}))


def main():
    parser = argparse.ArgumentParser(
        description="Validate brush component editing over MCP; HTTP requires a clean idle map with empty history."
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
