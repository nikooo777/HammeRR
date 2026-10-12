import argparse
import json
from pathlib import Path

from validate_component_picking import screen
from validate_face_lighting import ROOT, Client, detail


def validate(client, output, render=False, adjusted=False):
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
                "snapshot": client.call("get_saved_component_partitions")["snapshot"],
            },
        )["preview"]

    names = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
    check("preview_partition_extrusion" in names, "Extrusion is discoverable")
    for owned in [False, True]:
        for oblique in [False, True]:
            for direction in range(2):
                label = f"owned={owned} oblique={oblique} direction={direction}"
                client.call("new_map", {"discard_changes": True})
                object_id = client.call(
                    "create_block",
                    {"mins": [0.125, -7.25, 16.5], "maxs": [64.875, 48.125, 80.25]},
                )["id"]
                solid = int(object_id.split(":")[1])
                if oblique:
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
                side = max(brush["faces"], key=lambda face: face["normal"][2])["side"]
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
                                "edges": selected["crease_choices"][0]["pairs"][
                                    direction
                                ]["edges"],
                            }
                        ],
                        "snapshot": inspect()["snapshot"],
                    },
                )["preview"]
                if adjusted:
                    original_token = draft["token"]
                    draft = client.call(
                        "update_component_partition_cut",
                        {"token": original_token, "edge_parameters": [0.2, 0.8]},
                    )["preview"]
                    check(
                        draft["partition"]["edge_parameters"] == [0.2, 0.8],
                        f"{label}: adjusted cut reports shared positions",
                    )
                    client.call(
                        "update_component_partition_cut",
                        {"token": original_token, "edge_parameters": [0.3, 0.7]},
                        error=True,
                    )
                    for parameters in [[0, 0.8], [0.2, 1], [0.2, -0.1]]:
                        client.call(
                            "update_component_partition_cut",
                            {"token": draft["token"], "edge_parameters": parameters},
                            error=True,
                        )
                        check(
                            inspect()["preview"] == draft,
                            f"{label}: invalid cut retains exact draft",
                        )
                    client.call(
                        "raise_component_partition",
                        {"token": draft["token"], "rise": 16},
                        error=True,
                    )
                    check(
                        inspect()["preview"] == draft,
                        f"{label}: midpoint-only ridge rejects without snapping",
                    )
                client.call("store_component_partition", {"token": draft["token"]})
                if adjusted:
                    check(
                        client.call("get_saved_component_partitions")["entries"][0][
                            "edge_parameters"
                        ]
                        == [0.2, 0.8],
                        f"{label}: saved inspection exposes edge positions",
                    )
                    client.call(
                        "transform_objects",
                        {"ids": [object_id], "reflection": [True, False, False]},
                    )
                    source = detail(client, [object_id])[0]
                    brush = source["brushes"][0] if owned else source
                draft = resume(solid, side)
                if adjusted:
                    check(
                        draft["partition"]["edge_parameters"] == [0.2, 0.8],
                        f"{label}: reflected saved cut retains parameters",
                    )
                history = client.call("get_map_info")["undo"]
                render_args = {
                    "surface": "flat",
                    "camera": {
                        "projection": "perspective",
                        "position": [170, -200, 180],
                        "look_at": [30, 20, 55],
                    },
                }
                before_view = (
                    client.render(
                        output / f"before-{int(owned)}-{int(oblique)}-{direction}.png",
                        **render_args,
                    )
                    if render
                    else None
                )
                arguments = {
                    "token": draft["token"],
                    "height": 32.125,
                    "wall_material": "concrete/test",
                    "texture_lock": True,
                }
                draft = client.call("preview_partition_extrusion", arguments)["preview"]
                token = draft["token"]
                pieces = draft["extrusion"]["pieces"]
                check(
                    draft["operation"] == "extrusion" and draft["can_apply"],
                    f"{label}: valid shared preview",
                )
                check(
                    len(pieces) == 2
                    and all(len(piece["faces"]) == 6 for piece in pieces),
                    f"{label}: two six-face prisms",
                )
                check(
                    detail(client, [object_id])[0] == source,
                    f"{label}: authored source untouched",
                )
                check(
                    client.call("get_map_info")["undo"] == history,
                    f"{label}: preview adds no history",
                )
                for piece in pieces:
                    walls = [
                        face
                        for face in piece["faces"]
                        if face["surface"] in {"boundary", "divider"}
                    ]
                    check(
                        len(walls) == 4
                        and all(face["material"] == "concrete/test" for face in walls),
                        f"{label}: all walls use chosen material",
                    )
                    check(
                        piece["owner"] == (object_id if owned else "world"),
                        f"{label}: output owner retained",
                    )
                    top = next(
                        face for face in piece["faces"] if face["surface"] == "top"
                    )
                    center = [
                        sum(point[axis] for point in top["plane"]) / 3
                        for axis in range(3)
                    ]
                    client.call(
                        "set_camera",
                        {"projection": "top", "center": center, "ortho_height": 256},
                    )
                    camera = client.call("get_map_info")["camera"]
                    picked = client.call(
                        "pick_component_partitions",
                        {"token": token, **screen(camera, center)},
                    )
                    check(
                        any(
                            hit["partition"]["solid"] == solid
                            and hit["partition"]["partition"] == piece["partition"]
                            for hit in picked["candidates"]
                        ),
                        f"{label}: visible prism picks source identity",
                    )
                client.call(
                    "frame_component_partitions", {"token": token, "aspect": 1.6}
                )
                check(
                    client.call("get_map_info")["undo"] == history,
                    f"{label}: framing adds no history",
                )
                if render:
                    after_view = client.render(
                        output
                        / f"extruded-{int(owned)}-{int(oblique)}-{direction}.png",
                        **render_args,
                    )
                    check(before_view != after_view, f"{label}: extrusion is visible")
                for update in [
                    {"height": 0},
                    {"height": -1},
                    {"height": 4097},
                    {"wall_material": " "},
                ]:
                    client.call(
                        "preview_partition_extrusion",
                        {**arguments, "token": token, **update},
                        error=True,
                    )
                    check(
                        inspect()["preview"] == draft,
                        f"{label}: invalid update retains exact draft",
                    )
                if adjusted:
                    client.call(
                        "update_component_partition_cut",
                        {"token": token, "edge_parameters": [0.3, 0.7]},
                        error=True,
                    )
                    check(
                        inspect()["preview"] == draft,
                        f"{label}: extrusion must return to flat before adjusting",
                    )
                client.call("store_component_partition", {"token": token}, error=True)
                client.call(
                    "save_map", {"path": str(output / "pending.vmf")}, error=True
                )
                client.call(
                    "select_component_partitions",
                    {
                        "token": token,
                        "partitions": [{"solid": solid, "partition": 0}],
                        "mode": "remove",
                    },
                )
                client.call("commit_component_edit", {"token": token}, error=True)
                check(
                    not inspect()["preview"]["can_apply"],
                    f"{label}: changed highlights require repreview",
                )
                draft = client.call(
                    "preview_partition_extrusion",
                    {**arguments, "token": token, "height": 48.125},
                )["preview"]
                check(
                    len(draft["extrusion"]["pieces"]) == 1,
                    f"{label}: one-piece update replaces prior geometry",
                )
                client.call(
                    "preview_partition_extrusion",
                    {**arguments, "token": token},
                    error=True,
                )
                flat = client.call(
                    "raise_component_partition", {"token": draft["token"], "rise": 0}
                )["preview"]
                check(
                    flat["extrusion"] is None and flat["partition"]["rise"] == 0,
                    f"{label}: return to flat",
                )
                client.call("cancel_component_edit", {"token": flat["token"]})
                check(
                    detail(client, [object_id])[0] == source,
                    f"{label}: cancel preserves source",
                )
                check(
                    client.call("get_map_info")["undo"] == history,
                    f"{label}: cancel adds no history",
                )
                draft = resume(solid, side)
                if render:
                    restored_view = client.render(
                        output
                        / f"cancelled-{int(owned)}-{int(oblique)}-{direction}.png",
                        **render_args,
                    )
                    check(
                        restored_view == before_view,
                        f"{label}: cancelled geometry restores exact pixels",
                    )
                draft = client.call(
                    "preview_partition_extrusion",
                    {**arguments, "token": draft["token"]},
                )["preview"]
                check(
                    draft["extrusion"]["pieces"] == pieces,
                    f"{label}: cancellation preserves allocation replay",
                )
                client.call("commit_component_edit", {"token": draft["token"]})
                check(inspect()["preview"] is None, f"{label}: Apply closes draft")
                check(
                    len(client.call("get_map_info")["undo"]) == len(history) + 1,
                    f"{label}: one undo step",
                )
                selection = client.call("get_selection")["face_selection"]
                check(
                    {face["solid"] for face in selection["faces"]}
                    == {piece["solid"] for piece in pieces},
                    f"{label}: selection contains only newly allocated volumes",
                )
                check(len(selection["faces"]) == 12, f"{label}: all new faces selected")
                applied = detail(client, [object_id])[0]
                if owned:
                    check(
                        len(applied["brushes"]) == 3 and applied["brushes"][0] == brush,
                        f"{label}: entity child insertion leaves original intact",
                    )
                else:
                    check(
                        applied == source, f"{label}: world source intact after Apply"
                    )
                saved = client.call("get_saved_component_partitions")
                check(
                    saved["entries"][0]["valid"],
                    f"{label}: saved intent remains reusable",
                )
                path = output / f"extruded-{int(owned)}-{int(oblique)}-{direction}.vmf"
                client.call("save_map", {"path": str(path)})
                client.call("undo")
                check(
                    detail(client, [object_id])[0] == source,
                    f"{label}: exact source undo",
                )
                client.call("redo")
                check(detail(client, [object_id])[0] == applied, f"{label}: exact redo")
                client.call("open_map", {"path": str(path), "discard_changes": True})
                check(
                    detail(client, [object_id])[0] == applied,
                    f"{label}: exported geometry reopens",
                )
                check(
                    client.call("get_saved_component_partitions")["entries"][0][
                        "valid"
                    ],
                    f"{label}: saved source reopens valid",
                )
    client.call("new_map", {"discard_changes": True})
    components = []
    for index in range(16):
        object_id = client.call(
            "create_block",
            {"mins": [index * 128, 0, 0], "maxs": [index * 128 + 64, 64, 64]},
        )["id"]
        solid = int(object_id.split(":")[1])
        brush = detail(client, [object_id])[0]
        side = max(brush["faces"], key=lambda face: face["normal"][2])["side"]
        if index % 2:
            client.call(
                "tie_to_entity", {"ids": [object_id], "classname": "func_detail"}
            )
        components.append({"solid": solid, "sides": [side]})
    selected = client.call(
        "select_brush_components",
        {"components": components, "snapshot": inspect()["snapshot"]},
    )
    pairs = [
        {
            "solid": choice["component"]["solid"],
            "edges": choice["pairs"][index % 2]["edges"],
        }
        for index, choice in enumerate(selected["crease_choices"])
    ]
    draft = client.call(
        "preview_component_partition",
        {"pairs": pairs, "snapshot": inspect()["snapshot"]},
    )["preview"]
    draft = client.call(
        "preview_partition_extrusion", {"token": draft["token"], "height": 32.0}
    )["preview"]
    check(
        len(draft["extrusion"]["pieces"]) == 32,
        "Maximum mixed-owner batch previews 32 prisms",
    )
    client.call("commit_component_edit", {"token": draft["token"]})
    check(
        len(client.call("get_selection")["face_selection"]["faces"]) == 192,
        "Maximum batch selects 192 output faces",
    )
    check(
        inspect()["preview"] is None and not inspect()["selection"],
        "Maximum batch closes without overflowing component scope",
    )
    client.call("undo")
    check(
        len(client.call("get_selection")["face_selection"]["faces"]) == 0,
        "Maximum batch undo removes output faces",
    )
    return checks


def render_independent_heights(client, output, adjusted=False):
    client.call("new_map", {"discard_changes": True})
    object_id = client.call(
        "create_block",
        {"mins": [0, 0, 0], "maxs": [128, 96, 32], "material": "brick/brickwall001a"},
    )["id"]
    solid = int(object_id.split(":")[1])
    source = detail(client, [object_id])[0]
    side = max(source["faces"], key=lambda face: face["normal"][2])["side"]
    report = client.call("get_brush_components")
    selected = client.call(
        "select_brush_components",
        {
            "components": [{"solid": solid, "sides": [side]}],
            "snapshot": report["snapshot"],
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
            "snapshot": selected["snapshot"],
        },
    )["preview"]
    if adjusted:
        draft = client.call(
            "update_component_partition_cut",
            {"token": draft["token"], "edge_parameters": [0.2, 0.8]},
        )["preview"]
    client.call("store_component_partition", {"token": draft["token"]})
    for piece, height in [(0, 32), (1, 64)]:
        saved = client.call("get_saved_component_partitions")
        draft = client.call(
            "resume_component_partitions",
            {
                "sources": [{"solid": solid, "side": side}],
                "snapshot": saved["snapshot"],
            },
        )["preview"]
        client.call(
            "select_component_partitions",
            {
                "token": draft["token"],
                "partitions": [{"solid": solid, "partition": piece}],
            },
        )
        draft = client.call(
            "preview_partition_extrusion",
            {
                "token": draft["token"],
                "height": height,
                "wall_material": "concrete/concretefloor001a",
                "texture_lock": True,
            },
        )["preview"]
        if piece == 0:
            client.call("commit_component_edit", {"token": draft["token"]})
    client.render(
        output / "independent-heights.png",
        surface="textured",
        camera={
            "projection": "perspective",
            "position": [250, -260, 210],
            "look_at": [64, 48, 48],
        },
    )
    assert detail(client, [object_id])[0] == source
    client.call("cancel_component_edit", {"token": draft["token"]})


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-extrusion-validation")
    )
    parser.add_argument("--render", action="store_true")
    parser.add_argument("--adjusted", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output, args.render, args.adjusted)
        if args.render:
            render_independent_heights(client, args.output, args.adjusted)
        (args.output / "results.json").write_text(
            json.dumps({"checks": len(checks), "passed": checks}, indent=2) + "\n"
        )
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
