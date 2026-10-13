import argparse
import json
import math
from pathlib import Path

from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_direct_lighting import center_sample, floor_samples
from validate_face_lighting import ROOT, Client, detail
from validate_visibility import ROOM


def validate(client, output):
    results = []
    for name in ["down", "target", "hidden", "invalid"]:
        client.call("new_map", {"discard_changes": True})
        for low, high in ROOM:
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )
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
            "_light": "255 0 0 200",
            "_constant_attn": "1",
            "_linear_attn": "0",
            "_quadratic_attn": "0",
            "pitch": "-90",
            "_inner_cone": "20",
            "_cone": "45",
            "_exponent": "2",
        }
        if name == "target":
            client.call(
                "create_entity",
                {
                    "classname": "info_target",
                    "origin": [64, 0, 16],
                    "properties": {"targetname": "aim"},
                },
            )
            properties.update({"target": "aim", "pitch": "90"})
        if name == "invalid":
            properties["_inner_cone"] = "60"
        light = client.call(
            "create_entity",
            {
                "classname": "light_spot",
                "origin": [0, 0, 100],
                "properties": properties,
            },
        )["id"]
        if name == "hidden":
            client.call("set_object_visibility", {"ids": [light], "action": "hide"})
        vmf = output / f"{name}.vmf"
        client.call("save_map", {"path": str(vmf)})
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        before = detail(client, [light])
        history = client.call("get_map_info")["undo"]
        source = vmf.read_bytes()
        bsp = output / f"{name}.bsp"
        if name == "invalid":
            bsp.write_bytes(b"previous BSP")
            failed = client.call("compile_map", {"path": str(bsp)}, error=True)
            value = json.loads(failed["content"][0]["text"])
            assert not value["success"] and bsp.read_bytes() == b"previous BSP"
            assert any(
                row["object"] == light and "Spotlight cones" in row["message"]
                for row in value["diagnostics"]
            )
        else:
            compiled = client.call("compile_map", {"path": str(bsp)})
            assert compiled["success"], compiled
            data = bsp.read_bytes()
            lights = records(data, 15, "<9f3i7f3i")
            assert (
                len(lights)
                == compiled["stats"]["world_lights"]
                == (0 if name == "hidden" else 1)
            )
            if lights:
                light_record = lights[0]
                assert light_record[10] == 2 and light_record[14] == 2
                assert abs(light_record[12] - math.cos(math.radians(20))) < 1e-6
                assert abs(light_record[13] - math.cos(math.radians(45))) < 1e-6
                expected = (
                    [0, 0, -1]
                    if name == "down"
                    else [64 / math.hypot(64, 84), 0, -84 / math.hypot(64, 84)]
                )
                assert all(
                    abs(a - b) < 1e-6
                    for a, b in zip(light_record[6:9], expected, strict=True)
                )
            if name == "down":
                assert center_sample(data) == [200, 0, 0]
                samples = floor_samples(data)
                assert any(
                    abs(point[0] - 128) < 0.001
                    and abs(point[1]) < 0.001
                    and color == [0, 0, 0]
                    for point, color, _, _ in samples
                )
            elif name == "hidden":
                assert center_sample(data) == [0, 0, 0]
            background = output / f"{name}-background.bsp"
            client.call("start_compile", {"path": str(background)})
            job = wait_job(client)
            assert job["status"] == "completed" and background.read_bytes() == data, job
            results.append(
                {"name": name, "stats": compiled["stats"], "records": lights}
            )
        assert (
            before == detail(client, [light])
            and history == client.call("get_map_info")["undo"]
            and source == vmf.read_bytes()
        )
    (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-spotlights"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        validate(client, args.output)
        print(json.dumps({"output": str(args.output), "cases": 4}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
