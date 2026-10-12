import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail
from validate_texture_chain import uv


def validate(client, output, render=False):
    checks = []
    worst_error = 0.0

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def inspect(solid=None):
        return client.call("get_brush_components", {"solids": [solid]} if solid else {})

    schemas = {
        tool["name"]: tool["inputSchema"]
        for tool in client.rpc("tools/list", {})["tools"]
    }
    check(
        all(
            "roof_texture_lock" in schemas[name]["properties"]
            for name in ["preview_component_crease", "raise_component_partition"]
        ),
        "Roof-specific attachment inputs are discoverable in both workflows",
    )
    for workflow in ["crease", "partition"]:
        for owned in [False, True]:
            for oblique in [False, True]:
                for direction in range(2):
                    label = f"{workflow} owned={owned} oblique={oblique} direction={direction}"
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
                    source_brush = source["brushes"][0] if owned else source
                    faces = [
                        {"solid": solid, "side": face["side"]}
                        for face in source_brush["faces"]
                    ]
                    client.call(
                        "edit_faces",
                        {
                            "faces": faces,
                            "scale_u": 0.3125,
                            "scale_v": -0.523456789,
                            "shift_u": 17.123456789,
                            "shift_v": -9.125,
                            "lightmap_scale": 8,
                            "smoothing_groups": [1, 3],
                        },
                    )
                    source_path = (
                        output
                        / f"source-{workflow}-{int(owned)}-{int(oblique)}-{direction}.vmf"
                    )
                    client.call("save_map", {"path": str(source_path)})
                    source = detail(client, [object_id])[0]
                    source_brush = source["brushes"][0] if owned else source
                    face = max(
                        source_brush["faces"], key=lambda face: face["normal"][2]
                    )
                    selected = client.call(
                        "select_brush_components",
                        {
                            "components": [{"solid": solid, "sides": [face["side"]]}],
                            "snapshot": inspect()["snapshot"],
                        },
                    )
                    pairs = [
                        {
                            "solid": solid,
                            "edges": selected["crease_choices"][0]["pairs"][direction][
                                "edges"
                            ],
                        }
                    ]
                    stage = client.call(
                        "preview_component_partition",
                        {"pairs": pairs, "snapshot": inspect()["snapshot"]},
                    )
                    flat = stage["preview"]["partition"]["partitions"][0]
                    check(
                        not stage["preview"]["partition"]["roof_texture_lock"],
                        f"{label}: staged policy defaults to world projection",
                    )
                    client.call(
                        "cancel_component_edit", {"token": stage["preview"]["token"]}
                    )
                    history = client.call("get_map_info")["undo"]
                    policies = {}
                    for lock in [False, True]:
                        client.call(
                            "open_map",
                            {"path": str(source_path), "discard_changes": True},
                        )
                        client.call(
                            "select_brush_components",
                            {
                                "components": [
                                    {"solid": solid, "sides": [face["side"]]}
                                ],
                                "snapshot": inspect()["snapshot"],
                            },
                        )
                        history = client.call("get_map_info")["undo"]
                        if workflow == "crease":
                            args = {
                                "pairs": pairs,
                                "rise": 16.125,
                                "snapshot": inspect()["snapshot"],
                            }
                            if lock:
                                args["roof_texture_lock"] = True
                            draft = client.call("preview_component_crease", args)
                        else:
                            staged = client.call(
                                "preview_component_partition",
                                {"pairs": pairs, "snapshot": inspect()["snapshot"]},
                            )
                            draft = client.call(
                                "raise_component_partition",
                                {
                                    "token": staged["preview"]["token"],
                                    "rise": 16.125,
                                    "roof_texture_lock": lock,
                                },
                            )
                            draft = client.call(
                                "raise_component_partition",
                                {"token": draft["preview"]["token"], "rise": 0},
                            )
                            check(
                                draft["preview"]["partition"]["roof_texture_lock"]
                                == lock,
                                f"{label} lock={lock}: omitted updates and flat restoration retain policy",
                            )
                            draft = client.call(
                                "raise_component_partition",
                                {"token": draft["preview"]["token"], "rise": 16.125},
                            )
                        token = draft["preview"]["token"]
                        change = draft["preview"]["crease"]["changes"][0]
                        check(
                            draft["preview"]["crease"]["roof_texture_lock"] == lock
                            and draft["texture_mode"]
                            == ("roof_attached" if lock else "world"),
                            f"{label} lock={lock}: report names the scoped projection policy",
                        )
                        check(
                            detail(client, [object_id])[0] == source
                            and client.call("get_map_info")["undo"] == history,
                            f"{label} lock={lock}: draft preserves committed source and history",
                        )
                        client.call("cancel_component_edit", {"token": token})
                        if workflow == "crease":
                            draft = client.call(
                                "preview_component_crease",
                                {
                                    "pairs": pairs,
                                    "rise": 16.125,
                                    "roof_texture_lock": lock,
                                    "snapshot": inspect()["snapshot"],
                                },
                            )
                        else:
                            staged = client.call(
                                "preview_component_partition",
                                {"pairs": pairs, "snapshot": inspect()["snapshot"]},
                            )
                            draft = client.call(
                                "raise_component_partition",
                                {
                                    "token": staged["preview"]["token"],
                                    "rise": 16.125,
                                    "roof_texture_lock": lock,
                                },
                            )
                        check(
                            draft["preview"]["crease"]["changes"][0]["new_side"]
                            == change["new_side"],
                            f"{label} lock={lock}: cancelled UV drafts preserve side allocation",
                        )
                        client.call(
                            "commit_component_edit",
                            {"token": draft["preview"]["token"]},
                        )
                        applied = detail(client, [object_id])[0]
                        applied_brush = applied["brushes"][0] if owned else applied
                        roof_ids = [face["side"], change["new_side"]]
                        error = 0.0
                        for piece in flat["loops"]:
                            roof = next(
                                side
                                for side in applied_brush["faces"]
                                if side["side"] == roof_ids[piece["id"]]
                            )
                            for vertex in piece["vertices"]:
                                point = flat["vertices"][vertex]["position"]
                                moved = [
                                    point[axis] + face["normal"][axis] * 16.125
                                    if vertex >= 4
                                    else point[axis]
                                    for axis in range(3)
                                ]
                                expected = uv(face, point)
                                actual = uv(roof, moved)
                                error = max(error, math.dist(expected, actual))
                        if lock:
                            check(
                                error < 1e-4,
                                f"{label}: all eight roof corner UVs stay attached",
                            )
                            worst_error = max(worst_error, error)
                        elif oblique:
                            check(
                                error > 1e-3,
                                f"{label}: world projection remains independent on oblique roofs",
                            )
                        for side in applied_brush["faces"]:
                            original = (
                                face
                                if side["side"] in roof_ids
                                else next(
                                    candidate
                                    for candidate in source_brush["faces"]
                                    if candidate["side"] == side["side"]
                                )
                            )
                            fields = ["material", "lightmap_scale", "smoothing_groups"]
                            if side["side"] not in roof_ids or not lock:
                                fields += ["uaxis", "vaxis"]
                            check(
                                all(side[field] == original[field] for field in fields),
                                f"{label} lock={lock}: side {side['side']} preserves inherited metadata and neighboring projections",
                            )
                        if render and oblique and not owned and workflow == "partition":
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
                            pixels = client.render(
                                output / f"roof-{direction}-{int(lock)}.png",
                                surface="textured",
                                camera={
                                    "position": [150, -160, 150],
                                    "look_at": [30, 20, 45],
                                },
                            )
                            policies[f"pixels-{lock}"] = pixels
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
                        path = (
                            output
                            / f"{workflow}-{int(owned)}-{int(oblique)}-{direction}-{int(lock)}.vmf"
                        )
                        client.call("save_map", {"path": str(path)})
                        client.call(
                            "open_map", {"path": str(path), "discard_changes": True}
                        )
                        check(
                            detail(client, [object_id])[0] == applied,
                            f"{label} lock={lock}: VMF round trip retains exact authored projections",
                        )
                    if render and oblique and not owned and workflow == "partition":
                        check(
                            policies["pixels-False"] != policies["pixels-True"],
                            f"Direction {direction}: textured renderer distinguishes world and attached roof projection",
                        )
    return checks, worst_error


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-roof-texture-validation")
    )
    parser.add_argument("--render", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks, worst_error = validate(client, args.output, args.render)
        result = {
            "checks": len(checks),
            "passed": checks,
            "render": args.render,
            "worst_uv_error": worst_error,
        }
        (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
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
