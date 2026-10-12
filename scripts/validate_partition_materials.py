import argparse
import json
from pathlib import Path

from validate_component_picking import screen
from validate_face_lighting import ROOT, Client, detail
from validate_partition_material_metadata import membership, without_save_counter


def validate(client, output, render=False):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def inspect():
        return client.call("get_brush_components")

    def style(draft, refs, material):
        return client.call(
            "update_component_partition_material",
            {"token": draft["token"], "partitions": refs, "material": material},
        )["preview"]

    def resume(solid, side):
        return client.call(
            "resume_component_partitions",
            {
                "sources": [{"solid": solid, "side": side}],
                "snapshot": client.call("get_saved_component_partitions")["snapshot"],
            },
        )["preview"]

    if render:
        for material in [
            "brick/brickwall001a",
            "brick/brickwall003a",
            "de_cbble/woodfloor01",
        ]:
            check(
                not client.call("preview_material", {"material": material})["missing"],
                f"Offscreen fixture texture is available: {material}",
            )
    tools = {item["name"] for item in client.rpc("tools/list", {})["tools"]}
    check(
        "update_component_partition_material" in tools,
        "Material authoring is discoverable",
    )
    for owned in [False, True]:
        for direction in range(2):
            label = f"owned={owned} direction={direction}"
            client.call("new_map", {"discard_changes": True})
            object_id = client.call(
                "create_block",
                {
                    "mins": [0, 0, 0],
                    "maxs": [64, 64, 64],
                    "material": "brick/brickwall001a",
                },
            )["id"]
            solid = int(object_id.split(":")[1])
            if owned:
                object_id = client.call(
                    "tie_to_entity", {"ids": [object_id], "classname": "func_detail"}
                )["id"]
            source = detail(client, [object_id])[0]
            brush = source["brushes"][0] if owned else source
            face = max(brush["faces"], key=lambda item: item["normal"][2])
            side = face["side"]
            selected = client.call(
                "select_brush_components",
                {
                    "components": [{"solid": solid, "sides": [side]}],
                    "snapshot": inspect()["snapshot"],
                },
            )
            edges = selected["crease_choices"][0]["pairs"][direction]["edges"]
            client.call(
                "set_component_picking",
                {
                    "enabled": True,
                    "kind": "face",
                    "solids": [solid],
                    "snapshot": inspect()["snapshot"],
                },
            )
            draft = client.call(
                "preview_component_partition",
                {
                    "pairs": [{"solid": solid, "edges": edges}],
                    "snapshot": inspect()["snapshot"],
                },
            )["preview"]
            refs = [{"solid": solid, "partition": index} for index in range(2)]
            initial = draft["token"]
            identity = membership(draft["partition"]["partitions"][0])
            history = client.call("get_map_info")["undo"]
            before = None
            if render:
                client.call(
                    "set_camera",
                    {
                        "position": [32, 32, 160],
                        "look_at": [32, 32, 64],
                        "projection": "perspective",
                    },
                )
                before = client.render(output / f"{int(owned)}-{direction}-before.png")
            draft = style(draft, [refs[0]], " brick/brickwall003a ")
            loops = draft["partition"]["partitions"][0]["loops"]
            check(
                loops[0]["top_material"] == "brick/brickwall003a"
                and not loops[0]["inherited"],
                f"{label}: explicit material normalized",
            )
            check(
                loops[1]["top_material"] is None
                and loops[1]["effective_material"] == face["material"],
                f"{label}: sibling inherits",
            )
            client.call(
                "update_component_partition_material",
                {"token": initial, "partitions": [refs[0]], "material": "wood/stale"},
                error=True,
            )
            for targets, material in [
                ([], "test"),
                ([refs[0], refs[0]], "test"),
                ([{"solid": solid, "partition": 2}], "test"),
                ([refs[0]], ""),
                ([refs[0]], "bad\n"),
                ([refs[0]], "x" * 65537),
            ]:
                client.call(
                    "update_component_partition_material",
                    {
                        "token": draft["token"],
                        "partitions": targets,
                        "material": material,
                    },
                    error=True,
                )
                check(
                    inspect()["preview"]["token"] == draft["token"],
                    f"{label}: invalid material update retains token",
                )
            check(
                detail(client, [object_id])[0] == source
                and client.call("get_map_info")["undo"] == history,
                f"{label}: draft leaves geometry and history unchanged",
            )
            if render:
                after = client.render(output / f"{int(owned)}-{direction}-styled.png")
                check(after != before, f"{label}: shared textured preview changes")
            draft = style(draft, [refs[1]], "de_cbble/woodfloor01")
            draft = style(draft, [refs[1]], None)
            check(
                draft["partition"]["partitions"][0]["loops"][1]["inherited"],
                f"{label}: inheritance restored",
            )
            client.call("cancel_component_edit", {"token": draft["token"]})
            check(
                detail(client, [object_id])[0] == source
                and client.call("get_map_info")["undo"] == history,
                f"{label}: Cancel stores nothing",
            )
            draft = client.call(
                "preview_component_partition",
                {
                    "pairs": [{"solid": solid, "edges": edges}],
                    "snapshot": inspect()["snapshot"],
                },
            )["preview"]
            if render:
                check(
                    client.render(output / f"{int(owned)}-{direction}-cancel.png")
                    == before,
                    f"{label}: Cancel restores exact rendered pixels",
                )
            draft = style(draft, [refs[0]], "brick/brickwall003a")
            draft = style(draft, [refs[1]], "de_cbble/woodfloor01")
            client.call("store_component_partition", {"token": draft["token"]})
            path = output / f"saved-{int(owned)}-{direction}.vmf"
            client.call("save_map", {"path": str(path)})
            canonical = without_save_counter(path)
            client.call("open_map", {"path": str(path), "discard_changes": True})
            saved = client.call("get_saved_component_partitions")["entries"][0]
            check(
                saved["top_materials"]
                == ["brick/brickwall003a", "de_cbble/woodfloor01"],
                f"{label}: reopening retains plans",
            )
            draft = resume(solid, side)
            history = client.call("get_map_info")["undo"]
            client.call("store_component_partition", {"token": draft["token"]})
            client.call("save_map", {"path": str(path)})
            check(
                client.call("get_map_info")["undo"] == history
                and without_save_counter(path) == canonical,
                f"{label}: repeated Keep is exact and adds no history",
            )
            client.call(
                "transform_objects",
                {"ids": [object_id], "reflection": [True, False, False]},
            )
            draft = resume(solid, side)
            check(
                membership(draft["partition"]["partitions"][0]) == identity,
                f"{label}: reflected identity retained",
            )
            draft = client.call(
                "update_component_partition_cut",
                {"token": draft["token"], "edge_parameters": [0.2, 0.8]},
            )["preview"]
            check(
                [
                    item["top_material"]
                    for item in draft["partition"]["partitions"][0]["loops"]
                ]
                == saved["top_materials"],
                f"{label}: adjusted cuts retain materials",
            )
            draft = client.call(
                "preview_partition_extrusion",
                {
                    "token": draft["token"],
                    "height": 24.125,
                    "wall_material": "concrete/concretefloor001a",
                    "texture_lock": True,
                },
            )["preview"]
            for item in draft["extrusion"]["pieces"]:
                for surface in item["faces"]:
                    expected = (
                        saved["top_materials"][item["partition"]]
                        if surface["surface"] == "top"
                        else face["material"]
                        if surface["surface"] == "base"
                        else "concrete/concretefloor001a"
                    )
                    check(
                        surface["material"] == expected,
                        f"{label}: extrusion {item['partition']} {surface['surface']} material",
                    )
            client.call("commit_component_edit", {"token": draft["token"]})
            path = output / f"extruded-{int(owned)}-{direction}.vmf"
            client.call("save_map", {"path": str(path)})
            client.call("open_map", {"path": str(path), "discard_changes": True})
            check(
                client.call("get_saved_component_partitions")["entries"][0][
                    "top_materials"
                ]
                == saved["top_materials"],
                f"{label}: extrusion retains source plans",
            )
            draft = resume(solid, side)
            draft = client.call(
                "update_component_partition_cut",
                {"token": draft["token"], "edge_parameters": [0.5, 0.5]},
            )["preview"]
            draft = client.call(
                "raise_component_partition",
                {"token": draft["token"], "rise": 16.125, "roof_texture_lock": True},
            )["preview"]
            client.call("commit_component_edit", {"token": draft["token"]})
            roof = detail(client, [object_id])[0]
            roof = roof["brushes"][0] if owned else roof
            check(
                sorted(
                    item["material"]
                    for item in roof["faces"]
                    if item["material"] in saved["top_materials"]
                )
                == sorted(saved["top_materials"]),
                f"{label}: roof Apply uses both plans",
            )
            check(
                not client.call("get_saved_component_partitions")["entries"],
                f"{label}: roof Apply consumes source plan",
            )
            client.call("undo")
            check(
                client.call("get_saved_component_partitions")["entries"][0][
                    "top_materials"
                ]
                == saved["top_materials"],
                f"{label}: roof undo restores plans",
            )
    client.call("new_map", {"discard_changes": True})
    sources = []
    for index in range(2):
        object_id = client.call(
            "create_block",
            {"mins": [index * 96, 0, 0], "maxs": [index * 96 + 64, 64, 64]},
        )["id"]
        solid = int(object_id.split(":")[1])
        brush = detail(client, [object_id])[0]
        side = max(brush["faces"], key=lambda face: face["normal"][2])["side"]
        sources.append({"solid": solid, "sides": [side]})
    selected = client.call(
        "select_brush_components",
        {"components": sources, "snapshot": inspect()["snapshot"]},
    )
    draft = client.call(
        "preview_component_partition",
        {
            "pairs": [
                {
                    "solid": choice["component"]["solid"],
                    "edges": choice["pairs"][index]["edges"],
                }
                for index, choice in enumerate(selected["crease_choices"])
            ],
            "snapshot": inspect()["snapshot"],
        },
    )["preview"]
    refs = [
        {"solid": source["solid"], "partition": index}
        for source in sources
        for index in range(2)
    ]
    draft = style(draft, refs, "de_cbble/woodfloor01")
    check(
        all(
            piece["top_material"] == "de_cbble/woodfloor01"
            for part in draft["partition"]["partitions"]
            for piece in part["loops"]
        ),
        "Batch material plan covers all captured brushes",
    )
    for material in ["x" * 65536, "x" * 17000]:
        client.call(
            "update_component_partition_material",
            {"token": draft["token"], "partitions": refs, "material": material},
            error=True,
        )
        check(
            inspect()["preview"]["token"] == draft["token"],
            "Shared material budget failure retains batch draft",
        )
    history = client.call("get_map_info")["undo"]
    client.call("store_component_partition", {"token": draft["token"]})
    check(
        len(client.call("get_saved_component_partitions")["entries"]) == 2,
        "Batch Keep stores both source plans",
    )
    check(
        len(client.call("get_map_info")["undo"]) == len(history) + 1,
        "Batch Keep adds one undo step",
    )
    client.call("undo")
    check(
        not client.call("get_saved_component_partitions")["entries"],
        "Batch undo removes both plans",
    )
    client.call("redo")
    check(
        len(client.call("get_saved_component_partitions")["entries"]) == 2,
        "Batch redo restores both plans",
    )
    for owned in [False, True]:
        for tool_source in [False, True]:
            label = f"visibility owned={owned} tool_source={tool_source}"
            client.call("new_map", {"discard_changes": True})
            object_id = client.call(
                "create_block",
                {
                    "mins": [0, 0, 0],
                    "maxs": [64, 64, 64],
                    "material": "tools/toolsnodraw"
                    if tool_source
                    else "brick/brickwall001a",
                },
            )["id"]
            solid = int(object_id.split(":")[1])
            if owned:
                object_id = client.call(
                    "tie_to_entity", {"ids": [object_id], "classname": "func_detail"}
                )["id"]
            source = detail(client, [object_id])[0]
            brush = source["brushes"][0] if owned else source
            side = max(brush["faces"], key=lambda item: item["normal"][2])["side"]
            selected = client.call(
                "select_brush_components",
                {
                    "components": [{"solid": solid, "sides": [side]}],
                    "snapshot": inspect()["snapshot"],
                },
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
            draft = client.call(
                "preview_component_partition",
                {
                    "pairs": [
                        {
                            "solid": solid,
                            "edges": selected["crease_choices"][0]["pairs"][0]["edges"],
                        }
                    ],
                    "snapshot": inspect()["snapshot"],
                },
            )["preview"]
            refs = [{"solid": solid, "partition": index} for index in range(2)]
            draft = style(
                draft,
                [refs[0]],
                "brick/brickwall003a" if tool_source else "tools/toolsnodraw",
            )
            client.call("set_view", {"tool_textures": False})
            part = draft["partition"]["partitions"][0]
            for item in part["loops"]:
                center = [
                    sum(
                        part["vertices"][index]["position"][axis]
                        for index in item["vertices"]
                    )
                    / 4
                    for axis in range(3)
                ]
                client.call(
                    "set_camera",
                    {"projection": "top", "center": center, "ortho_height": 128},
                )
                camera = client.call("get_map_info")["camera"]
                picks = client.call(
                    "pick_component_partitions",
                    {"token": draft["token"], **screen(camera, center)},
                )["candidates"]
                visible = tool_source if item["id"] == 0 else not tool_source
                check(
                    len(picks) == int(visible),
                    f"{label}: piece {item['id']} visibility matches picking",
                )
            if render:
                for surface in ["wireframe", "flat", "textured"]:
                    image = client.render(
                        output
                        / f"visible-{int(owned)}-{int(tool_source)}-{surface}.png",
                        surface=surface,
                        tool_textures=False,
                    )
                    client.call(
                        "select_component_partitions",
                        {"token": draft["token"], "partitions": [], "mode": "replace"},
                    )
                    plain = client.render(
                        output / f"plain-{int(owned)}-{int(tool_source)}-{surface}.png",
                        surface=surface,
                        tool_textures=False,
                    )
                    check(
                        image != plain,
                        f"{label}: {surface} displays visible piece guides",
                    )
                    client.call(
                        "select_component_partitions",
                        {
                            "token": draft["token"],
                            "partitions": refs,
                            "mode": "replace",
                        },
                    )
            draft = style(draft, refs, "tools/toolsnodraw")
            draft = client.call(
                "preview_partition_extrusion",
                {
                    "token": draft["token"],
                    "height": 32,
                    "wall_material": "brick/brickwall001a",
                },
            )["preview"]
            client.call(
                "set_camera",
                {"projection": "top", "center": [32, 32, 64], "ortho_height": 128},
            )
            camera = client.call("get_map_info")["camera"]
            picks = client.call(
                "pick_component_partitions",
                {"token": draft["token"], **screen(camera, [16, 32, 64])},
            )["candidates"]
            check(
                not picks,
                f"{label}: visible walls do not make hidden source/top pickable",
            )
            client.call("cancel_component_edit", {"token": draft["token"]})
            check(
                detail(client, [object_id])[0] == source,
                f"{label}: cancellation preserves source",
            )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-partition-materials")
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
