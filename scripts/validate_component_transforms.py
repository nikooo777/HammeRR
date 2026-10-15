import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail


def validate(client, output, render=False):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def inspect(solid=None):
        return client.call("get_brush_components", {"solids": [solid]} if solid else {})

    def select(components, solid):
        return client.call(
            "select_brush_components",
            {"components": components, "snapshot": inspect(solid)["snapshot"]},
        )

    tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
    check(
        {"preview_component_scale", "preview_component_rotate"} <= tools,
        "Scaling and rotation tools are discoverable",
    )
    fixtures = [
        ("face-scale", "scale", [0.5, 0.5, 1], False, None),
        ("face-rotate", "rotate", [30, 0, 0], False, [32, 32, 64]),
        ("edge-scale", "scale", [1, 1, 0.75], False, [0, 0, 0]),
        ("whole-scale", "scale", [1.5, 0.75, 1.25], True, [8.125, 16.25, 24.5]),
        ("whole-rotate", "rotate", [15, -20, 30], True, None),
    ]
    for label, kind, value, attached, pivot in fixtures:
        client.call("new_map", {"discard_changes": True})
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
        solid = int(ids[0].split(":")[1])
        shape = inspect(solid)["brushes"][0]
        vertices = shape["vertices"]
        if label.startswith("whole"):
            components = [
                {"solid": solid, "sides": vertex["sides"]} for vertex in vertices
            ]
        elif label.startswith("edge"):
            edge = next(
                edge
                for edge in shape["edges"]
                if all(
                    abs(vertices[index]["position"][1]) < 1e-5
                    and abs(vertices[index]["position"][2] - 64) < 1e-5
                    for index in edge["vertices"]
                )
            )
            components = [{"solid": solid, "sides": edge["sides"]}]
        else:
            face = next(
                face
                for face in shape["faces"]
                if all(
                    abs(vertices[index]["position"][2] - 64) < 1e-5
                    for index in face["vertices"]
                )
            )
            components = [{"solid": solid, "sides": [face["side"]]}]
        selected = select(components, solid)
        client.call(
            "set_component_move_options",
            {"enabled": True, "snapshot": inspect(solid)["snapshot"]},
        )
        expected_center = (
            [32, 32, 32]
            if label.startswith("whole")
            else [32, 0, 64]
            if label.startswith("edge")
            else [32, 32, 64]
        )
        check(
            selected["component_center"] == expected_center,
            f"{label}: source selection exposes its bounding-box center",
        )
        check(
            bool(client.call("get_component_handles")["handles"]),
            f"{label}: source selection has move handles",
        )
        if render:
            client.call(
                "set_component_move_options",
                {"enabled": False, "snapshot": inspect(solid)["snapshot"]},
            )
            client.call(
                "set_camera", {"position": [160, -180, 160], "look_at": [32, 32, 32]}
            )
            baseline_pixels = client.render(
                output / f"{label}-before.png", surface="flat"
            )
        original = detail(client, ids)
        history = client.call("get_map_info")["undo"]
        args = {
            "factors" if kind == "scale" else "degrees": value,
            "texture_lock": attached,
        }
        if pivot is not None:
            args["pivot"] = pivot
        tool = f"preview_component_{kind}"

        def preview(solid_id=solid, tool_name=tool, parameters=args):
            return client.call(
                tool_name, {**parameters, "snapshot": inspect(solid_id)["snapshot"]}
            )

        snapshot = inspect(solid)["snapshot"]
        draft = preview()
        token = draft["preview"]["token"]
        target_shape = draft["brushes"][0]
        transform = draft["preview"]["transform"]
        check(
            draft["preview"]["operation"] == kind
            and transform["value"] == value
            and transform["pivot"] == pivot
            and transform["resolved_pivot"] == (pivot or expected_center)
            and draft["source_selection"] == selected["selection"],
            f"{label}: draft reports exact operation, values and resolved pivot",
        )
        check(
            detail(client, ids) == original
            and client.call("get_map_info")["undo"] == history,
            f"{label}: preview preserves source geometry and history",
        )
        check(
            not client.call("get_component_handles")["handles"],
            f"{label}: affine preview pauses move handles",
        )
        if render:
            preview_pixels = client.render(
                output / f"{label}-preview.png", surface="flat"
            )
            check(
                preview_pixels != baseline_pixels,
                f"{label}: shared renderer shows the draft",
            )
        client.call(tool, {**args, "snapshot": snapshot}, error=True)
        client.call("commit_component_move", {"token": token}, error=True)
        client.call(
            tool,
            {**args, "pivot": [65537, 0, 0], "snapshot": inspect(solid)["snapshot"]},
            error=True,
        )
        if kind == "scale":
            client.call(
                tool,
                {"factors": [0, 1, 1], "snapshot": inspect(solid)["snapshot"]},
                error=True,
            )
        else:
            client.call(
                tool,
                {"degrees": [0, 0, 360], "snapshot": inspect(solid)["snapshot"]},
                error=True,
            )
        check(
            inspect(solid)["preview"]["token"] == token,
            f"{label}: stale and invalid updates retain the valid draft",
        )
        client.call("cancel_component_edit", {"token": token})
        check(
            inspect(solid)["selection"] == selected["selection"]
            and detail(client, ids) == original,
            f"{label}: discard retains source component selection and geometry",
        )
        if render:
            check(
                client.render(output / f"{label}-discarded.png", surface="flat")
                == baseline_pixels,
                f"{label}: discard restores identical pixels",
            )
        draft = preview()
        client.call("commit_component_edit", {"token": token}, error=True)
        client.call("commit_component_edit", {"token": draft["preview"]["token"]})
        result = detail(client, ids)
        check(
            result[1] == original[1]
            and len(client.call("get_map_info")["undo"]) == len(history) + 1,
            f"{label}: apply changes only the target in one undo step",
        )
        source_faces = {face["side"]: face for face in original[0]["faces"]}
        check(
            all(
                all(
                    face[field] == source_faces[face["side"]][field]
                    for field in ["material", "lightmap_scale", "smoothing_groups"]
                )
                for face in result[0]["faces"]
            ),
            f"{label}: side IDs, materials and lighting survive",
        )
        if not attached:
            check(
                all(
                    face[axis] == source_faces[face["side"]][axis]
                    for face in result[0]["faces"]
                    for axis in ["uaxis", "vaxis"]
                ),
                f"{label}: world projections remain exact",
            )
        else:
            after_by_sides = {
                tuple(vertex["sides"]): vertex["position"]
                for vertex in target_shape["vertices"]
            }
            mapped_faces = {face["side"]: face for face in result[0]["faces"]}

            def texel(face, axis, point):
                projection = [
                    float(value)
                    for value in face[axis].replace("[", "").replace("]", "").split()
                ]
                return (
                    sum(a * b for a, b in zip(projection[:3], point)) / projection[4]
                    + projection[3]
                )

            check(
                all(
                    math.isclose(
                        texel(source_faces[side], axis, vertex["position"]),
                        texel(
                            mapped_faces[side],
                            axis,
                            after_by_sides[tuple(vertex["sides"])],
                        ),
                        abs_tol=1e-4,
                    )
                    for vertex in vertices
                    for side in vertex["sides"]
                    for axis in ["uaxis", "vaxis"]
                ),
                f"{label}: attached UVs match at every corresponding corner",
            )
        if render:
            check(
                client.render(output / f"{label}-applied.png", surface="flat")
                == preview_pixels,
                f"{label}: apply matches draft pixels",
            )
        client.call("undo")
        check(
            detail(client, ids) == original,
            f"{label}: undo restores exact planes and projections",
        )
        client.call("redo")
        check(detail(client, ids) == result, f"{label}: redo restores exact result")
        path = output / f"{label}.vmf"
        client.call("save_map", {"path": str(path)})
        client.call("open_map", {"path": str(path)})
        reopened = inspect(solid)["brushes"][0]
        expected = {
            tuple(vertex["sides"]): vertex["position"]
            for vertex in target_shape["vertices"]
        }
        check(
            all(
                all(
                    abs(a - b) < 1e-4
                    for a, b in zip(
                        vertex["position"], expected[tuple(vertex["sides"])]
                    )
                )
                for vertex in reopened["vertices"]
            ),
            f"{label}: transformed topology survives VMF save/reopen",
        )
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
        compiled = client.call(
            "compile_map", {"path": str(output / f"{label}-room.bsp")}
        )
        check(
            compiled["success"]
            and compiled["leak"] is None
            and compiled["stats"]["brushes"] == 8,
            f"{label}: native compiler accepts transformed geometry in a sealed room",
        )
        select(components, solid)
        token = preview()["preview"]["token"]
        client.call(
            "create_entity", {"classname": "info_target", "origin": [0, 0, 128]}
        )
        client.call("commit_component_edit", {"token": token}, error=True)
        check(
            inspect(solid)["preview"] is None,
            f"{label}: source edits invalidate draft tokens",
        )

    client.call("new_map", {"discard_changes": True})
    ids = [
        client.call(
            "create_block",
            {
                "mins": [x * 96, 0, 0],
                "maxs": [x * 96 + 64, 64, 64],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]
        for x in range(20)
    ]
    owner = client.call("tie_to_entity", {"ids": ids, "classname": "func_detail"})["id"]
    solid = int(ids[0].split(":")[1])
    shape = inspect(solid)["brushes"][0]
    components = [
        {"solid": solid, "sides": vertex["sides"]} for vertex in shape["vertices"]
    ]
    select(components, solid)
    before = detail(client, [owner])[0]
    draft = client.call(
        "preview_component_rotate",
        {"degrees": [0, 0, 30], "snapshot": inspect(solid)["snapshot"]},
    )
    check(
        len(draft["brushes"]) == 1,
        "Large owner: rotation response stays scoped to the edited brush",
    )
    applied = client.call("commit_component_edit", {"token": draft["preview"]["token"]})
    after = detail(client, [owner])[0]
    check(
        len(applied["brushes"]) == 1
        and len(after["brushes"]) == 20
        and after["properties"] == before["properties"]
        and after["brushes"][1:] == before["brushes"][1:],
        "Large owner: ownership, properties and nineteen siblings remain exact",
    )
    client.call("undo")
    check(
        detail(client, [owner])[0] == before,
        "Large owner: undo restores every child exactly",
    )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-component-transforms")
    )
    parser.add_argument(
        "--render",
        action="store_true",
        help="Validate offscreen pixels without desktop input",
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output, args.render)
        result = {"passed": len(checks), "checks": checks}
        (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))
    finally:
        client.close()


if __name__ == "__main__":
    main()
