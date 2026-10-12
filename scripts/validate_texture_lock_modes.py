import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail
from validate_texture_chain import uv


def rotate(point, angles, inverse=False):
    result = list(point)
    order = list(enumerate(angles))
    if inverse:
        order.reverse()
    for axis, angle in order:
        angle = math.radians(-angle if inverse else angle)
        a, b = [(1, 2), (2, 0), (0, 1)][axis]
        x, y = result[a], result[b]
        result[a] = x * math.cos(angle) - y * math.sin(angle)
        result[b] = x * math.sin(angle) + y * math.cos(angle)
    return result


def local(point, basis):
    return [sum(a * b for a, b in zip(point, axis, strict=True)) for axis in basis]


def world(point, basis):
    return [sum(point[a] * basis[a][b] for a in range(3)) for b in range(3)]


def validate(client, output, render=False):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def fixture():
        client.call("new_map", {"discard_changes": True})
        client.call("set_transform_tool", {"mode": "select"})
        return client.call(
            "create_block",
            {
                "mins": [0, 0, 0],
                "maxs": [64, 64, 32],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]

    schemas = {
        tool["name"]: tool["inputSchema"]
        for tool in client.rpc("tools/list", {})["tools"]
    }
    check(
        all(
            "texture_scale_lock" in schemas[name]["properties"]
            for name in ["transform_objects", "set_transform_tool"]
        ),
        "Numeric and handle texture scale-lock settings are discoverable",
    )
    translation = [12.125, -24.25, 8.5]
    angles = [13, -27, 39]
    scale = [2, 0.5, 1.25]
    pivot = [-19.125, 42.25, 5.5]
    for space in ["world", "local"]:
        for movement in [False, True]:
            for stretch in [False, True]:
                label = f"{space} move={movement} stretch={stretch}"
                object_id = fixture()
                before = detail(client, [object_id])[0]
                history = client.call("get_map_info")["undo"]
                args = {
                    "ids": [object_id],
                    "translation": translation,
                    "rotation": angles,
                    "scale": scale,
                    "pivot": pivot,
                    "space": space,
                    "texture_lock": movement,
                    "texture_scale_lock": stretch,
                    "preview": True,
                }
                draft = client.call("transform_objects", args)["state"]["preview"]
                basis = draft["basis"]
                changed = draft["objects"][0]
                world_translation = world(translation, basis)
                geometry_error = 0.0
                uv_error = 0.0
                for old, new in zip(before["faces"], changed["faces"], strict=True):
                    for old_point, new_point in zip(
                        old["plane"], new["plane"], strict=True
                    ):
                        delta = [old_point[a] - pivot[a] for a in range(3)]
                        delta = [
                            value * scale[a]
                            for a, value in enumerate(local(delta, basis))
                        ]
                        moved = world(rotate(delta, angles), basis)
                        moved = [
                            moved[a] + pivot[a] + world_translation[a] for a in range(3)
                        ]
                        geometry_error = max(
                            geometry_error, math.dist(moved, new_point)
                        )
                        sample = [
                            new_point[a]
                            - pivot[a]
                            - (world_translation[a] if movement else 0)
                            for a in range(3)
                        ]
                        sample = local(sample, basis)
                        if movement:
                            sample = rotate(sample, angles, inverse=True)
                        if stretch:
                            sample = [
                                value / scale[a] for a, value in enumerate(sample)
                            ]
                        sample = [
                            value + pivot[a]
                            for a, value in enumerate(world(sample, basis))
                        ]
                        uv_error = max(
                            uv_error, math.dist(uv(new, new_point), uv(old, sample))
                        )
                check(
                    geometry_error < 1e-8,
                    f"{label}: texture policy leaves the full geometry mapping intact",
                )
                check(
                    uv_error < 1e-8,
                    f"{label}: every authored plane point uses the independent projection mapping",
                )
                check(
                    draft["texture_lock"] == movement
                    and draft["texture_scale_lock"] == stretch,
                    f"{label}: preview reports both effective settings",
                )
                check(
                    detail(client, [object_id])[0] == before
                    and client.call("get_map_info")["undo"] == history,
                    f"{label}: numeric preview preserves source records and history",
                )
                client.call(
                    "transform_objects", {**args, "scale": [0, 1, 1]}, error=True
                )
                check(
                    client.call("get_transform_handles")["state"]["preview"] == draft,
                    f"{label}: invalid scale retains the valid draft",
                )
                client.call("cancel_transform_preview")
                check(
                    detail(client, [object_id])[0] == before,
                    f"{label}: cancellation preserves exact source records",
                )
                client.call("transform_objects", args)
                client.call("commit_transform_preview")
                after = detail(client, [object_id])[0]
                check(
                    after == changed,
                    f"{label}: applied planes and projections match the draft",
                )
                check(
                    len(client.call("get_map_info")["undo"]) == len(history) + 1,
                    f"{label}: apply records one undo step",
                )
                for old, new in zip(before["faces"], after["faces"], strict=True):
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
                        f"{label}: face {old['side']} keeps identity and surface metadata",
                    )
                client.call("undo")
                check(
                    detail(client, [object_id])[0] == before,
                    f"{label}: undo restores exact source data",
                )
                client.call("redo")
                check(
                    detail(client, [object_id])[0] == after,
                    f"{label}: redo restores exact transformed data",
                )
                saved = output / f"{space}-{int(movement)}-{int(stretch)}.vmf"
                client.call("save_map", {"path": str(saved)})
                client.call("open_map", {"path": str(saved)})
                reopened = detail(client, [object_id])[0]
                check(
                    all(
                        old[key] == new[key]
                        for old, new in zip(
                            after["faces"], reopened["faces"], strict=True
                        )
                        for key in ["uaxis", "vaxis"]
                    ),
                    f"{label}: exact texture projections survive save and reopen",
                )

    for movement in [False, True]:
        object_id = fixture()
        args = {
            "ids": [object_id],
            "translation": translation,
            "rotation": angles,
            "scale": scale,
            "texture_lock": movement,
            "preview": True,
        }
        inherited = client.call("transform_objects", args)["state"]["preview"]
        client.call("cancel_transform_preview")
        explicit = client.call(
            "transform_objects", {**args, "texture_scale_lock": movement}
        )["state"]["preview"]
        check(
            inherited == explicit,
            f"Legacy move={movement}: omitted scale setting retains prior behavior",
        )
        client.call("cancel_transform_preview")

    for movement in [False, True]:
        for stretch in [False, True]:
            label = f"handle move={movement} stretch={stretch}"
            object_id = fixture()
            before = detail(client, [object_id])[0]
            tool = client.call(
                "set_transform_tool",
                {
                    "mode": "scale",
                    "space": "world",
                    "texture_lock": movement,
                    "texture_scale_lock": stretch,
                },
            )
            check(
                tool["settings"]["texture_lock"] == movement
                and tool["settings"]["texture_scale_lock"] == stretch,
                f"{label}: shared settings report independent flags",
            )
            client.call("begin_transform_drag", {"handle": "x"})
            draft = client.call("update_transform_drag", {"amount": 2})["preview"]
            changed = draft["objects"][0]
            check(
                draft["texture_lock"] == movement
                and draft["texture_scale_lock"] == stretch,
                f"{label}: scale handles use the shared projection policy",
            )
            if stretch:
                check(
                    all(
                        math.dist(uv(old, a), uv(new, b)) < 1e-8
                        for old, new in zip(
                            before["faces"], changed["faces"], strict=True
                        )
                        for a, b in zip(old["plane"], new["plane"], strict=True)
                    ),
                    f"{label}: stretch keeps every corresponding corner texel",
                )
            else:
                check(
                    all(
                        old[key] == new[key]
                        for old, new in zip(
                            before["faces"], changed["faces"], strict=True
                        )
                        for key in ["uaxis", "vaxis"]
                    ),
                    f"{label}: disabled stretch keeps texture density and world projections",
                )
            client.call("cancel_transform_preview")
            check(
                detail(client, [object_id])[0] == before,
                f"{label}: cancellation restores exact source",
            )
            client.call("begin_transform_drag", {"handle": "x"})
            client.call("update_transform_drag", {"amount": 2})
            client.call("commit_transform_preview")
            check(
                detail(client, [object_id])[0] == changed,
                f"{label}: apply matches the handle draft",
            )
            client.call("undo")
            check(
                detail(client, [object_id])[0] == before,
                f"{label}: undo restores source",
            )

    if render:
        pixels = {}
        results = {}
        for stretch in [False, True]:
            object_id = fixture()
            client.call(
                "set_camera", {"position": [135, -150, 100], "look_at": [32, 32, 16]}
            )
            baseline = client.render(
                output / f"texture-scale-{int(stretch)}-before.png"
            )
            args = {
                "ids": [object_id],
                "scale": [2, 1, 1],
                "texture_lock": True,
                "texture_scale_lock": stretch,
                "preview": True,
            }
            client.call("transform_objects", args)
            pixels[stretch] = client.render(
                output / f"texture-scale-{int(stretch)}-preview.png"
            )
            check(
                pixels[stretch] != baseline,
                f"Render stretch={stretch}: shared renderer shows the resized brush",
            )
            client.call("cancel_transform_preview")
            check(
                client.render(output / f"texture-scale-{int(stretch)}-cancelled.png")
                == baseline,
                f"Render stretch={stretch}: cancel restores exact pixels",
            )
            client.call("transform_objects", args)
            client.call("commit_transform_preview")
            check(
                client.render(output / f"texture-scale-{int(stretch)}-applied.png")
                == pixels[stretch],
                f"Render stretch={stretch}: Apply matches draft pixels",
            )
            client.call("select", {"ids": []})
            results[stretch] = client.render(
                output / f"texture-scale-{int(stretch)}-result.png"
            )
        check(
            results[False] != results[True],
            "Unselected surfaces show different texture repetition and stretch",
        )
        check(
            pixels[False] != pixels[True],
            "Renderer distinguishes repeated textures from stretched textures",
        )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-texture-lock-validation")
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
