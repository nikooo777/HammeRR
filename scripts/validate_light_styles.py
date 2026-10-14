import argparse
import json
import os
import re
import struct
import subprocess
import sys
import threading
import time
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_direct_lighting import FACE_FORMAT, lightmaps
from validate_face_lighting import ROOT, Client, detail
from validate_visibility import ROOM


def payload(data, index):
    offset, size = struct.unpack_from("<2i", data, 8 + index * 16)
    return data[offset : offset + size]


def channels(data):
    planes = records(data, 1, "<4fi")
    colors = lightmaps(data)
    result = []
    for face in records(data, 7, FACE_FORMAT):
        if planes[face[0]][2] < 0.99 or abs(planes[face[0]][3]) > 0.01:
            continue
        styles = [style for style in face[8:12] if style != 255]
        assert styles == [0, 32]
        count = (face[16] + 1) * (face[17] + 1)
        samples = []
        for index, style in enumerate(styles):
            first = face[12] + index * count * 4
            values = [
                [component * 2**exponent for component in rgb]
                for *rgb, exponent in struct.iter_unpack(
                    "<3Bb", colors[first : first + count * 4]
                )
            ]
            averages = [sum(row[axis] for row in values) / count for axis in range(3)]
            average = struct.unpack_from("<3Bb", colors, face[12] - (index + 1) * 4)
            stored = [component * 2 ** average[3] for component in average[:3]]
            assert all(
                abs(a - b) <= max(1, a / 128)
                for a, b in zip(averages, stored, strict=True)
            )
            assert (
                all(row[0] == row[1] == 0 for row in values)
                if style == 0
                else all(row[1] == row[2] == 0 for row in values)
            )
            assert max(row[2 if style == 0 else 0] for row in values) > 0
            samples.append({"style": style, "average": stored})
        result.append(samples)
    assert result
    return result


def fixture(args):
    client = Client(None, args.binary, args.output)
    try:
        for mins, maxs in ROOM:
            client.call(
                "create_block",
                {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"},
            )
        lights = []
        objects = []
        for x in [-96, 96]:
            entity = client.call(
                "create_entity",
                {
                    "classname": "light",
                    "origin": [x, 0, 180],
                    "properties": {
                        "targetname": "switch_light",
                        "spawnflags": "1",
                        "_light": "255 0 0 80",
                        "_constant_attn": "1",
                        "_quadratic_attn": "0",
                    },
                },
            )["id"]
            lights.append(entity)
            objects.append(entity)
        for classname, origin, properties in [
            (
                "light",
                [0, 0, 128],
                {"_light": "0 0 255 35", "_constant_attn": "1", "_quadratic_attn": "0"},
            ),
            ("info_player_terrorist", [-192, -176, 8], {}),
            ("info_player_counterterrorist", [192, 176, 8], {}),
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
        for name, input_name, parameter in [
            ("lights_on", "TurnOn", ""),
            ("lights_dark", "TurnOff", ""),
            ("lights_bright", "SetPattern", "z"),
            ("lights_animate", "SetPattern", "am"),
        ]:
            relay = client.call(
                "create_entity",
                {
                    "classname": "logic_relay",
                    "origin": [-192, -176, 96],
                    "properties": {"targetname": name},
                },
            )["id"]
            client.call("begin_connection_edit", {"id": relay})
            client.call(
                "update_connection_draft",
                {
                    "output": "OnTrigger",
                    "target": "switch_light",
                    "input": input_name,
                    "parameter": parameter,
                    "delay": "0",
                    "times": "-1",
                },
            )
            client.call("commit_connection_edit")
            objects.append(relay)
        source = args.output / "styles.vmf"
        client.call("save_map", {"path": str(source)})
        source_bytes = source.read_bytes()
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        before = detail(client, objects)
        history = client.call("get_map_info")["undo"]
        path = args.output / "styles.bsp"
        result = client.call("compile_map", {"path": str(path)})
        data = path.read_bytes()
        stats = result["stats"]
        assert (
            stats["switchable_light_groups"] == 1
            and stats["light_styles"] == 2
            and stats["styled_faces"] > 0
        )
        maps = channels(data)
        assert len(re.findall(r'"style" "32"', payload(data, 0).decode())) == 2
        background = args.output / "background.bsp"
        client.call("start_compile", {"path": str(background)})
        job = wait_job(client)
        assert job["status"] == "completed" and background.read_bytes() == data
        assert (
            before == detail(client, objects)
            and history == client.call("get_map_info")["undo"]
            and source.read_bytes() == source_bytes
        )
        client.call("set_compile_settings", {"bounces": 1})
        client.call("compile_map", {"path": str(path)}, error=True)
        assert path.read_bytes() == data
        client.call("set_compile_settings", {"bounces": 0})
        client.call("set_entity_properties", {"id": lights[0], "set": {"pattern": "A"}})
        client.call("compile_map", {"path": str(path)}, error=True)
        client.call("start_compile", {"path": str(path)})
        assert wait_job(client)["status"] == "failed" and path.read_bytes() == data
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "stats": stats,
                    "floor_channels": maps,
                    "builds_identical": True,
                    "source_history_preserved": True,
                    "rejected_output_preserved": True,
                },
                indent=2,
            )
            + "\n"
        )
        return path
    finally:
        client.close()


OBSERVER = (
    runtime.OBSERVER.replace("stage <- 0;", "stage <- 0;\nlight_stage <- 0;")
    .replace(
        "setpos -64 -64 32; setang 25 45 0; mat_fullbright",
        "setpos -192 -176 32; setang 20 45 0; r_drawothermodels 0; r_drawviewmodel 0; mat_fullbright",
    )
    .replace(
        '    local player = Entities.FindByClassname(null, "player");',
        """
    if (Time() > 14 && light_stage == 0) { EntFire("lights_on", "Trigger"); light_stage = 1; }
    else if (Time() > 19 && light_stage == 1) { EntFire("lights_dark", "Trigger"); light_stage = 2; }
    else if (Time() > 24 && light_stage == 2) { EntFire("lights_bright", "Trigger"); light_stage = 3; }
    else if (Time() > 29 && light_stage == 3) { EntFire("lights_animate", "Trigger"); light_stage = 4; }
    if (stage == 2) printl("HAMMERR_LIGHT_STATE " + light_stage);
    local player = Entities.FindByClassname(null, "player");
""",
    )
)


def capture_states(output, errors):
    try:
        log = output / "cstrike/hammerr-runtime.log"
        started = time.monotonic()
        captured = set()
        while time.monotonic() - started < 45:
            text = log.read_text(errors="replace") if log.exists() else ""
            states = re.findall(r"HAMMERR_LIGHT_STATE (\d+)", text)
            if states and int(states[-1]) == 4:
                animated = []
                for index in range(8):
                    path = output / f"animation-{index}.ppm"
                    subprocess.run(
                        ["grim", "-t", "ppm", str(path)], check=True, timeout=5
                    )
                    pixels = path.read_bytes().split(b"\n", 3)[3]
                    animated.append(sum(pixels[::3]) / (len(pixels) / 3))
                    time.sleep(0.06)
                assert max(animated) > 10 and max(animated) - min(animated) > 10, (
                    animated
                )
                (output / "animation.json").write_text(
                    json.dumps({"mean_red": animated, "pattern": "am"}, indent=2) + "\n"
                )
                break
            if states and (phase := int(states[-1])) not in captured:
                time.sleep(1)
                subprocess.run(
                    ["grim", str(output / f"phase-{phase}.png")], check=True, timeout=5
                )
                subprocess.run(
                    ["grim", "-t", "ppm", str(output / f"phase-{phase}.ppm")],
                    check=True,
                    timeout=5,
                )
                captured.add(phase)
            time.sleep(0.1)
        assert captured == {0, 1, 2, 3} and (output / "animation.json").exists(), (
            captured
        )
    except (OSError, subprocess.SubprocessError, AssertionError, ValueError) as error:
        errors.append(str(error))


def worker(args):
    runtime.OBSERVER = OBSERVER
    args.observe_seconds = 40
    errors = []
    capture = threading.Thread(
        target=capture_states, args=(args.output, errors), daemon=True
    )
    capture.start()
    runtime.worker(args)
    capture.join(timeout=10)
    assert not errors, errors
    means = []
    for phase in range(4):
        magic, _dimensions, maximum, pixels = (
            (args.output / f"phase-{phase}.ppm").read_bytes().split(b"\n", 3)
        )
        assert magic == b"P6" and maximum == b"255"
        means.append([sum(pixels[axis::3]) / (len(pixels) / 3) for axis in range(3)])
    assert abs(means[0][0] - means[2][0]) < 2, means
    assert means[1][0] > means[0][0] + 10 and means[3][0] > means[1][0] + 5, means
    (args.output / "style-states.json").write_text(
        json.dumps(
            {
                "mean_rgb": means,
                "relay_inputs": ["TurnOn", "TurnOff", "SetPattern z"],
            },
            indent=2,
        )
        + "\n"
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-light-styles")
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
    bsp = fixture(args)
    if args.runtime:
        output = args.output / "runtime"
        output.mkdir(exist_ok=True)
        env = {
            **os.environ,
            "WLR_BACKENDS": "headless",
            "WLR_HEADLESS_OUTPUTS": "1",
            "WLR_RENDERER": "pixman",
        }
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
    print(json.dumps({"output": str(args.output), "light_styles": True}))


if __name__ == "__main__":
    main()
