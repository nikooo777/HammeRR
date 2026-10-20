import argparse
import json
import os
import re
import struct
from pathlib import Path
from unittest.mock import patch

import validate_native_runtime as runtime
import validate_water_currents as current
from validate_build_recipes import protected_state
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_engine_physics import physics_records
from validate_face_lighting import ROOT, Client, detail

KEY = "_hammerr_water_damping"
RAW_INTERIOR = " 5e-3 "
MAX_SOURCE = 8 * 1024**2
CASES = (
    ("absent", None, None),
    ("explicit-default", "0.01", None),
    ("zero", "-0", None),
    ("interior", RAW_INTERIOR, None),
    ("current-only-128", None, [128, 0, 0]),
    ("current-128-damping-zero", "0", [128, 0, 0]),
)

require = current.require
digest = current.digest
write_json = current.write_json


def read_bounded(path, limit):
    require(path.is_file() and 0 < path.stat().st_size <= limit, f"Invalid bounded input: {path}")
    data = path.read_bytes()
    require(0 < len(data) <= limit, f"Input changed beyond its byte bound: {path}")
    return data


def world_rows(preview):
    return [row for row in preview["properties"] if row["field"]["key"].lower() == KEY]


def world_field(client):
    rows = world_rows(client.call("get_property_inspector", {"world": True}))
    require(len(rows) == 1, "Expected one shared World damping field")
    return rows[0]


def check_raw_field(row, raw):
    field = row["field"]
    require(field["label"] == "Water pressure damping" and field["kind"] == "float" and field["control"] == "number", "Missing shared scalar Number metadata")
    require(not field["advanced"] and not field["components"] and field["range"] == [0, 0.01], "Scalar metadata changed its product range or component role")
    effective = "0.01" if raw is None else raw
    require(field["value"] == effective and row["stored_count"] == int(raw is not None), "Scalar inspector changed raw source or synthesized an override")
    require(row["values"] == [{"object": "world", "stored": raw, "default": None, "effective": effective}] and not row["can_reset"], "Scalar per-object effective/default metadata differs")


def set_world(client, key, raw):
    token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
    change = {"action": "remove", "key": key} if raw is None else {"action": "set", "key": key, "value": raw}
    client.call("update_property_edit", {"token": token, "changes": [change]})
    client.call("commit_property_edit", {"token": token})


def snapshot(client, source, ids):
    info = client.call("get_map_info")
    require(not info["rendering"]["initialized"] and not info["game_running"] and not info["launching"], "Headless scalar validation initialized graphics or a game")
    return {
        "state": protected_state(client), "history": client.call("get_history"),
        "objects": detail(client, ids), "world": client.call("get_property_inspector", {"world": True}),
        "settings": client.call("get_compile_settings"), "recipes": client.call("get_build_recipes"),
        "source_sha256": digest(read_bounded(source, MAX_SOURCE)),
        "drafts": {name: info[name] for name in ("property_draft", "connection_draft", "rename_draft", "context")},
    }


def check_bsp(data, damping, vector=None):
    framed = current.lumps(data)
    require(framed[29][:2] == (0, 0), "Unexpected compressed/versioned physics")
    native = physics_records(data)
    require(len(native) == 1 and native[0][0] == 0, "Expected one native world model")
    _, solids, raw = native[0]
    blocks = current.key_blocks(raw)
    fluids = [fields for name, fields in blocks if name == "fluid"]
    require(len(fluids) == 1 and len(solids) == 2, "Expected one solid-world body and one WATER body")
    fluid = fluids[0]
    require(set(fluid) == {"index", "surfaceprop", "damping", "contents", "surfaceplane", "currentvelocity"}, "Unexpected/missing fluid fields")
    require(fluid["index"] == "1" and fluid["surfaceprop"] == "water" and current.numbers(fluid["contents"], 1) == [32], "Fluid body identity/material/contents changed")
    require(current.numbers(fluid["surfaceplane"], 4) == [0, 0, 1, current.POOL_MAX[2]], "Fluid plane changed")
    actual = current.f32(current.numbers(fluid["damping"], 1)[0])
    expected = 0.0 if damping == 0.0 else current.f32(damping)
    require(struct.pack("<f", actual) == struct.pack("<f", expected), "Compiled damping differs from the exact requested native coefficient")
    expected_current = [current.f32(current.f32(value) * current.f32(0.0254)) for value in (vector or [0, 0, 0])]
    require([current.f32(value) for value in current.numbers(fluid["currentvelocity"], 3)] == expected_current, "Compiled current differs from the independent single SI conversion")
    require(all(key.encode() not in framed[0][2].lower() for key in (KEY, current.KEY)), "Compiler-only World property leaked into entities")
    other = [(name, fields) for name, fields in blocks if name != "fluid"]
    require([fields for name, fields in other if name == "staticsolid"] == [{"index": "0", "contents": str(current.WORLD_SOLID_MASK)}], "Solid-world identity/contents changed")
    tables = [fields for name, fields in other if name == "materialtable"]
    require(len(other) == 2 and len(tables) == 1, "Unexpected native body/material roles")
    table = tables[0]
    require(1 <= len(table) <= 127 and table.get("default") == "1", "Invalid material table bounds/default")
    require(all(re.fullmatch(r"[A-Za-z0-9_.-]{1,127}", name) for name in table), "Unsafe native material name")
    require(all(re.fullmatch(r"[0-9]+", value) for value in table.values()) and sorted(map(int, table.values())) == list(range(1, len(table) + 1)), "Native material indices differ")
    without_damping, count = re.subn(rb'("damping"\s+")[^"]*(")', rb'\1<authored-pressure>\2', raw)
    require(count == 1, "Native scalar text is not uniquely framed")
    return {
        "fluid": fluid, "model_indices": [model[0] for model in native], "body_counts": [len(model[1]) for model in native],
        "solid_sha256": [digest(solid) for solid in solids], "other_blocks": other,
        "keys_except_damping_sha256": digest(without_damping),
        "bsp_version_revision": [struct.unpack_from("<i", data, 4)[0], struct.unpack_from("<i", data, 1032)[0]],
        "lump_versions": [[version, compression] for version, compression, _ in framed],
        "lump_sha256": [digest(payload) for _, _, payload in framed],
        "game_lump_relative_sha256": current.game_lump_relative_digest(data),
    }


def damping_only(reference, measured):
    for key in ("model_indices", "body_counts", "solid_sha256", "other_blocks", "keys_except_damping_sha256", "bsp_version_revision", "lump_versions", "game_lump_relative_sha256"):
        require(measured[key] == reference[key], f"Damping changed unrelated {key}")
    require({key: value for key, value in measured["fluid"].items() if key != "damping"} == {key: value for key, value in reference["fluid"].items() if key != "damping"}, "Damping changed another fluid field")
    require(all(a == b for index, (a, b) in enumerate(zip(reference["lump_sha256"], measured["lump_sha256"], strict=True)) if index not in (29, 35)), "Damping changed an unrelated BSP payload")


def compile_pair(client, output, label, ids, damping, vector=None):
    source = output / f"{label}.vmf"
    client.call("save_map", {"path": str(source)})
    client.call("open_map", {"path": str(source)})
    before = snapshot(client, source, ids)
    destination = output / f"{label}.bsp"
    result = client.call("compile_map", {"path": str(destination)})
    require(result["success"] and result["stats"]["physics_fluids"] == 1 and result["stats"]["physics_models"] == 1, "Scalar fixture did not compile its exact fluid/model roles")
    data = read_bounded(destination, current.MAX_BSP)
    checked = check_bsp(data, damping, vector)
    background = output / f"{label}-background.bsp"
    started = client.call("start_compile", {"path": str(background)})["job"]
    job = wait_job(client)
    require(job["token"] == started["token"] and job["status"] == "completed" and job["result"]["published"] and not job["stale"], "Background scalar publication failed or changed its captured token")
    require(read_bounded(background, current.MAX_BSP) == data, "Foreground/background scalar BSP bytes differ")
    require(snapshot(client, source, ids) == before, "Scalar compilation changed source/history/protected Session state")
    return {"bsp": str(destination), "bsp_sha256": digest(data), "source": str(source), "source_sha256": before["source_sha256"], "requested_damping": damping, "vector_hu_per_second": vector or [0, 0, 0], "stats": result["stats"], "checked": checked}, data


def rejected_pair(client, source, ids, previous, expected_previous, fragment):
    before = snapshot(client, source, ids)
    failure = client.call("compile_map", {"path": str(previous)}, error=True)
    require(fragment.lower() in json.dumps(failure).lower(), "Rejected scalar import lacks its expected source diagnostic")
    require(read_bounded(previous, current.MAX_BSP) == expected_previous and snapshot(client, source, ids) == before, "Foreground rejection changed prior output/source/protected Session state")
    started = client.call("start_compile", {"path": str(previous)})["job"]
    job = wait_job(client)
    require(job["token"] == started["token"] and job["status"] == "failed" and not job["result"]["published"], "Rejected scalar background build did not remain unpublished")
    require(fragment.lower() in json.dumps(job).lower(), "Background rejection lost the scalar source diagnostic")
    require(read_bounded(previous, current.MAX_BSP) == expected_previous and snapshot(client, source, ids) == before, "Background rejection changed prior output/source/protected Session state")
    return {"foreground": failure, "background": job, "prior_output_preserved": True, "protected_state_unchanged": True}


def authoring(client, output, ids):
    source = output / "authoring-source.vmf"
    client.call("save_map", {"path": str(source)})
    before = snapshot(client, source, ids)
    metadata = world_field(client)
    check_raw_field(metadata, None)
    require(snapshot(client, source, ids) == before, "Discovering scalar metadata mutated source/history")
    token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
    staged = client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": KEY, "value": RAW_INTERIOR}]})
    preview = world_rows(staged["preview"])
    require(len(preview) == 1, "Scalar draft lost its field")
    check_raw_field(preview[0], RAW_INTERIOR)
    retained = snapshot(client, source, ids)
    errors = []
    for value in ("NaN", "0.0100000001", "1e-999", "1e-46"):
        failed = client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": "skyname", "value": "must_not_apply"}, {"action": "set", "key": KEY.upper(), "value": value}]}, error=True)
        require(KEY in json.dumps(failed) and client.call("get_property_edit") == staged, "Invalid late scalar batch replaced its retained valid draft")
        require(snapshot(client, source, ids) == retained, "Invalid scalar draft changed source/history/protected Session state")
        errors.append(failed)
    client.call("cancel_property_edit", {"token": token})
    require(snapshot(client, source, ids) == before, "Scalar Cancel changed source/history/protected Session state")
    count = client.call("get_history")["undo_count"]
    set_world(client, KEY, RAW_INTERIOR)
    check_raw_field(world_field(client), RAW_INTERIOR)
    history = client.call("get_history")
    require(history["undo_count"] == count + 1 and history["entries"][0]["actor"] == "agent", "MCP scalar Apply was not one Agent-authored transaction")
    client.call("undo")
    check_raw_field(world_field(client), None)
    client.call("redo")
    check_raw_field(world_field(client), RAW_INTERIOR)
    reopened = output / "authoring-roundtrip.vmf"
    client.call("save_map", {"path": str(reopened)})
    saved = read_bounded(reopened, MAX_SOURCE)
    client.call("open_map", {"path": str(reopened)})
    check_raw_field(world_field(client), RAW_INTERIOR)
    require(read_bounded(reopened, MAX_SOURCE) == saved, "Reopen changed exact scalar exponent/whitespace source")
    set_world(client, KEY, None)
    check_raw_field(world_field(client), None)
    set_world(client, KEY, "0.01")
    check_raw_field(world_field(client), "0.01")
    set_world(client, KEY, None)
    check_raw_field(world_field(client), None)
    return {"world_field": metadata, "invalid_draft_results": errors, "cancel_preserved_state": True, "one_agent_transaction": True, "undo_redo_exact_raw_value": True, "save_reopen_exact_raw_value": True, "remove_distinct_from_explicit_default": True, "actor_path": "public MCP Actor::Agent; User/Agent parity is covered by shared Session/UI tests"}


def negative_checker(data, output):
    offset, length = struct.unpack_from("<2i", data, 8 + 29 * 16)
    matches = list(re.finditer(rb'"damping"\s+"([^"\r\n]+)"', data[offset:offset + length]))
    require(len(matches) == 1 and len(matches[0].group(1)) >= 4, "Cannot identify the unique interior coefficient")
    match = matches[0]
    substitute = b"0.01".ljust(len(match.group(1)), b" ")
    changed = data[:offset + match.start(1)] + substitute + data[offset + match.end(1):]
    require(physics_records(changed)[0][1] == physics_records(data)[0][1], "Checker negative control changed collision bodies")
    (output / "rejected-allowed-coefficient.bsp").write_bytes(changed)
    try:
        check_bsp(changed, 0.005)
    except AssertionError as error:
        require("exact requested native coefficient" in str(error), "Scalar negative control failed for an unrelated reason")
    else:
        raise AssertionError("Allowed-but-wrong coefficient passed the scalar checker")


def compile_suite(args):
    game = runtime.private_game(args.game_dir, args.output / "source-game", engine_physics=True)
    output = args.output / "compile"
    output.mkdir()
    client = Client(None, args.binary, output, game / "gameinfo.txt")
    report = {"status": "failed", "cases": {}, "rejected": {}, "runtime_checked": False, "desktop_input": False, "modules": current.modules(args.game_dir)}
    try:
        with stopped_on_exit(client):
            ids, water = current.fixture(client)
            client.call("set_compile_settings", {"lighting": "fullbright", "physics": "engine", "generate_reflections": False, "pack_sky_assets": False})
            report["authoring"] = authoring(client, output, ids)
            seed = output / "water-damping-source.vmf"
            client.call("save_map", {"path": str(seed)})
            seed_bytes = read_bounded(seed, MAX_SOURCE)
            compiled = {}
            for label, raw, vector in CASES:
                client.call("open_map", {"path": str(seed)})
                if raw is not None:
                    set_world(client, KEY, raw)
                if vector is not None:
                    set_world(client, current.KEY, " ".join(map(str, vector)))
                damping = 0.01 if raw is None else float(raw)
                damping = 0.0 if damping == 0.0 else damping
                row, data = compile_pair(client, output, label, ids, damping, vector)
                if label in ("absent", "explicit-default", "current-only-128"):
                    current.check_bsp(data, vector)
                if label in ("explicit-default", "zero", "interior"):
                    damping_only(report["cases"]["absent"]["checked"], row["checked"])
                if label == "explicit-default":
                    require(data == compiled["absent"], "Explicit 0.01 changed complete absent-default BSP bytes")
                if label == "current-128-damping-zero":
                    damping_only(report["cases"]["current-only-128"]["checked"], row["checked"])
                report["cases"][label], compiled[label] = row, data
                write_json(args.output / "compile-results.json", report)
            require(read_bounded(seed, MAX_SOURCE) == seed_bytes, "Scalar suite changed its saved shared seed")
            negative_checker(compiled["interior"], output)
            current.reject_unscaled(compiled["current-only-128"], output)
            valid_source = Path(report["cases"]["interior"]["source"])
            saved = read_bounded(valid_source, MAX_SOURCE)
            text, needle = saved.decode(), f'"{KEY}" "{RAW_INTERIOR}"'
            require(text.count(needle) == 1, "Saved interior property is ambiguous")
            previous = output / "preserved.bsp"
            previous.write_bytes(compiled["interior"])
            for label, replacement, diagnostic in (
                ("malformed", f'"{KEY}" "0.0100000001"', "Stored damping is invalid"),
                ("duplicate", needle + f'\n"{KEY.upper()}" "0.005"', "duplicate damping keys"),
            ):
                source = output / f"invalid-{label}.vmf"
                source.write_text(text.replace(needle, replacement))
                client.call("open_map", {"path": str(source)})
                rows = world_rows(client.call("get_property_inspector", {"world": True}))
                require(rows and all(row["field"]["control"] == "text" and diagnostic in row["field"]["description"] for row in rows), "Invalid imported damping lost its visible Text diagnostic")
                if label == "malformed":
                    require(all(row["field"]["value"] == row["values"][0]["stored"] == "0.0100000001" for row in rows), "Invalid imported scalar text was changed or hidden")
                report["rejected"][label] = rejected_pair(client, source, ids, previous, compiled["interior"], KEY)
            for label in ("no-engine-zero", "no-water-default", "hidden-water", "detail-water", "model-water", "covered-water"):
                client.call("open_map", {"path": str(valid_source)})
                client.call("set_compile_settings", {"physics": "engine"})
                active_ids = ids.copy()
                if label == "no-engine-zero":
                    set_world(client, KEY, "0")
                    client.call("set_compile_settings", {"physics": "none"})
                elif label == "no-water-default":
                    set_world(client, KEY, "0.01")
                    client.call("delete_objects", {"ids": [water]})
                    active_ids.remove(water)
                elif label == "hidden-water":
                    client.call("set_object_visibility", {"ids": [water], "action": "hide"})
                elif label in ("detail-water", "model-water"):
                    tied = client.call("tie_to_entity", {"ids": [water], "classname": "func_detail" if label == "detail-water" else "func_brush"})["id"]
                    active_ids.remove(water)
                    active_ids.append(tied)
                else:
                    active_ids.append(client.call("create_block", {"mins": [value - 32 for value in current.POOL_MIN], "maxs": [value + 32 for value in current.POOL_MAX], "material": "dev/dev_measuregeneric01"})["id"])
                source = output / f"invalid-{label}.vmf"
                client.call("save_map", {"path": str(source)})
                report["rejected"][label] = rejected_pair(client, source, active_ids, previous, compiled["interior"], KEY)
                write_json(args.output / "compile-results.json", report)
            require(read_bounded(valid_source, MAX_SOURCE) == saved, "Rejected controls rewrote the saved valid scalar source")
            require(current.modules(args.game_dir) == report["modules"], "Native module inputs changed during scalar compilation")
            report.update({"status": "accepted", "shared_bytes_exact": True, "absent_explicit_default_whole_bsp_exact": True, "scalar_only_fields_and_framing": True, "native_collision_identity_unchanged": True, "all_unrelated_lumps_unchanged_after_game_offset_normalization": True, "allowed_wrong_scalar_checker_rejected": True, "current_units_unchanged": True, "previous_output_preserved": True})
            write_json(args.output / "compile-results.json", report)
            return report
    finally:
        write_json(args.output / "compile-results.json", report)
        client.close()


def main():
    parser = argparse.ArgumentParser(description="Validate optional water-pressure damping through isolated public MCP and independent compiled-byte controls")
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    require(args.output.is_relative_to(ROOT / "target"), "Retain evidence on disk below repository target/")
    require(not args.output.exists(), "Choose a new absent validation output")
    args.output.mkdir(parents=True, mode=0o700)
    with patch.dict(os.environ, {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent-maps"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json")}):
        report = compile_suite(args)
    write_json(args.output / "results.json", report)
    print(json.dumps({"status": report["status"], "runtime_checked": False, "cases": len(report["cases"]), "rejected": len(report["rejected"]), "output": str(args.output)}))


if __name__ == "__main__":
    main()
