import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail
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
        "texture_lock" in schemas["preview_component_split"]["properties"],
        "Split attachment input is discoverable",
    )
    for edge in [False, True]:
        for owned in [False, True]:
            for oblique in [False, True]:
                label = f"edge={edge} owned={owned} oblique={oblique}"
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
                path = output / f"source-{int(edge)}-{int(owned)}-{int(oblique)}.vmf"
                client.call("save_map", {"path": str(path)})
                source = detail(client, [object_id])[0]
                brush = source["brushes"][0] if owned else source
                shape = inspect(solid)["brushes"][0]
                top = max(brush["faces"], key=lambda face: face["normal"][2])
                sides = (
                    max(
                        (
                            candidate
                            for candidate in shape["edges"]
                            if top["side"] in candidate["sides"]
                        ),
                        key=lambda candidate: sum(
                            point[2] for point in candidate["positions"]
                        ),
                    )["sides"]
                    if edge
                    else [top["side"]]
                )
                points = [
                    vertex["position"]
                    for vertex in shape["vertices"]
                    if set(sides) <= set(vertex["sides"])
                ]
                center = [
                    sum(point[axis] for point in points) / len(points)
                    for axis in range(3)
                ]
                source_faces = {face["side"]: face for face in brush["faces"]}
                pixels = {}
                for lock in [False, True]:
                    client.call(
                        "open_map", {"path": str(path), "discard_changes": True}
                    )
                    client.call(
                        "select_brush_components",
                        {
                            "components": [{"solid": solid, "sides": sides}],
                            "snapshot": inspect()["snapshot"],
                        },
                    )
                    history = client.call("get_map_info")["undo"]
                    args = {"rise": 16.125, "snapshot": inspect()["snapshot"]}
                    if lock:
                        args["texture_lock"] = True
                    draft = client.call("preview_component_split", args)
                    token = draft["preview"]["token"]
                    change = draft["preview"]["split"]["changes"][0]
                    check(
                        draft["preview"]["texture_lock"] == lock
                        and draft["texture_mode"] == ("attached" if lock else "world"),
                        f"{label} lock={lock}: shared report exposes policy",
                    )
                    check(
                        detail(client, [object_id])[0] == source
                        and client.call("get_map_info")["undo"] == history,
                        f"{label} lock={lock}: preview preserves source and history",
                    )
                    client.call(
                        "preview_component_split",
                        {
                            "rise": 0,
                            "texture_lock": not lock,
                            "snapshot": inspect()["snapshot"],
                        },
                        error=True,
                    )
                    check(
                        inspect()["preview"] == draft["preview"],
                        f"{label} lock={lock}: invalid policy update retains valid draft",
                    )
                    client.call("cancel_component_edit", {"token": token})
                    args["snapshot"] = inspect()["snapshot"]
                    draft = client.call("preview_component_split", args)
                    check(
                        draft["preview"]["split"]["changes"][0]["new_sides"]
                        == change["new_sides"],
                        f"{label} lock={lock}: cancelled attachment consumes no IDs",
                    )
                    client.call("commit_component_edit", {"token": token}, error=True)
                    client.call(
                        "commit_component_edit", {"token": draft["preview"]["token"]}
                    )
                    applied = detail(client, [object_id])[0]
                    result = applied["brushes"][0] if owned else applied
                    generated = {
                        face["side"]: face["source"] for face in change["faces"]
                    }
                    error = 0.0
                    for face in result["faces"]:
                        original = source_faces[
                            generated.get(face["side"], face["side"])
                        ]
                        fields = ["material", "lightmap_scale", "smoothing_groups"]
                        if face["side"] not in generated or not lock:
                            fields += ["uaxis", "vaxis"]
                        check(
                            all(face[field] == original[field] for field in fields),
                            f"{label} lock={lock}: side {face['side']} preserves metadata and unaffected projections",
                        )
                        if face["side"] in generated:
                            for point in face["plane"]:
                                before = (
                                    center
                                    if math.dist(point, change["position"]) < 1e-5
                                    else point
                                )
                                error = max(
                                    error,
                                    math.dist(uv(original, before), uv(face, point)),
                                )
                    if lock:
                        check(
                            error < 1e-4,
                            f"{label}: every triangle preserves original corner and center/midpoint UVs",
                        )
                        worst_error = max(worst_error, error)
                    elif oblique or edge:
                        check(
                            error > 1e-3,
                            f"{label}: world projection remains independent from fan deformation",
                        )
                    check(
                        len(client.call("get_map_info")["undo"]) == len(history) + 1,
                        f"{label} lock={lock}: Apply adds one history step",
                    )
                    if render and oblique and not owned:
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
                            output / f"split-{int(edge)}-{int(lock)}.png",
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
                        / f"split-{int(edge)}-{int(owned)}-{int(oblique)}-{int(lock)}.vmf"
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
                        f"{label}: textured renderer distinguishes fan policies",
                    )
    return checks, worst_error


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-split-texture-validation")
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
