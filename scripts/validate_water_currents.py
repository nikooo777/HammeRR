import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import resource
import shutil
import struct
import sys
import time
from unittest.mock import patch

import validate_native_runtime as runtime
from private_runtime_config import private_launch_options, private_native_process, private_observer, verify_private_launch
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_engine_physics import physics_records
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail

KEY = "_hammerr_water_current_velocity"
MODEL = "models/props_junk/wood_crate001a.mdl"
POOL_MIN = [-2048, -2048, -1024]
POOL_MAX = [2048, 2048, 1024]
ROOM_MIN = [-2304, -2304, -1280]
ROOM_MAX = [2304, 2304, 2304]
ORIGINS = {"physics_crate": [0, 0, 0], "dry_crate": [0, 0, 1536]}
DURATION = 8.0
CASES = {
    "absent": None, "zero": [0, 0, 0],
    "positive-x": [128, 0, 0], "negative-x": [-128, 0, 0],
    "positive-y": [0, 128, 0], "negative-y": [0, -128, 0],
    "positive-z": [0, 0, 128], "negative-z": [0, 0, -128],
    "mixed": [64, -64, 32],
}
CONTROLS = {"sv_lan": 1, "sv_gravity": 0, "mat_hdr_level": 2, "mat_fullbright": 0}
MAX_BSP = 128 * 1024**2
MAX_LOG = 8 * 1024**2
CONTENTS_SOLID = 0x1
CONTENTS_WINDOW = 0x2
CONTENTS_GRATE = 0x8
CONTENTS_MOVEABLE = 0x4000
CONTENTS_MONSTER = 0x2000000
WORLD_SOLID_MASK = CONTENTS_SOLID | CONTENTS_WINDOW | CONTENTS_GRATE | CONTENTS_MOVEABLE | CONTENTS_MONSTER
MODULES = {
    "bin/linux64/vphysics.so": "2676cdc02be4b4188f7121c201d5f5a681f9c1c7c924fe7f9159cefd5a718277",
    "cstrike/bin/linux64/server.so": "7887c8cc24a3dec6b8739bfebcad48c00b95423ebcbfc647f49fef1ab904dbe2",
}
OBSERVER = '''
stage <- 0;
ready_at <- 0.0;
requested_at <- null;
epoch <- null;
enabled <- {};
function HammeRRCurrentSample(name, phase) {
    local ent = Entities.FindByName(null, name);
    if (ent == null) { printl("HAMMERR_CURRENT_ERROR missing " + name); return; }
    local p = ent.GetOrigin();
    local a = ent.GetAngles();
    local v = ent.GetPhysVelocity();
    local w = ent.GetPhysAngularVelocity();
    local lo = ent.GetBoundingMinsOriented();
    local hi = ent.GetBoundingMaxsOriented();
    printl("HAMMERR_CURRENT_SAMPLE " + phase + " " + Time() + " " + FrameTime() + " " + name + " " + ent.GetEntityIndex() + " " + ent.GetMoveType() + " " + (ent.IsSolid() ? 1 : 0) + " " + ent.GetHealth() + " " + p.x + " " + p.y + " " + p.z + " " + a.x + " " + a.y + " " + a.z + " " + v.x + " " + v.y + " " + v.z + " " + w.x + " " + w.y + " " + w.z + " " + lo.x + " " + lo.y + " " + lo.z + " " + hi.x + " " + hi.y + " " + hi.z);
}
function HammeRRCurrentEnabled(name) {
    if (name in enabled) { printl("HAMMERR_CURRENT_ERROR duplicate_enable " + name); return; }
    local ent = Entities.FindByName(null, name);
    if (ent == null) { printl("HAMMERR_CURRENT_ERROR enable_missing " + name); return; }
    enabled[name] <- Time();
    if (epoch == null) epoch = Time();
    if (name == "dry_crate") ent.SetPhysVelocity(Vector(128, 0, 0));
    printl("HAMMERR_CURRENT_ENABLED " + Time() + " " + name + " " + ent.GetEntityIndex());
    HammeRRCurrentSample(name, 1);
}
function HammeRRCurrentEnabled_physics_crate() { HammeRRCurrentEnabled("physics_crate"); }
function HammeRRCurrentEnabled_dry_crate() { HammeRRCurrentEnabled("dry_crate"); }
function HammeRRThink() {
    local player = Entities.FindByClassname(null, "player");
    if (player == null) return 0.1;
    if (stage == 0) {
        SendToConsole("sv_cheats 1; joystick 0; bot_quota 0; bot_kick; jointeam 2; joinclass 1; hidepanel info; hidepanel team; hidepanel class_ter; hidepanel class_ct; r_drawvgui 0; gameui_hide; sv_lan; sv_gravity; mat_hdr_level; mat_fullbright");
        ready_at = Time() + 6.0;
        stage = 1;
    }
    if (stage == 1 && Time() >= ready_at && player.GetTeam() == 2 && player.GetHealth() == 100) {
        HammeRRCurrentSample("physics_crate", 0);
        HammeRRCurrentSample("dry_crate", 0);
        requested_at = Time();
        EntFire("physics_crate", "EnableMotion");
        EntFire("dry_crate", "EnableMotion");
        printl("HAMMERR_CURRENT_REQUEST " + requested_at);
        stage = 2;
    }
    if (stage == 2) {
        if (epoch == null || enabled.len() != 2) {
            if (Time() > requested_at + 1.0) printl("HAMMERR_CURRENT_ERROR enable_timeout");
            return 0.02;
        }
        if (Time() > epoch) {
            HammeRRCurrentSample("physics_crate", 1);
            HammeRRCurrentSample("dry_crate", 1);
        }
        local p = player.GetOrigin();
        printl("HAMMERR_CURRENT_PLAYER " + Time() + " " + player.GetHealth() + " " + player.GetTeam() + " " + p.x + " " + p.y + " " + p.z);
        if (Time() >= epoch + __DURATION__) {
            printl("HAMMERR_CURRENT_DONE " + Time());
            stage = 3;
        }
    }
    return stage == 2 ? 0.02 : 0.1;
}
'''


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def modules(game):
    result = {}
    for name, expected in MODULES.items():
        path = game / name
        require(0 < path.stat().st_size <= 64 * 1024**2, "Native module exceeds identity-read bound")
        result[name] = digest(path.read_bytes())
        require(result[name] == expected, f"Reverify native current contract for changed module {name}")
    return result


def f32(value):
    return struct.unpack("<f", struct.pack("<f", value))[0]


def case_duration(label):
    return 4.0 if label in {"positive-z", "negative-z"} else DURATION


def world_field(client):
    return next(row for row in client.call("get_property_inspector", {"world": True})["properties"] if row["field"]["key"] == KEY)


def lumps(data):
    require(1036 <= len(data) <= MAX_BSP and data[:4] == b"VBSP", "Invalid bounded BSP")
    result = []
    for index in range(64):
        offset, length, version, compression = struct.unpack_from("<4i", data, 8 + 16 * index)
        require(length >= 0 and (length == 0 or 1036 <= offset <= len(data) - length), "BSP lump exceeds file")
        result.append((version, compression, data[offset:offset + length]))
    return result


def game_lump_relative_digest(data):
    offset, length = struct.unpack_from("<2i", data, 8 + 35 * 16)
    require(length >= 4 and 1036 <= offset <= len(data) - length, "Invalid bounded game lump")
    normalized = bytearray(data[offset:offset + length])
    count = struct.unpack_from("<i", normalized)[0]
    require(0 <= count <= 4096 and 4 + count * 16 <= length, "Invalid game-lump directory")
    for index in range(count):
        cursor = 4 + index * 16
        _, _, _, absolute, size = struct.unpack_from("<IHHii", normalized, cursor)
        require(size >= 0 and offset + 4 + count * 16 <= absolute <= offset + length - size, "Game-lump entry exceeds its payload")
        struct.pack_into("<i", normalized, cursor + 8, absolute - offset)
    return digest(normalized)


def key_blocks(raw):
    require(0 < len(raw) <= 65535 and raw[-1:] == b"\0" and b"\0" not in raw[:-1], "Invalid physics key framing")
    text = raw[:-1].decode("ascii")
    pattern = re.compile(r'\s+|//[^\n]*|"(?:\\.|[^"\\])*"|[{}]|[^\s{}"]+')
    tokens, cursor = [], 0
    for match in pattern.finditer(text):
        require(match.start() == cursor, "Unconsumed physics key input")
        cursor = match.end()
        word = match.group()
        if word.isspace() or word.startswith("//"):
            continue
        tokens.append(json.loads(word) if word.startswith('"') else word)
    require(cursor == len(text) and len(tokens) <= 16384, "Physics key token budget exceeded")
    blocks, cursor = [], 0
    while cursor < len(tokens):
        require(cursor + 2 < len(tokens) and tokens[cursor + 1] == "{", "Expected flat native key block")
        name, fields = tokens[cursor], {}
        cursor += 2
        while cursor < len(tokens) and tokens[cursor] != "}":
            require(cursor + 1 < len(tokens) and tokens[cursor] not in {"{", "}"} and tokens[cursor + 1] not in {"{", "}"}, "Nested/incomplete native key field")
            key, value = tokens[cursor:cursor + 2]
            require(key.lower() not in fields, "Duplicate native physics key")
            fields[key.lower()] = value
            cursor += 2
        require(cursor < len(tokens) and len(blocks) < 4096, "Unterminated/excessive native blocks")
        blocks.append((name, fields))
        cursor += 1
    return blocks


def numbers(text, count):
    values = text.split()
    require(len(values) == count and all(re.fullmatch(r"[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?", value) for value in values), "Invalid full-consumption native number")
    result = [float(value) for value in values]
    require(all(math.isfinite(value) for value in result), "Nonfinite native number")
    return result


def check_bsp(data, vector):
    framed = lumps(data)
    require(framed[29][:2] == (0, 0), "Unexpected compressed/versioned physics")
    native = physics_records(data)
    require(len(native) == 1 and native[0][0] == 0, "Expected exactly one native world model")
    _, solids, raw = native[0]
    blocks = key_blocks(raw)
    fluids = [fields for name, fields in blocks if name == "fluid"]
    require(len(fluids) == 1, "Expected exactly one compiled fluid block")
    fluid = fluids[0]
    require(set(fluid) == {"index", "surfaceprop", "damping", "contents", "surfaceplane", "currentvelocity"}, "Unexpected/missing fluid fields")
    require(len(solids) == 2 and fluid["index"] == "1", "Fluid body does not follow the one compiled solid-world body")
    require(fluid["surfaceprop"] == "water" and numbers(fluid["contents"], 1) == [32] and numbers(fluid["damping"], 1) == [0.01], "Fixed fluid role/damping changed")
    require(numbers(fluid["surfaceplane"], 4) == [0, 0, 1, POOL_MAX[2]], "Fluid plane does not match the authored top")
    expected = [f32(f32(value) * f32(0.0254)) for value in (vector or [0, 0, 0])]
    require([f32(value) for value in numbers(fluid["currentvelocity"], 3)] == expected, "Compiled current differs from the independent single SI conversion")
    require(KEY.lower().encode() not in framed[0][2].lower(), "Compiler-only world property leaked into BSP entities")
    other = [(name, fields) for name, fields in blocks if name != "fluid"]
    require([fields for name, fields in other if name == "staticsolid"] == [{"index": "0", "contents": str(WORLD_SOLID_MASK)}] and sum(name == "materialtable" for name, _ in other) == 1 and len(other) == 2, "Solid-world key/body correspondence changed")
    return {"fluid": fluid, "solid_sha256": [digest(solid) for solid in solids], "other_blocks": other, "lump_versions": [[version, compression] for version, compression, _ in framed], "lump_sha256": [digest(payload) for _, _, payload in framed], "game_lump_relative_sha256": game_lump_relative_digest(data)}


def world_current(client, value):
    token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
    change = {"action": "remove", "key": KEY} if value is None else {"action": "set", "key": KEY, "value": value}
    client.call("update_property_edit", {"token": token, "changes": [change]})
    client.call("commit_property_edit", {"token": token})


def reject_unscaled(data, output):
    offset, size = struct.unpack_from("<2i", data, 8 + 29 * 16)
    matches = list(re.finditer(rb'"currentvelocity"\s+"([^"\r\n]+)"', data[offset:offset + size]))
    require(len(matches) == 1 and len(matches[0].group(1)) >= 7, "Cannot identify the unique bounded current field")
    match = matches[0]
    bad = b"128 0 0".ljust(len(match.group(1)), b" ")
    changed = data[:offset + match.start(1)] + bad + data[offset + match.end(1):]
    (output / "rejected-unscaled-fixture.bsp").write_bytes(changed)
    try:
        check_bsp(changed, [128, 0, 0])
    except AssertionError as error:
        require("single SI conversion" in str(error), "Unscaled negative control failed for an unrelated reason")
    else:
        raise AssertionError("Unscaled native current passed the typed BSP checker")


def snapshot(client, ids):
    return {"objects": detail(client, ids), "history": client.call("get_history"), "world": client.call("get_property_inspector", {"world": True}), "selection": client.call("get_selection"), "camera": client.call("get_camera")}


def motion_connection(client, entity, name):
    token = client.call("begin_connection_edit", {"id": entity})["token"]
    client.call("update_connection_draft", {"token": token, "output": "OnMotionEnabled", "target": "hammerr_observer", "input": "RunScriptCode", "parameter": f"HammeRRCurrentEnabled_{name}()", "delay": "0", "times": "-1"})
    client.call("commit_connection_edit", {"token": token})


def fixture(client):
    ids = []
    for axis in range(3):
        for upper in (False, True):
            low, high = ROOM_MIN.copy(), ROOM_MAX.copy()
            if upper:
                low[axis] = high[axis]
                high[axis] += 32
            else:
                high[axis] = low[axis]
                low[axis] -= 32
            ids.append(client.call("create_block", {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"})["id"])
    water = client.call("create_block", {"mins": POOL_MIN, "maxs": POOL_MAX, "material": "tools/toolsnodraw"})["id"]
    ids.append(water)
    brush = detail(client, [water])[0]
    top = next(face for face in brush["faces"] if face["normal"] == [0, 0, 1])
    client.call("edit_faces", {"faces": [{"solid": brush["solid"], "side": top["side"]}], "material": "liquids/aztecwater"})
    for classname, origin, properties in [
        ("info_player_terrorist", [-2200, -2200, -1200], {}),
        ("info_player_counterterrorist", [2200, -2200, -1200], {}),
        ("logic_script", [-2200, -2200, -1168], {"targetname": "hammerr_observer", "vscripts": "hammerr_observer.nut", "thinkfunction": "HammeRRThink"}),
        *[("prop_physics", origin, {"targetname": name, "model": MODEL, "spawnflags": "8"}) for name, origin in ORIGINS.items()],
    ]:
        entity = client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"]
        ids.append(entity)
        if classname == "prop_physics":
            motion_connection(client, entity, properties["targetname"])
    return ids, water


def compile_pair(client, output, label, ids, vector):
    source = output / f"{label}.vmf"
    client.call("save_map", {"path": str(source)})
    client.call("open_map", {"path": str(source)})
    saved, before = source.read_bytes(), snapshot(client, ids)
    bsp = output / f"{label}.bsp"
    result = client.call("compile_map", {"path": str(bsp)})
    require(result["success"] and result["stats"]["physics_fluids"] == 1, "Current fixture did not compile its fluid")
    data = bsp.read_bytes()
    checked = check_bsp(data, vector)
    other = output / f"{label}-background.bsp"
    client.call("start_compile", {"path": str(other)})
    job = wait_job(client)
    require(job["status"] == "completed" and other.read_bytes() == data, "Shared compile APIs differ")
    require(snapshot(client, ids) == before and source.read_bytes() == saved, "Compile changed source/history/selection/camera")
    return {"bsp": str(bsp), "bsp_sha256": digest(data), "source": str(source), "source_sha256": digest(saved), "vector_hu_per_second": vector or [0, 0, 0], "stats": result["stats"], "checked": checked}, data


def compile_suite(args):
    game = runtime.private_game(args.game_dir, args.output / "source-game", engine_physics=True)
    output = args.output / "compile"
    output.mkdir()
    client = Client(None, args.binary, output, game / "gameinfo.txt")
    report = {"status": "failed", "cases": {}, "runtime_checked": False}
    try:
        with stopped_on_exit(client):
            ids, water = fixture(client)
            client.call("set_compile_settings", {"lighting": "fullbright", "physics": "engine", "generate_reflections": False, "pack_sky_assets": False})
            metadata = world_field(client)
            field = metadata["field"]
            require(field["label"] == "Water current · world units/s" and field["kind"] == field["control"] == "vector" and field["component_labels"] == ["X", "Y", "Z"], "Shared World current vector metadata is missing")
            require(field["value"] == "0 0 0" and field["components"] == ["0", "0", "0"] and metadata["stored_count"] == 0 and metadata["values"] == [{"object": "world", "stored": None, "default": None, "effective": "0 0 0"}], "Absent current lost its intentional unstored zero")
            report["world_field"] = metadata
            world_current(client, "0 64 0")
            require(world_field(client)["field"]["value"] == "0 64 0", "World current Apply did not retain the staged vector")
            client.call("undo")
            require(world_field(client)["stored_count"] == 0, "World current Undo did not restore absence")
            client.call("redo")
            require(world_field(client)["field"]["value"] == "0 64 0", "World current Redo changed the vector")
            client.call("undo")
            seed = output / "water-current-source.vmf"
            client.call("save_map", {"path": str(seed)})
            seed_bytes = seed.read_bytes()
            baseline = reference = None
            for label, vector in CASES.items():
                client.call("open_map", {"path": str(seed)})
                world_current(client, None if vector is None else " ".join(map(str, vector)))
                row, data = compile_pair(client, output, label, ids, vector)
                if baseline is None:
                    baseline, reference = data, row["checked"]
                else:
                    require(row["checked"]["solid_sha256"] == reference["solid_sha256"] and row["checked"]["other_blocks"] == reference["other_blocks"], "Current changed collision bodies or ordinary physics keys")
                    require(row["checked"]["lump_versions"] == reference["lump_versions"], "Current changed BSP lump version/compression metadata")
                    require(all(a == b for index, (a, b) in enumerate(zip(row["checked"]["lump_sha256"], reference["lump_sha256"], strict=True)) if index not in (29, 35)), "Current changed an unrelated BSP lump")
                    require(row["checked"]["game_lump_relative_sha256"] == reference["game_lump_relative_sha256"], "Current changed game-lump data beyond absolute pointer relocation")
                if label == "zero":
                    require(data == baseline, "Explicit zero current changed default BSP bytes")
                report["cases"][label] = row
                write_json(args.output / "compile-results.json", report)
            require(seed.read_bytes() == seed_bytes, "Current case changed the shared saved baseline")
            reject_unscaled(Path(report["cases"]["positive-x"]["bsp"]).read_bytes(), output)
            valid_source = Path(report["cases"]["mixed"]["source"])
            saved = valid_source.read_bytes()
            valid = report["cases"]["mixed"]["bsp"]
            previous = output / "preserved.bsp"
            previous.write_bytes(Path(valid).read_bytes())
            expected_previous = previous.read_bytes()
            token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
            client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": KEY, "value": "0 64 0"}]})
            staged, before = client.call("get_property_edit"), snapshot(client, ids)
            invalid_values = ["129 0 0", "128 1 0", "NaN 0 0", "1 2", "1 2 3 4", "1e100 0 0", "1e-1000 0 0"]
            for value in invalid_values:
                client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": KEY.upper(), "value": value}]}, error=True)
                require(client.call("get_property_edit") == staged and snapshot(client, ids) == before, "Invalid current changed a retained draft/source")
            client.call("cancel_property_edit", {"token": token})
            text, needle = saved.decode(), f'"{KEY}" "64 -64 32"'
            require(text.count(needle) == 1, "Saved current property is ambiguous")
            failures = []
            for index, value in enumerate([*invalid_values, None]):
                replacement = f'"{KEY}" "{value}"' if value is not None else needle + f'\n"{KEY.upper()}" "0 0 0"'
                source = output / f"invalid-{index}.vmf"
                source.write_text(text.replace(needle, replacement))
                client.call("open_map", {"path": str(source)})
                before = snapshot(client, ids)
                failure = client.call("compile_map", {"path": str(previous)}, error=True)
                require(KEY in json.dumps(failure), "Rejected import lacks current source diagnostic")
                client.call("start_compile", {"path": str(previous)})
                require(wait_job(client)["status"] == "failed" and previous.read_bytes() == expected_previous and snapshot(client, ids) == before, "Rejected current replaced output/source/history")
                failures.append(failure)
            client.call("open_map", {"path": str(valid_source)})
            client.call("set_compile_settings", {"physics": "none"})
            client.call("compile_map", {"path": str(previous)}, error=True)
            require(previous.read_bytes() == expected_previous, "Disabled-physics rejection replaced prior output")
            client.call("set_compile_settings", {"physics": "engine"})
            client.call("delete_objects", {"ids": [water]})
            client.call("compile_map", {"path": str(previous)}, error=True)
            require(previous.read_bytes() == expected_previous, "Missing-water rejection replaced prior output")
            client.call("undo")
            require(valid_source.read_bytes() == saved, "Validation rewrote saved authored current")
            report.update({"status": "accepted", "rejected": failures, "shared_bytes_exact": True, "zero_default_bytes_exact": True, "previous_output_preserved": True, "unscaled_key_rejected_before_launch": True})
            write_json(args.output / "compile-results.json", report)
            return report
    finally:
        client.close()


def parse_motion(text):
    samples, enabled, players = [], [], []
    for line in text.splitlines():
        words = line.strip().split()
        if not words:
            continue
        if words[0] == "HAMMERR_CURRENT_SAMPLE":
            require(len(words) == 27, "Malformed public motion sample")
            values = [float(value) for value in [*words[2:4], *words[5:]]]
            require(all(math.isfinite(value) for value in values), "Nonfinite public motion sample")
            samples.append({"phase": int(words[1]), "time": float(words[2]), "server_frame_seconds": float(words[3]), "name": words[4], "entity_index": int(words[5]), "move_type": int(words[6]), "solid": int(words[7]), "health": float(words[8]), "position": values[6:9], "angles": values[9:12], "velocity": values[12:15], "angular": values[15:18], "mins": values[18:21], "maxs": values[21:24]})
        elif words[0] == "HAMMERR_CURRENT_ENABLED":
            require(len(words) == 4, "Malformed enable callback")
            enabled.append({"time": float(words[1]), "name": words[2], "entity_index": int(words[3])})
        elif words[0] == "HAMMERR_CURRENT_PLAYER":
            require(len(words) == 7, "Malformed player sample")
            row = list(map(float, words[1:]))
            require(all(math.isfinite(value) for value in row), "Nonfinite player sample")
            players.append(row)
    return {"samples": samples, "enabled": enabled, "players": players}


def check_motion(parsed, vector, seconds):
    enabled, samples, players = (parsed[name] for name in ("enabled", "samples", "players"))
    require(len(enabled) == 2 and {row["name"] for row in enabled} == set(ORIGINS), "Each owned body needs exactly one motion-enable callback")
    require(len({row["entity_index"] for row in enabled}) == 2 and abs(enabled[0]["time"] - enabled[1]["time"]) < 0.04, "Body identities/start phases differ")
    require(len(players) >= 25 * seconds and all(row[1:3] == [100, 2] for row in players), "Player health/team control failed")
    require(all(math.dist(row[3:], players[0][3:]) < 1 for row in players), "Outside-pool player moved during measurement")
    metrics = {}
    for name, origin in ORIGINS.items():
        initial = [row for row in samples if row["name"] == name and row["phase"] == 0]
        rows = [row for row in samples if row["name"] == name and row["phase"] == 1]
        require(len(initial) == 1 and math.dist(initial[0]["position"], origin) < 1 and max(map(abs, initial[0]["velocity"])) < 0.01, "Motion-disabled initial state changed")
        require(25 * seconds <= len(rows) <= 1000 and rows[-1]["time"] - rows[0]["time"] >= seconds, "Incomplete motion window")
        gaps = [b["time"] - a["time"] for a, b in zip(rows, rows[1:])]
        require(all(0 < gap <= 0.1 for gap in gaps), "Public sample time is missing/nonmonotonic")
        require(all(0 < row["server_frame_seconds"] <= 0.05 for row in rows), "Unexpected server frame interval; no physics-step inference is made")
        identity = next(row["entity_index"] for row in enabled if row["name"] == name)
        require(all(row["entity_index"] == identity and row["move_type"] == 6 and row["solid"] == 1 and row["health"] == initial[0]["health"] for row in rows), "Owned dynamic body changed identity/role/health")
        low, high = (POOL_MIN, POOL_MAX) if name == "physics_crate" else (ROOM_MIN, ROOM_MAX)
        for row in rows:
            require(all(a < b for a, b in zip(row["mins"], row["maxs"], strict=True)), "Invalid observed prop bounds")
            radius = math.sqrt(sum(max(abs(a), abs(b))**2 for a, b in zip(row["mins"], row["maxs"], strict=True)))
            require(all(a + 64 + radius < value < b - 64 - radius for a, value, b in zip(low, row["position"], high, strict=True)), "Prop left its contact-free observation volume")
            if name == "dry_crate":
                require(row["position"][2] - radius > POOL_MAX[2] + 64, "Dry control entered water")
        speeds = [math.sqrt(sum(value**2 for value in row["velocity"])) for row in rows]
        require(max(speeds) < 256, "Current exceeded the declared coarse consumer-speed gate")
        integral = [sum((a["velocity"][axis] + b["velocity"][axis]) * 0.5 * (b["time"] - a["time"]) for a, b in zip(rows, rows[1:])) for axis in range(3)]
        displacement = [b - a for a, b in zip(rows[0]["position"], rows[-1]["position"], strict=True)]
        require(all(abs(a - b) <= 2 * max(speeds) * max(gaps) + 1 for a, b in zip(displacement, integral, strict=True)), "Public trajectory disagrees with integrated public velocity")
        late = [row for row in rows if row["time"] >= rows[-1]["time"] - 1]
        velocity = [sum(row["velocity"][axis] for row in late) / len(late) for axis in range(3)]
        if name == "physics_crate":
            for axis, requested in enumerate(vector):
                if requested:
                    require(velocity[axis] * math.copysign(1, requested) > 8 and displacement[axis] * math.copysign(1, requested) > 16, "Authored current direction lacks measured speed/displacement")
                else:
                    require(abs(velocity[axis]) < 8 and abs(displacement[axis]) < 8, "Zero current axis developed material motion")
        else:
            require(abs(rows[0]["velocity"][0] - 128) < 0.1 and abs(displacement[0]) > 16, "Public dry initial velocity/control did not apply")
            require(abs(displacement[2]) < 1 and max(abs(row["velocity"][2]) for row in rows) < 0.1, "Zero-gravity dry control developed vertical acceleration")
        metrics[name] = {"samples": len(rows), "elapsed_seconds": rows[-1]["time"] - rows[0]["time"], "late_velocity_hu_per_second": velocity, "displacement_hu": displacement, "maximum_speed_hu_per_second": max(speeds), "maximum_gap_seconds": max(gaps), "entity_index": identity}
    return metrics


def check_controls(text):
    spawn = text.index("HAMMERR_MAPSPAWN")
    observed = {}
    for name, expected in CONTROLS.items():
        matches = list(re.finditer(r'"' + re.escape(name) + r'" = "([^"]+)"', text))
        before = [float(match.group(1)) for match in matches if match.start() < spawn]
        after = [float(match.group(1)) for match in matches if match.start() > spawn]
        expected_after = 1 if name == "mat_fullbright" else expected
        require(before and after and all(value == expected for value in before) and all(value == expected_after for value in after), f"Missing/wrong pre-map and script control {name}")
        observed[name] = {"before_map": before, "after_map": after, "expected_before": expected, "expected_after": expected_after}
    return observed


def worker(args):
    require(args.runtime_case in CASES and args.bsp is not None, "Invalid owned runtime case")
    require(os.environ.get("WAYLAND_DISPLAY") and os.environ.get("XDG_RUNTIME_DIR") == str(args.output / "wayland"), "Private Cage environment is required")
    require(not os.environ.get("LD_PRELOAD") and not os.environ.get("LD_AUDIT"), "Inherited native injection variables are unsupported")
    check_bsp(args.bsp.read_bytes(), CASES[args.runtime_case])
    game = runtime.private_game(args.game_dir, args.output, args.bsp)
    observer = OBSERVER.replace("__DURATION__", str(case_duration(args.runtime_case)))
    (game / "scripts/vscripts/hammerr_observer.nut").write_text(private_observer(observer))
    options = private_launch_options(args.game_dir, game, hdr_level=CONTROLS["mat_hdr_level"], controls={**CONTROLS, "servercfgfile": "server.cfg"})
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    command = [str(args.game_dir / "cstrike_linux64"), "-game", str(game), "-windowed", "-w", "800", "-h", "600", "-novid", *options, "-nomaster", "-nojoy", "-console", "-port", str(args.port)]
    environment = {**os.environ, "LD_LIBRARY_PATH": str(args.game_dir / "bin/linux64"), "SteamAppId": "240", "SteamGameId": "240", "SDL_VIDEODRIVER": "x11"}
    report = {"status": "failed", "case": args.runtime_case, "bsp_sha256": digest(args.bsp.read_bytes()), "observer_sha256": digest(observer.encode()), "modules": modules(args.game_dir), "script_time_is_physics_timestep": False, "desktop_input": False}
    try:
        with private_native_process(command, args.game_dir, game, args.output, environment, report) as process:
            deadline = time.monotonic() + 35
            text = ""
            while time.monotonic() < deadline:
                require(process.poll() is None, "Private game exited before completing motion observation")
                log = game / "hammerr-runtime.log"
                if log.exists():
                    require(log.stat().st_size <= MAX_LOG, "Runtime log exceeds 8MiB")
                    text = log.read_text(errors="replace")
                    require("HAMMERR_CURRENT_ERROR" not in text, "Owned script setup failed; inspect retained log")
                    if "HAMMERR_CURRENT_DONE" in text:
                        break
                time.sleep(0.1)
            require("HAMMERR_CURRENT_DONE" in text, "Timed public motion observation did not complete")
            parsed = parse_motion(text)
            write_json(args.output / "motion.json", parsed)
            report["private_launch"] = verify_private_launch(game, text)
            report["control_observations"] = check_controls(text)
            report["metrics"] = check_motion(parsed, CASES[args.runtime_case] or [0, 0, 0], case_duration(args.runtime_case))
        require(modules(args.game_dir) == report["modules"], "Native module inputs changed during observation")
        report["status"] = "accepted"
    finally:
        write_json(args.output / "runtime.json", report)


def main():
    parser = argparse.ArgumentParser(description="Compile authored uniform water currents and measure owned game props")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--port", type=int, default=27639)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--runtime-case", choices=tuple(CASES), help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.game_dir = args.game_dir.resolve(strict=True)
    args.binary = args.binary.resolve(strict=True)
    require(args.output.is_relative_to(ROOT / "target"), "Retain evidence on disk below repository target/")
    require(1024 <= args.port <= 65535, "Invalid private game port")
    if args.worker:
        worker(args)
        return
    require(not args.output.exists(), "Choose a new absent validation output")
    args.output.mkdir(parents=True, mode=0o700)
    with patch.dict(os.environ, {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent-maps"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json")}):
        report = compile_suite(args)
    if args.runtime:
        require(shutil.which("cage"), "Private Cage is required")
        results = {}
        for label, case in report["cases"].items():
            folder = args.output / label
            runtime.private_display(Path(__file__), args, Path(case["bsp"]), extra_flags=("--runtime-case", label, "--port", str(args.port)), output=folder)
            result = json.loads((folder / "runtime.json").read_text())
            require(result["status"] == "accepted" and result["configuration_preservation"]["success"], "Native motion/preservation gate failed")
            results[label] = result
        report["runtime_checked"] = True
        report["runtime"] = results
    write_json(args.output / "results.json", report)
    print(json.dumps({"status": report["status"], "runtime_checked": report["runtime_checked"], "cases": len(report["cases"]), "output": str(args.output)}))


if __name__ == "__main__":
    main()
