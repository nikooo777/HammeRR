import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail


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
            "preview_component_split",
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
    validate_weld_targets(client, output, check, render)
    validate_splitting(client, output, check, render)
    return checks


def validate_weld_targets(client, output, check, render):
    for mode in ["position", "corner"]:
        client.call("new_map", {"discard_changes": True})
        target = client.call(
            "create_block",
            {
                "mins": [0, 0, 0],
                "maxs": [64, 64, 64],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]
        solid = int(target.split(":")[1])

        def inspect(solid_id=solid):
            return client.call("get_brush_components", {"solids": [solid_id]})

        shape = inspect()["brushes"][0]
        corners = [
            {"solid": solid, "sides": vertex["sides"]}
            for vertex in shape["vertices"]
            if vertex["position"][2] == 64
        ]
        client.call(
            "select_brush_components",
            {"components": corners, "snapshot": inspect()["snapshot"]},
        )
        source_choices = inspect()["weld_targets"]
        chosen = next(
            choice for choice in source_choices if choice["component"] == corners[0]
        )
        point = [16.125, 24.25, 96.5] if mode == "position" else chosen["position"]
        args = (
            {"target": point}
            if mode == "position"
            else {
                "target_corner": {
                    **chosen["component"],
                    "sides": list(reversed(chosen["component"]["sides"])),
                }
            }
        )
        original = detail(client, [target])
        history = client.call("get_map_info")["undo"]
        if render:
            client.call(
                "set_component_move_options",
                {"enabled": False, "snapshot": inspect()["snapshot"]},
            )
            client.call(
                "set_camera", {"position": [160, -180, 160], "look_at": [32, 32, 40]}
            )
            baseline = client.render(output / f"before-weld-{mode}.png", surface="flat")
        snapshot = inspect()["snapshot"]
        draft = client.call("preview_component_weld", {**args, "snapshot": snapshot})
        token = draft["preview"]["token"]
        check(
            draft["preview"]["weld_target"]["mode"] == mode
            and draft["preview"]["weld"][0]["position"] == point,
            f"{mode} weld: mode and exact target are shared",
        )
        check(
            draft["weld_targets"] == source_choices
            and len(draft["brushes"][0]["vertices"]) == 5,
            f"{mode} weld: original corner choices remain available in the changed draft",
        )
        check(
            detail(client, [target]) == original
            and client.call("get_map_info")["undo"] == history,
            f"{mode} weld: preview preserves map and history",
        )
        if render:
            pixels = client.render(output / f"preview-weld-{mode}.png", surface="flat")
            check(pixels != baseline, f"{mode} weld: renderer shows the chosen target")
        for invalid in [
            {"target": [32, 32, 65537]},
            {"target": [32, 32, 0]},
            {"target": point, "target_corner": chosen["component"]},
            {"target_corner": {"solid": solid, "sides": [shape["faces"][0]["side"]]}},
        ]:
            client.call(
                "preview_component_weld",
                {**invalid, "snapshot": inspect()["snapshot"]},
                error=True,
            )
        client.call(
            "preview_component_weld", {**args, "snapshot": snapshot}, error=True
        )
        check(
            inspect()["preview"]["token"] == token,
            f"{mode} weld: invalid targets, dual inputs and stale snapshots retain the draft",
        )
        client.call("cancel_component_edit", {"token": token})
        check(
            detail(client, [target]) == original
            and inspect()["selection"]
            == sorted(corners, key=lambda component: component["sides"]),
            f"{mode} weld: discard restores geometry and corner selection",
        )
        if render:
            check(
                client.render(output / f"cancelled-weld-{mode}.png", surface="flat")
                == baseline,
                f"{mode} weld: discard restores identical pixels",
            )
        draft = client.call(
            "preview_component_weld", {**args, "snapshot": inspect()["snapshot"]}
        )
        client.call("commit_component_edit", {"token": token}, error=True)
        client.call("commit_component_edit", {"token": draft["preview"]["token"]})
        applied = detail(client, [target])
        check(
            len(applied[0]["faces"]) == 5
            and len(client.call("get_map_info")["undo"]) == len(history) + 1,
            f"{mode} weld: exact target applies in one undo step",
        )
        if render:
            check(
                client.render(output / f"applied-weld-{mode}.png", surface="flat")
                == pixels,
                f"{mode} weld: applied target matches preview pixels",
            )
        client.call("undo")
        check(
            detail(client, [target]) == original,
            f"{mode} weld: undo restores the exact source",
        )
        client.call("redo")
        check(
            detail(client, [target]) == applied,
            f"{mode} weld: redo restores the exact target",
        )
        saved = output / f"weld-{mode}.vmf"
        client.call("save_map", {"path": str(saved)})
        client.call("open_map", {"path": str(saved)})
        vertices = inspect()["brushes"][0]["vertices"]
        check(
            any(math.dist(vertex["position"], point) < 1e-5 for vertex in vertices),
            f"{mode} weld: target survives VMF save/reopen",
        )


def validate_splitting(client, output, check, render):
    def inspect(solids=None):
        return client.call("get_brush_components", {"solids": solids} if solids else {})

    def select(components):
        return client.call(
            "select_brush_components",
            {"components": components, "snapshot": inspect()["snapshot"]},
        )

    def preview(rise=16):
        return client.call(
            "preview_component_split", {"rise": rise, "snapshot": inspect()["snapshot"]}
        )

    for kind in ["face", "edge"]:
        client.call("new_map", {"discard_changes": True})
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
        solid = int(ids[0].split(":")[1])
        shape = inspect([solid])["brushes"][0]
        original = detail(client, ids)
        if kind == "face":
            face = next(
                face
                for face in shape["faces"]
                if all(
                    shape["vertices"][index]["position"][2] == 64
                    for index in face["vertices"]
                )
            )
            component = {"solid": solid, "sides": [face["side"]]}
            expected_position = [32, 32, 80]
        else:
            edge = next(
                edge
                for edge in shape["edges"]
                if all(point[2] == 64 for point in edge["positions"])
            )
            component = {"solid": solid, "sides": edge["sides"]}
            normals = [
                face["normal"]
                for face in original[0]["faces"]
                if face["side"] in edge["sides"]
            ]
            normal = [sum(values) for values in zip(*normals, strict=True)]
            length = math.dist(normal, [0, 0, 0])
            expected_position = [
                sum(values) * 0.5 + normal[axis] * 16 / length
                for axis, values in enumerate(zip(*edge["positions"], strict=True))
            ]
        picking = client.call(
            "set_component_picking",
            {
                "enabled": True,
                "kind": kind,
                "solids": [solid],
                "snapshot": inspect()["snapshot"],
            },
        )
        selected = select([component])
        if render:
            client.call(
                "set_component_move_options",
                {"enabled": False, "snapshot": selected["snapshot"]},
            )
            client.call(
                "set_camera", {"position": [170, -220, 170], "look_at": [70, 32, 32]}
            )
            baseline_pixels = client.render(
                output / f"before-split-{kind}.png", surface="flat"
            )
        history = client.call("get_map_info")["undo"]
        snapshot = inspect()["snapshot"]
        draft = preview()
        token = draft["preview"]["token"]
        change = draft["preview"]["split"]["changes"][0]
        check(
            math.dist(change["position"], expected_position) < 1e-8,
            f"{kind}: split uses the outward face normal or edge bisector",
        )
        check(
            len(draft["brushes"][0]["vertices"]) == 9
            and len(draft["brushes"][0]["faces"]) == (9 if kind == "face" else 10),
            f"{kind}: split creates the expected closed topology",
        )
        check(
            all(
                any(
                    math.dist(vertex["position"], old["position"]) < 1e-8
                    for vertex in draft["brushes"][0]["vertices"]
                )
                for old in shape["vertices"]
            ),
            f"{kind}: every original corner stays in place",
        )
        check(
            draft["source_selection"] == [component]
            and len(draft["selection"]) == 1
            and len(change["new_sides"]) == (3 if kind == "face" else 4),
            f"{kind}: draft reports source selection, new IDs and inserted corner",
        )
        check(
            detail(client, ids) == original
            and client.call("get_map_info")["undo"] == history,
            f"{kind}: preview preserves source geometry and history",
        )
        if render:
            preview_pixels = client.render(
                output / f"preview-split-{kind}.png", surface="flat"
            )
            check(
                preview_pixels != baseline_pixels,
                f"{kind}: renderer shows the proposed split",
            )
        for rise in [0, -16, 0.01, 4097]:
            client.call(
                "preview_component_split",
                {"rise": rise, "snapshot": inspect()["snapshot"]},
                error=True,
            )
        client.call(
            "preview_component_split", {"rise": 32, "snapshot": snapshot}, error=True
        )
        check(
            inspect()["preview"]["token"] == token,
            f"{kind}: invalid rises and stale snapshots preserve the valid draft",
        )
        replacement = preview(32)
        new_token = replacement["preview"]["token"]
        client.call("commit_component_edit", {"token": token}, error=True)
        client.call("cancel_component_edit", {"token": token}, error=True)
        check(
            inspect()["preview"]["token"] == new_token,
            f"{kind}: stale tokens cannot apply or discard a newer split",
        )
        discarded = client.call("cancel_component_edit", {"token": new_token})
        check(
            discarded["selection"] == [component]
            and discarded["picking"]["kind"] == kind
            and detail(client, ids) == original,
            f"{kind}: discard restores source selection and picking kind",
        )
        if render:
            check(
                client.render(output / f"cancelled-split-{kind}.png", surface="flat")
                == baseline_pixels,
                f"{kind}: discard restores identical pixels",
            )
        draft = preview()
        check(
            draft["preview"]["split"]["changes"][0]["new_sides"] == change["new_sides"],
            f"{kind}: cancelled drafts do not consume face IDs",
        )
        applied = client.call(
            "commit_component_edit", {"token": draft["preview"]["token"]}
        )
        result = detail(client, ids)
        check(
            applied["selection"] == draft["selection"]
            and applied["picking"]["kind"] == "vertex"
            and applied["picking"]["solids"] == picking["picking"]["solids"],
            f"{kind}: apply selects the inserted corner and retains brush scope",
        )
        check(
            result[1] == original[1]
            and len(client.call("get_map_info")["undo"]) == len(history) + 1,
            f"{kind}: apply changes only the target in one undo step",
        )
        if render:
            check(
                client.render(output / f"applied-split-{kind}.png", surface="flat")
                == preview_pixels,
                f"{kind}: applied geometry and guides match draft pixels",
            )
        parents = {entry["side"]: entry["source"] for entry in change["faces"]}
        source_faces = {face["side"]: face for face in original[0]["faces"]}
        fields = ["material", "uaxis", "vaxis", "lightmap_scale", "smoothing_groups"]
        check(
            all(
                all(
                    face[field]
                    == source_faces[parents.get(face["side"], face["side"])][field]
                    for field in fields
                )
                for face in result[0]["faces"]
            ),
            f"{kind}: source and new faces preserve inherited materials, UVs and lighting",
        )
        client.call("undo")
        check(
            detail(client, ids) == original,
            f"{kind}: undo restores exact source planes",
        )
        client.call("redo")
        check(
            detail(client, ids) == result,
            f"{kind}: redo restores exact generated planes and IDs",
        )
        saved = output / f"split-{kind}.vmf"
        client.call("save_map", {"path": str(saved)})
        client.call("open_map", {"path": str(saved)})
        check(
            len(inspect([solid])["brushes"][0]["vertices"]) == 9,
            f"{kind}: new topology survives VMF save/reopen",
        )
        other = int(ids[1].split(":")[1])
        other_shape = inspect([other])["brushes"][0]
        select([{"solid": other, "sides": [other_shape["faces"][0]["side"]]}])
        stale = preview()["preview"]["token"]
        client.call(
            "create_entity", {"classname": "info_target", "origin": [0, 0, 128]}
        )
        client.call("commit_component_edit", {"token": stale}, error=True)
        check(
            inspect()["preview"] is None,
            f"{kind}: source edits invalidate split tokens",
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
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [-64, 0, 8]},
        )
        solid = int(block.split(":")[1])
        shape = inspect([solid])["brushes"][0]
        component = {
            "solid": solid,
            "sides": [shape["faces"][0]["side"]]
            if kind == "face"
            else shape["edges"][0]["sides"],
        }
        select([component])
        draft = preview()
        client.call("commit_component_edit", {"token": draft["preview"]["token"]})
        compiled = client.call(
            "compile_map", {"path": str(output / f"split-{kind}-room.bsp")}
        )
        check(
            compiled["success"],
            f"{kind}: native compiler accepts the split sealed-room fixture",
        )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
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
