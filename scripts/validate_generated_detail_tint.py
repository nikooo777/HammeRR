import argparse
import copy
import json
from pathlib import Path

import validate_detail_lighting as lighting
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_detail_props import SMALL_BUSH, compile_pair, detail_records
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref

KEY = "_generated_detail_lighting_tint"
MODEL_GROUND = "hammerr_detail_lighting/model_ground"


def world_tint(client, value):
    token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
    change = {"action": "remove", "key": KEY} if value is None else {"action": "set", "key": KEY, "value": value}
    client.call("update_property_edit", {"token": token, "changes": [change]})
    client.call("commit_property_edit", {"token": token})


def selected_rows(rows, sprites):
    result = {"models": rows["models"], "sprites": rows["sprites"], "props": []}
    names = ["style_lighting_ldr", "style_lighting_hdr"]
    for name in names:
        result[name] = None if rows[name] is None else []
    for prop in rows["props"]:
        if ("model" not in prop) != sprites:
            continue
        item = copy.deepcopy(prop)
        start, count = prop["light_styles"], prop["light_style_count"]
        item["light_styles"] = len(result[names[0]] or []) if count else 0
        for name in names:
            if result[name] is not None:
                result[name].extend(rows[name][start:start + count])
        result["props"].append(item)
    return result


def snapshot(client, ids):
    return {"objects": detail(client, ids), "history": client.call("get_history"), "world": client.call("get_property_inspector", {"world": True})}


def read_rows(data, orientation=2):
    return detail_records(data, allow_sprites=True, allow_baked=True, allow_styles=True, sprite_orientation=orientation)


def validate(args):
    setup_output = args.output / "lighting-fixture"
    setup_output.mkdir()
    setup = lighting.validate(argparse.Namespace(**{**vars(args), "output": setup_output}))
    game = Path(setup["fixture_game"])
    (game / f"materials/{MODEL_GROUND}.vmt").write_text(f'UnlitGeneric {{ $basetexture "{lighting.TEXTURE}" %detailtype hammerr_models }}\n')
    editor_output = args.output / "authoring"
    editor_output.mkdir()
    client = Client(None, args.binary, editor_output, game / "gameinfo.txt")
    try:
        with stopped_on_exit(client):
            client.call("load_entity_definitions", {"path": str(args.fgd)})
            client.call("open_map", {"path": setup["fixture_source"]})
            ids = list(setup["object_ids"])
            models = setup["explicit_model_ids"]
            light = setup["light_id"]
            client.call("set_entity_properties", {"id": light, "set": {"_light": lighting.CONTROLS["bright"], "_lightHDR": "255 255 255 100"}})
            for model in models:
                client.call("set_entity_properties", {"id": model, "set": {lighting.TINT_KEY: "64 255 128"}})
            checks = {}
            for mode in ["fullbright", "direct_ldr_hdr"]:
                client.call("set_compile_settings", {"lighting": mode, "generate_reflections": False, "pack_sky_assets": False})
                world_tint(client, None)
                _, reference, _ = compile_pair(client, args.output, f"mixed-{mode}-absent", ids)
                original = read_rows(reference)
                lighting.require(len(original["props"]) == 86, "Mixed fixture lost expected detail coverage")
                for name, factors in [("white", [255, 255, 255]), ("tinted", lighting.TINT), ("black", [0, 0, 0])]:
                    world_tint(client, " ".join(map(str, factors)))
                    _, data, _ = compile_pair(client, args.output, f"mixed-{mode}-{name}", ids)
                    rows = read_rows(data)
                    lighting.require(selected_rows(rows, False) == selected_rows(original, False), "World generated tint changed explicit detail tint or styles")
                    lighting.check_tint_colors(selected_rows(original, True), selected_rows(rows, True), factors)
                    if name == "white":
                        lighting.require(data == reference, f"Generated white tint changed whole BSP bytes in {mode}")
                    if mode == "fullbright":
                        lighting.require(all(row["lighting"] == [*factors, 0] for row in rows["props"] if "model" not in row), "Generated Fullbright did not use exact authored bytes")
                checks[f"mixed-{mode}"] = {"white_bytes_exact": True, "explicit_independent": True, "linear_once": True, "black_style_layout": True}
            world_tint(client, "255 32 16")
            source = args.output / "generated-tint.vmf"
            client.call("save_map", {"path": str(source)})
            saved = source.read_bytes()
            client.call("open_map", {"path": str(source)})
            field = next(row["field"] for row in client.call("get_property_inspector", {"world": True})["properties"] if row["field"]["key"] == KEY)
            lighting.require(field["label"] == "Generated detail lighting tint · linear RGB" and field["kind"] == "string" and field["control"] == "text" and field["value"] == "255 32 16", "World inspector does not expose the linear text contract")
            token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
            client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": KEY, "value": "128 64 0"}]})
            draft = client.call("get_property_edit")
            before = snapshot(client, ids)
            for value in ["", "1 2", "1 2 3 4", "-1 0 0", "256 0 0", "1.5 0 0", "NaN 0 0"]:
                client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": KEY.upper(), "value": value}]}, error=True)
                lighting.require(client.call("get_property_edit") == draft and snapshot(client, ids) == before, "Rejected world tint replaced a valid draft or source/history")
            client.call("cancel_property_edit", {"token": token})
            _, accepted, _ = compile_pair(client, args.output, "reopened", ids)
            previous = args.output / "rejected.bsp"
            previous.write_bytes(accepted)
            source_text = saved.decode()
            needle = f'"{KEY}" "255 32 16"'
            lighting.require(source_text.count(needle) == 1, "Saved tint key is ambiguous")
            failures = []
            for index, value in enumerate(["", "1 2", "1 2 3 4", "-1 0 0", "256 0 0", "1.5 0 0", "NaN 0 0", None]):
                replacement = f'"{KEY}" "{value}"' if value is not None else needle + f'\n"{KEY.upper()}" "255 255 255"'
                invalid = args.output / f"invalid-{index}.vmf"
                invalid.write_text(source_text.replace(needle, replacement))
                client.call("open_map", {"path": str(invalid)})
                before = snapshot(client, ids)
                failure = json.loads(client.call("compile_map", {"path": str(previous)}, error=True)["content"][0]["text"])
                lighting.require(not failure["success"] and KEY in json.dumps(failure["diagnostics"]), failure)
                client.call("start_compile", {"path": str(previous)})
                lighting.require(wait_job(client)["status"] == "failed" and previous.read_bytes() == accepted and snapshot(client, ids) == before, "Imported tint rejection changed output, source or history")
                failures.append(failure["diagnostics"])
            client.call("open_map", {"path": str(source)})
            lighting.require(source.read_bytes() == saved, "Tint controls rewrote the saved source")
            client.call("delete_objects", {"ids": models})
            ids = [item for item in ids if item not in models]
            patch = client.call("create_block", {"mins": [-152, 32, 0], "maxs": [-40, 176, 8], "material": lighting.PLAIN})["id"]
            ids.append(patch)
            brush = detail(client, [patch])[0]
            top = next(face for face in brush["faces"] if face["normal"] == [0, 0, 1])
            client.call("edit_faces", {"faces": [face_ref(brush, top)], "material": MODEL_GROUND})
            controls = {}
            for suite, orientation, styled, modes in [("fast", 2, False, ["direct_ldr"]), ("slow", 0, False, ["direct_ldr"]), ("styled", 2, True, ["direct_ldr", "direct_ldr_hdr"])]:
                (game / lighting.DICTIONARY).write_text(f'detail {{ hammerr_tufts {{ density 4096 group {{ alpha 1 sprite {{ sprite "0 0 128 128 512" spritesize "0.5 0 28 40" detailOrientation {orientation} upright 1 }} }} }} hammerr_models {{ density 128 group {{ alpha 1 model {{ model "{SMALL_BUSH}" upright 1 }} }} }} }}\n')
                if styled:
                    client.call("set_entity_properties", {"id": light, "set": {"targetname": "hammerr_detail_keylight"}})
                    ids.append(client.call("create_entity", {"classname": "light", "origin": [0, 96, 180], "properties": {"_light": "255 255 255 20", "_lightHDR": "255 255 255 10", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}})["id"])
                for mode in modes:
                    client.call("set_compile_settings", {"lighting": mode})
                    reference = None
                    for name, factors in [("white", [255, 255, 255]), ("tinted", lighting.TINT)]:
                        world_tint(client, " ".join(map(str, factors)))
                        bsp, data, _ = compile_pair(client, args.output, f"{suite}-{mode}-{name}", ids)
                        rows = read_rows(data, orientation)
                        lighting.require(len(rows["props"]) == 86 and sum("model" in row for row in rows["props"]) == 2, "Generated native fixture must retain two models and 84 cards")
                        lighting.require(all(row["light_style_count"] == int(styled) for row in rows["props"]), "Generated native fixture selected a different precision/style path")
                        if reference is None:
                            reference = rows
                        else:
                            lighting.check_tint_colors(reference, rows, factors)
                        controls[f"{suite}-{mode}-{name}"] = {"bsp": str(bsp), "bsp_sha256": lighting.probe.digest(data), "orientation": orientation, "styled": styled, "hdr_level": 2 if mode == "direct_ldr_hdr" else 0, "count": 86, "sprites": 84}
                    checks[f"{suite}-{mode}"] = {"all_generated": True, "linear_once": True, "foreground_background_exact": True}
            client.call("save_map", {"path": str(args.output / "generated-native.vmf")})
            report = {"success": True, "tint": lighting.TINT, "checks": checks, "controls": controls, "rejected": failures, "fixture_game": str(game), "source_history_preserved": True, "foreground_background_exact": True, "saved_reopened": True, "map_wide_generated_only": True, "native_exercised": False}
            (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
            return report
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--fgd", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/SourceSDK/bin/source2009/bin/cstrike.fgd")
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--probe-library", type=Path, default=Path(__file__).resolve().parent.parent / "target/debug/hammerr_hdr_probe.so")
    parser.add_argument("--hdr-control", type=Path)
    parser.add_argument("--port", type=int, default=27729)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    args.output = args.output.resolve()
    if args.hdr_control is not None:
        args.hdr_control = args.hdr_control.resolve(strict=True)
    lighting.require(not args.output.exists(), "Use a fresh absent output directory")
    lighting.require(1024 <= args.port <= 65530, "Invalid native port")
    args.output.mkdir(parents=True, mode=0o700)
    if args.runtime:
        args.probe_library = args.probe_library.resolve(strict=True)
    report = validate(args)
    if args.runtime:
        lighting.native_tint(args, report)
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"success": True, "results": str(args.output / "results.json"), "native_exercised": report["native_exercised"]}))


if __name__ == "__main__":
    main()
