import argparse
import json
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail
from validate_partition_material_metadata import membership, without_save_counter


def validate(client, output, render=False):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def inspect():
        return client.call("get_brush_components")

    def picks(token, points, normal):
        center = [
            sum(point[axis] for point in points) / len(points) for axis in range(3)
        ]
        return client.call(
            "pick_component_partitions",
            {
                "token": token,
                "x": 550,
                "y": 350,
                "width": 1100,
                "height": 700,
                "camera": {
                    "projection": "perspective",
                    "position": [
                        point + axis * 256
                        for point, axis in zip(center, normal, strict=True)
                    ],
                    "look_at": center,
                },
            },
        )["candidates"]

    for owned in [False, True]:
        for reflected in [False, True]:
            for direction in range(4):
                label = f"owned={owned} reflected={reflected} corner={direction}"
                client.call("new_map", {"discard_changes": True})
                object_id = client.call(
                    "create_block",
                    {
                        "mins": [0.125, -7.25, 16.5],
                        "maxs": [64.875, 48.125, 80.25],
                        "material": "brick/brickwall001a",
                    },
                )["id"]
                solid = int(object_id.split(":")[1])
                client.call(
                    "transform_objects", {"ids": [object_id], "rotation": [17, -23, 31]}
                )
                if owned:
                    object_id = client.call(
                        "tie_to_entity",
                        {"ids": [object_id], "classname": "func_detail"},
                    )["id"]
                source = detail(client, [object_id])[0]
                brush = source["brushes"][0] if owned else source
                side = max(brush["faces"], key=lambda face: face["normal"][2])["side"]
                selected = client.call(
                    "select_brush_components",
                    {
                        "components": [{"solid": solid, "sides": [side]}],
                        "snapshot": inspect()["snapshot"],
                    },
                )
                choices = selected["partition_choices"][0]["pairs"]
                check(
                    len(choices) == 6
                    and choices[:2] == selected["crease_choices"][0]["pairs"],
                    f"{label}: legacy directions and four adjacent choices",
                )
                flat = client.call(
                    "preview_component_partition",
                    {
                        "pairs": [
                            {"solid": solid, "edges": choices[direction + 2]["edges"]}
                        ],
                        "snapshot": inspect()["snapshot"],
                    },
                )["preview"]
                part = flat["partition"]["partitions"][0]
                check(
                    not flat["partition"]["ridge_supported"],
                    f"{label}: shared inspection identifies unsupported roof shapes",
                )
                check(
                    [len(piece["vertices"]) for piece in part["loops"]] == [3, 5],
                    f"{label}: triangle and pentagon",
                )
                flat = client.call(
                    "update_component_partition_cut",
                    {"token": flat["token"], "edge_parameters": [0.2, 0.8]},
                )["preview"]
                for piece, material in enumerate(
                    ["brick/brickwall003a", "de_cbble/woodfloor01"]
                ):
                    flat = client.call(
                        "update_component_partition_material",
                        {
                            "token": flat["token"],
                            "partitions": [{"solid": solid, "partition": piece}],
                            "material": material,
                        },
                    )["preview"]
                part = flat["partition"]["partitions"][0]
                original_membership = membership(part)
                client.call("store_component_partition", {"token": flat["token"]})
                check(
                    detail(client, [object_id])[0] == source,
                    f"{label}: Keep retains authored planes",
                )
                if reflected:
                    client.call(
                        "transform_objects",
                        {"ids": [object_id], "reflection": [True, False, False]},
                    )
                saved = client.call("get_saved_component_partitions")["entries"][0]
                check(
                    saved["valid"] and saved["edge_parameters"] == [0.2, 0.8],
                    f"{label}: saved adjacent positions resolve",
                )
                path = (
                    output / f"boundary-{int(owned)}-{int(reflected)}-{direction}.vmf"
                )
                client.call("save_map", {"path": str(path)})
                kept = without_save_counter(path)
                client.call("open_map", {"path": str(path), "discard_changes": True})
                source = detail(client, [object_id])[0]
                history = client.call("get_map_info")["undo"]
                brush = source["brushes"][0] if owned else source
                normal = next(
                    face["normal"] for face in brush["faces"] if face["side"] == side
                )
                client.call(
                    "set_component_picking",
                    {
                        "enabled": True,
                        "kind": "face",
                        "solids": [solid],
                        "snapshot": inspect()["snapshot"],
                    },
                )
                flat = client.call(
                    "resume_component_partitions",
                    {
                        "sources": [{"solid": solid, "side": side}],
                        "snapshot": inspect()["snapshot"],
                    },
                )["preview"]
                part = flat["partition"]["partitions"][0]
                check(
                    membership(part) == original_membership,
                    f"{label}: stable corner identity after reopening and reflection",
                )
                check(
                    [piece["top_material"] for piece in part["loops"]]
                    == ["brick/brickwall003a", "de_cbble/woodfloor01"],
                    f"{label}: independent materials reopen",
                )
                error = client.call(
                    "raise_component_partition",
                    {"token": flat["token"], "rise": 16},
                    error=True,
                )
                check(
                    "two quad" in str(error) and inspect()["preview"] == flat,
                    f"{label}: unsupported roofs retain flat token and report",
                )
                error = client.call(
                    "update_component_partition_cut",
                    {"token": flat["token"], "edge_parameters": [0.0, 0.8]},
                    error=True,
                )
                check(
                    error and inspect()["preview"] == flat,
                    f"{label}: collapsed cuts retain the last valid draft",
                )
                for piece in part["loops"]:
                    points = [
                        part["vertices"][index]["position"]
                        for index in piece["vertices"]
                    ]
                    hits = picks(flat["token"], points, normal)
                    check(
                        any(
                            hit["partition"]
                            == {"solid": solid, "partition": piece["id"]}
                            for hit in hits
                        ),
                        f"{label}: flat loop containment uses its actual length",
                    )
                camera = {
                    "position": [90, -85, 135],
                    "look_at": [32, 20, 60],
                    "projection": "perspective",
                }
                if render:
                    baseline = client.render(
                        output / f"flat-{int(owned)}-{int(reflected)}-{direction}.png",
                        surface="textured",
                        camera=camera,
                    )
                arguments = {
                    "token": flat["token"],
                    "height": 16.125,
                    "texture_lock": True,
                    "wall_material": "brick/brickwall001a",
                }
                draft = client.call("preview_partition_extrusion", arguments)["preview"]
                pieces = draft["extrusion"]["pieces"]
                check(
                    [len(piece["faces"]) for piece in pieces] == [5, 7],
                    f"{label}: bounded prism planes",
                )
                check(
                    detail(client, [object_id])[0] == source
                    and client.call("get_map_info")["undo"] == history,
                    f"{label}: extrusion draft retains map and history",
                )
                for piece in pieces:
                    top = next(
                        face for face in piece["faces"] if face["surface"] == "top"
                    )
                    check(
                        top["material"]
                        == part["loops"][piece["partition"]]["effective_material"],
                        f"{label}: planned top material follows original piece",
                    )
                    check(
                        sum(face["surface"] == "divider" for face in piece["faces"])
                        == 1,
                        f"{label}: one divider wall per prism",
                    )
                    points = [
                        [
                            value + axis * arguments["height"]
                            for value, axis in zip(
                                part["vertices"][index]["position"], normal, strict=True
                            )
                        ]
                        for index in part["loops"][piece["partition"]]["vertices"]
                    ]
                    hits = picks(draft["token"], points, normal)
                    check(
                        any(
                            hit["partition"]
                            == {"solid": solid, "partition": piece["partition"]}
                            for hit in hits
                        ),
                        f"{label}: triangle/pentagon top picks the correct body",
                    )
                if render:
                    raised = client.render(
                        output
                        / f"extruded-{int(owned)}-{int(reflected)}-{direction}.png",
                        surface="textured",
                        camera=camera,
                    )
                    check(
                        raised != baseline, f"{label}: prisms change rendered geometry"
                    )
                client.call("cancel_component_edit", {"token": draft["token"]})
                client.call("save_map", {"path": str(path)})
                check(
                    without_save_counter(path) == kept,
                    f"{label}: Cancel restores exact saved map",
                )
                flat = client.call(
                    "resume_component_partitions",
                    {
                        "sources": [{"solid": solid, "side": side}],
                        "snapshot": inspect()["snapshot"],
                    },
                )["preview"]
                if render:
                    cancelled = client.render(
                        output
                        / f"cancelled-{int(owned)}-{int(reflected)}-{direction}.png",
                        surface="textured",
                        camera=camera,
                    )
                    check(
                        cancelled == baseline, f"{label}: Cancel restores exact pixels"
                    )
                draft = client.call(
                    "preview_partition_extrusion", {**arguments, "token": flat["token"]}
                )["preview"]
                check(
                    draft["extrusion"]["pieces"] == pieces,
                    f"{label}: exact allocation replay after Cancel",
                )
                client.call("commit_component_edit", {"token": draft["token"]})
                check(
                    inspect()["preview"] is None
                    and len(client.call("get_map_info")["undo"]) == len(history) + 1,
                    f"{label}: Apply records one shared edit",
                )
                client.call("undo")
                check(
                    detail(client, [object_id])[0] == source
                    and client.call("get_saved_component_partitions")["entries"][0][
                        "valid"
                    ],
                    f"{label}: undo preserves source and saved intent",
                )
                client.call("redo")
                client.call("save_map", {"path": str(path)})
                client.call("open_map", {"path": str(path), "discard_changes": True})
                check(
                    client.call("get_saved_component_partitions")["entries"][0][
                        "valid"
                    ],
                    f"{label}: applied VMF reopens with valid reusable source",
                )
    client.call("new_map", {"discard_changes": True})
    components = []
    for index in range(16):
        object_id = client.call(
            "create_block",
            {
                "mins": [index * 96, 0, 0],
                "maxs": [index * 96 + 64, 64, 64],
                "material": "brick/brickwall001a",
            },
        )["id"]
        solid = int(object_id.split(":")[1])
        if index % 2:
            object_id = client.call(
                "tie_to_entity", {"ids": [object_id], "classname": "func_detail"}
            )["id"]
        source = detail(client, [object_id])[0]
        brush = source["brushes"][0] if index % 2 else source
        side = max(brush["faces"], key=lambda face: face["normal"][2])["side"]
        components.append({"solid": solid, "sides": [side]})
    selected = client.call(
        "select_brush_components",
        {"components": components, "snapshot": inspect()["snapshot"]},
    )
    choices = selected["partition_choices"]
    inputs = [
        {
            "solid": choice["component"]["solid"],
            "edges": choice["pairs"][index % 6]["edges"],
        }
        for index, choice in enumerate(choices)
    ]
    flat = client.call(
        "preview_component_partition",
        {"pairs": inputs, "snapshot": inspect()["snapshot"]},
    )["preview"]
    flat = client.call(
        "update_component_partition_cut",
        {"token": flat["token"], "edge_parameters": [0.2, 0.8]},
    )["preview"]
    for piece, material in enumerate(["brick/brickwall003a", "de_cbble/woodfloor01"]):
        flat = client.call(
            "update_component_partition_material",
            {
                "token": flat["token"],
                "partitions": [
                    {"solid": component["solid"], "partition": piece}
                    for component in components
                ],
                "material": material,
            },
        )["preview"]
    check(
        len(flat["partition"]["partitions"]) == 16,
        "batch: sixteen mixed boundary sources",
    )
    client.call("store_component_partition", {"token": flat["token"]})
    check(
        len(client.call("get_saved_component_partitions")["entries"]) == 16,
        "batch: all sources stored atomically",
    )
    client.call("undo")
    check(
        not client.call("get_saved_component_partitions")["entries"],
        "batch: one undo removes all source intents",
    )
    client.call("redo")
    sources = [
        {"solid": component["solid"], "side": component["sides"][0]}
        for component in components
    ]
    flat = client.call(
        "resume_component_partitions",
        {"sources": sources, "snapshot": inspect()["snapshot"]},
    )["preview"]
    error = client.call(
        "raise_component_partition", {"token": flat["token"], "rise": 16}, error=True
    )
    check(
        "two quad" in str(error) and inspect()["preview"] == flat,
        "batch: unsupported mixed roofs reject the complete batch",
    )
    arguments = {"token": flat["token"], "height": 16.125, "texture_lock": True}
    draft = client.call("preview_partition_extrusion", arguments)["preview"]
    pieces = draft["extrusion"]["pieces"]
    check(
        len(pieces) == 32 and sum(len(piece["faces"]) for piece in pieces) == 192,
        "batch: thirty-two prisms retain the bounded face budget",
    )
    client.call("cancel_component_edit", {"token": draft["token"]})
    flat = client.call(
        "resume_component_partitions",
        {"sources": sources, "snapshot": inspect()["snapshot"]},
    )["preview"]
    draft = client.call(
        "preview_partition_extrusion", {**arguments, "token": flat["token"]}
    )["preview"]
    check(
        draft["extrusion"]["pieces"] == pieces,
        "batch: exact thirty-two-prism allocation replay",
    )
    client.call("commit_component_edit", {"token": draft["token"]})
    check(
        len(client.call("get_selection")["face_selection"]["faces"]) == 192,
        "batch: Apply selects every resulting face",
    )
    client.call("undo")
    check(
        all(
            entry["valid"]
            for entry in client.call("get_saved_component_partitions")["entries"]
        ),
        "batch: undo restores every reusable source",
    )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-boundary-partitions")
    )
    parser.add_argument("--render", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output, args.render)
        (args.output / "results.json").write_text(
            json.dumps({"checks": len(checks), "passed": checks}, indent=2) + "\n"
        )
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
