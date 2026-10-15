import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_texture_chain import uv


def average(points):
    return [sum(point[axis] for point in points) / len(points) for axis in range(3)]


def validate(client, output, render=False):
    checks = []
    worst_error = 0.0

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def inspect(solid=None):
        return client.call("get_brush_components", {"solids": [solid]} if solid else {})

    tools = {
        tool["name"]: tool["inputSchema"]
        for tool in client.rpc("tools/list", {})["tools"]
    }
    check(
        "texture_lock" in tools["preview_component_weld"]["properties"],
        "Weld attachment policy is discoverable",
    )
    for mode in ["average", "position", "selected_corner", "unselected_corner"]:
        for owned in [False, True]:
            for oblique in [False, True]:
                label = f"{mode} owned={owned} oblique={oblique}"
                client.call("new_map", {"discard_changes": True})
                object_id = client.call(
                    "create_block",
                    {
                        "mins": [0.125, -7.25, 16.5],
                        "maxs": [64.875, 48.125, 80.25],
                        "material": "dev/dev_measuregeneric01",
                    },
                )["id"]
                solid = int(object_id.split(":")[1])
                if oblique:
                    client.call(
                        "transform_objects",
                        {
                            "ids": [object_id],
                            "rotation": [17, -23, 31],
                            "scale": [1.125, 0.75, 1.5],
                            "texture_lock": False,
                            "pivot": [0, 0, 0],
                        },
                    )
                if owned:
                    object_id = client.call(
                        "tie_to_entity",
                        {"ids": [object_id], "classname": "func_detail"},
                    )["id"]
                source = detail(client, [object_id])[0]
                brush = source["brushes"][0] if owned else source
                client.call(
                    "edit_faces",
                    {
                        "faces": [
                            {"solid": solid, "side": face["side"]}
                            for face in brush["faces"]
                        ],
                        "scale_u": 0.3125,
                        "scale_v": -0.523456789,
                        "shift_u": 17.123456789,
                        "shift_v": -9.125,
                        "lightmap_scale": 8,
                        "smoothing_groups": [1, 3],
                    },
                )
                path = output / f"source-{mode}-{int(owned)}-{int(oblique)}.vmf"
                client.call("save_map", {"path": str(path)})
                source = detail(client, [object_id])[0]
                brush = source["brushes"][0] if owned else source
                shape = inspect(solid)["brushes"][0]
                top = max(brush["faces"], key=lambda face: face["normal"][2])
                top_indices = next(
                    face for face in shape["faces"] if face["side"] == top["side"]
                )["vertices"]
                selected_indices = (
                    top_indices[:-1] if mode == "unselected_corner" else top_indices
                )
                selection = [
                    {"solid": solid, "sides": shape["vertices"][index]["sides"]}
                    for index in selected_indices
                ]
                args = {}
                if mode == "position":
                    position = average(
                        [shape["vertices"][index]["position"] for index in top_indices]
                    )
                    args["target"] = [
                        position[axis] + top["normal"][axis] * 8.125
                        for axis in range(3)
                    ]
                elif mode.endswith("corner"):
                    index = (
                        top_indices[-1]
                        if mode == "unselected_corner"
                        else top_indices[0]
                    )
                    args["target_corner"] = {
                        "solid": solid,
                        "sides": shape["vertices"][index]["sides"],
                    }
                source_faces = {face["side"]: face for face in brush["faces"]}
                source_loops = {
                    face["side"]: face["vertices"] for face in shape["faces"]
                }
                pixels = {}
                for lock in [False, True]:
                    client.call(
                        "open_map", {"path": str(path), "discard_changes": True}
                    )
                    client.call(
                        "select_brush_components",
                        {"components": selection, "snapshot": inspect()["snapshot"]},
                    )
                    history = client.call("get_map_info")["undo"]
                    request = {**args, "snapshot": inspect()["snapshot"]}
                    if lock:
                        request["texture_lock"] = True
                    draft = client.call("preview_component_weld", request)
                    token = draft["preview"]["token"]
                    position = draft["preview"]["weld"][0]["position"]
                    check(
                        draft["preview"]["texture_lock"] == lock
                        and draft["texture_mode"]
                        == ("weld_attached" if lock else "world"),
                        f"{label} lock={lock}: shared report distinguishes averaged merged UVs",
                    )
                    check(
                        detail(client, [object_id])[0] == source
                        and client.call("get_map_info")["undo"] == history,
                        f"{label} lock={lock}: preview preserves source and history",
                    )
                    client.call(
                        "preview_component_weld",
                        {
                            "snapshot": inspect()["snapshot"],
                            "target": [65537, 0, 0],
                            "texture_lock": not lock,
                        },
                        error=True,
                    )
                    check(
                        inspect()["preview"] == draft["preview"],
                        f"{label} lock={lock}: invalid update retains valid draft",
                    )
                    client.call("cancel_component_edit", {"token": token})
                    request["snapshot"] = inspect()["snapshot"]
                    draft = client.call("preview_component_weld", request)
                    client.call("commit_component_edit", {"token": token}, error=True)
                    client.call(
                        "commit_component_edit", {"token": draft["preview"]["token"]}
                    )
                    applied = detail(client, [object_id])[0]
                    result = applied["brushes"][0] if owned else applied
                    topology = inspect(solid)["brushes"][0]
                    error = 0.0
                    for face in result["faces"]:
                        original = source_faces[face["side"]]
                        source_indices = source_loops[face["side"]]
                        affected = bool(set(source_indices) & set(selected_indices))
                        fields = ["material", "lightmap_scale", "smoothing_groups"]
                        if not lock or not affected:
                            fields += ["uaxis", "vaxis"]
                        check(
                            all(face[field] == original[field] for field in fields),
                            f"{label} lock={lock}: side {face['side']} preserves metadata and unaffected projections",
                        )
                        if affected:
                            contributors = [
                                shape["vertices"][index]["position"]
                                for index in source_indices
                                if index in selected_indices
                                or math.dist(
                                    shape["vertices"][index]["position"], position
                                )
                                < 1e-5
                            ]
                            merged_source = average(contributors)
                            loop = next(
                                loop
                                for loop in topology["faces"]
                                if loop["side"] == face["side"]
                            )
                            for index in loop["vertices"]:
                                point = topology["vertices"][index]["position"]
                                before = (
                                    merged_source
                                    if math.dist(point, position) < 1e-5
                                    else point
                                )
                                error = max(
                                    error,
                                    math.dist(uv(original, before), uv(face, point)),
                                )
                    if lock:
                        check(
                            error < 1e-4,
                            f"{label}: surviving corner UVs and face-local averaged merged UVs are preserved",
                        )
                        worst_error = max(worst_error, error)
                    elif oblique or mode != "average":
                        check(
                            error > 1e-3,
                            f"{label}: world projection remains independent from welding",
                        )
                    check(
                        len(client.call("get_map_info")["undo"]) == len(history) + 1,
                        f"{label} lock={lock}: Apply adds one undo step",
                    )
                    if render and oblique and not owned and mode == "position":
                        client.call(
                            "set_component_picking",
                            {"enabled": False, "snapshot": inspect()["snapshot"]},
                        )
                        client.call("select", {"ids": []})
                        client.call("select_faces", {"faces": []})
                        client.call(
                            "select_brush_components",
                            {"components": [], "snapshot": inspect()["snapshot"]},
                        )
                        pixels[lock] = client.render(
                            output / f"weld-{int(lock)}.png",
                            surface="textured",
                            camera={
                                "position": [150, -160, 150],
                                "look_at": [30, 20, 45],
                            },
                        )
                    client.call("undo")
                    check(
                        detail(client, [object_id])[0] == source,
                        f"{label} lock={lock}: exact undo",
                    )
                    client.call("redo")
                    check(
                        detail(client, [object_id])[0] == applied,
                        f"{label} lock={lock}: exact redo",
                    )
                    saved = (
                        output
                        / f"weld-{mode}-{int(owned)}-{int(oblique)}-{int(lock)}.vmf"
                    )
                    client.call("save_map", {"path": str(saved)})
                    client.call(
                        "open_map", {"path": str(saved), "discard_changes": True}
                    )
                    check(
                        detail(client, [object_id])[0] == applied,
                        f"{label} lock={lock}: VMF round trip retains authored projections",
                    )
                if pixels:
                    check(
                        pixels[False] != pixels[True],
                        "Textured renderer distinguishes world and averaged-UV weld policies",
                    )
    return checks, worst_error


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-weld-texture-validation")
    )
    parser.add_argument("--render", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks, worst_error = validate(client, args.output, args.render)
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "checks": len(checks),
                    "passed": checks,
                    "render": args.render,
                    "worst_uv_error": worst_error,
                },
                indent=2,
            )
            + "\n"
        )
        print(
            json.dumps(
                {
                    "checks": len(checks),
                    "worst_uv_error": worst_error,
                    "output": str(args.output),
                }
            )
        )
    finally:
        client.close()


if __name__ == "__main__":
    main()
