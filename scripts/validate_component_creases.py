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

    def inspect(ids=None):
        return client.call("get_brush_components", {"solids": ids} if ids else {})

    def block(mins, maxs):
        return client.call(
            "create_block",
            {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"},
        )["id"]

    def select_top(solid):
        shape = inspect([solid])["brushes"][0]
        face = next(
            face
            for face in shape["faces"]
            if all(
                abs(shape["vertices"][index]["position"][2] - 64) < 1e-8
                for index in face["vertices"]
            )
        )
        return client.call(
            "select_brush_components",
            {
                "components": [{"solid": solid, "sides": [face["side"]]}],
                "snapshot": inspect()["snapshot"],
            },
        )

    def preview(pairs, rise=16.125, snapshot=None, error=False):
        return client.call(
            "preview_component_crease",
            {
                "pairs": pairs,
                "rise": rise,
                "snapshot": snapshot or inspect()["snapshot"],
            },
            error=error,
        )

    tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
    check(
        {
            "preview_component_crease",
            "commit_component_edit",
            "cancel_component_edit",
        }
        <= tools,
        "Ridge preview, apply and cancel tools are discoverable",
    )
    for index in range(2):
        client.call("new_map", {"discard_changes": True})
        ids = [block([x, 0, 0], [x + 64, 64, 64]) for x in [0, 128]]
        owner = client.call("tie_to_entity", {"ids": ids, "classname": "func_detail"})[
            "id"
        ]
        solids = [int(value.split(":")[1]) for value in ids]
        original = detail(client, [owner])[0]
        client.call("set_transform_tool", {"mode": "select"})
        selected = select_top(solids[0])
        shape = selected["brushes"][0]
        source_selection = selected["selection"]
        choices = selected["crease_choices"]
        pair = choices[0]["pairs"][index]
        inputs = [{"solid": solids[0], "edges": pair["edges"]}]
        check(
            len(choices) == 1
            and len(choices[0]["pairs"]) == 2
            and all(len(pair["edges"]) == 2 for pair in choices[0]["pairs"]),
            f"Pair {index}: source face exposes both stable opposing edge choices",
        )
        client.call(
            "set_component_picking",
            {
                "enabled": True,
                "kind": "face",
                "solids": [solids[0]],
                "snapshot": inspect()["snapshot"],
            },
        )
        client.call(
            "set_component_move_options",
            {"enabled": False, "snapshot": inspect()["snapshot"]},
        )
        if render:
            client.call(
                "set_camera", {"position": [170, -200, 150], "look_at": [45, 32, 38]}
            )
            baseline = client.render(
                output / f"ridge-{index}-before.png", surface="flat"
            )
        history = client.call("get_map_info")["undo"]
        stamp = inspect()["snapshot"]
        draft = preview(inputs)
        token = draft["preview"]["token"]
        change = draft["preview"]["crease"]["changes"][0]
        check(
            draft["preview"]["operation"] == "crease"
            and len(draft["brushes"][0]["vertices"]) == 10
            and len(draft["brushes"][0]["faces"]) == 7,
            f"Pair {index}: crease generates two ridge corners and one extra roof face",
        )
        check(
            all(
                math.dist(position, [point[0], point[1], point[2] + 16.125]) < 1e-8
                for position, point in zip(
                    change["positions"], pair["midpoints"], strict=True
                )
            ),
            f"Pair {index}: ridge raises both selected edge midpoints along the face normal",
        )
        check(
            all(
                any(
                    math.dist(old["position"], vertex["position"]) < 1e-8
                    for vertex in draft["brushes"][0]["vertices"]
                )
                for old in shape["vertices"]
            ),
            f"Pair {index}: every original corner stays fixed",
        )
        check(
            draft["source_selection"] == source_selection
            and len(draft["selection"]) == 2
            and draft["crease_choices"] == choices,
            f"Pair {index}: original choices remain while new ridge corners are selected",
        )
        check(
            detail(client, [owner])[0] == original
            and client.call("get_map_info")["undo"] == history,
            f"Pair {index}: preview preserves source objects and history",
        )
        if render:
            draft_pixels = client.render(
                output / f"ridge-{index}-preview.png", surface="flat"
            )
            check(
                draft_pixels != baseline,
                f"Pair {index}: shared renderer displays the ridge",
            )
        for rise in [0, -1, 0.01, 4097]:
            preview(inputs, rise, error=True)
        for invalid in [
            [],
            inputs * 2,
            [{"solid": solids[0], "edges": [pair["edges"][0]] * 2}],
            [{"solid": solids[1], "edges": pair["edges"]}],
            [*inputs, {"solid": solids[1], "edges": pair["edges"]}],
        ]:
            preview(invalid, error=True)
        preview(inputs, snapshot=stamp, error=True)
        client.call("commit_component_move", {"token": token}, error=True)
        client.call(
            "select_brush_components",
            {"components": [], "snapshot": inspect()["snapshot"]},
            error=True,
        )
        check(
            inspect()["preview"]["token"] == token
            and detail(client, [owner])[0] == original,
            f"Pair {index}: invalid rises, pairs, stale stamps and incompatible edits retain the draft",
        )
        cancelled = client.call("cancel_component_edit", {"token": token})
        check(
            cancelled["selection"] == source_selection
            and cancelled["picking"]["kind"] == "face"
            and client.call("get_map_info")["undo"] == history,
            f"Pair {index}: discard restores source selection and picking without an edit",
        )
        if render:
            check(
                client.render(output / f"ridge-{index}-cancelled.png", surface="flat")
                == baseline,
                f"Pair {index}: cancellation restores exact pixels",
            )
        reversed_inputs = [{"solid": solids[0], "edges": pair["edges"][::-1]}]
        current = preview(reversed_inputs)
        new_token = current["preview"]["token"]
        check(
            current["preview"]["crease"]["changes"]
            == draft["preview"]["crease"]["changes"],
            f"Pair {index}: edge order is canonical and cancelled drafts consume no IDs",
        )
        client.call("commit_component_edit", {"token": token}, error=True)
        client.call("cancel_component_edit", {"token": token}, error=True)
        applied = client.call("commit_component_edit", {"token": new_token})
        result = detail(client, [owner])[0]
        check(
            applied["selection"] == current["selection"]
            and applied["picking"]["kind"] == "vertex"
            and applied["picking"]["solids"] == [solids[0]],
            f"Pair {index}: apply retains brush scope and selects both ridge corners",
        )
        check(
            applied["brushes"] == current["brushes"]
            and len(client.call("get_map_info")["undo"]) == len(history) + 1,
            f"Pair {index}: applied topology matches the draft in one undo step",
        )
        check(
            result["properties"] == original["properties"]
            and result["brushes"][1] == original["brushes"][1],
            f"Pair {index}: ownership, entity properties and sibling brush are preserved",
        )
        old_faces = {face["side"]: face for face in original["brushes"][0]["faces"]}
        fields = ["material", "uaxis", "vaxis", "lightmap_scale", "smoothing_groups"]
        check(
            all(
                all(
                    face[field]
                    == old_faces[
                        change["source_side"]
                        if face["side"] == change["new_side"]
                        else face["side"]
                    ][field]
                    for field in fields
                )
                for face in result["brushes"][0]["faces"]
            ),
            f"Pair {index}: source and new sides preserve material, precise world UVs and lighting",
        )
        if render:
            check(
                client.render(output / f"ridge-{index}-applied.png", surface="flat")
                == draft_pixels,
                f"Pair {index}: applied geometry and guides match preview pixels",
            )
        client.call("undo")
        check(
            detail(client, [owner])[0] == original,
            f"Pair {index}: undo restores exact planes and IDs",
        )
        client.call("redo")
        check(
            detail(client, [owner])[0] == result,
            f"Pair {index}: redo restores exact generated planes and IDs",
        )
        saved = output / f"ridge-{index}.vmf"
        client.call("save_map", {"path": str(saved)})
        client.call("open_map", {"path": str(saved)})
        check(
            len(inspect([solids[0]])["brushes"][0]["vertices"]) == 10,
            f"Pair {index}: closed ridge topology survives save and reopen",
        )
        select_top(solids[1])
        other_pairs = inspect()["crease_choices"][0]["pairs"]
        token = preview([{"solid": solids[1], "edges": other_pairs[0]["edges"]}])[
            "preview"
        ]["token"]
        client.call(
            "create_entity", {"classname": "info_target", "origin": [0, 0, 128]}
        )
        client.call("commit_component_edit", {"token": token}, error=True)
        check(
            inspect()["preview"] is None,
            f"Pair {index}: source edits invalidate the draft",
        )

        client.call("new_map", {"discard_changes": True})
        for mins, maxs in [
            ([-272, -272, -16], [272, 272, 0]),
            ([-272, -272, 256], [272, 272, 272]),
            ([-272, -272, 0], [-256, 272, 256]),
            ([256, -272, 0], [272, 272, 256]),
            ([-256, -272, 0], [256, -256, 256]),
            ([-256, 256, 0], [256, 272, 256]),
            ([0, 0, 0], [64, 64, 64]),
        ]:
            ridge = block(mins, maxs)
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [-64, 0, 8]},
        )
        solid = int(ridge.split(":")[1])
        selected = select_top(solid)
        pair = selected["crease_choices"][0]["pairs"][index]
        draft = preview([{"solid": solid, "edges": pair["edges"]}])
        client.call("commit_component_edit", {"token": draft["preview"]["token"]})
        saved = output / f"ridge-{index}-room.vmf"
        client.call("save_map", {"path": str(saved)})
        client.call("open_map", {"path": str(saved)})
        compiled = client.call(
            "compile_map", {"path": str(output / f"ridge-{index}-room.bsp")}
        )
        check(
            compiled["success"]
            and compiled["leak"] is None
            and compiled["stats"]["brushes"] == 7,
            f"Pair {index}: native compiler accepts the saved sealed-room ridge",
        )

    client.call("new_map", {"discard_changes": True})
    solids = [int(block([x, 0, 0], [x + 64, 64, 64]).split(":")[1]) for x in [0, 128]]
    components = []
    pairs = []
    for index, solid in enumerate(solids):
        report = select_top(solid)
        components.extend(report["selection"])
        pairs.append(
            {
                "solid": solid,
                "edges": report["crease_choices"][0]["pairs"][index]["edges"],
            }
        )
    client.call(
        "select_brush_components",
        {"components": components, "snapshot": inspect()["snapshot"]},
    )
    draft = preview(pairs[::-1])
    check(
        len(draft["preview"]["crease"]["changes"]) == 2
        and len(draft["selection"]) == 4,
        "Batch crease supports independently chosen ridge directions across two brushes",
    )
    history = client.call("get_map_info")["undo"]
    client.call("commit_component_edit", {"token": draft["preview"]["token"]})
    check(
        len(client.call("get_map_info")["undo"]) == len(history) + 1,
        "Batch apply records one undo step for all edited brushes",
    )
    client.call("undo")
    check(
        all(len(shape["vertices"]) == 8 for shape in inspect(solids)["brushes"]),
        "Batch undo restores both original brushes",
    )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-crease-validation")
    )
    parser.add_argument("--render", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output, args.render)
        result = {"checks": len(checks), "passed": checks, "render": args.render}
        (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
