import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_partition_material_metadata import membership, without_save_counter


def validate(client, output, render=False):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def inspect():
        return client.call("get_brush_components")

    def resume(solid, side):
        return client.call(
            "resume_component_partitions",
            {
                "sources": [{"solid": solid, "side": side}],
                "snapshot": inspect()["snapshot"],
            },
        )["preview"]

    def pick(token, points, normal):
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
                        value + axis * 256
                        for value, axis in zip(center, normal, strict=True)
                    ],
                    "look_at": center,
                },
            },
        )["candidates"]

    materials = ["brick/brickwall003a", "de_cbble/woodfloor01"]
    for corners in [3, 5, 6, 8]:
        for owned in [False, True]:
            for reflected in [False, True]:
                for direction in [0, corners // 2, corners * (corners - 1) // 2 - 1]:
                    label = f"corners={corners} owned={owned} reflected={reflected} pair={direction}"
                    client.call("new_map", {"discard_changes": True})
                    object_id = client.call(
                        "create_primitive",
                        {
                            "shape": "cylinder",
                            "segments": corners,
                            "mins": [0.125, -7.25, 16.5],
                            "maxs": [64.875, 48.125, 80.25],
                            "material": "brick/brickwall001a",
                        },
                    )["objects"][0]["id"]
                    solid = int(object_id.split(":")[1])
                    client.call(
                        "transform_objects",
                        {"ids": [object_id], "rotation": [17, -23, 31]},
                    )
                    if owned:
                        object_id = client.call(
                            "tie_to_entity",
                            {"ids": [object_id], "classname": "func_detail"},
                        )["id"]
                    source = detail(client, [object_id])[0]
                    brush = source["brushes"][0] if owned else source
                    side = max(brush["faces"], key=lambda face: face["normal"][2])[
                        "side"
                    ]
                    selected = client.call(
                        "select_brush_components",
                        {
                            "components": [{"solid": solid, "sides": [side]}],
                            "snapshot": inspect()["snapshot"],
                        },
                    )
                    choices = selected["partition_choices"][0]["pairs"]
                    check(
                        len(choices) == corners * (corners - 1) // 2
                        and not selected["crease_choices"],
                        f"{label}: complete polygon choices and gated direct ridge",
                    )
                    flat = client.call(
                        "preview_component_partition",
                        {
                            "pairs": [
                                {"solid": solid, "edges": choices[direction]["edges"]}
                            ],
                            "snapshot": inspect()["snapshot"],
                        },
                    )["preview"]
                    for parameters in [[0.2, 0.8], [0.8, 0.2]]:
                        flat = client.call(
                            "update_component_partition_cut",
                            {"token": flat["token"], "edge_parameters": parameters},
                        )["preview"]
                        part = flat["partition"]["partitions"][0]
                        check(
                            len(part["vertices"]) == corners + 2
                            and sum(len(piece["vertices"]) for piece in part["loops"])
                            == corners + 4
                            and all(
                                3 <= len(piece["vertices"]) <= 9
                                for piece in part["loops"]
                            ),
                            f"{label}: bounded complete loops at {parameters}",
                        )
                    for piece, material in enumerate(materials):
                        flat = client.call(
                            "update_component_partition_material",
                            {
                                "token": flat["token"],
                                "partitions": [{"solid": solid, "partition": piece}],
                                "material": material,
                            },
                        )["preview"]
                    original_membership = membership(flat["partition"]["partitions"][0])
                    client.call("store_component_partition", {"token": flat["token"]})
                    check(
                        detail(client, [object_id])[0] == source,
                        f"{label}: Keep retains source geometry",
                    )
                    if reflected:
                        client.call(
                            "transform_objects",
                            {"ids": [object_id], "reflection": [True, False, False]},
                        )
                    path = (
                        output
                        / f"polygon-{corners}-{int(owned)}-{int(reflected)}-{direction}.vmf"
                    )
                    client.call("save_map", {"path": str(path)})
                    kept = without_save_counter(path)
                    check(
                        '"polygon_boundary_edges_materials"' in path.read_text()
                        and path.read_text().count('"piece_0_corner"')
                        == len(original_membership[0])
                        and path.read_text().count('"piece_1_corner"')
                        == len(original_membership[1]),
                        f"{label}: explicit original membership saved for both pieces",
                    )
                    client.call(
                        "open_map", {"path": str(path), "discard_changes": True}
                    )
                    source = detail(client, [object_id])[0]
                    brush = source["brushes"][0] if owned else source
                    normal = next(
                        face["normal"]
                        for face in brush["faces"]
                        if face["side"] == side
                    )
                    history = client.call("get_map_info")["undo"]
                    saved = client.call("get_saved_component_partitions")["entries"][0]
                    check(
                        saved["valid"] and saved["edge_parameters"] == [0.8, 0.2],
                        f"{label}: saved polygon resolves after reflection and reopening",
                    )
                    flat = resume(solid, side)
                    part = flat["partition"]["partitions"][0]
                    check(
                        membership(part) == original_membership
                        and [piece["top_material"] for piece in part["loops"]]
                        == materials,
                        f"{label}: original piece identity and material plans survive",
                    )
                    for operation, arguments in [
                        ("raise_component_partition", {"rise": 16}),
                        (
                            "update_component_partition_cut",
                            {"edge_parameters": [0.0, 0.8]},
                        ),
                    ]:
                        error = client.call(
                            operation, {"token": flat["token"], **arguments}, error=True
                        )
                        check(
                            error and inspect()["preview"] == flat,
                            f"{label}: {operation} rejects without replacing draft",
                        )
                    client.call(
                        "frame_component_partitions",
                        {"token": flat["token"], "aspect": 1100 / 700},
                    )
                    for piece in part["loops"]:
                        points = [
                            part["vertices"][index]["position"]
                            for index in piece["vertices"]
                        ]
                        hits = pick(flat["token"], points, normal)
                        check(
                            any(
                                hit["partition"]
                                == {"solid": solid, "partition": piece["id"]}
                                for hit in hits
                            ),
                            f"{label}: flat {piece['shape']} containment",
                        )
                    camera = {
                        "position": [90, -85, 135],
                        "look_at": [32, 20, 60],
                        "projection": "perspective",
                    }
                    rendered = render and direction == 0
                    if rendered:
                        baseline = client.render(
                            output
                            / f"polygon-flat-{corners}-{int(owned)}-{int(reflected)}.png",
                            surface="textured",
                            camera=camera,
                        )
                    arguments = {
                        "height": 16.125,
                        "texture_lock": True,
                        "wall_material": "brick/brickwall001a",
                    }
                    draft = client.call(
                        "preview_partition_extrusion",
                        {"token": flat["token"], **arguments},
                    )["preview"]
                    pieces = draft["extrusion"]["pieces"]
                    check(
                        len(pieces) == 2
                        and sum(len(piece["faces"]) for piece in pieces) == corners + 8,
                        f"{label}: variable prism plane budget",
                    )
                    for piece in pieces:
                        top = next(
                            face for face in piece["faces"] if face["surface"] == "top"
                        )
                        check(
                            top["material"] == materials[piece["partition"]]
                            and sum(
                                face["surface"] == "divider" for face in piece["faces"]
                            )
                            == 1,
                            f"{label}: independent top and single shared divider",
                        )
                        points = [
                            [
                                value + axis * arguments["height"]
                                for value, axis in zip(
                                    part["vertices"][index]["position"],
                                    normal,
                                    strict=True,
                                )
                            ]
                            for index in part["loops"][piece["partition"]]["vertices"]
                        ]
                        check(
                            any(
                                hit["partition"]
                                == {"solid": solid, "partition": piece["partition"]}
                                for hit in pick(draft["token"], points, normal)
                            ),
                            f"{label}: extruded polygon top picking",
                        )
                    check(
                        detail(client, [object_id])[0] == source
                        and client.call("get_map_info")["undo"] == history,
                        f"{label}: extrusion preview preserves source history",
                    )
                    if rendered:
                        raised = client.render(
                            output
                            / f"polygon-extruded-{corners}-{int(owned)}-{int(reflected)}.png",
                            surface="textured",
                            camera=camera,
                        )
                        check(
                            raised != baseline,
                            f"{label}: extrusion changes private rendered geometry",
                        )
                    client.call("cancel_component_edit", {"token": draft["token"]})
                    client.call("save_map", {"path": str(path)})
                    check(
                        without_save_counter(path) == kept,
                        f"{label}: exact cancelled saved map",
                    )
                    flat = resume(solid, side)
                    if rendered:
                        cancelled = client.render(
                            output
                            / f"polygon-cancelled-{corners}-{int(owned)}-{int(reflected)}.png",
                            surface="textured",
                            camera=camera,
                        )
                        check(cancelled == baseline, f"{label}: exact Cancel pixels")
                    draft = client.call(
                        "preview_partition_extrusion",
                        {"token": flat["token"], **arguments},
                    )["preview"]
                    check(
                        draft["extrusion"]["pieces"] == pieces,
                        f"{label}: exact allocation replay",
                    )
                    client.call("commit_component_edit", {"token": draft["token"]})
                    check(
                        len(client.call("get_selection")["face_selection"]["faces"])
                        == corners + 8,
                        f"{label}: Apply selects all output faces",
                    )
                    client.call("undo")
                    check(
                        detail(client, [object_id])[0] == source
                        and client.call("get_saved_component_partitions")["entries"][0][
                            "valid"
                        ],
                        f"{label}: undo retains reusable saved source",
                    )
    client.call("new_map", {"discard_changes": True})
    components = []
    expected_faces = 0
    for index in range(16):
        corners = [3, 4, 5, 6, 7, 8][index % 6]
        object_id = client.call(
            "create_primitive",
            {
                "shape": "cylinder",
                "segments": corners,
                "mins": [index * 128, 0, 16],
                "maxs": [index * 128 + 64, 64, 80],
                "material": "brick/brickwall001a",
            },
        )["objects"][0]["id"]
        solid = int(object_id.split(":")[1])
        if index % 2:
            object_id = client.call(
                "tie_to_entity", {"ids": [object_id], "classname": "func_detail"}
            )["id"]
        source = detail(client, [object_id])[0]
        brush = source["brushes"][0] if index % 2 else source
        side = max(brush["faces"], key=lambda face: face["normal"][2])["side"]
        components.append({"solid": solid, "sides": [side]})
        expected_faces += corners + 8
    client.call(
        "set_component_picking",
        {
            "enabled": True,
            "kind": "face",
            "solids": [component["solid"] for component in components],
            "snapshot": inspect()["snapshot"],
        },
    )
    selected = client.call(
        "select_brush_components",
        {"components": components, "snapshot": inspect()["snapshot"]},
    )
    pairs = [
        {
            "solid": row["component"]["solid"],
            "edges": row["pairs"][index % len(row["pairs"])]["edges"],
        }
        for index, row in enumerate(selected["partition_choices"])
    ]
    flat = client.call(
        "preview_component_partition",
        {"pairs": pairs, "snapshot": inspect()["snapshot"]},
    )["preview"]
    check(
        len(flat["partition"]["partitions"]) == 16,
        "batch: sixteen mixed polygon and quad sources",
    )
    client.call("store_component_partition", {"token": flat["token"]})
    check(
        len(client.call("get_saved_component_partitions")["entries"]) == 16,
        "batch: atomic mixed Keep",
    )
    client.call("undo")
    check(
        not client.call("get_saved_component_partitions")["entries"],
        "batch: one undo removes all mixed intents",
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
    draft = client.call(
        "preview_partition_extrusion",
        {"token": flat["token"], "height": 16.125, "texture_lock": True},
    )["preview"]
    pieces = draft["extrusion"]["pieces"]
    check(
        len(pieces) == 32
        and sum(len(piece["faces"]) for piece in pieces) == expected_faces <= 256,
        "batch: bounded mixed prism planes",
    )
    client.call("cancel_component_edit", {"token": draft["token"]})
    flat = client.call(
        "resume_component_partitions",
        {"sources": sources, "snapshot": inspect()["snapshot"]},
    )["preview"]
    draft = client.call(
        "preview_partition_extrusion",
        {"token": flat["token"], "height": 16.125, "texture_lock": True},
    )["preview"]
    check(
        draft["extrusion"]["pieces"] == pieces, "batch: exact mixed allocation replay"
    )
    client.call("commit_component_edit", {"token": draft["token"]})
    check(
        len(client.call("get_selection")["face_selection"]["faces"]) == expected_faces,
        "batch: Apply selects all mixed output faces",
    )
    client.call("undo")
    check(
        all(
            entry["valid"]
            for entry in client.call("get_saved_component_partitions")["entries"]
        ),
        "batch: undo restores every mixed source",
    )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-polygon-partitions")
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
