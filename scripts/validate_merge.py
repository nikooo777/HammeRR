import argparse
import json
import random
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail


def validate(client, output, render=False):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def block(mins, maxs, material="dev/dev_measuregeneric01"):
        return client.call(
            "create_block", {"mins": mins, "maxs": maxs, "material": material}
        )["id"]

    def number(value):
        return int(value.split(":")[1])

    def state():
        return client.call("get_merge_preview")

    def preview(ids, error=False):
        return client.call(
            "preview_merge",
            {"sources": [number(value) for value in ids], "stamp": state()["stamp"]},
            error=error,
        )

    def topology(ids):
        return client.call("get_brush_components", {"solids": ids})["brushes"]

    tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
    check(
        {"get_merge_preview", "preview_merge", "commit_merge", "cancel_merge"} <= tools,
        "Merge inspection, preview, Apply and Cancel tools are discoverable",
    )
    first = block([0, 0, 0], [64, 64, 64])
    second = block([64, 0, 0], [128, 64, 64], "brick/brickwall001a")
    unrelated = block([256, 0, 0], [288, 32, 32])
    ids = [first, second, unrelated]
    for index, brush in enumerate(detail(client, [first, second])):
        client.call(
            "edit_faces",
            {
                "faces": [
                    {"solid": brush["solid"], "side": face["side"]}
                    for face in brush["faces"]
                ],
                "shift_u": 17.25 + index,
                "scale_v": -0.75,
                "lightmap_scale": 32,
                "smoothing_groups": [1, 3],
            },
        )
    client.call("select", {"ids": ids})
    original = detail(client, ids)
    history = client.call("get_map_info")["undo"]
    if render:
        client.call("set_transform_tool", {"mode": "select"})
        client.call(
            "set_camera", {"position": [230, -210, 190], "look_at": [64, 32, 32]}
        )
        baseline = client.render(output / "merge-before.png", surface="flat")
    stamp = state()["stamp"]
    draft = preview([second, first])
    token = draft["preview"]["token"]
    result = draft["preview"]["result"]
    check(
        result["sources"] == [number(first), number(second)]
        and result["solid"] == number(first)
        and result["removed"] == [number(second)]
        and len(result["faces"]) == 6
        and len(draft["sources"]) == 3,
        "Convex union keeps the lowest brush ID, six outer faces and unchecked candidates",
    )
    shape = topology([number(first)])
    check(
        len(shape[0]["faces"]) == 6,
        "Read-only topology inspects the same merged preview",
    )
    check(
        detail(client, ids) == original
        and client.call("get_map_info")["undo"] == history,
        "Preview preserves committed geometry and Undo history",
    )
    check(
        client.call("get_transform_handles")["handles"] == [],
        "Object transform handles pause while the merge preview is visible",
    )
    if render:
        pixels = client.render(output / "merge-preview.png", surface="flat")
    client.call(
        "preview_merge",
        {"sources": [number(first), number(second)], "stamp": stamp},
        error=True,
    )
    for invalid in [[], [first], [first, first], [first, unrelated]]:
        preview(invalid, error=True)
    check(
        state()["preview"]["token"] == token,
        "Stale stamps, duplicates, missing sources and disconnected unions keep the valid draft",
    )
    replacement = preview([first, second])
    replacement_token = replacement["preview"]["token"]
    client.call("commit_merge", {"token": token}, error=True)
    client.call("cancel_merge", {"token": token}, error=True)
    check(
        state()["preview"]["token"] == replacement_token,
        "Stale Apply and Cancel cannot affect a replacement merge",
    )
    client.call("cancel_merge", {"token": replacement_token})
    check(
        detail(client, ids) == original
        and client.call("get_map_info")["undo"] == history,
        "Discard restores unchanged geometry and history",
    )
    if render:
        check(
            client.render(output / "merge-cancelled.png", surface="flat") == baseline,
            "Discard restores identical baseline pixels",
        )
    repeated = preview([first, second])
    check(
        repeated["preview"]["result"] == result,
        "Discard and re-preview preserve exact brush and face identities",
    )
    client.call("commit_merge", {"token": repeated["preview"]["token"]})
    applied = detail(client, [first, unrelated])
    check(
        topology([number(first)]) == shape,
        "Apply stores the exact preview topology and face IDs",
    )
    check(
        len(client.call("get_map_info")["undo"]) == len(history) + 1,
        "Apply records one Undo step",
    )
    check(applied[1] == original[2], "Apply preserves unrelated brushes")
    source_faces = {
        (brush["solid"], face["side"]): face
        for brush in original[:2]
        for face in brush["faces"]
    }
    inheritance = {face["side"]: face for face in result["faces"]}
    check(
        all(
            face
            == source_faces[
                (
                    inheritance[face["side"]]["source_solid"],
                    inheritance[face["side"]]["source_side"],
                )
            ]
            for face in applied[0]["faces"]
        ),
        "Every outer face keeps its exact source plane, material, UVs and lighting",
    )
    if render:
        check(
            client.render(output / "merge-applied.png", surface="flat") == pixels,
            "Applied pixels match the shared preview exactly",
        )
    client.call("undo")
    check(detail(client, ids) == original, "Undo restores exact source brushes")
    client.call("redo")
    check(
        detail(client, [first, unrelated]) == applied, "Redo restores the exact merge"
    )
    saved = output / "merged.vmf"
    client.call("save_map", {"path": str(saved)})
    client.call("open_map", {"path": str(saved)})
    check(
        detail(client, [first, unrelated]) == applied,
        "Merged IDs, projections and lighting survive VMF save and reopen",
    )

    client.call("new_map", {"discard_changes": True})
    ring = [
        block([0, 0, 0], [128, 16, 64]),
        block([0, 112, 0], [128, 128, 64]),
        block([0, 16, 0], [16, 112, 64]),
        block([112, 16, 0], [128, 112, 64]),
    ]
    ring_source = detail(client, ring)
    preview(ring, error=True)
    check(
        detail(client, ring) == ring_source and state()["preview"] is None,
        "Covered hull corners cannot hide an uncovered interior cavity",
    )
    for index, (mins, maxs) in enumerate(
        [
            ([64, 32, 0], [128, 96, 64]),
            ([64, 64, 0], [128, 128, 64]),
            ([64, 64, 64], [128, 128, 128]),
        ]
    ):
        client.call("new_map", {"discard_changes": True})
        pair = [block([0, 0, 0], [64, 64, 64]), block(mins, maxs)]
        source = detail(client, pair)
        preview(pair, error=True)
        check(
            detail(client, pair) == source, f"Nonconvex/tangent union {index} rejects"
        )

    rng = random.Random(10)
    for index in range(12):
        client.call("new_map", {"discard_changes": True})
        x, y, z = [rng.randint(-512, 512) / 8 for _ in range(3)]
        width, height, depth = [rng.randint(64, 256) / 4 for _ in range(3)]
        split_x, split_y = width * rng.uniform(0.2, 0.8), height * rng.uniform(0.2, 0.8)
        sources = [
            block([x + xa, y + ya, z], [x + xb, y + yb, z + depth])
            for xa, xb in [(0, split_x), (split_x, width)]
            for ya, yb in [(0, split_y), (split_y, height)]
        ]
        draft = preview(sources)
        merged_id = draft["preview"]["result"]["solid"]
        brush = topology([merged_id])[0]
        vertices = [vertex["position"] for vertex in brush["vertices"]]
        actual_min = [min(point[axis] for point in vertices) for axis in range(3)]
        actual_max = [max(point[axis] for point in vertices) for axis in range(3)]
        check(
            all(
                abs(actual - expected) < 0.0001
                for actual, expected in zip(
                    actual_min + actual_max, [x, y, z, x + width, y + height, z + depth]
                )
            )
            and len(brush["faces"]) == 6,
            f"Fractional tiled union {index} keeps the exact exterior bounds",
        )
        client.call("cancel_merge", {"token": draft["preview"]["token"]})

    client.call("new_map", {"discard_changes": True})
    pair = [block([0, 0, 0], [64, 64, 64]), block([64, 0, 0], [128, 64, 64])]
    draft = preview(pair)
    client.call(
        "transform_objects", {"ids": pair, "translation": [16, 0, 0], "preview": True}
    )
    client.call("cancel_merge", {"token": draft["preview"]["token"]}, error=True)
    check(
        state()["preview"] is None
        and client.call("get_transform_handles")["state"]["preview"] is not None,
        "A numeric preview replaces merge and survives stale merge cancellation",
    )
    draft = preview(pair)
    client.call(
        "create_primitive",
        {
            "shape": "cylinder",
            "mins": [256, 0, 0],
            "maxs": [320, 64, 64],
            "preview": True,
        },
    )
    primitive = client.call("get_primitive_preview")
    client.call("cancel_merge", {"token": draft["preview"]["token"]}, error=True)
    check(
        state()["preview"] is None
        and client.call("get_primitive_preview") == primitive,
        "A primitive preview survives stale merge cancellation",
    )
    draft = preview(pair)
    client.call("select", {"ids": [pair[0]]})
    client.call("commit_merge", {"token": draft["preview"]["token"]}, error=True)
    check(state()["preview"] is None, "Selection changes invalidate merge tokens")

    client.call("new_map", {"discard_changes": True})
    pair = [block([0, 0, 0], [64, 64, 64]), block([64, 0, 0], [128, 64, 64])]
    sibling = block([256, 0, 0], [288, 32, 32])
    owner = client.call(
        "tie_to_entity", {"ids": [*pair, sibling], "classname": "func_detail"}
    )["id"]
    original_owner = detail(client, [owner])[0]
    draft = preview(pair)
    check(
        draft["preview"]["result"]["object"] == owner,
        "Entity-child merges retain the brush entity owner",
    )
    client.call("commit_merge", {"token": draft["preview"]["token"]})
    applied_owner = detail(client, [owner])[0]
    check(
        len(applied_owner["brushes"]) == 2
        and applied_owner["properties"] == original_owner["properties"]
        and applied_owner["brushes"][1] == original_owner["brushes"][2],
        "Entity merge preserves sibling brushes and owner properties",
    )
    client.call("undo")
    check(
        detail(client, [owner])[0] == original_owner,
        "Undo restores all entity children",
    )
    world = block([128, 0, 0], [192, 64, 64])
    preview([pair[1], world], error=True)
    check(state()["preview"] is None, "Mixed entity/world ownership rejects")
    top = next(
        face
        for face in original_owner["brushes"][0]["faces"]
        if face["normal"][2] > 0.9
    )
    client.call(
        "create_terrain",
        {"faces": [{"solid": number(pair[0]), "side": top["side"]}], "power": 2},
    )
    preview(pair, error=True)
    check(state()["preview"] is None, "Displacement brush merging rejects")

    client.call("new_map", {"discard_changes": True})
    floor = [
        block([-144, -144, -16], [0, 144, 0]),
        block([0, -144, -16], [144, 144, 0]),
    ]
    draft = preview(floor)
    client.call("commit_merge", {"token": draft["preview"]["token"]})
    for mins, maxs in [
        ([-144, -144, 128], [144, 144, 144]),
        ([-144, -144, 0], [-128, 144, 128]),
        ([128, -144, 0], [144, 144, 128]),
        ([-128, -144, 0], [128, -128, 128]),
        ([-128, 128, 0], [128, 144, 128]),
    ]:
        block(mins, maxs)
    client.call(
        "create_entity",
        {"classname": "info_player_counterterrorist", "origin": [0, 0, 64]},
    )
    saved = output / "merged-room.vmf"
    client.call("save_map", {"path": str(saved)})
    client.call("open_map", {"path": str(saved)})
    compiled = client.call("compile_map", {"path": str(output / "merged-room.bsp")})
    check(
        compiled["success"]
        and compiled["leak"] is None
        and compiled["stats"]["brushes"] == 6,
        "A room with a merged floor saves, reopens and compiles sealed with six brushes",
    )
    return checks


def main():
    parser = argparse.ArgumentParser(
        description="Validate convex brush merging through private headless MCP"
    )
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-merge-validation")
    )
    parser.add_argument(
        "--render", action="store_true", help="Also check offscreen preview pixels"
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output, args.render)
        result = {"passed": len(checks), "checks": checks}
        (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))
    finally:
        client.close()


if __name__ == "__main__":
    main()
