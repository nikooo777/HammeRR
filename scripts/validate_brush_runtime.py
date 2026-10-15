import argparse
import json
import os
import re
import struct
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_direct_lighting import FACE_FORMAT, lightmaps
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail
from validate_visibility import ROOM

OBSERVER = (
    runtime.OBSERVER.replace(
        "stage <- 0;",
        'stage <- 0;\nmotion_stage <- 0;\nmotion_next <- 0.0;\nfunction HammeRRTrigger() { printl("HAMMERR_TRIGGER_TOUCH"); }',
    )
    .replace(
        "sv_cheats 1; joystick 0;", "sv_cheats 1; joystick 0; bot_quota 0; bot_kick;"
    )
    .replace(
        "setpos -64 -64 32; setang 25 45 0; mat_fullbright",
        "setpos -64 -64 32; setang 25 45 0; r_drawothermodels 0; r_drawviewmodel 0; mat_fullbright",
    )
    .replace(
        '    local player = Entities.FindByClassname(null, "player");',
        """
    foreach (name in ["slide_door", "hinge_door", "spin_brush"]) {
        local ent = Entities.FindByName(null, name);
        if (ent != null) {
            local pos = ent.GetOrigin();
            local angles = ent.GetAngles();
            printl("HAMMERR_BRUSH " + motion_stage + " " + name + " " + pos.x + " " + pos.y + " " + pos.z + " " + angles.x + " " + angles.y + " " + angles.z);
        }
    }
    if (motion_stage == 0 && Time() > 5) {
        EntFire("slide_door", "Open");
        EntFire("hinge_door", "Open");
        motion_stage = 1;
        motion_next = Time() + 5;
    } else if (motion_stage == 1 && Time() > motion_next) {
        EntFire("slide_door", "Close");
        EntFire("hinge_door", "Close");
        motion_stage = 2;
    }
    local player = Entities.FindByClassname(null, "player");
""",
    )
)


def payload(data, index):
    offset, size = struct.unpack_from("<2i", data, 8 + index * 16)
    return data[offset : offset + size]


def check_lightmaps(data, stats):
    models = records(data, 14, "<9f3i")
    faces = records(data, 7, FACE_FORMAT)
    colors = lightmaps(data)
    count = 0
    for model in models[1:4]:
        lit = False
        for face in faces[model[10] : model[10] + model[11]]:
            assert face[8:12] == (0, 255, 255, 255) and face[12] >= 4
            size = (face[16] + 1) * (face[17] + 1) * 4
            samples = colors[face[12] : face[12] + size]
            assert len(samples) == size
            lit |= any(
                sum(samples[index : index + 3]) > 0 for index in range(0, size, 4)
            )
            count += 1
        assert lit, model
    assert stats["lit_brush_models"] == 3 and stats["model_lighting_faces"] == count


def fixture(args):
    client = Client(None, args.binary, args.output)
    try:

        def block(low, high, material="dev/dev_measuregeneric01"):
            return client.call(
                "create_block", {"mins": low, "maxs": high, "material": material}
            )["id"]

        for low, high in ROOM:
            block(low, high)
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [-64, 0, 8]},
        )
        client.call(
            "create_entity",
            {
                "classname": "light",
                "origin": [0, 0, 128],
                "properties": {"_light": "255 170 100 300", "_quadratic_attn": "1"},
            },
        )
        observer = client.call(
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
        )["id"]
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
        slide = block([64.125, -7.75, 16.375], [96.125, 8.25, 80.375])
        slide = client.call(
            "tie_to_entity",
            {
                "ids": [slide],
                "classname": "func_door",
                "properties": {
                    "targetname": "slide_door",
                    "origin": "80.125 0.25 48.375",
                    "movedir": "0 90 0",
                    "speed": "64",
                    "lip": "0",
                    "wait": "-1",
                },
            },
        )["id"]
        hinge = block([-72.125, 72.25, 16.375], [-8.125, 88.25, 80.375])
        pivot = block(
            [-68.125, 76.25, 44.375], [-60.125, 84.25, 52.375], "tools/toolsorigin"
        )
        hinge = client.call(
            "tie_to_entity",
            {
                "ids": [hinge, pivot],
                "classname": "func_door_rotating",
                "properties": {
                    "targetname": "hinge_door",
                    "origin": "1 2 3",
                    "distance": "90",
                    "speed": "90",
                    "wait": "-1",
                    "spawnflags": "16",
                },
            },
        )["id"]
        spin = block([48.125, 72.25, 88.375], [112.125, 88.25, 104.375])
        spin = client.call(
            "tie_to_entity",
            {
                "ids": [spin],
                "classname": "func_rotating",
                "properties": {
                    "targetname": "spin_brush",
                    "origin": "80.125 80.25 96.375",
                    "maxspeed": "90",
                    "fanfriction": "0",
                    "spawnflags": "1",
                },
            },
        )["id"]
        trigger = block([-128, -128, 0], [-32, -32, 96], "tools/toolstrigger")
        trigger = client.call(
            "tie_to_entity",
            {
                "ids": [trigger],
                "classname": "trigger_multiple",
                "properties": {"targetname": "touch_volume", "spawnflags": "1"},
            },
        )["id"]
        draft = client.call("begin_connection_edit", {"id": trigger})
        client.call(
            "update_connection_draft",
            {
                "token": draft["token"],
                "output": "OnStartTouch",
                "target": "hammerr_observer",
                "input": "RunScriptCode",
                "parameter": "HammeRRTrigger()",
                "delay": "0",
                "times": "-1",
            },
        )
        client.call("commit_connection_edit", {"token": draft["token"]})
        targets = [slide, hinge, spin, trigger, observer]
        before = detail(client, targets)
        history = client.call("get_map_info")["undo"]
        vmf = args.output / "brush-runtime.vmf"
        client.call("save_map", {"path": str(vmf)})
        source = vmf.read_bytes()
        client.call(
            "set_compile_settings", {"lighting": "fullbright", "physics": "engine"}
        )
        unlit = args.output / "fullbright.bsp"
        client.call("compile_map", {"path": str(unlit)})
        unlit = unlit.read_bytes()
        client.call("set_compile_settings", {"lighting": "direct_ldr"})
        bsp = args.output / "brush-runtime.bsp"
        compiled = client.call("compile_map", {"path": str(bsp)})
        assert compiled["success"] and compiled["stats"]["models"] == 5
        data = bsp.read_bytes()
        check_lightmaps(data, compiled["stats"])
        for index in [1, 3, 5, 10, 12, 13, 14, 18, 19, 29]:
            assert payload(data, index) == payload(unlit, index), (
                f"Geometry/physics lump {index} changed"
            )
        background = args.output / "background.bsp"
        client.call("start_compile", {"path": str(background)})
        assert (
            wait_job(client)["status"] == "completed"
            and background.read_bytes() == data
        )
        assert (
            detail(client, targets) == before
            and client.call("get_map_info")["undo"] == history
            and vmf.read_bytes() == source
        )
        return bsp, compiled["stats"]
    finally:
        client.close()


def check(args):
    text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
    rows = [
        (int(stage), name, *map(float, values.split()))
        for stage, name, values in re.findall(
            r"HAMMERR_BRUSH (\d+) (\w+) ([\d.eE+ -]+)", text
        )
    ]
    assert len(rows) > 10, text[-4000:]
    slide = [row for row in rows if row[1] == "slide_door"]
    hinge = [row for row in rows if row[1] == "hinge_door"]
    spin = [row for row in rows if row[1] == "spin_brush"]
    near = lambda a, b: abs(a - b) < 0.02
    assert near(slide[0][2], 80.125) and near(slide[-1][2], 80.125), slide
    assert all(near(row[2], 80.125) for row in slide), slide
    assert near(slide[0][3], 0.25) and near(slide[-1][3], 0.25), slide
    assert any(near(row[3], 16.25) for row in slide), slide
    assert all(
        near(row[2], -64.125) and near(row[3], 80.25) and near(row[4], 48.375)
        for row in hinge
    ), hinge
    assert any(abs(row[6] - 90) < 0.02 for row in hinge) and abs(hinge[-1][6]) < 0.02, (
        hinge
    )
    assert all(
        near(row[2], 80.125) and near(row[3], 80.25) and near(row[4], 96.375)
        for row in spin
    ), spin
    assert max(row[6] for row in spin) - min(row[6] for row in spin) > 45, spin
    assert "HAMMERR_TRIGGER_TOUCH" in text, (
        "Trigger output did not fire on the teleport"
    )
    (args.output / "brush-observations.json").write_text(
        json.dumps(rows, indent=2) + "\n"
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-brush-runtime")
    )
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        runtime.OBSERVER = OBSERVER
        runtime.worker(args)
        check(args)
        return
    bsp, stats = fixture(args)
    output = args.output / "game"
    output.mkdir(exist_ok=True)
    env = {
        **os.environ,
        "WLR_BACKENDS": "headless",
        "WLR_HEADLESS_OUTPUTS": "1",
        "WLR_RENDERER": "pixman",
    }
    command = [
        "cage",
        "--",
        sys.executable,
        "-B",
        str(Path(__file__).resolve()),
        "--worker",
        "--game-dir",
        str(args.game_dir),
        "--output",
        str(output),
        "--bsp",
        str(bsp),
    ]
    with (output / "compositor.log").open("w") as log:
        subprocess.run(
            command, cwd=ROOT, env=env, stdout=log, stderr=log, check=True, timeout=50
        )
    (args.output / "results.json").write_text(
        json.dumps(
            {
                "stats": stats,
                "observations": json.loads(
                    (output / "brush-observations.json").read_text()
                ),
            },
            indent=2,
        )
        + "\n"
    )
    print(json.dumps({"output": str(args.output), "models": stats["models"]}))


if __name__ == "__main__":
    main()
