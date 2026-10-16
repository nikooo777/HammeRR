import argparse
import hashlib
import json
import math
import sys
from pathlib import Path
from unittest.mock import patch

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_terrain_lod import build
from validate_visibility import ROOM

MODEL = "models/props_junk/wood_crate001a.mdl"
DOORS = {"blocked_door": [-192, -176, 40], "clear_door": [64, -176, 40]}
ROTORS = {"contact_rotor": [-192, 64, 40], "control_rotor": [64, 64, 40]}
PROPS = {
    "blocked_crate": [-128, -112, 24],
    "contact_crate": [-128, 112, 24],
    "physics_crate": [128, 112, 24],
}
OUTPUTS = (
    "OnOpen",
    "OnClose",
    "OnFullyOpen",
    "OnFullyClosed",
    "OnBlockedOpening",
    "OnBlockedClosing",
    "OnUnblockedOpening",
    "OnUnblockedClosing",
)
NAMES = [*DOORS, *ROTORS, *PROPS]

CONTACT_GLOBALS = '''
contact_start <- null;
contact_phase <- 0;
rotors_stopped <- false;
function HammeRRContactEvent(name, event) {
    local elapsed = contact_start == null ? -1.0 : Time() - contact_start;
    local ent = Entities.FindByName(null, name);
    local yaw = ent == null ? 0.0 : ent.GetAngles().y;
    printl("HAMMERR_ROT_EVENT " + elapsed + " " + contact_phase + " " + name + " " + event + " " + yaw);
}
'''

CONTACT_THINK = '''
    if (stage == 2) {
        if (contact_start == null) contact_start = Time();
        local elapsed = Time() - contact_start;
        local next = elapsed >= 24 ? 4 : elapsed >= 16 ? 3 : elapsed >= 12 ? 2 : elapsed >= 4 ? 1 : 0;
        if (next > contact_phase) {
            contact_phase = next;
            printl("HAMMERR_ROT_PHASE " + elapsed + " " + next);
            if (next == 1) {
                EntFire("blocked_door", "Open");
                EntFire("clear_door", "Open");
                EntFire("contact_rotor", "StartForward");
                EntFire("control_rotor", "StartForward");
            } else if (next == 2) {
                EntFire("clear_door", "Close");
                EntFire("crate_anchor", "Break");
            } else if (next == 3) {
                EntFire("blocked_door", "Open");
                EntFire("clear_door", "Open");
            } else {
                EntFire("blocked_door", "Close");
                EntFire("clear_door", "Close");
            }
        }
        if (!rotors_stopped && elapsed >= 8) {
            EntFire("contact_rotor", "Stop");
            EntFire("control_rotor", "Stop");
            rotors_stopped = true;
            printl("HAMMERR_ROT_STOP " + elapsed);
        }
        foreach (name in __NAMES__) {
            local ent = Entities.FindByName(null, name);
            if (ent == null) {
                printl("HAMMERR_ROT_MISSING " + elapsed + " " + name);
                continue;
            }
            local p = ent.GetOrigin();
            local a = ent.GetAngles();
            local v = ent.GetVelocity();
            printl("HAMMERR_ROT_SAMPLE " + elapsed + " " + contact_phase + " " + name + " " + p.x + " " + p.y + " " + p.z + " " + a.x + " " + a.y + " " + a.z + " " + v.x + " " + v.y + " " + v.z);
        }
    }
    return 0.1;
'''.replace("__NAMES__", json.dumps(NAMES))


def observer():
    source = runtime.OBSERVER
    for marker in [
        "stage <- 0;",
        "    return 1.0;\n}",
        "sv_cheats 1; joystick 0;",
        "setpos -64 -64 32; setang 25 45 0; mat_fullbright",
    ]:
        assert source.count(marker) == 1, marker
    return (
        source.replace("stage <- 0;", "stage <- 0;\n" + CONTACT_GLOBALS)
        .replace("    return 1.0;\n}", CONTACT_THINK + "\n}")
        .replace(
            "sv_cheats 1; joystick 0;",
            "sv_cheats 1; joystick 0; bot_quota 0; bot_kick;",
        )
        .replace(
            "setpos -64 -64 32; setang 25 45 0; mat_fullbright",
            "setpos -224 -224 32; setang 20 45 0; mat_fullbright",
        )
    )


def connection(client, entity, output, name):
    token = client.call("begin_connection_edit", {"id": entity})["token"]
    client.call(
        "update_connection_draft",
        {
            "token": token,
            "output": output,
            "target": "hammerr_observer",
            "input": "RunScriptCode",
            "parameter": f'HammeRRContactEvent("{name}", "{output}")',
            "delay": "0",
            "times": "-1",
        },
    )
    client.call("commit_connection_edit", {"token": token})


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fixture(args):
    with patch.dict(
        "os.environ",
        {
            "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent-maps"),
            "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
        },
    ):
        client = Client(
            None,
            args.binary,
            args.output,
            gameinfo=args.game_dir / "cstrike/gameinfo.txt",
        )
    try:
        ids = []
        for low, high in ROOM:
            ids.append(
                client.call(
                    "create_block",
                    {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
                )["id"]
            )
        for classname, origin, properties in [
            ("info_player_terrorist", [-224, -224, 8], {}),
            ("info_player_counterterrorist", [224, -224, 8], {}),
            (
                "light",
                [0, 0, 208],
                {
                    "_light": "255 255 255 200",
                    "_constant_attn": "1",
                    "_quadratic_attn": "0",
                },
            ),
            (
                "logic_script",
                [-224, -224, 64],
                {
                    "targetname": "hammerr_observer",
                    "vscripts": "hammerr_observer.nut",
                    "thinkfunction": "HammeRRThink",
                },
            ),
            *[
                (
                    "prop_physics",
                    origin,
                    {
                        "targetname": name,
                        "model": MODEL,
                        "physdamagescale": "0",
                        "spawnflags": "0",
                    },
                )
                for name, origin in PROPS.items()
            ],
        ]:
            ids.append(
                client.call(
                    "create_entity",
                    {"classname": classname, "origin": origin, "properties": properties},
                )["id"]
            )
        for name, origin in {**DOORS, **ROTORS}.items():
            x, y, z = origin
            brush = client.call(
                "create_block",
                {
                    "mins": [x, y - 8, 0],
                    "maxs": [x + 96, y + 8, 80],
                    "material": "dev/dev_measuregeneric01",
                },
            )["id"]
            properties = {"targetname": name, "origin": f"{x} {y} {z}", "dmg": "0"}
            if name in DOORS:
                classname = "func_door_rotating"
                properties.update(
                    distance="90",
                    speed="30",
                    wait="4",
                    forceclosed="0",
                    ignoredebris="0",
                    spawnflags="48",
                    solidbsp="0",
                )
            else:
                classname = "func_rotating"
                properties.update(
                    maxspeed="30",
                    fanfriction="100",
                    solidbsp="0",
                    spawnflags="64" if name == "control_rotor" else "0",
                )
            entity = client.call(
                "tie_to_entity",
                {"ids": [brush], "classname": classname, "properties": properties},
            )["id"]
            ids.append(entity)
            if name in DOORS:
                for output in OUTPUTS:
                    connection(client, entity, output, name)
        anchor = client.call(
            "create_entity",
            {
                "classname": "phys_constraint",
                "origin": PROPS["blocked_crate"],
                "properties": {
                    "targetname": "crate_anchor",
                    "attach1": "blocked_crate",
                    "attach2": "",
                    "forcelimit": "0",
                    "torquelimit": "0",
                    "spawnflags": "0",
                },
            },
        )["id"]
        ids.append(anchor)
        connection(client, anchor, "OnBreak", "crate_anchor")
        client.call(
            "set_compile_settings",
            {"lighting": "direct_ldr", "physics": "engine", "visibility": "fast"},
        )
        vmf = args.output / "rotating-contacts.vmf"
        client.call("save_map", {"path": str(vmf)})
        before = detail(client, ids)
        source_hash = digest(vmf)
        history = client.call("get_map_info")["undo"]
        selection = client.call("get_selection")
        camera = client.call("get_camera")
        bsp, stats = build(client, args.output, "rotating-contacts")
        assert stats["models"] == stats["physics_models"] == 5, stats
        assert len(records(bsp.read_bytes(), 14, "<9f3i")) == 5
        assert digest(vmf) == source_hash and detail(client, ids) == before
        assert client.call("get_map_info")["undo"] == history
        assert client.call("get_selection") == selection
        assert client.call("get_camera") == camera
        report = {
            "schema": 1,
            "bsp": str(bsp),
            "bsp_sha256": digest(bsp),
            "source": str(vmf),
            "source_sha256": source_hash,
            "stats": stats,
            "shared_builds_identical": True,
            "source_history_selection_camera_preserved": True,
            "runtime_checked": False,
            "desktop_input": False,
        }
        (args.output / "compile.json").write_text(json.dumps(report, indent=2) + "\n")
        return report
    finally:
        client.close()


def angle_delta(a, b):
    return (a - b + 180) % 360 - 180


def distance(a, b):
    return math.dist(a, b)


def check(args):
    text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
    samples, events, phases = [], [], []
    for line in text.splitlines():
        words = line.strip().split()
        if not words:
            continue
        if words[0] == "HAMMERR_ROT_SAMPLE":
            assert len(words) == 13, words
            values = [float(value) for value in words[4:]]
            assert all(math.isfinite(value) for value in values), words
            samples.append(
                {
                    "time": float(words[1]),
                    "phase": int(words[2]),
                    "name": words[3],
                    "position": values[:3],
                    "angles": values[3:6],
                    "velocity": values[6:],
                }
            )
        elif words[0] == "HAMMERR_ROT_EVENT":
            assert len(words) == 6, words
            events.append(
                {
                    "time": float(words[1]),
                    "phase": int(words[2]),
                    "name": words[3],
                    "output": words[4],
                    "yaw": float(words[5]),
                }
            )
        elif words[0] == "HAMMERR_ROT_PHASE":
            phases.append([float(words[1]), int(words[2])])
    observations = {"samples": samples, "events": events, "phases": phases}
    (args.output / "observations.json").write_text(
        json.dumps(observations, indent=2) + "\n"
    )
    assert "HAMMERR_ROT_MISSING" not in text and samples, text[-6000:]
    assert [phase for _, phase in phases] == [1, 2, 3, 4], phases
    assert samples[-1]["time"] >= 30, samples[-1]

    def rows(name, phase):
        found = [row for row in samples if row["name"] == name and row["phase"] == phase]
        assert len(found) >= 10, (name, phase, found)
        return found

    def output(name, event, phase):
        return [
            row for row in events
            if row["name"] == name and row["output"] == event and row["phase"] == phase
        ]

    for name, origin in {**DOORS, **ROTORS}.items():
        for row in [row for row in samples if row["name"] == name]:
            assert distance(row["position"], origin) < 0.05, row
            assert abs(angle_delta(row["angles"][0], 0)) < 0.05, row
            assert abs(angle_delta(row["angles"][2], 0)) < 0.05, row
        assert all(
            abs(angle_delta(row["angles"][1], 0)) < 0.05 for row in rows(name, 0)
        ), name

    blocked = rows("blocked_door", 1)
    peak = max(abs(angle_delta(row["angles"][1], 0)) for row in blocked)
    assert 5 < peak < 85, ("No partial-angle obstruction", peak)
    closing = output("blocked_door", "OnClose", 1)
    assert closing and all(
        5 < abs(angle_delta(row["yaw"], 0)) < 85 for row in closing
    ), closing
    closed = output("blocked_door", "OnFullyClosed", 1)
    assert closed and closing[0]["time"] < closed[-1]["time"], events
    assert not output("blocked_door", "OnFullyOpen", 1)
    assert not [
        row for row in events
        if row["output"].startswith(("OnBlocked", "OnUnblocked"))
    ], events
    anchored = rows("blocked_crate", 0)[-1]["position"]
    assert distance(anchored, PROPS["blocked_crate"]) < 1, anchored
    assert all(
        distance(row["position"], anchored) < 1 for row in rows("blocked_crate", 1)
    ), anchored
    assert len(output("crate_anchor", "OnBreak", 2)) == 1, events
    assert not [
        row for row in events if row["name"] == "crate_anchor" and row["phase"] != 2
    ], events

    for phase in range(5):
        for name in DOORS:
            expected = 90 if phase == 3 or name == "clear_door" and phase == 1 else 0
            assert all(
                abs(angle_delta(row["angles"][1], expected)) < 0.1
                for row in rows(name, phase)[-10:]
            ), (name, phase)
    for name in DOORS:
        for phase in [1, 3]:
            assert len(output(name, "OnOpen", phase)) == 1, events
        assert output(name, "OnFullyOpen", 3), events
        assert output(name, "OnFullyClosed", 4), events
    assert output("clear_door", "OnFullyOpen", 1), events
    assert output("clear_door", "OnFullyClosed", 2), events
    assert distance(rows("blocked_crate", 3)[-1]["position"][:2], anchored[:2]) > 12

    control = rows("physics_crate", 0)[-1]["position"]
    contact = rows("contact_crate", 0)[-1]["position"]
    assert abs(control[2] - contact[2]) < 0.5 and 19 < control[2] < 22, (control, contact)
    for name, baseline in [("physics_crate", control), ("contact_crate", contact)]:
        assert all(
            distance(row["position"], baseline) < 0.5 for row in rows(name, 0)[-10:]
        ), (name, baseline)
    assert all(
        distance(row["position"], control) < 1
        for row in samples if row["name"] == "physics_crate" and row["time"] >= 3
    ), control
    moved = [row for row in rows("contact_crate", 1) if 4.5 <= row["time"] <= 8.5]
    assert any(row["position"][1] - contact[1] > 12 for row in moved), moved
    assert any(math.hypot(*row["velocity"][:2]) > 4 for row in moved), moved
    for name in ROTORS:
        final = rows(name, 1)[-10:]
        assert all(abs(angle_delta(row["angles"][1], 120)) < 6 for row in final), final
        yaw = final[-1]["angles"][1]
        assert all(
            abs(angle_delta(row["angles"][1], yaw)) < 0.1
            for phase in [2, 3, 4] for row in rows(name, phase)
        ), name
    assert abs(angle_delta(
        rows("contact_rotor", 1)[-1]["angles"][1],
        rows("control_rotor", 1)[-1]["angles"][1],
    )) < 1
    preservation = json.loads((args.output / "private-preservation.json").read_text())
    assert preservation["success"], preservation
    report = {
        "runtime_checked": True,
        "anchored_dynamic_prop_reverses_door": True,
        "blocked_peak_degrees": peak,
        "physics_blocked_outputs": "absent; VPhysics reconciliation calls Blocked directly",
        "constraint_break_output": True,
        "released_prop_allows_full_open_and_close": True,
        "solid_rotor_pushes_dynamic_prop": True,
        "nonsolid_rotor_control_stable": True,
        "start_forward_and_stop": True,
        "private_launch": json.loads((args.output / "private-launch.json").read_text()),
        "private_preservation": True,
        "desktop_input": False,
    }
    (args.output / "rotating-contacts.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


def main():
    parser = argparse.ArgumentParser(description="Compile rotating-contact controls and optionally observe private native contact")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--reuse-fixture", type=Path)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.game_dir = args.game_dir.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        runtime.OBSERVER = observer()
        args.observe_seconds = 48
        runtime.worker(args)
        check(args)
        return
    if args.reuse_fixture:
        assert args.runtime, "--reuse-fixture requires --runtime"
        compiled = json.loads(args.reuse_fixture.read_text())
        assert compiled["schema"] == 1
        for kind in ["bsp", "source"]:
            assert digest(Path(compiled[kind])) == compiled[f"{kind}_sha256"]
    else:
        compiled = fixture(args)
    report = {**compiled, "runtime_checked": False}
    if args.runtime:
        output = args.output / "runtime"
        command = [
            sys.executable, "-B", str(Path(__file__).resolve()), "--worker",
            "--binary", str(args.binary), "--bsp", compiled["bsp"],
            "--game-dir", str(args.game_dir), "--output", str(output),
        ]
        runtime.private_compositor(command, output, timeout=90)
        report["native"] = json.loads((output / "rotating-contacts.json").read_text())
        report["runtime_checked"] = True
        for kind in ["bsp", "source"]:
            assert digest(Path(compiled[kind])) == compiled[f"{kind}_sha256"]
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"output": str(args.output), "runtime_checked": report["runtime_checked"]}))


if __name__ == "__main__":
    main()
