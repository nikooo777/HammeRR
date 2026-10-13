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
from validate_face_lighting import ROOT, Client, detail
from validate_visibility import ROOM

OBSERVER = (
    runtime.OBSERVER.replace("stage <- 0;", "stage <- 0;\nportal_stage <- 0;")
    .replace(
        "setpos -64 -64 32; setang 25 45 0; mat_fullbright",
        "setpos -160 0 32; setang 0 0 0; r_drawothermodels 0; mat_fullbright",
    )
    .replace(
        "sv_cheats 1; joystick 0;", "sv_cheats 1; joystick 0; bot_quota 0; bot_kick;"
    )
    .replace(
        '    local player = Entities.FindByClassname(null, "player");',
        """
    local player = Entities.FindByClassname(null, "player");
    local gate = Entities.FindByName(null, "partition_gate");
    if (gate != null && Time() > 8 && portal_stage == 0) {
        EntFire("partition_gate", "Close"); portal_stage = 1;
    } else if (gate != null && Time() > 18 && portal_stage == 1) {
        EntFire("partition_gate", "Open"); portal_stage = 2;
    }
    if (player != null && player.GetHealth() > 0) {
        local key = "m_Local.m_chAreaBits";
        if (!NetProps.HasProp(player, key)) key = "m_chAreaBits";
        if (NetProps.HasProp(player, key)) {
            local areas = NetProps.GetPropIntArray(player, key, 0);
            printl("HAMMERR_AREA_STATE " + portal_stage + " " + areas);
        } else printl("HAMMERR_AREA_PROP_MISSING");
        local portal_key = "m_Local.m_chAreaPortalBits";
        if (NetProps.HasProp(player, portal_key)) {
            local bits = NetProps.GetPropIntArray(player, portal_key, 0);
            printl("HAMMERR_PORTAL_BITS " + portal_stage + " " + bits);
        }
        printl("HAMMERR_GATE_PRESENT " + (gate != null));
    }
""",
    )
)


def graph(data, stats):
    areas = records(data, 20, "<2i")
    portals = records(data, 21, "<4Hi")
    vertices = records(data, 41, "<3f")
    leaves = records(data, 10, "<ihh6h4Hhh", expected_version=1)
    assert len(areas) - 1 == stats["areas"] == 2
    assert len(portals) == 3 and stats["areaportals"] == 1
    assert len(vertices) == stats["areaportal_vertices"] == 8
    assert portals[0] == (0, 0, 0, 0, 0)
    assert portals[1][0] == portals[2][0] == 1 and portals[1][4] ^ 1 == portals[2][4]
    polygons = [vertices[portal[2] : portal[2] + portal[3]] for portal in portals[1:]]
    assert polygons[0] == polygons[1]
    nodes = records(data, 5, "<3i6h2H2h")
    for node in nodes:
        child_areas = [
            leaves[-child - 1][2] & 511 if child < 0 else nodes[child][-2]
            for child in node[1:3]
        ]
        assert node[-2] == (child_areas[0] if len(set(child_areas)) == 1 else -1)
    for owner, (count, first) in enumerate(areas):
        for portal in portals[first : first + count]:
            assert portal[1] != owner and 0 < portal[1] < len(areas)
            assert portal[2] + portal[3] <= len(vertices)
    assert {leaf[2] & 511 for leaf in leaves} >= {0, 1, 2}
    offset, length, _, _ = struct.unpack_from("<4i", data, 8)
    entities = data[offset : offset + length].decode().rstrip("\0")
    assert (
        '"portalnumber" "1"' in entities
        and '"PortalVersion" "1"' in entities
        and '"targetname" "partition_gate"' in entities
    )
    assert '"model" "*1"' not in entities


def fixture(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )
        dividers = []
        for low, high in [
            ([-8, -256, 0], [8, -64, 256]),
            ([-8, 64, 0], [8, 256, 256]),
            ([-8, -64, 192], [8, 64, 256]),
        ]:
            dividers.append(
                client.call(
                    "create_block",
                    {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
                )["id"]
            )
        for classname, origin, properties in [
            ("info_player_terrorist", [-64, -64, 8], {}),
            ("info_player_counterterrorist", [128, 64, 8], {}),
            (
                "light",
                [-160, 0, 180],
                {
                    "_light": "255 100 100 200",
                    "_constant_attn": "1",
                    "_quadratic_attn": "0",
                },
            ),
            (
                "light",
                [160, 0, 180],
                {
                    "_light": "100 100 255 200",
                    "_constant_attn": "1",
                    "_quadratic_attn": "0",
                },
            ),
            (
                "logic_script",
                [0, 0, 64],
                {
                    "targetname": "hammerr_observer",
                    "vscripts": "hammerr_observer.nut",
                    "thinkfunction": "HammeRRThink",
                },
            ),
            (
                "prop_physics",
                [160, -128, 128],
                {
                    "targetname": "physics_crate",
                    "model": "models/props_junk/wood_crate001a.mdl",
                },
            ),
        ]:
            client.call(
                "create_entity",
                {"classname": classname, "origin": origin, "properties": properties},
            )
        brush = client.call(
            "create_block",
            {
                "mins": [-4, -64, 0],
                "maxs": [4, 64, 192],
                "material": "tools/toolsareaportal",
            },
        )["id"]
        gate = client.call(
            "tie_to_entity",
            {
                "ids": [brush],
                "classname": "func_areaportal",
                "properties": {"targetname": "partition_gate", "StartOpen": "1"},
            },
        )["id"]
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        vmf = args.output / "areas.vmf"
        client.call("save_map", {"path": str(vmf)})
        source = vmf.read_bytes()
        before = detail(client, [gate])
        history = client.call("get_map_info")["undo"]
        path = args.output / "areas.bsp"
        built = client.call("compile_map", {"path": str(path)})
        assert built["success"], built
        data = path.read_bytes()
        graph(data, built["stats"])
        background = args.output / "background.bsp"
        client.call("start_compile", {"path": str(background)})
        job = wait_job(client)
        assert job["status"] == "completed" and background.read_bytes() == data
        assert job["result"]["stats"]["areaportals"] == 1
        assert (
            source == vmf.read_bytes()
            and before == detail(client, [gate])
            and history == client.call("get_map_info")["undo"]
        )
        client.render(
            args.output / "viewport.png",
            width=1000,
            height=700,
            camera={"position": [-180, -180, 170], "look_at": [0, 0, 96]},
        )
        client.call("set_object_visibility", {"action": "hide", "ids": dividers})
        failed = client.call("compile_map", {"path": str(path)}, error=True)
        assert path.read_bytes() == data and "two distinct areas" in json.dumps(failed)
        client.call("open_map", {"path": str(vmf), "discard_changes": True})
        client.call("set_object_visibility", {"action": "hide", "ids": [gate]})
        hidden = client.call("compile_map", {"path": str(args.output / "hidden.bsp")})
        assert (
            hidden["success"]
            and hidden["stats"]["areaportals"] == 0
            and hidden["stats"]["areas"] == 1
        )
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "stats": built["stats"],
                    "native_graph": True,
                    "invalid_boundary_preserves_output": True,
                    "hidden_portals_omitted": True,
                },
                indent=2,
            )
            + "\n"
        )
        return path
    finally:
        client.close()


def capture_states(output, errors):
    try:
        captured = set()
        deadline = time.monotonic() + 30
        log = output / "cstrike/hammerr-runtime.log"
        while time.monotonic() < deadline and len(captured) < 2:
            if log.exists():
                text = log.read_text(errors="replace")
                states = re.findall(
                    r"HAMMERR_AREA_STATE (\d+) (\d+)",
                    text,
                )
                portal_bits = re.findall(r"HAMMERR_PORTAL_BITS (\d+) (\d+)", text)
                if len(states) >= 3 and len(set(states[-3:])) == 1:
                    phase, mask = map(int, states[-1])
                    ready = (
                        phase in [1, 2]
                        and ((mask & 6) in [2, 4] if phase == 1 else (mask & 6) == 6)
                        and portal_bits
                        and tuple(map(int, portal_bits[-1]))
                        == (phase, 0 if phase == 1 else 2)
                    )
                    if phase not in captured and ready:
                        name = ["open", "closed", "reopened"][phase]
                        subprocess.run(
                            ["grim", str(output / f"{name}.png")],
                            check=True,
                            timeout=10,
                        )
                        captured.add(phase)
            time.sleep(0.2)
        assert captured == {1, 2}, captured
    except (OSError, subprocess.SubprocessError) as error:
        errors.append(str(error))
    except AssertionError as error:
        errors.append(f"Missing stable runtime captures: {error}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-areaportals"))
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument(
        "--view", choices=["left", "right"], default="left", help=argparse.SUPPRESS
    )
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        runtime.OBSERVER = OBSERVER
        if args.view == "right":
            runtime.OBSERVER = runtime.OBSERVER.replace(
                "setpos -160 0 32; setang 0 0 0", "setpos 160 0 32; setang 0 180 0"
            )
        errors = []
        capture = threading.Thread(
            target=capture_states, args=(args.output, errors), daemon=True
        )
        capture.start()
        runtime.worker(args)
        capture.join(timeout=15)
        assert not errors and (args.output / "closed.png").exists(), errors
        text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
        states = [
            list(map(int, values))
            for values in re.findall(r"HAMMERR_AREA_STATE (\d+) (\d+)", text)
        ]
        stable = {
            stage: [mask & 6 for phase, mask in states if phase == stage]
            for stage in [0, 1, 2]
        }
        assert all(stable.values()), {"states": states, "log": text[-3000:]}
        assert stable[0][-1] == stable[2][-1] == 6 and stable[1][-1] in [2, 4], stable
        bits = [
            list(map(int, values))
            for values in re.findall(r"HAMMERR_PORTAL_BITS (\d+) (\d+)", text)
        ]
        portal_states = {
            stage: [mask & 2 for phase, mask in bits if phase == stage]
            for stage in [0, 1, 2]
        }
        assert all(portal_states.values()), portal_states
        assert [portal_states[stage][-1] for stage in [0, 1, 2]] == [2, 0, 2], (
            portal_states
        )
        (args.output / "area-states.json").write_text(
            json.dumps({"areas": stable, "client_portals": portal_states}, indent=2)
            + "\n"
        )
        return
    bsp = fixture(args)
    if args.runtime:
        env = {
            **os.environ,
            "WLR_BACKENDS": "headless",
            "WLR_HEADLESS_OUTPUTS": "1",
            "WLR_RENDERER": "pixman",
        }
        for view in ["left", "right"]:
            output = args.output / f"runtime-{view}"
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
                        "--view",
                        view,
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
    print(json.dumps({"output": str(args.output), "areaportals": True}))


if __name__ == "__main__":
    main()
