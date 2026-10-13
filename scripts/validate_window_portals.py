import argparse
import json
import os
import re
import struct
import subprocess
import sys
import threading
from pathlib import Path

import validate_areaportals as areas
import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import ROOT, Client, detail

OBSERVER = areas.OBSERVER.replace(
    "setpos -160 0 32; setang 0 0 0; r_drawothermodels 0;",
    "setpos -48 0 32; setang 0 0 0; r_drawothermodels 0;",
).replace(
    'if (gate != null && Time() > 8 && portal_stage == 0) {\n        EntFire("partition_gate", "Close"); portal_stage = 1;\n    } else if (gate != null && Time() > 18 && portal_stage == 1) {\n        EntFire("partition_gate", "Open"); portal_stage = 2;\n    }',
    """if (gate != null && Time() > 12 && portal_stage == 0) {
        SendToConsole("setpos -224 0 32; setang 0 0 0"); portal_stage = 1;
    } else if (gate != null && Time() > 20 && portal_stage == 1) {
        EntFire("partition_gate", "SetFadeStartDistance", "32");
        EntFire("partition_gate", "SetFadeEndDistance", "512"); portal_stage = 2;
    }
    if (gate != null) {
        local target = Entities.FindByName(null, "rendered_window");
        local origin = gate.GetOrigin();
        printl("HAMMERR_WINDOW " + portal_stage + " " + NetProps.GetPropFloat(gate, "m_flFadeStartDist") + " " + NetProps.GetPropFloat(gate, "m_flFadeDist") + " " + NetProps.GetPropFloat(gate, "m_flTranslucencyLimit") + " " + NetProps.GetPropInt(gate, "m_iBackgroundModelIndex") + " " + gate.GetModelName() + " " + target.GetModelName());
        printl("HAMMERR_WINDOW_ORIGIN " + origin.x + " " + origin.y + " " + origin.z);
    }""",
)


def fixture(args):
    areas.fixture(args)
    client = Client(None, args.binary, args.output)
    try:
        client.call("open_map", {"path": str(args.output / "areas.vmf")})
        gate = client.call("find_objects", {"classname": "func_areaportal"})["objects"][
            0
        ]["id"]
        models = []
        for name, material in [
            ("rendered_window", "dev/dev_measuregeneric01"),
            ("window_foreground", "glass/glasswindow001b"),
        ]:
            brush = client.call(
                "create_block",
                {"mins": [-8, -64, 0], "maxs": [8, 64, 192], "material": material},
            )["id"]
            models.append(
                client.call(
                    "tie_to_entity",
                    {
                        "ids": [brush],
                        "classname": "func_brush",
                        "properties": {
                            "targetname": name,
                            "origin": "0 0 96",
                            "Solidity": "1",
                        },
                    },
                )["id"]
            )
        client.call(
            "set_entity_properties",
            {
                "id": gate,
                "set": {
                    "classname": "func_areaportalwindow",
                    "target": "rendered_window",
                    "BackgroundBModel": "window_foreground",
                    "FadeStartDist": "64",
                    "FadeDist": "128",
                    "TranslucencyLimit": "0.2",
                },
            },
        )
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        vmf = args.output / "windows.vmf"
        client.call("save_map", {"path": str(vmf)})
        source = vmf.read_bytes()
        before = detail(client, [gate, *models])
        history = client.call("get_map_info")["undo"]
        bsp = args.output / "windows.bsp"
        built = client.call("compile_map", {"path": str(bsp)})
        assert built["success"] and built["stats"]["areaportal_windows"] == 1, built
        assert built["stats"]["areas"] == 2 and built["stats"]["areaportals"] == 1
        data = bsp.read_bytes()
        portals = records(data, 21, "<4Hi")
        assert len(portals) == 3 and portals[1][4] ^ 1 == portals[2][4]
        assert len(records(data, 14, "<9f3i")) == 3
        offset, length, _, _ = struct.unpack_from("<4i", data, 8)
        entities = data[offset : offset + length].decode()
        window = next(
            chunk
            for chunk in entities.split("{\n")
            if '"classname" "func_areaportalwindow"' in chunk
        )
        assert '"model"' not in window and '"PortalVersion" "1"' in window
        assert (
            '"target" "rendered_window"' in window
            and '"BackgroundBModel" "window_foreground"' in window
        )
        background = args.output / "window-background.bsp"
        client.call("start_compile", {"path": str(background)})
        job = wait_job(client)
        assert job["status"] == "completed" and background.read_bytes() == data
        assert job["result"]["stats"]["areaportal_windows"] == 1
        assert (
            source == vmf.read_bytes()
            and before == detail(client, [gate, *models])
            and history == client.call("get_map_info")["undo"]
        )
        client.call("set_object_visibility", {"action": "hide", "ids": [models[0]]})
        failed = client.call("compile_map", {"path": str(bsp)}, error=True)
        assert (
            bsp.read_bytes() == data
            and "visible rendered brush model" in json.dumps(failed)
        )
        client.call("open_map", {"path": str(vmf), "discard_changes": True})
        client.call("set_entity_properties", {"id": gate, "set": {"FadeDist": "32"}})
        failed = client.call("compile_map", {"path": str(bsp)}, error=True)
        assert bsp.read_bytes() == data and "Fade End Distance" in json.dumps(failed)
        (args.output / "window-results.json").write_text(
            json.dumps(
                {
                    "stats": built["stats"],
                    "source_history_unchanged": True,
                    "failed_output_preserved": True,
                },
                indent=2,
            )
            + "\n"
        )
        return bsp
    finally:
        client.close()


def worker(args):
    runtime.OBSERVER = OBSERVER
    errors = []
    capture = threading.Thread(
        target=areas.capture_states, args=(args.output, errors), daemon=True
    )
    capture.start()
    runtime.worker(args)
    capture.join(timeout=15)
    assert not errors, errors
    text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
    snapshots = [
        list(map(float, row[:5])) + list(row[5:])
        for row in re.findall(
            r"HAMMERR_WINDOW (\d+) ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+) (-?\d+) (\S+) (\S+)",
            text,
        )
    ]
    assert snapshots and all(
        row[5] == row[6] == "*1" and row[4] > 0 for row in snapshots
    ), snapshots
    assert all(abs(row[3] - 0.2) < 0.0001 for row in snapshots)
    assert any(row[:3] == [2, 32, 512] for row in snapshots), snapshots
    bits = [
        list(map(int, row))
        for row in re.findall(r"HAMMERR_PORTAL_BITS (\d+) (\d+)", text)
    ]
    masks = [
        list(map(int, row))
        for row in re.findall(r"HAMMERR_AREA_STATE (\d+) (\d+)", text)
    ]
    stable_bits = {
        stage: [mask & 2 for phase, mask in bits if phase == stage]
        for stage in [0, 1, 2]
    }
    stable_areas = {
        stage: [mask & 6 for phase, mask in masks if phase == stage]
        for stage in [0, 1, 2]
    }
    assert all(stable_bits.values()) and [
        stable_bits[stage][-1] for stage in [0, 1, 2]
    ] == [2, 0, 2], stable_bits
    assert all(stable_areas.values()) and [
        stable_areas[stage][-1] for stage in [0, 1, 2]
    ] == [6, 4, 6], stable_areas
    origins = [
        list(map(float, row))
        for row in re.findall(
            r"HAMMERR_WINDOW_ORIGIN ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+)", text
        )
    ]
    assert origins and all(origin == [0, 0, 96] for origin in origins[-3:]), origins
    (args.output / "window-states.json").write_text(
        json.dumps(
            {
                "windows": snapshots,
                "client_portals": stable_bits,
                "areas": stable_areas,
                "origins": origins,
            },
            indent=2,
        )
        + "\n"
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-window-portals")
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
    print(json.dumps({"output": str(args.output), "window_portals": True}))


if __name__ == "__main__":
    main()
