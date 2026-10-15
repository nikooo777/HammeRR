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
        return client.call("get_hollow_preview")

    def preview(thickness=16, outward=False, ids=None, open_faces=None):
        return client.call(
            "preview_hollow",
            {
                "thickness": thickness,
                "outward": outward,
                "stamp": state()["stamp"],
                "open_faces": open_faces or [],
                **({"ids": ids} if ids is not None else {}),
            },
        )["preview"]

    with preserved_map(client):
        tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
        check(
            {
                "preview_hollow",
                "get_hollow_preview",
                "commit_hollow",
                "cancel_hollow",
                "look_inside_hollow",
            }
            <= tools,
            "Hollow tools discoverable",
        )
        block = client.call(
            "create_block",
            {
                "mins": [-128, -128, 0],
                "maxs": [128, 128, 256],
                "material": "brick/brickwall001a",
            },
        )["id"]
        brush = detail(client, [block])[0]
        client.call(
            "edit_faces",
            {
                "faces": [face_ref(brush, face) for face in brush["faces"]],
                "shift_u": 13.5,
                "lightmap_scale": 8,
                "smoothing_groups": [3, 9],
            },
        )
        client.call("select", {"ids": [block]})
        client.call(
            "set_camera",
            {
                "projection": "perspective",
                "position": [420, -430, 360],
                "look_at": [0, 0, 128],
                "fov": 70,
            },
        )
        camera = client.call("get_map_info")["camera"]
        original = detail(client, [block])
        history = client.call("get_map_info")["undo"]
        baseline = client.render(output / "before.png", surface="wireframe")
        stamp = state()["stamp"]
        draft = preview()
        token = draft["token"]
        check(
            draft["result"]["sources"] == 1 and len(draft["result"]["brushes"]) == 6,
            "A block becomes six wall brushes",
        )
        cavity = draft["result"]["cavities"][0]
        check(
            cavity["mins"] == [-112, -112, 16]
            and cavity["maxs"] == [112, 112, 240]
            and cavity["center"] == [0, 0, 128],
            "Inward walls preserve outer size and reduce the cavity",
        )
        check(
            detail(client, [block]) == original
            and client.call("get_map_info")["undo"] == history,
            "Hollow preview leaves map/history unchanged",
        )
        wire = client.render(output / "preview.png", surface="wireframe")
        check(wire != baseline, "Wireframe reveals the preview's inner walls")
        client.call("preview_hollow", {"thickness": 8, "stamp": stamp}, error=True)
        checks.append("Stale preview stamp rejected")
        for thickness in [0, -1, 128, 256, 4097]:
            client.call(
                "preview_hollow",
                {"thickness": thickness, "stamp": state()["stamp"]},
                error=True,
            )
            check(
                state()["preview"]["token"] == token,
                "Invalid thickness preserves valid preview",
            )
        before_camera = client.call("get_camera")
        client.call("look_inside_hollow", {"token": token, "index": 1}, error=True)
        check(
            client.call("get_camera") == before_camera,
            "Invalid cavity index preserves camera",
        )
        inside = client.call("look_inside_hollow", {"token": token})
        check(
            inside["position"] == [0, 0, 128]
            and inside["projection"] == "perspective"
            and inside["fov"] == 70,
            "Look inside uses the shared perspective camera",
        )
        interior = client.render(output / "interior-preview.png")
        check(
            state()["preview"]["token"] == token
            and client.call("get_map_info")["undo"] == history,
            "Looking inside preserves draft and map history",
        )
        client.call("cancel_hollow", {"token": token})
        check(
            client.call("get_camera")["position"] == [0, 0, 128],
            "Cancel retains explicit camera navigation",
        )
        client.call(
            "set_camera",
            {key: camera[key] for key in ["position", "yaw", "pitch", "fov"]},
        )
        check(
            client.render(output / "cancelled.png", surface="wireframe") == baseline,
            "Cancel restores original geometry pixels",
        )
        choices = state()["faces"]
        check(
            len(choices) == 6
            and all("center" in face and "material" in face for face in choices),
            "Opening choices expose face identity, direction and material",
        )
        top = next(face for face in choices if face["normal"] == [0, 0, 1])
        top_ref = {key: top[key] for key in ["solid", "side"]}
        opened = preview(open_faces=[top_ref])
        check(
            len(opened["result"]["brushes"]) == 5 and opened["open_faces"] == [top_ref],
            "An open roof produces five walls and inspectable parameters",
        )
        check(
            opened["result"]["cavities"][0]["maxs"][2] == 256,
            "Open roof retains the full source height",
        )
        open_pixels = client.render(output / "open-roof-preview.png")
        check(
            client.call("raycast", {"origin": [0, 0, 128], "direction": [0, 0, 1]})
            == {"hit": None},
            "The roof opening has a clear ray through the shared preview",
        )
        check(
            client.call("raycast", {"origin": [0, 0, 128], "direction": [1, 0, 0]}).get(
                "hit", True
            )
            is not None,
            "The side wall remains solid beside the opening",
        )
        for openings in [
            [{key: face[key] for key in ["solid", "side"]} for face in choices],
            [{"solid": top["solid"], "side": 999999}],
            [{"solid": 999999, "side": top["side"]}],
        ]:
            client.call(
                "preview_hollow",
                {"thickness": 16, "open_faces": openings, "stamp": state()["stamp"]},
                error=True,
            )
            check(
                state()["preview"] == opened,
                "Invalid openings preserve the exact previous draft",
            )
        result = client.call("commit_hollow", {"token": opened["token"]})
        check(
            result == opened["result"]
            and client.render(output / "open-roof-applied.png") == open_pixels,
            "Open shell apply preserves preview IDs and pixels",
        )
        open_walls = detail(client, result["objects"])
        client.call("undo")
        check(
            detail(client, [block]) == original,
            "Open shell undo restores the original solid",
        )
        client.call("redo")
        check(
            detail(client, result["objects"]) == open_walls,
            "Open shell redo restores every wall",
        )
        client.call("undo")
        opened = preview(outward=True, open_faces=[top_ref])
        check(
            opened["result"]["cavities"][0]["maxs"] == [128, 128, 256]
            and len(opened["result"]["brushes"]) == 5,
            "Outward open shell preserves the source cavity",
        )
        client.call("cancel_hollow", {"token": opened["token"]})
        draft = preview(16, outward=True)
        current = draft["token"]
        check(
            draft["result"]["cavities"][0]["mins"] == [-128, -128, 0]
            and draft["result"]["cavities"][0]["maxs"] == [128, 128, 256],
            "Outward walls preserve the original cavity size",
        )
        client.call("commit_hollow", {"token": token}, error=True)
        client.call("cancel_hollow", {"token": token}, error=True)
        check(
            state()["preview"]["token"] == current,
            "Stale tokens preserve a replacement shell",
        )
        client.call("cancel_hollow", {"token": current})
        draft = preview()
        client.call("look_inside_hollow", {"token": draft["token"]})
        check(
            client.render(output / "interior-again.png") == interior,
            "Repeated preview produces identical geometry",
        )
        committed = client.call("commit_hollow", {"token": draft["token"]})
        check(
            committed == draft["result"],
            "Apply preserves planned object IDs and cavity metadata",
        )
        check(
            client.render(output / "interior-applied.png") == interior,
            "Applied shell matches preview pixels",
        )
        walls = detail(client, committed["objects"])
        faces = [face for wall in walls for face in wall["faces"]]
        check(
            len({face["side"] for face in faces}) == len(faces),
            "Generated wall face IDs are unique",
        )
        check(
            all(
                face["lightmap_scale"] == 8
                and face["smoothing_groups"] == [3, 9]
                and face["shift_u"] == 13.5
                for face in faces
            ),
            "Inner and outer surfaces inherit source mapping and lighting",
        )
        check(
            len(client.call("get_map_info")["undo"]) == len(history) + 1,
            "All walls share one undo step",
        )
        client.call("undo")
        check(detail(client, [block]) == original, "Undo restores the original solid")
        client.call("redo")
        check(
            detail(client, committed["objects"]) == walls,
            "Redo restores every wall and face",
        )
        saved = output / "hollow.vmf"
        client.call("save_map", {"path": str(saved)})
        client.call("open_map", {"path": str(saved)})
        check(
            detail(client, committed["objects"]) == walls,
            "Shells survive VMF save and reopen",
        )
        client.call(
            "create_block",
            {
                "mins": [-64, -64, 16],
                "maxs": [64, 64, 96],
                "material": "dev/dev_measuregeneric01",
            },
        )
        choices = state()["faces"]
        top = next(face for face in choices if face["normal"] == [0, 0, 1])
        opened = preview(8, open_faces=[{key: top[key] for key in ["solid", "side"]}])
        nested_walls = client.call("commit_hollow", {"token": opened["token"]})
        nested_detail = detail(client, nested_walls["objects"])
        saved_open = output / "hollow-openings.vmf"
        client.call("save_map", {"path": str(saved_open)})
        client.call("open_map", {"path": str(saved_open)})
        check(
            detail(client, nested_walls["objects"]) == nested_detail,
            "Open shells survive VMF save and reopen",
        )
        client.call(
            "create_entity",
            {"classname": "info_player_counterterrorist", "origin": [0, 96, 32]},
        )
        compiled = client.call("compile_map", {"path": str(output / "hollow.bsp")})
        check(
            compiled["success"] and Path(compiled["bsp"]).is_file(),
            "Generated room containing an open shell is sealed and compiles natively",
        )
        (output / "compile.json").write_text(json.dumps(compiled, indent=2) + "\n")
        other = client.call(
            "create_block", {"mins": [384, 0, 0], "maxs": [512, 128, 128]}
        )["id"]
        entity = client.call(
            "tie_to_entity", {"ids": [other], "classname": "func_detail"}
        )["id"]
        before_entity = detail(client, [entity])
        choices = state()["faces"]
        top = next(face for face in choices if face["normal"] == [0, 0, 1])
        draft = preview(8, open_faces=[{key: top[key] for key in ["solid", "side"]}])
        check(
            draft["result"]["objects"] == [entity],
            "Brush-entity preview retains its owner",
        )
        result = client.call("commit_hollow", {"token": draft["token"]})
        check(
            result["objects"] == [entity]
            and len(detail(client, [entity])[0]["brushes"]) == 5,
            "Entity owns all generated wall pieces",
        )
        client.call("undo")
        check(
            detail(client, [entity]) == before_entity,
            "Entity hollow is atomically reversible",
        )
        wedge = client.call(
            "create_primitive",
            {
                "shape": "wedge",
                "mins": [-96, -80, 112],
                "maxs": [96, 80, 224],
                "material": "dev/dev_measuregeneric01",
            },
        )["objects"][0]["id"]
        wedge_entity = client.call(
            "tie_to_entity", {"ids": [wedge], "classname": "func_detail"}
        )["id"]
        wedge_before = detail(client, [wedge_entity])
        choices = state()["faces"]
        slope = next(
            face
            for face in choices
            if sum(abs(value) > 0.01 for value in face["normal"]) > 1
        )
        slope_ref = {key: slope[key] for key in ["solid", "side"]}
        for outward in [False, True]:
            closed = preview(4, outward, ids=[wedge_entity])
            center = closed["result"]["cavities"][0]["center"]
            direction = [a - b for a, b in zip(slope["center"], center, strict=True)]
            hit = client.call("raycast", {"origin": center, "direction": direction})
            check(
                hit.get("solid") in closed["result"]["brushes"],
                "Closed sloped shell blocks a ray toward its sloped face",
            )
            opened = preview(4, outward, ids=[wedge_entity], open_faces=[slope_ref])
            center = opened["result"]["cavities"][0]["center"]
            direction = [a - b for a, b in zip(slope["center"], center, strict=True)]
            hit = client.call("raycast", {"origin": center, "direction": direction})
            check(
                hit.get("solid") not in opened["result"]["brushes"],
                "Opening the sloped face clears the passage in both wall directions",
            )
            check(
                len(opened["result"]["brushes"]) == len(choices) - 1
                and opened["result"]["objects"] == [wedge_entity],
                "Sloped opening keeps the remaining walls in their entity",
            )
            client.call("commit_hollow", {"token": opened["token"]})
            if outward:
                compiled = client.call(
                    "compile_map", {"path": str(output / "hollow-sloped.bsp")}
                )
                check(
                    compiled["success"] and compiled["leak"] is None,
                    "Sealed fixture with an open sloped brush entity compiles",
                )
                (output / "compile-sloped.json").write_text(
                    json.dumps(compiled, indent=2) + "\n"
                )
            client.call("undo")
            check(
                detail(client, [wedge_entity]) == wedge_before,
                "Sloped shell undo restores exact entity geometry",
            )
        draft = preview(8, ids=[entity])
        client.call("select", {"ids": [block]})
        client.call("commit_hollow", {"token": draft["token"]}, error=True)
        check(state()["preview"] is None, "Selection changes invalidate the draft")
        draft = preview(8, ids=[entity])
        client.call(
            "create_primitive",
            {
                "shape": "wedge",
                "mins": [0, 0, 0],
                "maxs": [64, 64, 64],
                "preview": True,
            },
        )
        client.call("commit_hollow", {"token": draft["token"]}, error=True)
        check(
            state()["preview"] is None, "Competing geometry preview invalidates hollow"
        )
    checks.append("Original session restored")
    report = {
        "transport": "http" if client.url else "stdio",
        "tools": len(tools),
        "checks": len(checks),
        "passed": checks,
    }
    (output / "validation.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({key: value for key, value in report.items() if key != "passed"}))


def main():
    parser = argparse.ArgumentParser(
        description="Validate brush hollowing through MCP; HTTP requires a clean idle map with empty history."
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
