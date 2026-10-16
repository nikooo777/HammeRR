import argparse
import hashlib
import json
import math
import sys
from functools import partial
from pathlib import Path
from unittest.mock import patch

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_terrain_lod import build
from validate_visibility import ROOM

MODEL = "models/props_junk/wood_crate001a.mdl"
FIXTURE_SCHEMA = 5
EXPERIMENTAL_SCHEMA = 6
DISABLE_WAKE_RELEASE = "disable_braces_and_wake_same_crate_before_retry"
DISABLE_SCHEMA = 7
DISABLE_RELEASE = "disable_both_perpendicular_braces_without_wake"
REFRESH_SCHEMA = 8
REFRESH_RELEASE = "disable_braces_and_recheck_same_crate_contacts_once"
SETTLED_SAMPLE_COUNT = 5
SETTLED_SECONDS = 0.5
MOVETYPE_VPHYSICS = 6
MOVETYPE_PUSH = 7
DOORS = {"blocked_door": [-192, -176, 40], "clear_door": [64, -176, 40]}
ROTORS = {"contact_rotor": [-192, 64, 40], "control_rotor": [64, 64, 40]}
PROPS = {
    "blocked_crate": [-128, -112, 24],
    "contact_crate": [-128, 112, 24],
    "physics_crate": [128, 112, 24],
}
BRACE_OFFSETS = {
    "side_brace": ([32, 36, 0], [43, 96, 96]),
    "back_brace": ([43, 85, 0], [96, 96, 96]),
}
BRACES = [f"{name}_{kind}" for name in DOORS for kind in BRACE_OFFSETS]
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

CONTACT_GLOBALS = """
contact_start <- null;
contact_phase <- 0;
rotors_stopped <- false;
function HammeRRContactEvent(name, event) {
    local elapsed = contact_start == null ? -1.0 : Time() - contact_start;
    local ent = Entities.FindByName(null, name);
    local yaw = ent == null ? 0.0 : ent.GetAngles().y;
    printl("HAMMERR_ROT_EVENT " + elapsed + " " + contact_phase + " " + name + " " + event + " " + yaw);
}
"""

CONTACT_THINK = """
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
                EntFire("release_braces", "Trigger");
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
            local v = ent.GetPhysVelocity();
            printl("HAMMERR_ROT_SAMPLE " + elapsed + " " + contact_phase + " " + name + " " + p.x + " " + p.y + " " + p.z + " " + a.x + " " + a.y + " " + a.z + " " + v.x + " " + v.y + " " + v.z + " " + ent.GetHealth());
            local lo = ent.GetBoundingMinsOriented();
            local hi = ent.GetBoundingMaxsOriented();
            printl("HAMMERR_ROT_BOUNDS " + elapsed + " " + contact_phase + " " + name + " " + ent.GetEntityIndex() + " " + lo.x + " " + lo.y + " " + lo.z + " " + hi.x + " " + hi.y + " " + hi.z);
            printl("HAMMERR_ROT_STATE " + elapsed + " " + contact_phase + " " + name + " " + ent.GetMoveType() + " " + (ent.IsSolid() ? 1 : 0));
        }
        foreach (name in __BRACES__) {
            local brace = Entities.FindByName(null, name);
            if (brace == null) {
                if (contact_phase < 2) printl("HAMMERR_ROT_MISSING " + elapsed + " " + name);
                printl("HAMMERR_ROT_BRACE " + elapsed + " " + contact_phase + " " + name + " 0 absent");
                continue;
            }
            printl("HAMMERR_ROT_BRACE " + elapsed + " " + contact_phase + " " + name + " 1 " + (brace.IsSolid() ? 1 : 0));
        }
    }
    return 0.1;
""".replace("__NAMES__", json.dumps(NAMES)).replace("__BRACES__", json.dumps(BRACES))


def callback_name(name, output):
    return f"HammeRRContact_{name}_{output}"


def observer(release_mode="kill"):
    source = runtime.OBSERVER
    callbacks = [(name, output) for name in DOORS for output in OUTPUTS]
    callbacks.extend((name, "OnBreak") for name in PROPS)
    callbacks.append(("release_braces", "OnTrigger"))
    functions = "\n".join(
        f'function {callback_name(name, output)}() {{ HammeRRContactEvent("{name}", "{output}"); }}'
        for name, output in callbacks
    )
    for marker in [
        "stage <- 0;",
        "    return 1.0;\n}",
        "sv_cheats 1; joystick 0;",
        "setpos -64 -64 32; setang 25 45 0; mat_fullbright",
    ]:
        assert source.count(marker) == 1, marker
    think = CONTACT_THINK
    if release_mode == "disable-wake":
        marker = '            } else if (next == 3) {\n'
        assert think.count(marker) == 1
        think = think.replace(
            marker,
            marker
            + '                EntFire("blocked_crate", "Wake");\n'
            + '                printl("HAMMERR_ROT_WAKE " + elapsed + " " + contact_phase + " blocked_crate");\n',
        )
    return (
        source.replace("stage <- 0;", "stage <- 0;\n" + CONTACT_GLOBALS + functions)
        .replace("    return 1.0;\n}", think + "\n}")
        .replace(
            "sv_cheats 1; joystick 0;",
            "sv_cheats 1; joystick 0; bot_quota 0; bot_kick;",
        )
        .replace(
            "setpos -64 -64 32; setang 25 45 0; mat_fullbright",
            "setpos -224 -224 32; setang 20 45 0; mat_fullbright",
        )
    )


def connection(
    client,
    entity,
    output,
    name,
    *,
    target="hammerr_observer",
    input_name="RunScriptCode",
):
    token = client.call("begin_connection_edit", {"id": entity})["token"]
    client.call(
        "update_connection_draft",
        {
            "token": token,
            "output": output,
            "target": target,
            "input": input_name,
            "parameter": f"{callback_name(name, output)}()"
            if input_name == "RunScriptCode"
            else "",
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
            (
                "filter_activator_class",
                [-224, -192, 64],
                {
                    "targetname": "contact_no_damage",
                    "filterclass": "*",
                    "Negated": "1",
                },
            ),
            *[
                (
                    "prop_physics",
                    origin,
                    {
                        "targetname": name,
                        "model": MODEL,
                        "damagefilter": "contact_no_damage",
                        "spawnflags": "0",
                    },
                )
                for name, origin in PROPS.items()
            ],
        ]:
            entity = client.call(
                "create_entity",
                {
                    "classname": classname,
                    "origin": origin,
                    "properties": properties,
                },
            )["id"]
            ids.append(entity)
            if classname == "prop_physics":
                name = properties["targetname"]
                connection(client, entity, "OnBreak", name)
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
        for door, origin in DOORS.items():
            x, y, _ = origin
            for kind, (low, high) in BRACE_OFFSETS.items():
                name = f"{door}_{kind}"
                brush = client.call(
                    "create_block",
                    {
                        "mins": [x + low[0], y + low[1], low[2]],
                        "maxs": [x + high[0], y + high[1], high[2]],
                        "material": "dev/dev_measuregeneric01",
                    },
                )["id"]
                ids.append(
                    client.call(
                        "tie_to_entity",
                        {
                            "ids": [brush],
                            "classname": "func_brush",
                            "properties": {
                                "targetname": name,
                                "StartDisabled": "0",
                                "Solidity": "0",
                                "solidbsp": "0",
                            },
                        },
                    )["id"]
                )
        relay = client.call(
            "create_entity",
            {
                "classname": "logic_relay",
                "origin": [-224, -160, 64],
                "properties": {"targetname": "release_braces", "StartDisabled": "0"},
            },
        )["id"]
        ids.append(relay)
        for name in BRACES:
            connection(
                client,
                relay,
                "OnTrigger",
                "release_braces",
                target=name,
                input_name="Disable" if args.release_mode != "kill" else "Kill",
            )
        connection(client, relay, "OnTrigger", "release_braces")
        client.call(
            "set_compile_settings",
            {"lighting": "direct_ldr", "physics": "engine", "visibility": "fast"},
        )
        vmf = args.output / "rotating-removal-recovery.vmf"
        client.call("save_map", {"path": str(vmf)})
        before = detail(client, ids)
        source_hash = digest(vmf)
        history = client.call("get_map_info")["undo"]
        selection = client.call("get_selection")
        camera = client.call("get_camera")
        bsp, stats = build(client, args.output, "rotating-removal-recovery")
        assert stats["models"] == stats["physics_models"] == 9, stats
        assert len(records(bsp.read_bytes(), 14, "<9f3i")) == 9
        assert digest(vmf) == source_hash and detail(client, ids) == before
        assert client.call("get_map_info")["undo"] == history
        assert client.call("get_selection") == selection
        assert client.call("get_camera") == camera
        report = {
            "schema": FIXTURE_SCHEMA,
            "obstruction": "movable_prop_braced_by_removable_brushes",
            "release": "remove_both_perpendicular_braces_with_kill",
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
        if args.release_mode == "disable-wake":
            report["schema"] = EXPERIMENTAL_SCHEMA
            report["release"] = DISABLE_WAKE_RELEASE
        elif args.release_mode == "disable":
            report["schema"] = DISABLE_SCHEMA
            report["release"] = DISABLE_RELEASE
        elif args.release_mode == "disable-refresh":
            report["schema"] = REFRESH_SCHEMA
            report["release"] = REFRESH_RELEASE
        (args.output / "compile.json").write_text(json.dumps(report, indent=2) + "\n")
        return report
    finally:
        client.close()


def angle_delta(a, b):
    return (a - b + 180) % 360 - 180


def distance(a, b):
    return math.dist(a, b)


def check(args):
    release_mode = getattr(args, "release_mode", "kill")
    experimental = release_mode != "kill"
    text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
    samples, events, phases, braces, bounds, states = [], [], [], [], [], []
    wakes = []
    for line in text.splitlines():
        words = line.strip().split()
        if not words:
            continue
        if words[0] == "HAMMERR_ROT_SAMPLE":
            assert len(words) == 14, words
            values = [float(value) for value in words[4:]]
            assert all(math.isfinite(value) for value in values), words
            samples.append(
                {
                    "time": float(words[1]),
                    "phase": int(words[2]),
                    "name": words[3],
                    "position": values[:3],
                    "angles": values[3:6],
                    "velocity": values[6:9],
                    "health": values[9],
                }
            )
        elif words[0] == "HAMMERR_ROT_BOUNDS":
            assert len(words) == 11, words
            values = [float(value) for value in words[5:]]
            assert all(math.isfinite(value) for value in values), words
            assert all(low < high for low, high in zip(values[:3], values[3:])), words
            bounds.append(
                {
                    "time": float(words[1]),
                    "phase": int(words[2]),
                    "name": words[3],
                    "entity_index": int(words[4]),
                    "mins_oriented": values[:3],
                    "maxs_oriented": values[3:],
                }
            )
        elif words[0] == "HAMMERR_ROT_STATE":
            assert len(words) == 6 and words[5] in ["0", "1"], words
            states.append(
                {
                    "time": float(words[1]),
                    "phase": int(words[2]),
                    "name": words[3],
                    "move_type": int(words[4]),
                    "solid": words[5] == "1",
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
        elif words[0] == "HAMMERR_ROT_BRACE":
            assert len(words) == 6 and words[4] in ["0", "1"], words
            present = words[4] == "1"
            assert words[5] in (["0", "1"] if present else ["absent"]), words
            braces.append(
                {
                    "time": float(words[1]),
                    "phase": int(words[2]),
                    "name": words[3],
                    "present": present,
                    "solid": words[5] == "1" if present else None,
                }
            )
        elif words[0] == "HAMMERR_ROT_WAKE":
            assert len(words) == 4, words
            wakes.append(
                {"time": float(words[1]), "phase": int(words[2]), "name": words[3]}
            )
    observations = {
        "samples": samples,
        "events": events,
        "phases": phases,
        "braces": braces,
        "bounds": bounds,
        "states": states,
    }
    if experimental:
        observations["wakes"] = wakes
    (args.output / "observations.json").write_text(
        json.dumps(observations, indent=2) + "\n"
    )
    assert "HAMMERR_ROT_MISSING" not in text and samples, text[-6000:]
    assert [phase for _, phase in phases] == [1, 2, 3, 4], phases
    assert samples[-1]["time"] >= 30, samples[-1]
    assert len(bounds) == len(samples), (len(bounds), len(samples))
    assert [(row["time"], row["name"]) for row in bounds] == [
        (row["time"], row["name"]) for row in samples
    ]
    assert [(row["time"], row["name"]) for row in states] == [
        (row["time"], row["name"]) for row in samples
    ]
    if release_mode == "disable-wake":
        assert len(wakes) == 1 and wakes[0]["name"] == "blocked_crate"
        assert wakes[0]["phase"] == 3 and wakes[0]["time"] == phases[2][0], wakes
    elif release_mode in {"disable", "disable-refresh"}:
        assert not wakes, "This Disable mode must not queue Wake"

    def rows(name, phase, source=None):
        source = samples if source is None else source
        found = [row for row in source if row["name"] == name and row["phase"] == phase]
        assert len(found) >= 10, (name, phase, found)
        return found

    def output(name, event, phase):
        return [
            row
            for row in events
            if row["name"] == name and row["output"] == event and row["phase"] == phase
        ]

    def settled_door(name, phase, expected):
        found = rows(name, phase)
        if phase == 0:
            return found[-10:]
        completed_phase = 1 if phase == 2 and name == "blocked_door" else phase
        event = "OnFullyOpen" if expected == 90 else "OnFullyClosed"
        completed = output(name, event, completed_phase)
        assert completed, (name, phase, event, events)
        assert all(abs(angle_delta(row["yaw"], expected)) < 0.1 for row in completed)
        found = [row for row in found if row["time"] >= completed[-1]["time"]]
        assert len(found) >= SETTLED_SAMPLE_COUNT, (name, phase, completed, found)
        assert found[-1]["time"] - found[0]["time"] >= SETTLED_SECONDS, (
            name,
            phase,
            completed,
            found,
        )
        return found

    indices = {}
    for name in NAMES:
        values = {row["entity_index"] for row in bounds if row["name"] == name}
        assert len(values) == 1 and min(values) > 0, (name, values)
        indices[name] = values.pop()
    assert len(set(indices.values())) == len(NAMES), indices

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
        row for row in events if row["output"].startswith(("OnBlocked", "OnUnblocked"))
    ], events
    braced = rows("blocked_crate", 0)[-1]["position"]
    assert distance(braced[:2], PROPS["blocked_crate"][:2]) < 1, braced
    assert 19 < braced[2] < 22, braced
    x, y, _ = DOORS["blocked_door"]
    assert all(
        x + BRACE_OFFSETS["side_brace"][1][0]
        < row["position"][0]
        < x + BRACE_OFFSETS["back_brace"][1][0]
        and y + BRACE_OFFSETS["side_brace"][0][1]
        < row["position"][1]
        < y + BRACE_OFFSETS["back_brace"][0][1]
        for row in rows("blocked_crate", 1)
    ), braced
    assert len(output("release_braces", "OnTrigger", 2)) == 1, events
    assert not [
        row for row in events if row["name"] == "release_braces" and row["phase"] != 2
    ], events
    for name in BRACES:
        for phase in range(5):
            found = rows(name, phase, braces)
            if phase < 2:
                assert all(
                    row["present"] and row["solid"] is True for row in found
                ), (name, phase)
            elif experimental:
                release = output("release_braces", "OnTrigger", 2)[0]["time"]
                after_disable = [row for row in found if row["time"] >= release + 1]
                assert len(after_disable) >= 10 and all(
                    row["present"] and row["solid"] is False
                    for row in after_disable
                ), (name, phase)
            else:
                release = output("release_braces", "OnTrigger", 2)[0]["time"]
                after_removal = [row for row in found if row["time"] >= release + 1]
                assert len(after_removal) >= 10 and all(
                    not row["present"] and row["solid"] is None
                    for row in after_removal
                ), (name, phase)
    assert not [
        row for row in events if row["name"] in PROPS and row["output"] == "OnBreak"
    ], events
    for name in PROPS:
        health = rows(name, 0)[-1]["health"]
        assert health > 0 and all(
            row["health"] == health for row in samples if row["name"] == name
        ), (name, health)
        assert all(
            row["move_type"] == MOVETYPE_VPHYSICS and row["solid"]
            for row in states
            if row["name"] == name
        ), name
    for name in DOORS:
        assert all(
            row["move_type"] == MOVETYPE_PUSH and row["solid"]
            for row in states
            if row["name"] == name
        ), name

    for phase in range(5):
        for name in DOORS:
            expected = 90 if phase == 3 or name == "clear_door" and phase == 1 else 0
            assert all(
                abs(angle_delta(row["angles"][1], expected)) < 0.1
                for row in settled_door(name, phase, expected)
            ), (name, phase)
    for name in DOORS:
        for phase in [1, 3]:
            assert len(output(name, "OnOpen", phase)) == 1, events
        assert output(name, "OnFullyOpen", 3), events
        assert output(name, "OnFullyClosed", 4), events
    assert output("clear_door", "OnFullyOpen", 1), events
    assert output("clear_door", "OnFullyClosed", 2), events
    assert distance(rows("blocked_crate", 3)[-1]["position"][:2], braced[:2]) > 12
    released = rows("blocked_crate", 2)[-1]["position"]
    assert all(
        distance(row["position"], released) < 0.5
        and math.hypot(*row["velocity"][:2]) < 0.5
        for row in rows("blocked_crate", 2)[-SETTLED_SAMPLE_COUNT:]
    )
    recovery_displacement = distance(
        rows("blocked_crate", 3)[-1]["position"][:2], released[:2]
    )
    assert recovery_displacement > 12
    assert any(math.hypot(*row["velocity"][:2]) > 4 for row in rows("blocked_crate", 3))
    assert any(math.hypot(*row["velocity"][:2]) > 4 for row in rows("blocked_crate", 1))

    control = rows("physics_crate", 0)[-1]["position"]
    contact = rows("contact_crate", 0)[-1]["position"]
    assert abs(control[2] - contact[2]) < 0.5 and 19 < control[2] < 22, (
        control,
        contact,
    )
    for name, baseline in [("physics_crate", control), ("contact_crate", contact)]:
        assert all(
            distance(row["position"], baseline) < 0.5 for row in rows(name, 0)[-10:]
        ), (name, baseline)
    assert all(
        distance(row["position"], control) < 1
        for row in samples
        if row["name"] == "physics_crate" and row["time"] >= 3
    ), control
    moved = [row for row in rows("contact_crate", 1) if 4.5 <= row["time"] <= 8.5]
    assert any(row["position"][1] - contact[1] > 12 for row in moved), moved
    assert any(math.hypot(*row["velocity"][:2]) > 4 for row in moved), moved
    for name in ROTORS:
        final = rows(name, 1)[-10:]
        yaw = final[-1]["angles"][1]
        if name == "control_rotor":
            assert all(abs(angle_delta(row["angles"][1], 120)) < 6 for row in final), (
                final
            )
        else:
            assert angle_delta(yaw, 0) > 0.1, final
        assert all(abs(angle_delta(row["angles"][1], yaw)) < 0.1 for row in final), (
            final
        )
        assert all(
            abs(angle_delta(row["angles"][1], yaw)) < 0.1
            for phase in [2, 3, 4]
            for row in rows(name, phase)
        ), name
    preservation = json.loads((args.output / "private-preservation.json").read_text())
    assert preservation["success"], preservation
    report = {
        "runtime_checked": True,
        "braced_movable_prop_reverses_door": True,
        "blocked_peak_degrees": peak,
        "physics_blocked_outputs": "absent; VPhysics reconciliation calls Blocked directly",
        "authored_brace_removal_and_native_absence": True,
        "release_method": "Kill",
        "disable_contact_refresh_validated": False,
        "released_braces": BRACES,
        "native_oriented_bounds_and_entity_indices": True,
        "entity_indices": indices,
        "door_settling": {
            "anchor": "matching_native_completion_output",
            "minimum_samples": SETTLED_SAMPLE_COUNT,
            "minimum_seconds": SETTLED_SECONDS,
        },
        "prop_health_preserved": True,
        "prop_health": {name: rows(name, 0)[-1]["health"] for name in PROPS},
        "native_crate_remains_vphysics": True,
        "released_crate_settled_before_reopen": released,
        "recovery_horizontal_displacement": recovery_displacement,
        "blocked_crate_peak_horizontal_speed": {
            str(phase): max(
                math.hypot(*row["velocity"][:2])
                for row in rows("blocked_crate", phase)
            )
            for phase in [1, 3]
        },
        "released_prop_allows_full_open_and_close": True,
        "solid_rotor_pushes_dynamic_prop": True,
        "nonsolid_rotor_control_stable": True,
        "start_forward_and_stop": True,
        "rotor_stop_yaws": {name: rows(name, 1)[-1]["angles"][1] for name in ROTORS},
        "private_launch": json.loads((args.output / "private-launch.json").read_text()),
        "private_preservation": True,
        "desktop_input": False,
    }
    if experimental:
        report["authored_brace_removal_and_native_absence"] = False
        report["authored_brace_disable_and_native_presence"] = True
        if release_mode == "disable-wake":
            report["release_method"] = "Disable plus one Wake before retry Open"
            report["single_same_crate_wake_queued"] = wakes[0]
            report["awake_state_observed"] = False
            report["experimental_limits"] = (
                "Wake is queued before Open in the same script tick; no impulse, "
                "teleport or replacement. A pass does not establish a contact-cache "
                "cause or Disable-only recovery. A failure does not prove a hull defect."
            )
        else:
            report["release_method"] = (
                "Disable plus one native RecheckContactPoints"
                if release_mode == "disable-refresh" else "Disable without Wake"
            )
            report["awake_state_observed"] = False
            report["wake_inputs_queued"] = 0
            report["experimental_limits"] = (
                "All braces remain present and are disabled; no Wake, impulse, "
                "teleport or replacement is authored. A pass applies to this "
                "observed fixture and does not establish a contact-cache cause."
            )
            if release_mode == "disable-refresh":
                report["native_contact_refresh_telemetry_required"] = True
                report["experimental_limits"] = (
                    "One native RecheckContactPoints call is required by separate telemetry. "
                    "The method itself requests waking; no separate Wake is authored. "
                    "A pass does not isolate waking from contact reconciliation or "
                    "establish a compiler defect."
                )
    (args.output / "rotating-removal-recovery.json").write_text(
        json.dumps(report, indent=2) + "\n"
    )
    return report


def main():
    parser = argparse.ArgumentParser(
        description="Compile a genuine rotating-door obstruction and observe recovery after authored brace removal"
    )
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument(
        "--release-mode", choices=["kill", "disable", "disable-wake", "disable-refresh"], default="kill"
    )
    parser.add_argument("--reuse-fixture", type=Path)
    parser.add_argument("--contact-observer-library", type=Path)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.game_dir = args.game_dir.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.release_mode == "disable-refresh":
        assert args.contact_observer_library, "disable-refresh requires the native contact library"
    if args.contact_observer_library:
        args.contact_observer_library = args.contact_observer_library.resolve(strict=True)
        assert args.runtime or args.worker, "--contact-observer-library requires --runtime"
    if args.worker:
        runtime.OBSERVER = observer(args.release_mode)
        args.observe_seconds = 48
        if args.contact_observer_library:
            import contact_runtime_observer as contact

            failures = []
            try:
                runtime.worker(
                    args,
                    launch_adapter=partial(contact.prepare, args.contact_observer_library, args.release_mode),
                )
            except BaseException as error:
                failures.append(error)
            try:
                check(args)
            except BaseException as error:
                failures.append(error)
            try:
                contact.verify(args.output, args.release_mode)
            except BaseException as error:
                failures.append(error)
            if failures:
                first, *others = failures
                for error in others:
                    first.add_note(f"Contact/recovery observation also failed: {type(error).__name__}: {error}")
                raise first
            return
        runtime.worker(args)
        check(args)
        return
    if args.reuse_fixture:
        assert args.runtime, "--reuse-fixture requires --runtime"
        compiled = json.loads(args.reuse_fixture.read_text())
        expected_schema = (
            EXPERIMENTAL_SCHEMA if args.release_mode == "disable-wake" else FIXTURE_SCHEMA
        )
        expected_release = (
            DISABLE_WAKE_RELEASE
            if args.release_mode == "disable-wake"
            else "remove_both_perpendicular_braces_with_kill"
        )
        if args.release_mode == "disable":
            expected_schema, expected_release = DISABLE_SCHEMA, DISABLE_RELEASE
        elif args.release_mode == "disable-refresh":
            expected_schema, expected_release = REFRESH_SCHEMA, REFRESH_RELEASE
        assert compiled["schema"] == expected_schema
        assert compiled["release"] == expected_release
        for kind in ["bsp", "source"]:
            assert digest(Path(compiled[kind])) == compiled[f"{kind}_sha256"]
    else:
        compiled = fixture(args)
    report = {**compiled, "runtime_checked": False}
    if args.runtime:
        output = args.output / "runtime"
        command = [
            sys.executable,
            "-B",
            str(Path(__file__).resolve()),
            "--worker",
            "--binary",
            str(args.binary),
            "--bsp",
            compiled["bsp"],
            "--game-dir",
            str(args.game_dir),
            "--output",
            str(output),
        ]
        if args.release_mode != "kill":
            command.extend(["--release-mode", args.release_mode])
        if args.contact_observer_library:
            command.extend(["--contact-observer-library", str(args.contact_observer_library)])
        runtime.private_compositor(command, output, timeout=90)
        report["native"] = json.loads((output / "rotating-removal-recovery.json").read_text())
        report["runtime_checked"] = True
        if args.contact_observer_library:
            report["contact_observer"] = json.loads((output / "contact-observer-analysis.json").read_text())
        for kind in ["bsp", "source"]:
            assert digest(Path(compiled[kind])) == compiled[f"{kind}_sha256"]
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(
        json.dumps(
            {"output": str(args.output), "runtime_checked": report["runtime_checked"]}
        )
    )


if __name__ == "__main__":
    main()
