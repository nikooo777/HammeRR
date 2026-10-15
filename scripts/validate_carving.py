import argparse
import json
import math
import random
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail


def validate(client, output, render):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def block(mins, maxs):
        return client.call(
            "create_block",
            {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"},
        )["id"]

    def number(value):
        return int(value.split(":")[1])

    def state():
        return client.call("get_carve_preview")

    def preview(cutter, targets, error=False):
        return client.call(
            "preview_carve",
            {
                "cutter": number(cutter),
                "targets": [number(t) for t in targets],
                "stamp": state()["stamp"],
            },
            error=error,
        )

    def topology(ids):
        return client.call("get_brush_components", {"solids": ids})["brushes"]

    tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
    check(
        {"get_carve_preview", "preview_carve", "commit_carve", "cancel_carve"} <= tools,
        "Carve inspection, preview, apply and cancel are discoverable",
    )
    target = block([0, 0, 0], [128, 128, 128])
    untouched = block([256, 0, 0], [288, 32, 32])
    cutter = block([32, 32, 32], [96, 96, 96])
    ids = [target, untouched, cutter]
    client.call("select", {"ids": ids})
    original = detail(client, ids)
    history = client.call("get_map_info")["undo"]
    if render:
        client.call(
            "set_camera", {"position": [260, -260, 230], "look_at": [64, 64, 64]}
        )
        baseline = client.render(output / "before-carve.png", surface="flat")
    snapshot = state()["stamp"]
    first = preview(cutter, [target])
    token = first["preview"]["token"]
    result = first["preview"]["result"]
    fragments = result["changes"][0]["fragments"]
    draft_shape = topology(fragments)
    check(
        len(fragments) == 6 and fragments[0] == number(target),
        "An enclosed cutter produces six convex fragments with the source ID retained",
    )
    check(
        len(first["sources"]) == 3,
        "Unchecked brush candidates remain available while previewing",
    )
    check(
        detail(client, ids) == original
        and client.call("get_map_info")["undo"] == history,
        "Carve preview preserves geometry and history",
    )
    check(
        detail(client, [cutter])[0] == original[2],
        "The cutter remains unchanged in the preview",
    )
    check(
        client.call("get_transform_handles")["handles"] == [],
        "Carve previews suppress object transform handles",
    )
    if render:
        draft_pixels = client.render(output / "preview-carve.png", surface="flat")
        check(draft_pixels != baseline, "Shared renderer shows the fragmented preview")
    client.call(
        "preview_carve",
        {"cutter": number(cutter), "targets": [number(target)], "stamp": snapshot},
        error=True,
    )
    preview(cutter, [cutter], error=True)
    preview(cutter, [untouched], error=True)
    check(
        state()["preview"]["token"] == token,
        "Invalid targets, disjoint batches and stale stamps retain the valid draft",
    )
    newer = preview(cutter, [target])
    new_token = newer["preview"]["token"]
    client.call("commit_carve", {"token": token}, error=True)
    client.call("cancel_carve", {"token": token}, error=True)
    check(
        state()["preview"]["token"] == new_token,
        "Stale tokens cannot apply or discard a replacement carve",
    )
    client.call("cancel_carve", {"token": new_token})
    check(
        detail(client, ids) == original
        and client.call("get_map_info")["undo"] == history,
        "Discard preserves source geometry and history",
    )
    if render:
        check(
            client.render(output / "cancelled-carve.png", surface="flat") == baseline,
            "Discard restores identical baseline pixels",
        )
    repeated = preview(cutter, [target])
    check(
        repeated["preview"]["result"] == result,
        "Cancelled drafts do not consume object or face IDs",
    )
    client.call("commit_carve", {"token": repeated["preview"]["token"]})
    check(
        topology(fragments) == draft_shape,
        "Apply stores the exact fragment planes, IDs and topology",
    )
    check(
        len(client.call("get_map_info")["undo"]) == len(history) + 1,
        "Apply records one undo step",
    )
    check(
        detail(client, [cutter])[0] == original[2]
        and detail(client, [untouched])[0] == original[1],
        "Apply retains the cutter and every unrelated object",
    )
    applied = detail(client, result["objects"])
    source_faces = {
        (number(brush["id"]), face["side"]): face
        for brush in original
        for face in brush["faces"]
    }
    inheritance = {(entry["solid"], entry["side"]): entry for entry in result["faces"]}
    fields = ["material", "uaxis", "vaxis", "lightmap_scale", "smoothing_groups"]
    check(
        all(
            all(
                face[field]
                == source_faces[
                    (
                        inheritance[(number(brush["id"]), face["side"])][
                            "source_solid"
                        ],
                        inheritance[(number(brush["id"]), face["side"])]["source_side"],
                    )
                ][field]
                for field in fields
            )
            for brush in applied
            for face in brush["faces"]
        ),
        "Every fragment face keeps its source or cutter material, UVs and lighting",
    )
    if render:
        check(
            client.render(output / "applied-carve.png", surface="flat") == draft_pixels,
            "Applied fragments match preview pixels exactly",
        )
    client.call("undo")
    check(detail(client, ids) == original, "Undo restores exact original brushes")
    client.call("redo")
    check(
        detail(client, result["objects"]) == applied,
        "Redo restores exact carved geometry and IDs",
    )
    saved = output / "carved.vmf"
    client.call("save_map", {"path": str(saved)})
    client.call("open_map", {"path": str(saved)})
    check(topology(fragments) == draft_shape, "Fragments survive VMF save and reopen")

    client.call("new_map", {"discard_changes": True})
    target = block([0, 0, 0], [128, 128, 128])
    cutter = block([32, 32, 32], [96, 96, 96])
    draft = preview(cutter, [target])
    token = draft["preview"]["token"]
    hollow = client.call("get_hollow_preview")
    client.call(
        "preview_hollow",
        {
            "ids": [target, cutter],
            "thickness": 8,
            "outward": False,
            "stamp": hollow["stamp"],
        },
    )
    check(
        state()["preview"] is None,
        "A competing hollow preview clears carving even with identical selection",
    )
    client.call("commit_carve", {"token": token}, error=True)
    preview(cutter, [target])
    check(
        client.call("get_hollow_preview")["preview"] is None,
        "A carving preview replaces the shared hollow draft",
    )

    client.call("new_map", {"discard_changes": True})
    target = block([0, 0, 0], [64, 64, 64])
    full = block([40, 16, 16], [56, 32, 32])
    far = block([128, 128, 128], [160, 160, 160])
    tangent = block([80, 0, 0], [96, 64, 64])
    cutter = block([32, -16, -16], [80, 80, 80])
    ids = [target, full, far, tangent, cutter]
    original = detail(client, ids)
    draft = preview(cutter, ids[:-1])
    result = draft["preview"]["result"]
    check(
        result["removed"] == [number(full)]
        and result["unchanged"] == [number(far), number(tangent)],
        "Full removal, disjoint brushes and tangent brushes are reported explicitly",
    )
    client.call("commit_carve", {"token": draft["preview"]["token"]})
    client.call("get_objects", {"ids": [full]}, error=True)
    check(
        detail(client, [far, tangent, cutter]) == original[2:],
        "Partial carving leaves disjoint/tangent brushes and cutter untouched",
    )
    client.call("undo")
    check(detail(client, ids) == original, "Undo restores a fully removed world brush")
    owner = client.call("tie_to_entity", {"ids": [full], "classname": "func_detail"})[
        "id"
    ]
    original = detail(client, [owner])
    preview(cutter, [full], error=True)
    check(
        detail(client, [owner]) == original and state()["preview"] is None,
        "Removing the last brush of an entity rejects without changing it",
    )

    client.call("new_map", {"discard_changes": True})
    target = block([0, 0, 0], [128, 128, 128])
    children = [target] + [
        block([256 + i * 64, 0, 0], [288 + i * 64, 32, 32]) for i in range(24)
    ]
    owner = client.call("tie_to_entity", {"ids": children, "classname": "func_detail"})[
        "id"
    ]
    cutter = block([32, 32, 32], [96, 96, 96])
    original = detail(client, [owner])[0]
    draft = preview(cutter, [target])
    check(
        len(draft["sources"]) == 2 and "source_error" not in draft,
        "Explicit carve reports stay scoped inside a large brush entity",
    )
    client.call("commit_carve", {"token": draft["preview"]["token"]})
    changed = detail(client, [owner])[0]
    check(
        len(changed["brushes"]) == len(children) + 5
        and changed["properties"] == original["properties"],
        "Carving retains entity ownership, properties and sibling brushes",
    )
    client.call("undo")
    check(
        detail(client, [owner])[0] == original,
        "Entity carve undo restores the exact owner",
    )

    draft = preview(cutter, [target])
    token = draft["preview"]["token"]
    hollow = client.call("get_hollow_preview")
    client.call(
        "preview_hollow",
        {"ids": [owner], "thickness": 8, "outward": False, "stamp": hollow["stamp"]},
        error=True,
    )
    check(
        state()["preview"]["token"] == token,
        "Rejected competing previews preserve the valid carve",
    )
    client.call("select", {"ids": [cutter]})
    check(state()["preview"] is None, "Selection changes invalidate carving previews")
    client.call("commit_carve", {"token": token}, error=True)
    draft = preview(cutter, [target])
    token = draft["preview"]["token"]
    client.call(
        "create_entity", {"classname": "info_target", "origin": [200, 200, 200]}
    )
    client.call("commit_carve", {"token": token}, error=True)
    check(state()["preview"] is None, "Source edits invalidate carving tokens")

    samples = random.Random(777)
    for index in range(24):
        client.call("new_map", {"discard_changes": True})
        target = block([0, 0, 0], [64, 64, 64])
        mins = [samples.randrange(-192, 320) / 4 for _ in range(3)]
        maxs = [value + samples.randrange(16, 384) / 4 for value in mins]
        cutter = block(mins, maxs)
        overlap = math.prod(
            max(0, min(64, high) - max(0, low))
            for low, high in zip(mins, maxs, strict=True)
        )
        if overlap == 0:
            original = detail(client, [target, cutter])
            preview(cutter, [target], error=True)
            check(
                detail(client, [target, cutter]) == original,
                f"Fractional box sample {index}: disjoint subtraction preserves inputs",
            )
            continue
        draft = preview(cutter, [target])
        result = client.call("commit_carve", {"token": draft["preview"]["token"]})
        pieces = detail(client, result["objects"])
        volumes = [
            math.prod(
                high - low
                for low, high in zip(
                    piece["bounds"]["mins"], piece["bounds"]["maxs"], strict=True
                )
            )
            for piece in pieces
        ]
        check(
            abs(sum(volumes) - (64**3 - overlap)) < 1e-5,
            f"Fractional box sample {index}: fragments retain the exact difference volume",
        )

    client.call("new_map", {"discard_changes": True})
    outer = block([-144, -144, -16], [144, 144, 144])
    cutter = block([-128, -128, 0], [128, 128, 128])
    draft = preview(cutter, [outer])
    client.call("commit_carve", {"token": draft["preview"]["token"]})
    client.call("delete_objects", {"ids": [cutter]})
    client.call(
        "create_entity",
        {"classname": "info_player_counterterrorist", "origin": [0, 0, 64]},
    )
    saved = output / "carved-room.vmf"
    client.call("save_map", {"path": str(saved)})
    client.call("open_map", {"path": str(saved)})
    compiled = client.call("compile_map", {"path": str(output / "carved-room.bsp")})
    check(
        compiled["success"]
        and compiled["leak"] is None
        and compiled["stats"]["brushes"] == 6,
        "A carved room compiles after cutter removal and VMF save/reopen",
    )
    if render:
        client.call("set_camera", {"position": [100, -100, 90], "look_at": [0, 0, 64]})
        client.render(output / "carved-room.png", surface="flat")
        check(
            (output / "carved-room.png").is_file(),
            "Shared renderer displays the carved room interior",
        )
    return checks


def main():
    parser = argparse.ArgumentParser(
        description="Validate guarded carving through a private headless MCP session"
    )
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-carving-validation")
    )
    parser.add_argument(
        "--render",
        action="store_true",
        help="Also validate offscreen GPU render pixels",
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
