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

    for owned in [False, True]:
        for reflected in [False, True]:
            for direction in range(2):
                label = f"owned={owned} reflected={reflected} direction={direction}"
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
                    "transform_objects",
                    {
                        "ids": [object_id],
                        "rotation": [17, -23, 31],
                        "reflection": [reflected, False, False],
                    },
                )
                if owned:
                    object_id = client.call(
                        "tie_to_entity",
                        {"ids": [object_id], "classname": "func_detail"},
                    )["id"]
                source = detail(client, [object_id])[0]
                brush = source["brushes"][0] if owned else source
                face = max(brush["faces"], key=lambda face: face["normal"][2])
                side = face["side"]
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
                identity = membership(draft["partition"]["partitions"][0])
                refs = [{"solid": solid, "partition": index} for index in range(2)]
                for ref, material in zip(
                    refs, ["brick/brickwall003a", "de_cbble/woodfloor01"], strict=True
                ):
                    draft = client.call(
                        "update_component_partition_material",
                        {
                            "token": draft["token"],
                            "partitions": [ref],
                            "material": material,
                        },
                    )["preview"]
                draft = client.call(
                    "update_component_partition_cut",
                    {"token": draft["token"], "edge_parameters": [0.2, 0.2]},
                )["preview"]
                client.call("store_component_partition", {"token": draft["token"]})
                path = output / f"ridge-{int(owned)}-{int(reflected)}-{direction}.vmf"
                client.call("save_map", {"path": str(path)})
                saved = without_save_counter(path)
                client.call("open_map", {"path": str(path), "discard_changes": True})
                report = client.call("get_saved_component_partitions")
                draft = client.call(
                    "resume_component_partitions",
                    {
                        "sources": [{"solid": solid, "side": side}],
                        "snapshot": report["snapshot"],
                    },
                )["preview"]
                check(
                    draft["partition"]["edge_parameters"] == [0.2, 0.2],
                    f"{label}: unequal widths survive reopening",
                )
                check(
                    membership(draft["partition"]["partitions"][0]) == identity,
                    f"{label}: saved corner membership retained",
                )
                history = client.call("get_map_info")["undo"]
                if render:
                    client.call(
                        "set_camera",
                        {
                            "projection": "perspective",
                            "position": [170, -200, 180],
                            "look_at": [32, 32, 80],
                        },
                    )
                    before = client.render(
                        output / f"before-{int(owned)}-{int(reflected)}-{direction}.png"
                    )
                for texture_lock in [False, True]:
                    flat = draft
                    part = flat["partition"]["partitions"][0]
                    draft = client.call(
                        "raise_component_partition",
                        {
                            "token": flat["token"],
                            "rise": 16.125,
                            "roof_texture_lock": texture_lock,
                        },
                    )["preview"]
                    raised = draft["partition"]["partitions"][0]
                    check(
                        draft["partition"]["edge_parameters"] == [0.2, 0.2],
                        f"{label}: raising does not reset positions",
                    )
                    check(
                        [item["top_material"] for item in raised["loops"]]
                        == ["brick/brickwall003a", "de_cbble/woodfloor01"],
                        f"{label}: raised material plans retained",
                    )
                    for original, vertex in zip(
                        part["vertices"], raised["vertices"], strict=True
                    ):
                        if original["source"]["kind"] == "corner":
                            check(
                                vertex["preview_position"] == original["position"],
                                f"{label}: old corner remains exact",
                            )
                        else:
                            delta = (
                                sum(
                                    (a - b) ** 2
                                    for a, b in zip(
                                        vertex["preview_position"],
                                        original["position"],
                                        strict=True,
                                    )
                                )
                                ** 0.5
                            )
                            check(
                                abs(delta - 16.125) < 1e-8,
                                f"{label}: actual cut endpoint rises requested distance",
                            )
                    check(
                        client.call("get_map_info")["undo"] == history,
                        f"{label}: ridge preview has no history",
                    )
                    if render:
                        after = client.render(
                            output
                            / f"raised-{int(owned)}-{int(reflected)}-{direction}-{int(texture_lock)}.png"
                        )
                        check(
                            after != before,
                            f"{label}: raised geometry changes shared image",
                        )
                    draft = client.call(
                        "raise_component_partition",
                        {"token": draft["token"], "rise": 0},
                    )["preview"]
                    if render:
                        check(
                            client.render(
                                output
                                / f"flat-{int(owned)}-{int(reflected)}-{direction}-{int(texture_lock)}.png"
                            )
                            == before,
                            f"{label}: Return to flat restores exact pixels",
                        )
                draft = client.call(
                    "update_component_partition_cut",
                    {"token": draft["token"], "edge_parameters": [0.2, 0.8]},
                )["preview"]
                token = draft["token"]
                client.call(
                    "raise_component_partition",
                    {"token": token, "rise": 16.125},
                    error=True,
                )
                check(
                    inspect()["preview"]["token"] == token,
                    f"{label}: nonplanar rejection preserves flat draft",
                )
                check(
                    client.call("get_map_info")["undo"] == history,
                    f"{label}: nonplanar rejection has no history",
                )
                draft = client.call(
                    "update_component_partition_cut",
                    {"token": token, "edge_parameters": [0.8, 0.8]},
                )["preview"]
                draft = client.call(
                    "raise_component_partition",
                    {
                        "token": draft["token"],
                        "rise": 24.125,
                        "roof_texture_lock": True,
                    },
                )["preview"]
                client.call(
                    "frame_component_partitions",
                    {"token": draft["token"], "aspect": 1.6},
                )
                part = draft["partition"]["partitions"][0]
                for item in part["loops"]:
                    center = [
                        sum(
                            part["vertices"][index]["preview_position"][axis]
                            for index in item["vertices"]
                        )
                        / 4
                        for axis in range(3)
                    ]
                    pick_camera = {
                        "projection": "perspective",
                        "position": [
                            point + normal * 256
                            for point, normal in zip(
                                center, face["normal"], strict=True
                            )
                        ],
                        "look_at": center,
                    }
                    picks = client.call(
                        "pick_component_partitions",
                        {
                            "token": draft["token"],
                            "x": 550,
                            "y": 350,
                            "width": 1100,
                            "height": 700,
                            "camera": pick_camera,
                        },
                    )["candidates"]
                    check(
                        any(
                            hit["partition"]
                            == {"solid": solid, "partition": item["id"]}
                            for hit in picks
                        ),
                        f"{label}: raised adjusted loop picks stable piece",
                    )
                client.call("cancel_component_edit", {"token": draft["token"]})
                client.call("save_map", {"path": str(path)})
                check(
                    without_save_counter(path) == saved,
                    f"{label}: Cancel preserves saved intent and source",
                )
                draft = client.call(
                    "resume_component_partitions",
                    {
                        "sources": [{"solid": solid, "side": side}],
                        "snapshot": client.call("get_saved_component_partitions")[
                            "snapshot"
                        ],
                    },
                )["preview"]
                draft = client.call(
                    "raise_component_partition",
                    {
                        "token": draft["token"],
                        "rise": 16.125,
                        "roof_texture_lock": True,
                    },
                )["preview"]
                client.call("commit_component_edit", {"token": draft["token"]})
                committed = detail(client, [object_id])[0]
                result = committed["brushes"][0] if owned else committed
                check(
                    len(result["faces"]) == 7,
                    f"{label}: Apply creates exactly two roof faces",
                )
                check(
                    sorted(
                        face["material"]
                        for face in result["faces"]
                        if face["material"]
                        in ["brick/brickwall003a", "de_cbble/woodfloor01"]
                    )
                    == ["brick/brickwall003a", "de_cbble/woodfloor01"],
                    f"{label}: Apply assigns corresponding roof materials",
                )
                check(
                    not client.call("get_saved_component_partitions")["entries"],
                    f"{label}: Apply consumes saved intent",
                )
                client.call("undo")
                check(
                    detail(client, [object_id])[0] == source,
                    f"{label}: undo restores original brush",
                )
                check(
                    client.call("get_saved_component_partitions")["entries"][0][
                        "edge_parameters"
                    ]
                    == [0.2, 0.2],
                    f"{label}: undo restores adjusted saved plan",
                )
                client.call("redo")
                client.call("save_map", {"path": str(path)})
                client.call("open_map", {"path": str(path), "discard_changes": True})
                reopened = detail(client, [object_id])[0]
                check(reopened == committed, f"{label}: committed VMF reopens")
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-adjusted-partition-ridges")
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
