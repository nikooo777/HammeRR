import argparse
import json
import math
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
        return client.call("get_carve_preview")

    def preview(cutters, targets, error=False):
        return client.call(
            "preview_carve_many",
            {
                "cutters": [number(value) for value in cutters],
                "targets": [number(value) for value in targets],
                "stamp": state()["stamp"],
            },
            error=error,
        )

    tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
    check(
        "preview_carve_many" in tools and state()["limits"]["cutters"] == 8,
        "Multiple-cutter tool and bounded inputs are discoverable",
    )
    ids = [
        block([0, 0, 0], [128, 128, 128]),
        block([16, -16, 16], [80, 80, 144], "brick/brickwall001a"),
        block([48, 48, -16], [112, 144, 112], "dev/dev_measuregeneric01"),
        block([256, 0, 0], [288, 32, 32]),
    ]
    target, first, second, untouched = ids
    client.call("select", {"ids": ids})
    source = detail(client, ids)
    history = client.call("get_map_info")["undo"]
    if render:
        client.call("set_transform_tool", {"mode": "select"})
        client.call(
            "set_camera", {"position": [230, -230, 240], "look_at": [64, 64, 64]}
        )
        baseline = client.render(output / "multiple-cutters-before.png", surface="flat")
    stamp = state()["stamp"]
    draft = preview([second, first], [target])
    token = draft["preview"]["token"]
    result = draft["preview"]["result"]
    check(
        result["cutters"] == [number(first), number(second)]
        and len(draft["sources"]) == 4,
        "Draft normalizes cutter order and retains unchecked candidates",
    )
    check(
        detail(client, ids) == source
        and client.call("get_map_info")["undo"] == history,
        "Batch preview leaves source brushes, cutters and history unchanged",
    )
    fragments = result["changes"][0]["fragments"]
    topology = client.call("get_brush_components", {"solids": fragments})["brushes"]
    check(
        all(4 <= len(shape["faces"]) <= 64 for shape in topology),
        "Every preview fragment has valid inspectable closed topology",
    )
    if render:
        pixels = client.render(output / "multiple-cutters-preview.png", surface="flat")
        check(
            pixels != baseline,
            "Shared renderer displays the combined subtraction draft",
        )
    for cutters, targets in [
        ([], [target]),
        ([first, first], [target]),
        ([first, second], [first]),
        ([first] * 9, [target]),
    ]:
        preview(cutters, targets, error=True)
    client.call(
        "preview_carve_many",
        {
            "cutters": [number(first), number(second)],
            "targets": [number(target)],
            "stamp": stamp,
        },
        error=True,
    )
    check(
        state()["preview"]["token"] == token,
        "Invalid roles, duplicate cutters, budgets and stale stamps preserve the valid draft",
    )
    client.call("cancel_carve", {"token": token})
    check(
        detail(client, ids) == source
        and client.call("get_map_info")["undo"] == history,
        "Discard preserves all original objects and undo history",
    )
    if render:
        check(
            client.render(output / "multiple-cutters-discarded.png", surface="flat")
            == baseline,
            "Discard restores identical baseline pixels",
        )
    repeated = preview([first, second], [target])
    check(
        repeated["preview"]["result"] == result,
        "Reversed caller order produces identical fragment IDs and inheritance",
    )
    if render:
        check(
            client.render(output / "multiple-cutters-reordered.png", surface="flat")
            == pixels,
            "Reordered input produces identical draft pixels",
        )
    client.call("commit_carve", {"token": token}, error=True)
    applied = client.call("commit_carve", {"token": repeated["preview"]["token"]})
    pieces = detail(client, applied["objects"])
    check(
        client.call("get_brush_components", {"solids": fragments})["brushes"]
        == topology,
        "Applied geometry matches every draft vertex, edge and face",
    )
    cutter_faces = {
        number(key): {face["side"]: face for face in value["faces"]}
        for key, value in zip(ids, source)
    }
    piece_faces = {
        piece["solid"]: {face["side"]: face for face in piece["faces"]}
        for piece in pieces
    }
    fields = ["material", "uaxis", "vaxis", "lightmap_scale", "smoothing_groups"]
    check(
        all(
            all(
                piece_faces[face["solid"]][face["side"]][field]
                == cutter_faces[face["source_solid"]][face["source_side"]][field]
                for field in fields
            )
            for face in applied["faces"]
        ),
        "Every resulting face inherits material, world UVs and lighting from its reported source",
    )
    check(
        {face["source_solid"] for face in applied["faces"] if face["cutter"]}
        == {number(first), number(second)},
        "Both cutters contribute correctly attributed new faces",
    )
    check(
        detail(client, [first, second, untouched]) == source[1:]
        and len(client.call("get_map_info")["undo"]) == len(history) + 1,
        "Apply keeps both cutters and unchecked brushes exact in one undo step",
    )
    if render:
        client.call("select", {"ids": []})
        client.render(output / "multiple-cutters-applied.png", surface="flat")
        client.call(
            "set_object_visibility",
            {"action": "hide", "ids": [first, second, untouched]},
        )
        client.render(output / "multiple-cutters-cavities.png", surface="flat")
        client.call("undo")
        check(
            detail(client, [first, second, untouched]) == source[1:],
            "Temporary cutter hiding restores exact input visibility",
        )
    client.call("undo")
    check(
        detail(client, ids) == source,
        "Undo restores all source IDs, planes and settings",
    )
    client.call("redo")
    check(
        detail(client, applied["objects"]) == pieces,
        "Redo restores exact batch fragments and IDs",
    )
    path = output / "multiple-cutters.vmf"
    client.call("save_map", {"path": str(path)})
    client.call("open_map", {"path": str(path)})
    check(
        detail(client, applied["objects"]) == pieces,
        "Batch fragments survive VMF save/reopen",
    )

    for entity in [False, True]:
        client.call("new_map", {"discard_changes": True})
        target = block([0, 0, 0], [64, 64, 64])
        cutters = [block([0, 0, 0], [32, 64, 64]), block([32, 0, 0], [64, 64, 64])]
        owner = (
            client.call("tie_to_entity", {"ids": [target], "classname": "func_detail"})[
                "id"
            ]
            if entity
            else target
        )
        before = detail(client, [owner, *cutters])
        if entity:
            preview(cutters, [target], error=True)
            check(
                detail(client, [owner, *cutters]) == before,
                "Combined full removal rejects an empty brush owner atomically",
            )
        else:
            draft = preview(cutters, [target])
            check(
                draft["preview"]["result"]["removed"] == [number(target)],
                "Combined cutter halves fully remove a world target",
            )
            client.call("commit_carve", {"token": draft["preview"]["token"]})
            client.call("undo")
            check(
                detail(client, [owner, *cutters]) == before,
                "Undo restores a target fully covered by two cutters",
            )

    samples = random.Random(806)
    for index in range(24):
        client.call("new_map", {"discard_changes": True})
        target = block([0, 0, 0], [64, 64, 64])
        boxes = []
        cutters = []
        for _ in range(2):
            mins = [samples.randrange(-64, 240) / 4 for _ in range(3)]
            maxs = [value + samples.randrange(32, 224) / 4 for value in mins]
            boxes.append((mins, maxs))
            cutters.append(block(mins, maxs))

        def overlap(bounds):
            return math.prod(
                max(0, min([64, *highs]) - max([0, *lows]))
                for lows, highs in zip(
                    zip(*(box[0] for box in bounds)), zip(*(box[1] for box in bounds))
                )
            )

        removed = overlap([boxes[0]]) + overlap([boxes[1]]) - overlap(boxes)
        if removed == 0:
            before = detail(client, [target, *cutters])
            preview(cutters, [target], error=True)
            check(
                detail(client, [target, *cutters]) == before,
                f"Fractional union {index}: disjoint cutters preserve inputs",
            )
        else:
            draft = preview(cutters, [target])
            applied = client.call("commit_carve", {"token": draft["preview"]["token"]})
            pieces = detail(client, applied["objects"])
            volume = sum(
                math.prod(
                    b - a
                    for a, b in zip(piece["bounds"]["mins"], piece["bounds"]["maxs"])
                )
                for piece in pieces
            )
            check(
                abs(volume - (64**3 - removed)) < 1e-5,
                f"Fractional union {index}: combined difference volume matches inclusion-exclusion",
            )

    client.call("new_map", {"discard_changes": True})
    outer = block([-144, -144, -16], [144, 144, 144])
    cutters = [
        block([-128, -128, 0], [0, 128, 128]),
        block([0, -128, 0], [128, 128, 128]),
    ]
    draft = preview(cutters, [outer])
    client.call("commit_carve", {"token": draft["preview"]["token"]})
    client.call("delete_objects", {"ids": cutters})
    client.call(
        "create_entity",
        {"classname": "info_player_counterterrorist", "origin": [0, 0, 64]},
    )
    path = output / "multi-cutter-room.vmf"
    client.call("save_map", {"path": str(path)})
    client.call("open_map", {"path": str(path)})
    compiled = client.call(
        "compile_map", {"path": str(output / "multi-cutter-room.bsp")}
    )
    check(
        compiled["success"]
        and compiled["leak"] is None
        and compiled["stats"]["brushes"] >= 6,
        "Two retained cutter halves form a sealed compiled room after removal and VMF round trip",
    )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-carve-many"))
    parser.add_argument(
        "--render",
        action="store_true",
        help="Check offscreen renders without desktop windows or input",
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
