import argparse
import json
import math
import re
from pathlib import Path

from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_direct_lighting import center_sample, floor_samples
from validate_face_lighting import ROOT, Client, detail
from validate_visibility import ROOM


def sample(data, target):
    return next(
        color
        for point, color, _, _ in floor_samples(data)
        if all(abs(a - b) < 0.001 for a, b in zip(point, target, strict=True))
    )


def validate(client, output, reference=None):
    results = []
    for name in [
        "clear",
        "shade",
        "enclosed",
        "tilted",
        "terrain-shade",
        "hidden",
        "spread",
        "duplicate",
        "ambient",
        "ambient-shade",
        "ambient-enclosed",
        "ambient-only",
        "ambient-fallback",
    ]:
        client.call("new_map", {"discard_changes": True})
        walls = [
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )["id"]
            for low, high in ROOM
        ]
        if name not in ["enclosed", "ambient-enclosed"]:
            ceiling = detail(client, [walls[1]])[0]
            bottom = next(
                face for face in ceiling["faces"] if face["normal"][2] < -0.99
            )
            client.call(
                "select_faces",
                {"faces": [{"solid": ceiling["solid"], "side": bottom["side"]}]},
            )
            client.call("edit_faces", {"material": "tools/toolsskybox"})
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [-64, 0, 8]},
        )
        client.call(
            "create_entity",
            {
                "classname": "logic_script",
                "origin": [0, 0, 32],
                "properties": {
                    "targetname": "hammerr_observer",
                    "vscripts": "hammerr_observer.nut",
                    "thinkfunction": "HammeRRThink",
                },
            },
        )
        client.call(
            "create_entity",
            {
                "classname": "prop_physics",
                "origin": [160, -128, 128],
                "properties": {
                    "targetname": "physics_crate",
                    "model": "models/props_junk/wood_crate001a.mdl",
                },
            },
        )
        properties = {
            "_light": "0 0 0 0" if name == "ambient-only" else "255 0 0 200",
            "_ambient": "0 0 255 100" if name.startswith("ambient") else "0 0 0 0",
            "SunSpreadAngle": "1" if name == "spread" else "0",
            "pitch": "-45" if name == "tilted" else "-90",
        }
        sun = client.call(
            "create_entity",
            {
                "classname": "light_environment",
                "origin": [0, 0, 128],
                "properties": properties,
            },
        )["id"]
        if name == "ambient-fallback":
            client.call("set_entity_properties", {"id": sun, "remove": ["_ambient"]})
        if name == "duplicate":
            client.call(
                "create_entity",
                {
                    "classname": "light_environment",
                    "origin": [64, 0, 128],
                    "properties": properties,
                },
            )
        if name == "hidden":
            client.call("set_object_visibility", {"ids": [sun], "action": "hide"})
        terrain_side = None
        if name in ["shade", "ambient-shade"]:
            client.call(
                "create_block",
                {
                    "mins": [-16, -16, 32],
                    "maxs": [16, 16, 48],
                    "material": "dev/dev_measuregeneric01",
                },
            )
        if name == "terrain-shade":
            terrain = client.call(
                "create_block",
                {
                    "mins": [0, 0, 0],
                    "maxs": [128, 128, 16],
                    "material": "dev/dev_measuregeneric01",
                },
            )["id"]
            terrain_side = next(
                face["side"]
                for face in detail(client, [terrain])[0]["faces"]
                if face["normal"][2] > 0.99
            )
        vmf = output / f"{name}.vmf"
        client.call("save_map", {"path": str(vmf)})
        if terrain_side:
            normals = "\n".join(
                f'"row{row}" "{" ".join(["0 0 1"] * 5)}"' for row in range(5)
            )
            distances = "\n".join(
                f'"row{row}" "{" ".join(["16"] * 5)}"' for row in range(5)
            )
            disp = f'dispinfo {{ "power" "2" "startposition" "[0 0 16]" "flags" "0" "elevation" "0" "subdiv" "0" normals {{ {normals} }} distances {{ {distances} }} }}'
            contents, count = re.subn(
                rf'(\bside\s*\{{\s*"id"\s*"{terrain_side}"[^{{}}]*)(\}})',
                lambda match, disp=disp: match[1] + disp + "\n" + match[2],
                vmf.read_text(),
            )
            assert count == 1
            vmf.write_text(contents)
        client.call("open_map", {"path": str(vmf)})
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        before = detail(client, [sun])
        history = client.call("get_map_info")["undo"]
        source = vmf.read_bytes()
        bsp = output / f"{name}.bsp"
        if name in ["spread", "duplicate"]:
            bsp.write_bytes(b"previous BSP")
            failed = client.call("compile_map", {"path": str(bsp)}, error=True)
            value = json.loads(failed["content"][0]["text"])
            assert not value["success"] and bsp.read_bytes() == b"previous BSP"
            assert any(
                row["level"] == "error"
                and any(word in row["message"].lower() for word in ["sky", "sun"])
                for row in value["diagnostics"]
            ), value
        else:
            compiled = client.call("compile_map", {"path": str(bsp)})
            assert compiled["success"], compiled
            data = bsp.read_bytes()
            expected = [
                0
                if name in ["shade", "enclosed", "terrain-shade", "hidden"]
                else 141
                if name == "tilted"
                else 200,
                0,
                0,
            ]
            ambient_expected = {
                "ambient": [200, 0, 60],
                "ambient-shade": [0, 0, 37.5],
                "ambient-enclosed": [0, 0, 0],
                "ambient-only": [0, 0, 60.75],
                "ambient-fallback": [260, 0, 0],
            }
            expected = ambient_expected.get(name, expected)
            assert center_sample(data) == expected, (name, center_sample(data))
            if reference and not name.startswith("ambient"):
                assert data == (reference / f"{name}.bsp").read_bytes(), (
                    f"{name}: zero-ambient BSP changed"
                )
            if name == "tilted":
                assert sample(data, [-128, 0, 0]) == [0, 0, 0]
                assert sample(data, [128, 0, 0]) == [141, 0, 0]
            elif name in ["shade", "terrain-shade"]:
                assert sample(data, [-128, 0, 0]) == [200, 0, 0]
            lights = records(data, 15, "<9f3i7f3i")
            assert (
                len(lights)
                == compiled["stats"]["world_lights"]
                == (0 if name == "hidden" else 2 if name.startswith("ambient") else 1)
            )
            if lights:
                light = lights[0]
                direction = (
                    [math.sqrt(0.5), 0, -math.sqrt(0.5)]
                    if name == "tilted"
                    else [0, 0, -1]
                )
                assert light[9:12] == (-1, 3, 0)
                assert all(
                    abs(a - b) < 1e-6
                    for a, b in zip(light[6:9], direction, strict=True)
                )
                assert (
                    abs(light[3] - (0 if name == "ambient-only" else 200 / 255)) < 1e-6
                )
                if name.startswith("ambient"):
                    assert lights[1][10] == 5 and lights[1][6:9] == (0, 0, 0)
            background = output / f"{name}-background.bsp"
            client.call("start_compile", {"path": str(background)})
            job = wait_job(client)
            assert job["status"] == "completed" and background.read_bytes() == data
            results.append(
                {"name": name, "stats": compiled["stats"], "center": expected}
            )
        assert (
            before == detail(client, [sun])
            and history == client.call("get_map_info")["undo"]
            and source == vmf.read_bytes()
        )
    (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-direct-sky"))
    parser.add_argument(
        "--reference", type=Path, help="Compare zero-ambient BSPs against a prior run"
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        validate(client, args.output, args.reference)
    finally:
        client.close()
    print(json.dumps({"output": str(args.output), "cases": 13}))


if __name__ == "__main__":
    main()
