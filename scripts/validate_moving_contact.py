import argparse
import json
import re
from pathlib import Path
from unittest.mock import patch

import validate_native_runtime as runtime
from validate_face_lighting import ROOT, Client, detail
from validate_terrain_lod import build
from validate_visibility import ROOM

OBSERVER = (
    runtime.OBSERVER.replace(
        "stage <- 0;",
        '''stage <- 0;
contact_start <- null;
contact_phase <- 0;
function HammeRROpened() { printl("HAMMERR_CONTACT_OPENED"); }
function HammeRRClosed() { printl("HAMMERR_CONTACT_CLOSED"); }''',
    )
    .replace("sv_cheats 1; joystick 0;", "sv_cheats 1; joystick 0; bot_quota 0; bot_kick;")
    .replace("setpos -64 -64 32; setang 25 45 0; mat_fullbright", "setpos -96 -128 32; setang 10 45 0; mat_fullbright")
    .replace("    return 1.0;", '''
    if (stage == 2) {
        if (contact_start == null) contact_start = Time();
        local elapsed = Time() - contact_start;
        local next = elapsed >= 24 ? 4 : elapsed >= 18 ? 3 : elapsed >= 12 ? 2 : elapsed >= 6 ? 1 : 0;
        if (next > contact_phase) {
            if (next == 1) EntFire("contact_lift", "Open");
            else if (next == 2) EntFire("contact_lift", "Close");
            else if (next == 3) {
                EntFire("contact_lift", "SetSpeed", "64");
                EntFire("contact_lift", "SetPosition", "1", 0.1);
            } else EntFire("contact_lift", "SetPosition", "0");
            contact_phase = next;
        }
        local lift = Entities.FindByName(null, "contact_lift");
        local crate = Entities.FindByName(null, "physics_crate");
        if (lift != null && crate != null) {
            local a = lift.GetOrigin();
            local b = crate.GetOrigin();
            printl("HAMMERR_CONTACT " + contact_phase + " " + a.x + " " + a.y + " " + a.z + " " + b.x + " " + b.y + " " + b.z);
        }
    }
    return 1.0;
''')
)


def fixture(args):
    with patch.dict("os.environ", {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent-maps"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json")}):
        client = Client(None, args.binary, args.output)
    try:
        ids = []
        for low, high in ROOM:
            ids.append(client.call("create_block", {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"})["id"])
        for classname, origin, properties in [
            ("info_player_terrorist", [-96, -128, 8], {}),
            ("info_player_counterterrorist", [-192, 128, 8], {}),
            ("light", [0, 0, 192], {"_light": "255 255 255 200", "_constant_attn": "1", "_quadratic_attn": "0"}),
            ("logic_script", [-128, -128, 64], {"targetname": "hammerr_observer", "vscripts": "hammerr_observer.nut", "thinkfunction": "HammeRRThink"}),
            ("prop_physics", [96, 0, 96], {"targetname": "physics_crate", "model": "models/props_junk/wood_crate001a.mdl"}),
        ]:
            ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
        brush = client.call("create_block", {"mins": [32, -64, 0], "maxs": [160, 64, 16], "material": "dev/dev_measuregeneric01"})["id"]
        lift = client.call("tie_to_entity", {"ids": [brush], "classname": "func_movelinear", "properties": {"targetname": "contact_lift", "movedir": "-90 0 0", "movedistance": "64", "speed": "32", "startposition": "0", "blockdamage": "0"}})["id"]
        ids.append(lift)
        for output, function in [("OnFullyOpen", "HammeRROpened()"), ("OnFullyClosed", "HammeRRClosed()")]:
            token = client.call("begin_connection_edit", {"id": lift})["token"]
            client.call("update_connection_draft", {"token": token, "output": output, "target": "hammerr_observer", "input": "RunScriptCode", "parameter": function, "delay": "0", "times": "-1"})
            client.call("commit_connection_edit", {"token": token})
        client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"})
        source = args.output / "moving-contact.vmf"
        client.call("save_map", {"path": str(source)})
        saved = source.read_bytes()
        before = detail(client, ids)
        history = client.call("get_map_info")["undo"]
        path, stats = build(client, args.output, "moving-contact")
        assert stats["models"] == 2 and stats["physics_models"] == 2, stats
        assert source.read_bytes() == saved and detail(client, ids) == before and client.call("get_map_info")["undo"] == history
        (args.output / "compile.json").write_text(json.dumps(stats, indent=2) + "\n")
        return path
    finally:
        client.close()


def check(args):
    text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
    rows = [[float(value) for value in match] for match in re.findall(r"HAMMERR_CONTACT (\d+) ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+)", text)]
    phases = [[row for row in rows if row[0] == phase][-3:] for phase in range(5)]
    assert all(len(phase) == 3 for phase in phases), phases
    base = phases[0][-1]
    for index, phase in enumerate(phases):
        expected = 64 if index in [1, 3] else 0
        for row in phase:
            assert abs(row[3] - base[3] - expected) < 0.05, row
            assert abs(row[6] - base[6] - expected) < 0.5, row
            assert abs((row[6] - row[3]) - (base[6] - base[3])) < 0.5, row
            assert abs(row[4] - base[4]) < 1 and abs(row[5] - base[5]) < 1, row
    assert text.count("HAMMERR_CONTACT_OPENED") >= 2 and text.count("HAMMERR_CONTACT_CLOSED") >= 2, text[-8000:]
    assert '"sv_lan" = "1"' in text
    result = {"stable_phases": phases, "rise_units": 64, "open_close_and_position_inputs": True, "carried_physics_prop": True, "outputs_fired": True, "insecure": True, "sv_lan": True}
    (args.output / "moving-contact.json").write_text(json.dumps(result, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        runtime.OBSERVER = OBSERVER
        args.observe_seconds = 45
        runtime.worker(args)
        check(args)
        return
    bsp = fixture(args)
    if args.runtime:
        output = runtime.private_display(Path(__file__), args, bsp)
        print((output / "moving-contact.json").read_text())
    else:
        print(json.dumps({"compiled_moving_contact": True, "runtime_checked": False}))


if __name__ == "__main__":
    main()
