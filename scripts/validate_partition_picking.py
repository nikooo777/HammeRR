import argparse
import json
import math
from pathlib import Path

from validate_component_picking import screen
from validate_face_lighting import ROOT, Client, detail


def average(points):
    return [sum(point[axis] for point in points) / len(points) for axis in range(3)]


def validate(client, output, render=False):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def inspect(solid=None):
        return client.call("get_brush_components", {"solids": [solid]} if solid else {})

    def stage(solid, direction=0):
        brush = inspect(solid)["brushes"][0]
        top = max(
            brush["faces"],
            key=lambda face: (
                sum(
                    brush["vertices"][index]["position"][2]
                    for index in face["vertices"]
                )
                / len(face["vertices"])
            ),
        )
        selected = client.call(
            "select_brush_components",
            {
                "components": [{"solid": solid, "sides": [top["side"]]}],
                "snapshot": inspect()["snapshot"],
            },
        )
        pairs = [
            {
                "solid": solid,
                "edges": selected["crease_choices"][0]["pairs"][direction]["edges"],
            }
        ]
        return client.call(
            "preview_component_partition",
            {"pairs": pairs, "snapshot": inspect()["snapshot"]},
        )

    def pick(token, point):
        camera = client.call("get_map_info")["camera"]
        return client.call(
            "pick_component_partitions", {"token": token, **screen(camera, point)}
        )

    names = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
    check(
        "pick_component_partitions" in names,
        "Draft loop viewport picking is discoverable",
    )
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
                        {
                            "ids": [object_id],
                            "rotation": [17, -23, 31],
                            "scale": [1.125, 0.75, 1.5],
                        },
                    )
                if owned:
                    object_id = client.call(
                        "tie_to_entity",
                        {"ids": [object_id], "classname": "func_detail"},
                    )["id"]
                source = detail(client, [object_id])
                history = client.call("get_map_info")["undo"]
                draft = stage(solid, direction)
                first_token = draft["preview"]["token"]
                source_selection = draft["source_selection"]
                for rise in [0, 16.125]:
                    if rise:
                        draft = client.call(
                            "raise_component_partition",
                            {"token": draft["preview"]["token"], "rise": rise},
                        )
                        client.call(
                            "pick_component_partitions",
                            {"token": first_token, "x": 550, "y": 350},
                            error=True,
                        )
                    token = draft["preview"]["token"]
                    part = draft["preview"]["partition"]["partitions"][0]
                    center = average(
                        [vertex["preview_position"] for vertex in part["vertices"]]
                    )
                    for projection in ["top", "perspective"]:
                        camera = (
                            {
                                "projection": projection,
                                "center": center,
                                "ortho_height": 256,
                            }
                            if projection == "top"
                            else {
                                "projection": projection,
                                "position": [
                                    center[0] + 5,
                                    center[1] + 7,
                                    center[2] + 300,
                                ],
                                "look_at": center,
                            }
                        )
                        client.call("set_camera", camera)
                        for piece in part["loops"]:
                            point = average(
                                [
                                    part["vertices"][index]["preview_position"]
                                    for index in piece["vertices"]
                                ]
                            )
                            before = inspect()["preview"]
                            query = pick(token, point)
                            check(
                                len(query["candidates"]) == 1
                                and query["candidates"][0]["partition"]
                                == {"solid": solid, "partition": piece["id"]},
                                f"{label} rise={rise} {projection} piece={piece['id']}: viewport coordinates resolve the correct loop",
                            )
                            check(
                                math.dist(query["candidates"][0]["position"], point)
                                < 1e-3,
                                f"{label} rise={rise} {projection} piece={piece['id']}: hit matches the shared surface",
                            )
                            check(
                                inspect()["preview"] == before,
                                f"{label} rise={rise} {projection}: picking is read-only",
                            )
                            selected = client.call(
                                "select_component_partitions",
                                {
                                    "token": token,
                                    "partitions": [query["candidates"][0]["partition"]],
                                },
                            )
                            check(
                                selected["preview"]["partition"]["selection"]
                                == [{"solid": solid, "partition": piece["id"]}]
                                and selected["preview"]["token"] == token,
                                f"{label} rise={rise} {projection}: query refs highlight without replacing the draft",
                            )
                        empty = client.call(
                            "pick_component_partitions",
                            {"token": token, "x": 1, "y": 1},
                        )
                        check(
                            not empty["candidates"],
                            f"{label} rise={rise} {projection}: empty viewport query",
                        )
                        client.call(
                            "pick_component_partitions",
                            {"token": token, "x": -1, "y": 350},
                            error=True,
                        )
                        client.call(
                            "pick_component_partitions",
                            {"token": token, "x": 1100, "y": 350, "width": 1100},
                            error=True,
                        )
                        check(
                            detail(client, [object_id]) == source
                            and client.call("get_map_info")["undo"] == history
                            and inspect()["source_selection"] == source_selection,
                            f"{label} rise={rise} {projection}: queries and highlighting preserve source, policy and history",
                        )
                        if render and not owned and not oblique:
                            client.render(
                                output
                                / f"partition-{direction}-{int(rise)}-{projection}.png",
                                surface="flat",
                            )
                    if not oblique:
                        client.call(
                            "set_camera",
                            {
                                "projection": "top",
                                "center": center,
                                "ortho_height": 256,
                            },
                        )
                        boundary = average(
                            [
                                part["vertices"][index]["preview_position"]
                                for index in [4, 5]
                            ]
                        )
                        query = pick(token, boundary)
                        check(
                            [
                                candidate["partition"]["partition"]
                                for candidate in query["candidates"]
                            ]
                            == [0, 1],
                            f"{label} rise={rise}: shared boundary has stable piece ordering",
                        )
                client.call("cancel_component_edit", {"token": token})
                client.call(
                    "pick_component_partitions",
                    {"token": token, "x": 550, "y": 350},
                    error=True,
                )
                check(
                    detail(client, [object_id]) == source,
                    f"{label}: Cancel restores source after loop inspection",
                )
    for hidden_owner in [False, True]:
        client.call("new_map", {"discard_changes": True})
        object_id = client.call(
            "create_block", {"mins": [0, 0, 0], "maxs": [64, 64, 64]}
        )["id"]
        solid = int(object_id.split(":")[1])
        if hidden_owner:
            object_id = client.call(
                "tie_to_entity", {"ids": [object_id], "classname": "func_detail"}
            )["id"]
        client.call("set_object_visibility", {"action": "hide", "ids": [object_id]})
        draft = stage(solid)
        client.call(
            "set_camera",
            {"projection": "top", "center": [32, 32, 32], "ortho_height": 256},
        )
        check(
            not pick(draft["preview"]["token"], [16, 16, 64])["candidates"],
            f"Hidden owner={hidden_owner}: draft loops are excluded",
        )
        client.call("cancel_component_edit", {"token": draft["preview"]["token"]})
    client.call("new_map", {"discard_changes": True})
    source_id = client.call("create_block", {"mins": [0, 0, 0], "maxs": [64, 64, 64]})[
        "id"
    ]
    solid = int(source_id.split(":")[1])
    blocker = client.call(
        "create_block",
        {"mins": [-1, -1, 80], "maxs": [65, 65, 96], "material": "tools/toolsclip"},
    )["id"]
    draft = stage(solid)
    token = draft["preview"]["token"]
    client.call("set_view", {"tool_textures": True})
    client.call(
        "set_camera", {"projection": "top", "center": [32, 32, 32], "ortho_height": 256}
    )
    check(
        not pick(token, [16, 16, 64])["candidates"],
        "Visible foreground brush occludes draft loops",
    )
    client.call("set_view", {"tool_textures": False})
    check(
        len(pick(token, [16, 16, 64])["candidates"]) == 1,
        "Shared tool filter removes the occluder without changing the draft",
    )
    client.call("set_object_visibility", {"action": "hide", "ids": [blocker]})
    client.call(
        "pick_component_partitions", {"token": token, "x": 550, "y": 350}, error=True
    )
    check(inspect()["preview"] is None, "Source edits invalidate picking tokens")
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-partition-picking-validation")
    )
    parser.add_argument("--render", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output, args.render)
        (args.output / "results.json").write_text(
            json.dumps(
                {"checks": len(checks), "passed": checks, "render": args.render},
                indent=2,
            )
            + "\n"
        )
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
