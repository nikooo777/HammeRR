import argparse
import json
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail


def validate(client, output, render=False):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def inspect(solids=None):
        return client.call("get_brush_components", {"solids": solids} if solids else {})

    def select(components):
        return client.call(
            "select_brush_components",
            {"components": components, "snapshot": inspect()["snapshot"]},
        )

    def top_corners(shape):
        height = max(vertex["position"][2] for vertex in shape["vertices"])
        return [
            {"solid": shape["solid"], "sides": vertex["sides"]}
            for vertex in shape["vertices"]
            if abs(vertex["position"][2] - height) < 1e-5
        ]

    tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
    check(
        {
            "preview_component_weld",
            "commit_component_edit",
            "cancel_component_edit",
        }
        <= tools,
        "Topology preview, apply and cancel tools are discoverable",
    )
    ids = [
        client.call(
            "create_block",
            {
                "mins": [x, 0, 0],
                "maxs": [x + 64, 64, 64],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]
        for x in [0, 128]
    ]
    solid_ids = [int(value.split(":")[1]) for value in ids]
    shapes = inspect(solid_ids)["brushes"]
    corners = top_corners(shapes[0])
    corners = select(corners)["selection"]
    if render:
        client.call(
            "set_component_move_options",
            {"enabled": False, "snapshot": inspect()["snapshot"]},
        )
        client.call(
            "set_camera", {"position": [160, -180, 160], "look_at": [32, 32, 32]}
        )
        baseline_pixels = client.render(output / "before-weld.png", surface="flat")
    original = detail(client, ids)
    history = client.call("get_map_info")["undo"]
    snapshot = inspect()["snapshot"]
    draft = client.call("preview_component_weld", {"snapshot": snapshot})
    token = draft["preview"]["token"]
    change = draft["preview"]["weld"][0]
    if render:
        preview_pixels = client.render(output / "preview-weld.png", surface="flat")
        check(
            preview_pixels != baseline_pixels,
            "Shared renderer displays the changed weld topology",
        )
    check(
        detail(client, ids) == original
        and client.call("get_map_info")["undo"] == history,
        "Weld preview preserves source geometry and undo history",
    )
    check(
        draft["preview"]["operation"] == "weld"
        and len(draft["brushes"][0]["vertices"]) == 5
        and len(draft["brushes"][0]["faces"]) == 5
        and change["position"] == [32, 32, 64],
        "Four upper corners become the pyramid apex",
    )
    check(
        change["merged_vertices"] == 4 and len(change["removed_sides"]) == 1,
        "Preview reports merged corners and the removed face ID",
    )
    check(
        len(draft["selection"]) == 1 and draft["source_selection"] == corners,
        "Draft selection uses the new incidence while source selection remains available",
    )
    check(
        client.call("get_component_handles")["handles"] == [],
        "Topology preview suppresses movement handles",
    )
    client.call("preview_component_weld", {"snapshot": snapshot}, error=True)
    client.call(
        "select_brush_components",
        {"components": [], "snapshot": inspect()["snapshot"]},
        error=True,
    )
    client.call("commit_component_move", {"token": token}, error=True)
    check(
        inspect()["preview"]["token"] == token,
        "Stale snapshots and incompatible actions preserve the valid draft",
    )
    client.call("cancel_component_edit", {"token": token})
    if render:
        check(
            client.render(output / "cancelled-weld.png", surface="flat")
            == baseline_pixels,
            "Discard restores identical source geometry and marker pixels",
        )
    check(
        inspect()["selection"] == corners
        and detail(client, ids) == original
        and client.call("get_map_info")["undo"] == history,
        "Discard restores corner selection without map or history edits",
    )
    draft = client.call("preview_component_weld", {"snapshot": inspect()["snapshot"]})
    current = draft["preview"]["token"]
    client.call("cancel_component_edit", {"token": token}, error=True)
    client.call("commit_component_edit", {"token": token}, error=True)
    check(
        inspect()["preview"]["token"] == current,
        "Old tokens cannot discard or apply a newer draft",
    )
    applied = client.call("commit_component_edit", {"token": current})
    if render:
        check(
            client.render(output / "applied-weld.png", surface="flat")
            == preview_pixels,
            "Applied geometry and selection render identically to the draft",
        )
    result = detail(client, ids)
    check(
        result[1] == original[1]
        and len(client.call("get_map_info")["undo"]) == len(history) + 1,
        "Apply changes one brush in one undo step",
    )
    check(
        applied["selection"] == draft["selection"]
        and applied["brushes"][0]["vertices"] == draft["brushes"][0]["vertices"],
        "Applied geometry and merged-corner selection match the draft",
    )
    source_faces = {face["side"]: face for face in original[0]["faces"]}
    fields = ["material", "uaxis", "vaxis", "lightmap_scale", "smoothing_groups"]
    check(
        all(
            all(face[field] == source_faces[face["side"]][field] for field in fields)
            for face in result[0]["faces"]
        ),
        "Surviving faces retain IDs, materials, world UVs and lighting",
    )
    client.call("undo")
    check(detail(client, ids) == original, "Undo restores exact source geometry")
    client.call("redo")
    check(detail(client, ids) == result, "Redo restores exact welded geometry")
    saved = output / "weld.vmf"
    client.call("save_map", {"path": str(saved)})
    client.call("open_map", {"path": str(saved)})
    check(
        len(inspect([solid_ids[0]])["brushes"][0]["vertices"]) == 5,
        "Welded topology survives VMF save and reopen",
    )

    shapes = inspect([solid_ids[1]])["brushes"]
    select(top_corners(shapes[0])[:2])
    state = detail(client, ids)
    rejected = client.call(
        "preview_component_weld", {"snapshot": inspect()["snapshot"]}, error=True
    )
    check(
        "nonplanar" in json.dumps(rejected) and detail(client, ids) == state,
        "A partial box-ring weld rejects nonplanar faces without mutation",
    )
    select(top_corners(shapes[0]))
    stale = client.call("preview_component_weld", {"snapshot": inspect()["snapshot"]})[
        "preview"
    ]["token"]
    client.call("create_entity", {"classname": "info_target", "origin": [0, 0, 128]})
    client.call("commit_component_edit", {"token": stale}, error=True)
    check(inspect()["preview"] is None, "Source edits invalidate the weld token")

    client.call("new_map", {"discard_changes": True})
    children = [
        client.call(
            "create_block", {"mins": [x * 128, 0, 0], "maxs": [x * 128 + 64, 64, 64]}
        )["id"]
        for x in range(17)
    ]
    owner = client.call("tie_to_entity", {"ids": children, "classname": "func_detail"})[
        "id"
    ]
    solid_id = int(children[0].split(":")[1])
    shape = inspect([solid_id])["brushes"][0]
    selected = client.call(
        "select_brush_components",
        {"components": top_corners(shape), "snapshot": inspect([solid_id])["snapshot"]},
    )
    before_owner = detail(client, [owner])
    draft = client.call("preview_component_weld", {"snapshot": selected["snapshot"]})
    check(
        len(draft["brushes"]) == 1,
        "Weld report stays scoped to one edited child in a large entity",
    )
    client.call("commit_component_edit", {"token": draft["preview"]["token"]})
    after_owner = detail(client, [owner])
    check(
        len(after_owner[0]["brushes"]) == 17,
        "Welding retains every sibling brush in a large entity",
    )
    client.call("undo")
    check(
        detail(client, [owner]) == before_owner,
        "Large entity weld undo restores the exact owner",
    )

    client.call("new_map", {"discard_changes": True})
    for mins, maxs in [
        ([-272, -272, -16], [272, 272, 0]),
        ([-272, -272, 256], [272, 272, 272]),
        ([-272, -272, 0], [-256, 272, 256]),
        ([256, -272, 0], [272, 272, 256]),
        ([-256, -272, 0], [256, -256, 256]),
        ([-256, 256, 0], [256, 272, 256]),
        ([0, 0, 0], [64, 64, 64]),
    ]:
        block = client.call(
            "create_block",
            {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"},
        )["id"]
    client.call(
        "create_entity", {"classname": "info_player_terrorist", "origin": [-64, 0, 8]}
    )
    solid_id = int(block.split(":")[1])
    select(top_corners(inspect([solid_id])["brushes"][0]))
    token = client.call("preview_component_weld", {"snapshot": inspect()["snapshot"]})[
        "preview"
    ]["token"]
    client.call("commit_component_edit", {"token": token})
    compiled = client.call("compile_map", {"path": str(output / "weld-room.bsp")})
    check(compiled["success"], "Native compiler accepts the welded sealed-room fixture")
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--render",
        action="store_true",
        help="Also check offscreen GPU render pixels; no desktop window",
    )
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-topology-validation")
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
