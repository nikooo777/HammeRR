import argparse
import json
import subprocess
import sys
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_native_runtime as runtime
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_cubemap_reflections import ppm
from validate_detail_props import SMALL_BUSH, compile_pair, detail_records
from validate_explicit_detail_sprites import FIELDS as SPRITE_FIELDS
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_material_details import set_world, state
from validate_visibility import ROOM

PLAIN = "hammerr_detail_lighting/plain"
GROUND = "hammerr_detail_lighting/ground"
TEXTURE = "dev/dev_measuregeneric01"
DICTIONARY = "scripts/hammerr_detail_lighting.vbsp"
CONTROLS = {"bright": "255 255 255 180", "dim": "255 255 255 40", "colored": "255 48 48 180"}
REGIONS = {"models": (230, 215, 590, 620), "sprites": (680, 215, 1160, 620)}
TINT_KEY = "_detail_lighting_tint"
TINT = [255, 32, 16]


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def linear(row):
    red, green, blue, exponent = row["lighting"]
    return [value * 2.0**exponent for value in [red, green, blue]]


def placement(rows):
    return {**rows, "payload_sha256": None, "props": [{key: value for key, value in row.items() if key != "lighting"} for row in rows["props"]]}


def validate(args):
    game = runtime.private_game(args.game_dir, args.output / "source-game")
    materials = game / "materials/hammerr_detail_lighting"
    materials.mkdir(parents=True)
    (materials / "plain.vmt").write_text(f'UnlitGeneric {{ $basetexture "{TEXTURE}" }}\n')
    (materials / "ground.vmt").write_text(f'UnlitGeneric {{ $basetexture "{TEXTURE}" %detailtype hammerr_tufts }}\n')
    (game / DICTIONARY).write_text('detail { hammerr_tufts { density 4096 group { alpha 1 sprite { sprite "0 0 128 128 512" spritesize "0.5 0 28 40" detailOrientation 2 upright 1 } } } }\n')
    client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    try:
        with stopped_on_exit(client):
            client.call("load_entity_definitions", {"path": str(args.fgd)})
            ids = [client.call("create_block", {"mins": low, "maxs": high, "material": PLAIN})["id"] for low, high in ROOM]
            ceiling = detail(client, [ids[1]])[0]
            bottom = next(face for face in ceiling["faces"] if face["normal"] == [0, 0, -1])
            client.call("edit_faces", {"faces": [face_ref(ceiling, bottom)], "material": TEXTURE})
            patch = client.call("create_block", {"mins": [48, 32, 0], "maxs": [192, 176, 8], "material": PLAIN})["id"]
            ids.append(patch)
            brush = detail(client, [patch])[0]
            top = next(face for face in brush["faces"] if face["normal"] == [0, 0, 1])
            client.call("edit_faces", {"faces": [face_ref(brush, top)], "material": GROUND})
            explicit_models = []
            for classname, origin, fields in [
                ("info_player_terrorist", [-192, -176, 8], {}),
                ("info_player_counterterrorist", [192, 176, 8], {}),
                ("logic_script", [0, -176, 64], {"targetname": "hammerr_detail_observer", "vscripts": "hammerr_detail_observer.nut", "thinkfunction": "HammeRRThink"}),
                ("prop_detail", [-96, 64, 20], {"model": SMALL_BUSH}),
                ("prop_detail", [-96, 128, 20], {"model": SMALL_BUSH}),
            ]:
                entity = client.call("create_entity", {"classname": classname, "origin": origin, "properties": fields})["id"]
                ids.append(entity)
                if classname == "prop_detail":
                    explicit_models.append(entity)
            light = client.call("create_entity", {"classname": "light", "origin": [0, 96, 180], "properties": {"_light": CONTROLS["bright"], "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}})["id"]
            ids.append(light)
            set_world(client, "detailvbsp", DICTIONARY)
            client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "none", "bounces": 0, "sky_quality": "fast"})
            controls = {}
            for name, value in CONTROLS.items():
                client.call("set_entity_properties", {"id": light, "set": {"_light": value}})
                source = args.output / f"{name}.vmf"
                client.call("save_map", {"path": str(source)})
                saved = source.read_bytes()
                authored = detail(client, ids)
                client.call("open_map", {"path": str(source)})
                require(detail(client, ids) == authored, "Save/reopen changed the detail-lighting source")
                bsp, data, result = compile_pair(client, args.output, name, ids)
                rows = detail_records(data, allow_sprites=True, allow_baked=True)
                require(sum("model" in row for row in rows["props"]) == 2, rows)
                require(sum(row.get("kind") == 1 for row in rows["props"]) == 84, rows)
                require(len(rows["models"]) == len(rows["sprites"]) == 1, rows)
                require(result["stats"]["detail_props"] == 86 and result["stats"]["detail_prop_models"] == result["stats"]["detail_prop_sprites"] == 1, result["stats"])
                require(all(max(linear(row)) > 0 for row in rows["props"]), "Fixture contains black receivers")
                if name == "colored":
                    require(all(linear(row)[0] > linear(row)[1] * 4 for row in rows["props"]), "Colored detail controls did not bake red-dominant values")
                try:
                    detail_records(data, allow_sprites=True)
                except AssertionError:
                    pass
                else:
                    raise AssertionError("Default white reader accepted nonwhite baked records")
                require(source.read_bytes() == saved and detail(client, ids) == authored, "Compiling changed source or saved bytes")
                controls[name] = {"bsp": str(bsp), "bsp_sha256": probe.digest(data), "records": rows, "stats": result["stats"]}
            require(all(placement(item["records"]) == placement(controls["bright"]["records"]) for item in controls.values()), "Lighting controls changed detail placement")
            require(all(max(linear(a)) > max(linear(b)) * 4 for a, b in zip(controls["bright"]["records"]["props"], controls["dim"]["records"]["props"], strict=True)), "Bright/dim record ordering failed")
            client.call("set_compile_settings", {"lighting": "fullbright"})
            _, white, _ = compile_pair(client, args.output, "fullbright-white", ids)
            detail_records(white, allow_sprites=True)
            require(placement(detail_records(white, allow_sprites=True, allow_baked=True)) == placement(controls["bright"]["records"]), "Fullbright changed the detail layout")
            client.call("set_compile_settings", {"lighting": "direct_ldr_hdr"})
            client.call("set_entity_properties", {"id": light, "set": {"_lightHDR": "0 0 255 100"}})
            _, dual, _ = compile_pair(client, args.output, "dual-shared-base", ids)
            dual_rows = detail_records(dual, allow_sprites=True, allow_baked=True)
            require(all(linear(row)[2] > 0 and linear(row)[0] == linear(row)[1] == 0 for row in dual_rows["props"]), "Dual-channel details did not retain the final HDR shared base")
            result = {"success": True, "controls": controls, "fixture_game": str(game), "explicit_model_ids": explicit_models, "light_id": light, "object_ids": ids, "fixture_source": str(args.output / "bright.vmf"), "source_history_preserved": True, "foreground_background_exact": True, "fullbright_white_default_preserved": True, "hdr_shared_base": True, "native_exercised": False}
            (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
            return result
    finally:
        client.close()


def native_colors(args, result):
    frames = {}
    for index, (name, control) in enumerate(result["controls"].items()):
        output = args.output / f"native-{name}"
        rows = control["records"]["props"]
        subprocess.run([sys.executable, "-B", str(Path(__file__).with_name("validate_detail_props.py")),
            "--binary", str(args.binary), "--output", str(output), "--game-dir", str(args.game_dir),
            "--runtime", "--runtime-bsp", control["bsp"], "--fixture-game", result["fixture_game"],
            "--expected-count", str(len(rows)), "--expected-sprites", str(sum(row.get("kind") == 1 for row in rows)),
            "--baked-lighting", "--port", str(args.port + index)], check=True, timeout=150)
        native = json.loads((output / "results.json").read_text())
        require(native["native_exercised"] and native["native"]["success"], native)
        frames[name] = {"on": ppm(output / "runtime/details-on.ppm"), "off": ppm(output / "runtime/details-off.ppm")}
        control["native"] = native["native"]
    measurements = {}
    for kind, (left, top, right, bottom) in REGIONS.items():
        mask = []
        off_delta = []
        for y in range(top, bottom):
            for x in range(left, right):
                offset = (y * probe.WIDTH + x) * 3
                pixels = {name: {state: frame[state][offset:offset + 3] for state in ["on", "off"]} for name, frame in frames.items()}
                off_delta.extend(abs(a - b) for a, b in zip(pixels["bright"]["off"], pixels["dim"]["off"], strict=True))
                off_delta.extend(abs(a - b) for a, b in zip(pixels["bright"]["off"], pixels["colored"]["off"], strict=True))
                if all(max(abs(a - b) for a, b in zip(item["on"], item["off"], strict=True)) > 12 for item in pixels.values()):
                    mask.append(offset)
        require(sum(off_delta) / len(off_delta) < 0.25, f"{kind} background changed between lighting controls")
        require(len(mask) > 150, f"{kind} lacks a stable detail ON-minus-OFF mask: {len(mask)}")
        means = {name: [sum(frame["on"][offset + axis] for offset in mask) / len(mask) for axis in range(3)] for name, frame in frames.items()}
        bright, dim, colored = means["bright"], means["dim"], means["colored"]
        require(sum(bright) > sum(dim) * 1.2 + 3, f"{kind} did not brighten: {means}")
        require(colored[1] < bright[1] * 0.85 and colored[0] / max(bright[0], 1) > colored[1] / max(bright[1], 1) + 0.1, f"{kind} did not receive predicted red tint: {means}")
        measurements[kind] = {"region": [left, top, right, bottom], "stable_detail_pixels": len(mask), "foreground_rgb": means, "background_mean_difference": sum(off_delta) / len(off_delta)}
    result.update({"native_exercised": True, "native_detail_color": measurements})
    (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")



def tint_layout(rows):
    return {"models": rows["models"], "sprites": rows["sprites"], "props": [{key: value for key, value in row.items() if key != "lighting"} for row in rows["props"]], "ldr_styles": [row[4] for row in rows["style_lighting_ldr"] or []], "hdr_styles": [row[4] for row in rows["style_lighting_hdr"] or []]}


def check_tint_colors(reference, tinted, tint):
    require(tint_layout(reference) == tint_layout(tinted), "Tint changed placement, native precision path, dictionaries or style membership")
    pairs = [(left["lighting"], right["lighting"]) for left, right in zip(reference["props"], tinted["props"], strict=True)]
    for name in ["style_lighting_ldr", "style_lighting_hdr"]:
        require((reference[name] is None) == (tinted[name] is None), "Tint changed the channel style arrays")
        pairs.extend(zip(reference[name] or [], tinted[name] or [], strict=True))
    for left, right in pairs:
        for axis, channel in enumerate(tint):
            expected = left[axis] * 2.0**left[3] * channel / 255
            actual = right[axis] * 2.0**right[3]
            tolerance = 2.0**right[3] + 2.0**left[3] * channel / 255
            require(actual == 0 if channel == 0 else abs(actual - expected) <= tolerance, f"Tint was not one linear attenuation before RGBExp32 encoding: {left}, {right}, {tint}")


def set_tint(client, targets, tint):
    for target in targets:
        changes = {"remove": [TINT_KEY]} if tint is None else {"set": {TINT_KEY: " ".join(map(str, tint))}}
        client.call("set_entity_properties", {"id": target, **changes})


def validate_tint(args):
    setup_output = args.output / "lighting-fixture"
    setup_output.mkdir()
    setup = validate(argparse.Namespace(**{**vars(args), "output": setup_output}))
    game = Path(setup["fixture_game"])
    (game / f"materials/{GROUND}.vmt").write_text(f'UnlitGeneric {{ $basetexture "{TEXTURE}" }}\n')
    editor_output = args.output / "tint-authoring"
    editor_output.mkdir()
    client = Client(None, args.binary, editor_output, game / "gameinfo.txt")
    try:
        with stopped_on_exit(client):
            client.call("load_entity_definitions", {"path": str(args.fgd)})
            client.call("open_map", {"path": setup["fixture_source"]})
            ids = list(setup["object_ids"])
            models = setup["explicit_model_ids"]
            sprites = []
            for row in setup["controls"]["bright"]["records"]["props"]:
                if row.get("kind") == 1:
                    sprites.append(client.call("create_entity", {"classname": "prop_detail_sprite", "origin": [*row["origin"][:2], row["origin"][2] + 0.125], "properties": {**SPRITE_FIELDS, "position_ul": "-14 40", "position_lr": "14 0", "angles": "0 90 0"}})["id"])
            require(len(models) == 2 and len(sprites) == 84, "Tint fixture lost explicit model/card coverage")
            ids += sprites
            targets = [*models, *sprites]
            light = setup["light_id"]
            client.call("set_entity_properties", {"id": light, "set": {"_light": CONTROLS["bright"], "_lightHDR": "255 255 255 100"}})
            source = args.output / "detail-tint.vmf"
            client.call("save_map", {"path": str(source)})
            saved = source.read_bytes()
            authored = detail(client, ids)
            client.call("open_map", {"path": str(source)})
            require(detail(client, ids) == authored, "Explicit tint fixture changed on save/reopen")
            controls = {}
            checks = {}
            suites = [("fast", 2, False, ["fullbright", "direct_ldr", "direct_ldr_hdr"]), ("slow", 0, False, ["direct_ldr"]), ("tri", 0, False, ["direct_ldr"]), ("styled", 2, True, ["direct_ldr", "direct_ldr_hdr"])]
            for suite, orientation, styled, modes in suites:
                for sprite in sprites:
                    client.call("set_entity_properties", {"id": sprite, "set": {"detailOrientation": str(orientation), "sprite_shape": "tri" if suite == "tri" else "", "shape_angle": "35" if suite == "tri" else "0", "shape_size": "0.5" if suite == "tri" else "0"}})
                if styled:
                    client.call("set_entity_properties", {"id": light, "set": {"targetname": "hammerr_detail_keylight"}})
                    ids.append(client.call("create_entity", {"classname": "light", "origin": [0, 96, 180], "properties": {"_light": "255 255 255 20", "_lightHDR": "255 255 255 10", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}})["id"])
                for mode in modes:
                    client.call("set_compile_settings", {"lighting": mode, "physics": "none", "bounces": 0})
                    set_tint(client, targets, None)
                    _, baseline, _ = compile_pair(client, args.output, f"{suite}-{mode}-absent", ids)
                    reference = detail_records(baseline, allow_sprites=True, allow_baked=True, sprite_orientation=orientation, allow_shapes=suite == "tri", allow_styles=True)
                    require(len(reference["props"]) == 86 and len(reference["models"]) == len(reference["sprites"]) == 1, reference)
                    require(all(row["light_style_count"] == int(styled) for row in reference["props"]), "Fixture did not select the intended sprite precision/style path")
                    for name, tint in [("white", [255, 255, 255]), ("tinted", TINT), ("black", [0, 0, 0])]:
                        set_tint(client, targets, tint)
                        bsp, data, result = compile_pair(client, args.output, f"{suite}-{mode}-{name}", ids)
                        rows = detail_records(data, allow_sprites=True, allow_baked=True, sprite_orientation=orientation, allow_shapes=suite == "tri", allow_styles=True)
                        require(result["stats"]["detail_props"] == 86, result)
                        check_tint_colors(reference, rows, tint)
                        if mode == "fullbright":
                            require(all(row["lighting"] == [*tint, 0] for row in rows["props"]) and rows["style_lighting_ldr"] is rows["style_lighting_hdr"] is None, "Fullbright did not retain exact authored tint bytes without styles")
                        if name == "white":
                            require(data == baseline, f"Explicit white tint changed omitted-tint bytes in {suite}/{mode}")
                        for index, (before, after) in enumerate(zip(probe.bsp_lumps(baseline), probe.bsp_lumps(data), strict=True)):
                            if index != 35:
                                require(before == after, f"Tint changed unrelated BSP lump {index}")
                        key = f"{suite}-{mode}-{name}"
                        if mode != "fullbright" and suite != "tri" and name != "black":
                            controls[key] = {"bsp": str(bsp), "bsp_sha256": probe.digest(data), "orientation": orientation, "styled": styled, "hdr_level": 2 if mode == "direct_ldr_hdr" else 0, "count": len(rows["props"]), "sprites": 84}
                    checks[f"{suite}-{mode}"] = {"white_bytes_exact": True, "linear_attenuation_once": True, "black_preserves_style_layout": True, "unrelated_lumps_exact": True}
            set_tint(client, targets, TINT)
            client.call("save_map", {"path": str(args.output / "detail-tint-authored.vmf")})
            tinted_authored = detail(client, ids)
            client.call("open_map", {"path": str(args.output / "detail-tint-authored.vmf")})
            require(detail(client, ids) == tinted_authored, "Authored tint did not survive save/reopen")
            inspector = client.call("get_entity_inspector", {"id": sprites[0]})
            field = next(field for field in inspector["properties"] if field["key"] == TINT_KEY)
            require(field["advanced"] and field["value"] == "255 32 16", "Shared Advanced properties did not expose authored tint")
            require(field["label"] == "Baked lighting tint · linear RGB", "Inspector tint label does not state its baked linear meaning")
            _, accepted, _ = compile_pair(client, args.output, "tint-reopened", ids)
            previous = args.output / "tint-rejected.bsp"
            previous.write_bytes(accepted)
            failures = {}
            for index, value in enumerate(["", "1 2", "1 2 3 4", "128.5 64 32", "NaN 0 0", "-1 0 0", "256 0 0"]):
                target = models[0] if index % 2 else sprites[0]
                client.call("set_entity_properties", {"id": target, "set": {TINT_KEY: value}})
                before = state(client, ids, source)
                failure = json.loads(client.call("compile_map", {"path": str(previous)}, error=True)["content"][0]["text"])
                require(not failure["success"] and TINT_KEY in json.dumps(failure["diagnostics"]), failure)
                require(previous.read_bytes() == accepted and state(client, ids, source) == before, "Rejected tint changed output, source or history")
                client.call("start_compile", {"path": str(previous)})
                require(wait_job(client)["status"] == "failed" and previous.read_bytes() == accepted and state(client, ids, source) == before, "Background tint rejection changed output, source or history")
                failures[value] = failure["diagnostics"]
                client.call("undo")
                require(detail(client, ids) == tinted_authored, "Tint rejection Undo changed authored fields")
            require(source.read_bytes() == saved, "Tint controls rewrote the original saved source")
            report = {"success": True, "tint": TINT, "checks": checks, "controls": controls, "rejected": failures, "fixture_game": str(game), "source_history_preserved": True, "foreground_background_exact": True, "saved_reopened": True, "native_exercised": False}
            (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
            return report
    finally:
        client.close()


def tint_color_measurements(frames):
    measurements = {}
    for kind, (left, top, right, bottom) in REGIONS.items():
        mask = []
        background = []
        for y in range(top, bottom):
            for x in range(left, right):
                offset = (y * probe.WIDTH + x) * 3
                pixels = {name: {state: frame[state][offset:offset + 3] for state in ["on", "off"]} for name, frame in frames.items()}
                background.extend(abs(a - b) for a, b in zip(pixels["white"]["off"], pixels["tinted"]["off"], strict=True))
                if all(max(abs(a - b) for a, b in zip(value["on"], value["off"], strict=True)) > 12 for value in pixels.values()):
                    mask.append(offset)
        require(background and max(background) == 0, f"{kind} detail-disabled background changed under baked tint")
        require(len(mask) > 150, f"{kind} lacks stable visible detail pixels for tint control")
        means = {name: [sum(frame["on"][offset + axis] for offset in mask) / len(mask) for axis in range(3)] for name, frame in frames.items()}
        white, tinted = means["white"], means["tinted"]
        require(tinted[1] < white[1] * 0.85 and tinted[0] / max(white[0], 1) > tinted[1] / max(white[1], 1) + 0.1, f"{kind} did not show predicted linear red-preserving tint: {means}")
        measurements[kind] = {"stable_detail_pixels": len(mask), "foreground_rgb": means, "background_max_difference": max(background)}
    return measurements


def native_tint(args, report):
    captures = {}
    frames = {}
    choices = [("fast-direct_ldr", None), ("slow-direct_ldr", None), ("styled-direct_ldr", "on"), ("styled-direct_ldr", "off"), ("styled-direct_ldr_hdr", "on")]
    for suite, style in choices:
        pair = {}
        for color in ["white", "tinted"]:
            control = report["controls"][f"{suite}-{color}"]
            name = f"{suite}-{style or 'static'}-{color}"
            output = args.output / f"native-{name}"
            command = [sys.executable, "-B", str(Path(__file__).with_name("validate_detail_props.py")), "--binary", str(args.binary), "--output", str(output), "--game-dir", str(args.game_dir), "--runtime", "--runtime-bsp", control["bsp"], "--fixture-game", report["fixture_game"], "--expected-count", str(control["count"]), "--expected-sprites", str(control["sprites"]), "--sprite-orientation", str(control["orientation"]), "--baked-lighting", "--hdr-level", str(control["hdr_level"]), "--probe-library", str(args.probe_library), "--port", str(args.port)]
            if style is not None:
                command += ["--allow-detail-styles", "--detail-style-state", style]
            subprocess.run(command, check=True, timeout=150)
            result = json.loads((output / "results.json").read_text())
            require(result["native_exercised"] and result["native"]["success"] and result["bsp_sha256"] == control["bsp_sha256"], result)
            pair[color] = {"on": ppm(output / "runtime/details-on.ppm"), "off": ppm(output / "runtime/details-off.ppm")}
            captures[name] = result["native"]
        measurements = tint_color_measurements(pair)
        frames[f"{suite}-{style}"] = pair
        captures[f"{suite}-{style}-measurements"] = measurements
    switching = {}
    on, off = frames["styled-direct_ldr-on"], frames["styled-direct_ldr-off"]
    for kind, (left, top, right, bottom) in REGIONS.items():
        offsets = [(y * probe.WIDTH + x) * 3 for y in range(top, bottom) for x in range(left, right)]
        mask = [offset for offset in offsets if all(max(abs(a - b) for a, b in zip(pair[color]["on"][offset:offset + 3], pair[color]["off"][offset:offset + 3], strict=True)) > 12 for pair in [on, off] for color in ["white", "tinted"])]
        require(len(mask) > 150, f"{kind} lacks stable style-switching tint pixels")
        means = {state: {color: sum(sum(pair[color]["on"][offset:offset + 3]) for offset in mask) / len(mask) for color in ["white", "tinted"]} for state, pair in [("on", on), ("off", off)]}
        require(all(means["on"][color] > means["off"][color] * 1.15 + 3 for color in ["white", "tinted"]), f"Tinted detail styles did not switch: {means}")
        background = [abs(on[color]["off"][offset + axis] - off[color]["off"][offset + axis]) for color in ["white", "tinted"] for offset in offsets for axis in range(3)]
        require(background and max(background) == 0, "Tinted style controls changed the detail-disabled region background")
        switching[kind] = {"stable_detail_pixels": len(mask), "foreground_channel_sum": means}
    report.update({"native_exercised": True, "native_tint": captures, "actual_style_switching": switching})
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--fgd", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/SourceSDK/bin/source2009/bin/cstrike.fgd")
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--tint", action="store_true", help="Validate explicit linear baked-lighting tint, identity bytes and native fast/slow/style/HDR controls")
    parser.add_argument("--probe-library", type=Path, default=Path(__file__).resolve().parent.parent / "target/debug/hammerr_hdr_probe.so")
    parser.add_argument("--port", type=int, default=27719)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    args.output = args.output.resolve()
    require(not args.output.exists(), "Use a fresh absent output directory")
    require(1024 <= args.port <= 65530, "Invalid native port")
    args.output.mkdir(parents=True, mode=0o700)
    if args.tint and args.runtime:
        args.probe_library = args.probe_library.resolve(strict=True)
    result = validate_tint(args) if args.tint else validate(args)
    if args.runtime:
        native_tint(args, result) if args.tint else native_colors(args, result)
    print(json.dumps({"success": True, "results": str(args.output / "results.json"), "native_exercised": result["native_exercised"]}))


if __name__ == "__main__":
    main()
