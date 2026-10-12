import argparse
import json
import math
import re
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail
from validate_texture_chain import uv
from validate_texture_lock_modes import local, world


def validate(client, output, render=False):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def block(mins=None, maxs=None):
        return client.call(
            "create_block",
            {
                "mins": mins or [0, 0, 16],
                "maxs": maxs or [64, 32, 80],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]

    def fixture():
        client.call("new_map", {"discard_changes": True})
        client.call("set_transform_tool", {"mode": "select"})
        object_id = block()
        client.call(
            "transform_objects",
            {"ids": [object_id], "rotation": [17, -23, 31], "pivot": [32, 16, 48]},
        )
        return object_id

    schema = next(
        tool["inputSchema"]
        for tool in client.rpc("tools/list", {})["tools"]
        if tool["name"] == "transform_objects"
    )
    check(
        "shear" in schema["properties"],
        "Shear is discoverable in the shared transform schema",
    )
    pivot = [12.125, -7.25, 3.5]
    for space in ["world", "local"]:
        for axis in range(3):
            for reference in range(3):
                if axis == reference:
                    continue
                for factor in [-1.25, 0.75]:
                    for lock in [False, True]:
                        label = f"{space} axis={axis} ref={reference} factor={factor} lock={lock}"
                        object_id = fixture()
                        before = detail(client, [object_id])[0]
                        history = client.call("get_map_info")["undo"]
                        shear = {
                            "axis": "xyz"[axis],
                            "reference": "xyz"[reference],
                            "factor": factor,
                        }
                        args = {
                            "ids": [object_id],
                            "shear": shear,
                            "space": space,
                            "pivot": pivot,
                            "texture_scale_lock": lock,
                            "preview": True,
                        }
                        draft = client.call("transform_objects", args)["state"][
                            "preview"
                        ]
                        changed = draft["objects"][0]
                        basis = draft["basis"]
                        check(
                            draft["shear"] == shear,
                            f"{label}: draft reports exact inputs",
                        )
                        check(
                            detail(client, [object_id])[0] == before
                            and client.call("get_map_info")["undo"] == history,
                            f"{label}: preview leaves source and history intact",
                        )
                        error = 0.0
                        texture_error = 0.0
                        for old, new in zip(
                            before["faces"], changed["faces"], strict=True
                        ):
                            for point, actual in zip(
                                old["plane"], new["plane"], strict=True
                            ):
                                delta = local(
                                    [point[a] - pivot[a] for a in range(3)], basis
                                )
                                delta[axis] += factor * delta[reference]
                                moved = [
                                    value + pivot[a]
                                    for a, value in enumerate(world(delta, basis))
                                ]
                                error = max(error, math.dist(moved, actual))
                                if lock:
                                    texture_error = max(
                                        texture_error,
                                        math.dist(uv(old, point), uv(new, moved)),
                                    )
                            check(
                                all(
                                    old[key] == new[key]
                                    for key in [
                                        "side",
                                        "material",
                                        "lightmap_scale",
                                        "smoothing_groups",
                                    ]
                                ),
                                f"{label}: side {old['side']} preserves identity and metadata",
                            )
                            if not lock:
                                check(
                                    all(
                                        old[key] == new[key]
                                        for key in ["uaxis", "vaxis"]
                                    ),
                                    f"{label}: side {old['side']} keeps world projection",
                                )
                        check(
                            error < 1e-8 and texture_error < 1e-8,
                            f"{label}: exact geometry and attached corner UVs",
                        )
                        for invalid in [
                            {"shear": {**shear, "reference": shear["axis"]}},
                            {"shear": {**shear, "factor": 65}},
                            {"translation": [1000000, 1000000, 1000000]},
                        ]:
                            client.call(
                                "transform_objects", {**args, **invalid}, error=True
                            )
                            check(
                                client.call("get_transform_handles")["state"]["preview"]
                                == draft,
                                f"{label}: invalid {invalid} retains the valid draft",
                            )
                        client.call("cancel_transform_preview")
                        check(
                            detail(client, [object_id])[0] == before,
                            f"{label}: cancel restores source",
                        )
                        client.call("transform_objects", args)
                        client.call("commit_transform_preview")
                        after = detail(client, [object_id])[0]
                        check(
                            after == changed
                            and len(client.call("get_map_info")["undo"])
                            == len(history) + 1,
                            f"{label}: Apply matches the draft in one undo step",
                        )
                        client.call("undo")
                        check(
                            detail(client, [object_id])[0] == before,
                            f"{label}: exact undo",
                        )
                        client.call("redo")
                        check(
                            detail(client, [object_id])[0] == after,
                            f"{label}: exact redo",
                        )
                        path = (
                            output
                            / f"{space}-{axis}-{reference}-{factor}-{int(lock)}.vmf"
                        )
                        client.call("save_map", {"path": str(path)})
                        client.call("open_map", {"path": str(path)})
                        saved = detail(client, [object_id])[0]
                        check(
                            all(
                                old[key] == new[key]
                                for old, new in zip(
                                    after["faces"], saved["faces"], strict=True
                                )
                                for key in ["uaxis", "vaxis", "side"]
                            ),
                            f"{label}: exact saved projections and IDs",
                        )

    object_id = fixture()
    point = client.call(
        "create_entity", {"classname": "info_target", "origin": [8, 16, 4]}
    )["id"]
    before = detail(client, [object_id, point])
    history = client.call("get_map_info")["undo"]
    args = {
        "ids": [object_id, point],
        "shear": {"axis": "x", "reference": "y", "factor": 0.5},
    }
    client.call("transform_objects", args, error=True)
    check(
        detail(client, [object_id, point]) == before
        and client.call("get_map_info")["undo"] == history,
        "Mixed brush/point failure rolls back earlier geometry and history",
    )
    client.call("transform_objects", {**args, "shear": {**args["shear"], "factor": 0}})
    check(
        detail(client, [object_id, point]) == before
        and client.call("get_map_info")["undo"] == history,
        "Zero shear is an exact identity",
    )

    client.call("new_map", {"discard_changes": True})
    brush = block()
    terrain = block([128, 0, 16], [192, 32, 80])
    path = output / "terrain-source.vmf"
    client.call("save_map", {"path": str(path)})
    text = path.read_text()
    terrain_id = terrain.split(":")[1]
    start = re.search(
        r'\bsolid\s*\{\s*"id"\s*"' + re.escape(terrain_id) + '"', text
    ).end()
    side = text.index("side", start)
    brace = text.index("{", side) + 1
    text = (
        text[:brace]
        + '\n dispinfo { "power" "2" "startposition" "[128 0 80]" }\n'
        + text[brace:]
    )
    path.write_text(text)
    client.call("open_map", {"path": str(path)})
    before = detail(client, [brush, terrain])
    check(
        not any(face["displacement"] for face in before[0]["faces"])
        and any(face["displacement"] == 2 for face in before[1]["faces"]),
        "Terrain fixture marks the later brush, preserving the earlier valid brush",
    )
    history = client.call("get_map_info")["undo"]
    client.call(
        "transform_objects",
        {
            "ids": [brush, terrain],
            "shear": {"axis": "x", "reference": "y", "factor": 0.5},
        },
        error=True,
    )
    check(
        detail(client, [brush, terrain]) == before
        and client.call("get_map_info")["undo"] == history,
        "Unsupported terrain rolls back earlier valid brushes and history",
    )

    client.call("new_map", {"discard_changes": True})
    brushes = [block(), block([128, 0, 16], [192, 32, 80])]
    owner = client.call("tie_to_entity", {"ids": brushes, "classname": "func_detail"})[
        "id"
    ]
    client.call("set_entity_properties", {"id": owner, "set": {"origin": "8 16 4"}})
    before = detail(client, [owner])[0]
    client.call(
        "transform_objects",
        {
            "ids": [owner],
            "pivot": [0, 0, 0],
            "shear": {"axis": "x", "reference": "y", "factor": 0.5},
        },
    )
    after = detail(client, [owner])[0]
    check(
        after["origin"] == [16, 16, 4]
        and [b["solid"] for b in after["brushes"]]
        == [b["solid"] for b in before["brushes"]],
        "Brush entity shear transforms origin while retaining owned solid IDs",
    )
    client.call("undo")
    check(
        detail(client, [owner])[0] == before,
        "Brush entity undo restores complete source",
    )

    for factor in [-1.25, 0.75]:
        client.call("new_map", {"discard_changes": True})
        for mins, maxs in [
            ([-272, -272, -16], [272, 272, 0]),
            ([-272, -272, 256], [272, 272, 272]),
            ([-272, -272, 0], [-256, 272, 256]),
            ([256, -272, 0], [272, 272, 256]),
            ([-256, -272, 0], [256, -256, 256]),
            ([-256, 256, 0], [256, 272, 256]),
        ]:
            block(mins, maxs)
        brush = block()
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [-64, 0, 8]},
        )
        client.call(
            "transform_objects",
            {
                "ids": [brush],
                "shear": {"axis": "x", "reference": "y", "factor": factor},
                "space": "local",
            },
        )
        path = output / f"room-{factor}.vmf"
        client.call("save_map", {"path": str(path)})
        client.call("open_map", {"path": str(path)})
        compiled = client.call(
            "compile_map", {"path": str(output / f"room-{factor}.bsp")}
        )
        check(
            compiled["success"]
            and compiled["leak"] is None
            and compiled["stats"]["brushes"] == 7,
            f"Shear {factor}: native sealed-room compilation succeeds after save/reopen",
        )

    if render:
        for lock in [False, True]:
            object_id = fixture()
            client.call(
                "set_camera", {"position": [180, -200, 130], "look_at": [32, 16, 48]}
            )
            baseline = client.render(output / f"shear-{int(lock)}-before.png")
            args = {
                "ids": [object_id],
                "shear": {"axis": "x", "reference": "z", "factor": 0.75},
                "texture_scale_lock": lock,
                "preview": True,
            }
            client.call("transform_objects", args)
            preview = client.render(output / f"shear-{int(lock)}-preview.png")
            check(
                preview != baseline, f"Render lock={lock}: shared renderer shows shear"
            )
            client.call("cancel_transform_preview")
            check(
                client.render(output / f"shear-{int(lock)}-cancelled.png") == baseline,
                f"Render lock={lock}: cancel restores exact pixels",
            )
            client.call("transform_objects", args)
            client.call("commit_transform_preview")
            check(
                client.render(output / f"shear-{int(lock)}-applied.png") == preview,
                f"Render lock={lock}: applied shear matches draft pixels",
            )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-shear-validation")
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
