import argparse
import json
import os
import struct
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_compile_jobs import wait_job
from validate_direct_lighting import center_sample
from validate_face_lighting import ROOT, Client, detail
from validate_visibility import ROOM


def payload(data, index):
    offset, size = struct.unpack_from("<2i", data, 8 + index * 16)
    return data[offset : offset + size]


def fixture(args):
    client = Client(None, args.binary, args.output)
    try:
        settings = client.call("get_compile_settings")
        assert settings["bounces"] == 0 and len(settings["bounce_choices"]) == 2
        client.call(
            "set_compile_settings", {"bounces": 2, "lighting": "direct_ldr"}, error=True
        )
        assert client.call("get_compile_settings") == settings
        objects = []
        for low, high in ROOM + [([-96, -96, 96], [96, 96, 112])]:
            objects.append(
                client.call(
                    "create_block",
                    {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
                )["id"]
            )
        for classname, origin, properties in [
            ("info_player_terrorist", [-192, -176, 8], {}),
            ("info_player_counterterrorist", [192, 176, 8], {}),
            (
                "light",
                [-128, 0, 180],
                {
                    "_light": "255 70 20 250",
                    "_constant_attn": "1",
                    "_quadratic_attn": "0",
                },
            ),
            (
                "logic_script",
                [-192, -176, 64],
                {
                    "targetname": "hammerr_observer",
                    "vscripts": "hammerr_observer.nut",
                    "thinkfunction": "HammeRRThink",
                },
            ),
            (
                "prop_physics",
                [128, 128, 128],
                {
                    "targetname": "physics_crate",
                    "model": "models/props_junk/wood_crate001a.mdl",
                },
            ),
        ]:
            objects.append(
                client.call(
                    "create_entity",
                    {
                        "classname": classname,
                        "origin": origin,
                        "properties": properties,
                    },
                )["id"]
            )
        source = args.output / "shadow.vmf"
        client.call("save_map", {"path": str(source)})
        before = detail(client, objects)
        history = client.call("get_map_info")["undo"]
        source_bytes = source.read_bytes()
        results = {}
        files = []
        previous = None
        for count in [0, 1]:
            if count == 1:
                client.call(
                    "set_compile_settings",
                    {
                        "lighting": "direct_ldr",
                        "physics": "engine",
                        "sky_quality": "detailed",
                        "bounces": 1,
                    },
                )
                retained = args.output / "retained.bsp"
                retained.write_bytes(b"previous BSP")
                rejected = client.call(
                    "compile_map", {"path": str(retained)}, error=True
                )
                rejected = json.loads(rejected["content"][0]["text"])
                assert (
                    not rejected["success"] and retained.read_bytes() == b"previous BSP"
                )
                assert any(
                    "100-million work-unit" in diagnostic["message"]
                    for diagnostic in rejected["diagnostics"]
                )
            client.call(
                "set_compile_settings",
                {
                    "lighting": "direct_ldr",
                    "physics": "engine",
                    "sky_quality": "balanced",
                    "bounces": count,
                },
            )
            if previous is not None:
                assert client.call("get_compile_status")["job"]["stale"]
            name = "direct" if count == 0 else "bounced"
            path = args.output / f"{name}.bsp"
            result = client.call("compile_map", {"path": str(path)})
            assert result["success"] and result["stats"]["bounces"] == count
            data = path.read_bytes()
            color = center_sample(data)
            if count == 0:
                assert color == [0, 0, 0], color
            else:
                assert color[0] > 1 and color[0] > color[1] > color[2], color
                assert (
                    result["stats"]["bounce_rays"]
                    == result["stats"]["lightmap_samples"] * 128
                )
                assert (
                    result["stats"]["bounce_samples"] > 0
                    and result["stats"]["bounce_work"] > 0
                )
                assert result["stats"]["lighting_work"] <= 100_000_000
                for index in range(64):
                    if index not in [8, 52, 56]:
                        assert payload(data, index) == payload(previous, index), index
            background = args.output / f"{name}-background.bsp"
            client.call("start_compile", {"path": str(background)})
            client.call("set_compile_settings", {"bounces": 1 - count}, error=True)
            job = wait_job(client)
            assert job["status"] == "completed" and job["settings"]["bounces"] == count
            assert background.read_bytes() == data
            assert (
                job["result"]["stats"]["bounce_work"] == result["stats"]["bounce_work"]
            )
            assert (
                before == detail(client, objects)
                and history == client.call("get_map_info")["undo"]
                and source.read_bytes() == source_bytes
            )
            results[name] = {"center_color": color, "stats": result["stats"]}
            files.append((name, path))
            previous = data
        assert not client.call("get_compile_status")["job"]["stale"]
        client.call("set_compile_settings", {"bounces": 0})
        assert client.call("get_compile_status")["job"]["stale"]
        client.call("set_compile_settings", {"bounces": 1})
        assert not client.call("get_compile_status")["job"]["stale"]
        (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        return files
    finally:
        client.close()


def worker(args):
    args.capture_ppm = True
    runtime.OBSERVER = runtime.OBSERVER.replace(
        "setpos -64 -64 32; setang 25 45 0; mat_fullbright",
        "setpos -192 -176 32; setang 25 42 0; r_drawothermodels 0; r_drawviewmodel 0; mat_fullbright",
    )
    runtime.worker(args)


def compare(output):
    frames = []
    for name in ["direct", "bounced"]:
        magic, dimensions, maximum, pixels = (
            (output / name / "game.ppm").read_bytes().split(b"\n", 3)
        )
        assert magic == b"P6" and maximum == b"255"
        frames.append((dimensions, pixels))
    assert frames[0][0] == frames[1][0]
    gains = [b - a for a, b in zip(frames[0][1], frames[1][1], strict=True)]
    changed = sum(max(pixel) > 3 for pixel in zip(*[iter(gains)] * 3))
    assert changed > 1000, changed
    return {"brighter_pixels": changed, "mean_component_gain": sum(gains) / len(gains)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-bounced-lighting")
    )
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        worker(args)
        return
    files = fixture(args)
    if args.runtime:
        env = {
            **os.environ,
            "WLR_BACKENDS": "headless",
            "WLR_HEADLESS_OUTPUTS": "1",
            "WLR_RENDERER": "pixman",
        }
        for name, bsp in files:
            output = args.output / name
            output.mkdir(exist_ok=True)
            with (output / "compositor.log").open("w") as log:
                subprocess.run(
                    [
                        "cage",
                        "--",
                        sys.executable,
                        "-B",
                        str(Path(__file__).resolve()),
                        "--worker",
                        "--bsp",
                        str(bsp),
                        "--output",
                        str(output),
                        "--game-dir",
                        str(args.game_dir),
                    ],
                    cwd=ROOT,
                    env=env,
                    stdout=log,
                    stderr=log,
                    check=True,
                    timeout=55,
                )
        (args.output / "render-results.json").write_text(
            json.dumps(compare(args.output), indent=2) + "\n"
        )
    print(json.dumps({"output": str(args.output), "bounced_lighting": True}))


if __name__ == "__main__":
    main()
