import argparse
import json
import math
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
                "mins": mins or [0, 0, 0],
                "maxs": maxs or [64, 32, 16],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]

    def fixture():
        client.call("new_map", {"discard_changes": True})
        client.call("set_transform_tool", {"mode": "select"})
        object_id = block()
        client.call(
            "transform_objects",
            {
                "ids": [object_id],
                "rotation": [17, -23, 31],
                "pivot": [32, 16, 8],
            },
        )
        return object_id

    schema = next(
        tool["inputSchema"]
        for tool in client.rpc("tools/list", {})["tools"]
        if tool["name"] == "transform_objects"
    )
    check(
        "reflection" in schema["properties"],
        "Reflection axes are discoverable in the shared transform schema",
    )
    pivot = [12.125, -7.25, 3.5]
    for space in ["world", "local"]:
        for mask in range(1, 8):
            for lock in [False, True]:
                label = f"{space} axes={mask} textures={lock}"
                object_id = fixture()
                before = detail(client, [object_id])[0]
                history = client.call("get_map_info")["undo"]
                axes = [bool(mask & (1 << axis)) for axis in range(3)]
                args = {
                    "ids": [object_id],
                    "reflection": axes,
                    "pivot": pivot,
                    "space": space,
                    "texture_scale_lock": lock,
                    "preview": True,
                }
                draft = client.call("transform_objects", args)["state"]["preview"]
                changed = draft["objects"][0]
                basis = draft["basis"]
                check(
                    draft["reflection"] == axes,
                    f"{label}: the shared draft reports its axes",
                )
                check(
                    detail(client, [object_id])[0] == before
                    and client.call("get_map_info")["undo"] == history,
                    f"{label}: preview preserves committed geometry and history",
                )
                geometry_error = 0.0
                uv_error = 0.0
                for old, new in zip(before["faces"], changed["faces"], strict=True):
                    for point in old["plane"]:
                        delta = local([point[a] - pivot[a] for a in range(3)], basis)
                        delta = [
                            value * (-1 if axes[a] else 1)
                            for a, value in enumerate(delta)
                        ]
                        moved = [
                            value + pivot[a]
                            for a, value in enumerate(world(delta, basis))
                        ]
                        geometry_error = max(
                            geometry_error,
                            min(math.dist(moved, value) for value in new["plane"]),
                        )
                        if lock:
                            uv_error = max(
                                uv_error, math.dist(uv(old, point), uv(new, moved))
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
                        f"{label}: side {old['side']} preserves identity and material metadata",
                    )
                    if not lock:
                        check(
                            all(old[key] == new[key] for key in ["uaxis", "vaxis"]),
                            f"{label}: side {old['side']} keeps its world projection",
                        )
                check(
                    geometry_error < 1e-8 and uv_error < 1e-8,
                    f"{label}: source planes and attached UVs reflect exactly",
                )
                client.call(
                    "transform_objects",
                    {**args, "translation": [1000000, 1000000, 1000000]},
                    error=True,
                )
                check(
                    client.call("get_transform_handles")["state"]["preview"] == draft,
                    f"{label}: invalid bounds retain the valid draft",
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
                    and len(client.call("get_map_info")["undo"]) == len(history) + 1,
                    f"{label}: Apply matches the draft in one undo step",
                )
                client.call("undo")
                check(
                    detail(client, [object_id])[0] == before,
                    f"{label}: undo restores exact source",
                )
                client.call("redo")
                check(
                    detail(client, [object_id])[0] == after,
                    f"{label}: redo restores exact reflected geometry",
                )
                saved = output / f"{space}-{mask}-{int(lock)}.vmf"
                client.call("save_map", {"path": str(saved)})
                client.call("open_map", {"path": str(saved)})
                reopened = detail(client, [object_id])[0]
                check(
                    all(
                        old[key] == new[key]
                        for old, new in zip(
                            after["faces"], reopened["faces"], strict=True
                        )
                        for key in ["uaxis", "vaxis", "side"]
                    ),
                    f"{label}: exact projections and IDs survive save/reopen",
                )

    object_id = fixture()
    point = client.call(
        "create_entity", {"classname": "info_target", "origin": [8, 16, 4]}
    )["id"]
    before = detail(client, [object_id, point])
    history = client.call("get_map_info")["undo"]
    for axes in [[True, False, False], [True, True, False]]:
        client.call(
            "transform_objects",
            {"ids": [object_id, point], "reflection": axes},
            error=True,
        )
        check(
            detail(client, [object_id, point]) == before
            and client.call("get_map_info")["undo"] == history,
            f"Mixed point/brush axes={axes}: failure rolls back geometry and history",
        )
    client.call(
        "transform_objects", {"ids": [object_id], "scale": [-1, 1, 1]}, error=True
    )
    check(
        detail(client, [object_id])[0] == before[0],
        "Numeric scale remains positive; reflection uses separate axes",
    )

    for axes in [[True, False, False], [True, True, False], [True, True, True]]:
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
        brush = block([0, 0, 16], [64, 32, 80])
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [-64, 0, 8]},
        )
        client.call(
            "transform_objects",
            {"ids": [brush], "rotation": [17, -23, 31], "pivot": [32, 16, 48]},
        )
        client.call(
            "transform_objects",
            {
                "ids": [brush],
                "reflection": axes,
                "space": "local",
                "pivot": [32.125, 16.25, 64.5],
            },
        )
        saved = output / f"room-{sum(axes)}.vmf"
        client.call("save_map", {"path": str(saved)})
        client.call("open_map", {"path": str(saved)})
        compiled = client.call(
            "compile_map", {"path": str(output / f"room-{sum(axes)}.bsp")}
        )
        check(
            compiled["success"]
            and compiled["leak"] is None
            and compiled["stats"]["brushes"] == 7,
            f"{sum(axes)} reflected axes: native sealed-room compilation succeeds after save/reopen",
        )

    client.call("new_map", {"discard_changes": True})
    brushes = [block(), block([128, 0, 0], [192, 32, 16])]
    owner = client.call("tie_to_entity", {"ids": brushes, "classname": "func_detail"})[
        "id"
    ]
    before = detail(client, [owner])[0]
    client.call(
        "transform_objects",
        {"ids": [owner], "reflection": [True, False, False], "pivot": [0, 0, 0]},
    )
    after = detail(client, [owner])[0]
    check(
        [b["solid"] for b in after["brushes"]]
        == [b["solid"] for b in before["brushes"]],
        "Brush entity reflection preserves both owned solid IDs",
    )
    client.call("undo")
    check(
        detail(client, [owner])[0] == before,
        "Brush entity undo restores complete ownership and metadata",
    )

    if render:
        for lock in [False, True]:
            object_id = fixture()
            client.call(
                "set_camera", {"position": [180, -200, 130], "look_at": [0, 0, 16]}
            )
            baseline = client.render(output / f"mirror-{int(lock)}-before.png")
            args = {
                "ids": [object_id],
                "reflection": [True, False, False],
                "pivot": [0, 0, 0],
                "texture_scale_lock": lock,
                "preview": True,
            }
            client.call("transform_objects", args)
            preview = client.render(output / f"mirror-{int(lock)}-preview.png")
            check(
                preview != baseline,
                f"Render textures={lock}: reflected geometry is visible",
            )
            client.call("cancel_transform_preview")
            check(
                client.render(output / f"mirror-{int(lock)}-cancelled.png") == baseline,
                f"Render textures={lock}: cancellation restores exact pixels",
            )
            client.call("transform_objects", args)
            client.call("commit_transform_preview")
            check(
                client.render(output / f"mirror-{int(lock)}-applied.png") == preview,
                f"Render textures={lock}: applied reflection matches draft pixels",
            )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-reflection-validation")
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
