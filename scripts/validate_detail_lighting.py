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
HDR_CAPTURE_LIMITS = {"off_max": 1, "red_max": 2, "mean": 0.01}


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


def pixel_deviation(values):
    return {"maximum": max(values), "mean": sum(values) / len(values), "changed_samples": sum(value != 0 for value in values), "samples": len(values)}


def check_hdr_deviation(control, target, maximum, name):
    require(control["maximum"] <= maximum and control["mean"] <= HDR_CAPTURE_LIMITS["mean"], f"Identical-white HDR {name} control exceeds hard capture limits: {control}")
    require(target["maximum"] <= min(control["maximum"], maximum) and target["mean"] <= HDR_CAPTURE_LIMITS["mean"], f"HDR {name} differs beyond measured amplitude or hard mean limits: {control}, {target}")


def tint_color_measurements(frames, hdr_control=None):
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
        require(background, f"{kind} has no detail-disabled background control")
        if hdr_control is None:
            require(max(background) == 0, f"{kind} detail-disabled background changed under baked tint")
        require(len(mask) > 150, f"{kind} lacks stable visible detail pixels for tint control")
        means = {name: [sum(frame["on"][offset + axis] for offset in mask) / len(mask) for axis in range(3)] for name, frame in frames.items()}
        delta = [[frames["white"]["on"][offset + axis] - frames["tinted"]["on"][offset + axis] for offset in mask] for axis in range(3)]
        noise = None
        minimum_reduction = 3
        if hdr_control is None:
            require(all(value == 0 for value in delta[0]), f"{kind} tint changed the unattenuated red control")
        else:
            offsets = [(y * probe.WIDTH + x) * 3 for y in range(top, bottom) for x in range(left, right)]
            control_off = pixel_deviation([abs(frames["white"]["off"][offset + axis] - hdr_control["off"][offset + axis]) for offset in offsets for axis in range(3)])
            target_off = pixel_deviation(background)
            control_red = pixel_deviation([abs(frames["white"]["on"][offset] - hdr_control["on"][offset]) for offset in mask])
            target_red = pixel_deviation(list(map(abs, delta[0])))
            check_hdr_deviation(control_off, target_off, HDR_CAPTURE_LIMITS["off_max"], f"{kind} background")
            check_hdr_deviation(control_red, target_red, HDR_CAPTURE_LIMITS["red_max"], f"{kind} unattenuated red")
            minimum_reduction = max(minimum_reduction, control_red["maximum"] + 1)
            noise = {"identical_white_background": control_off, "tint_background": target_off, "identical_white_red": control_red, "tint_red": target_red, "hard_limits": HDR_CAPTURE_LIMITS, "changed_frequency_reproduced": False, "policy": "Target maxima must fit measured identical-white amplitudes without scaling; each mean independently stays at most 0.01 code values. Changed frequency did not match the repeat; the downstream cause is unconfirmed."}
        require(all(value >= 0 for channel in delta[1:] for value in channel), f"{kind} attenuated channels increased under tint")
        reductions = [sum(value >= minimum_reduction for value in channel) for channel in delta[1:]]
        require(all(count > 150 for count in reductions), f"{kind} lacks actual green/blue tint reductions: {reductions}")
        measurements[kind] = {"stable_detail_pixels": len(mask), "foreground_rgb": means, "background_max_difference": max(background), "red_max_difference": max(map(abs, delta[0])), "green_blue_decreased_pixels": reductions, "green_blue_max_increase": max(0, -min(min(channel) for channel in delta[1:])), "significant_minimum_decrease": minimum_reduction, "hdr_capture_control": noise, "oracle": "LDR red is exact; HDR red/background require independent identical-white amplitude controls and explicit hard limits. Green/blue attenuate monotonically per pixel; common alpha-background terms cancel."}
    return measurements



def checked_tint_capture(args, control, output, style):
    from validate_hdr_lighting import read_probe
    from validate_hdr_reflections import verify_hardware

    result = json.loads(probe.read_bounded(output / "results.json", 4 * 1024**2))
    native = result["native"]
    data = probe.read_bounded(Path(control["bsp"]), probe.MAX_BSP_BYTES)
    require(result["success"] and result["native_exercised"] and native["success"], "Existing native capture did not complete successfully")
    require(probe.digest(data) == control["bsp_sha256"] == result["bsp_sha256"] == native["bsp_sha256"], "Existing tint capture BSP identity changed")
    require(native == json.loads(probe.read_bounded(output / "runtime/runtime.json", 4 * 1024**2)), "Existing worker and parent native receipts differ")
    rows = detail_records(data, allow_sprites=True, allow_baked=True, sprite_orientation=control["orientation"], allow_styles=control["styled"])
    require(rows == result["records"] and len(rows["props"]) == control["count"] and sum(row.get("kind") == 1 for row in rows["props"]) == control["sprites"], "Existing capture record structure differs")
    require(all(row.get("light_style_count", 0) == int(control["styled"]) for row in rows["props"]), "Existing capture sprite precision/style path differs")
    require(native["exact_compiled_bsp"] and native["sprite_orientation"] == control["orientation"] and native["hdr_level"] == control["hdr_level"] and native["view"] == "level" and native["detail_style_state"] == style, "Existing native capture controls differ")
    require("-insecure" in native["command"] and "-default" in native["command"] and native["desktop_input"] is False and native["filesystem_isolation"] == {"external_paths": "read_only", "writable_directory": str(output / "runtime")}, "Existing native capture isolation differs")
    require(native["module_fingerprints"] == probe.module_fingerprints(args.game_dir), "Native game modules changed since the capture")
    controls = native["private_launch"]["controls"]
    require(controls["sv_lan"] == 1 and controls["mat_fullbright"] == 0 and controls["mat_hdr_level"] == control["hdr_level"], "Existing native launch settings differ")
    hardware = verify_hardware(read_probe(output / "runtime/hardware.jsonl"), control["hdr_level"])
    require(hardware is not None and hardware == native["hardware"], "Existing native hardware evidence differs")
    for name in ["worker", "compositor"]:
        audit = json.loads(probe.read_bounded(output / f"runtime/private-preservation-{name}.json", 4 * 1024**2))
        require(audit["success"] and not audit["errors"], "Existing native preservation audit failed")
    cleanup = json.loads(probe.read_bounded(output / "runtime/private-cleanup.json", 1024**2))
    require(cleanup == {"worker_result": 0, "worker_failure": None, "cleanup_failures": [], "remaining_live_processes": []}, "Existing native ownership cleanup failed")
    require(probe.digest(probe.read_bounded(output / "runtime/cstrike/maps/hammerr_detail_props.bsp", probe.MAX_BSP_BYTES)) == control["bsp_sha256"], "Existing staged native BSP changed")
    return result



def hdr_red_radiance(report):
    rows = {color: detail_records(probe.read_bounded(Path(report["controls"][f"styled-direct_ldr_hdr-{color}"]["bsp"]), probe.MAX_BSP_BYTES), allow_sprites=True, allow_baked=True, allow_styles=True) for color in ["white", "tinted"]}
    require(tint_layout(rows["white"]) == tint_layout(rows["tinted"]), "HDR tint source layout differs")
    fields = {color: {"base": [row["lighting"] for row in values["props"]], "ldr_styles": values["style_lighting_ldr"], "hdr_styles": values["style_lighting_hdr"]} for color, values in rows.items()}
    counts = {}
    for name in ["base", "ldr_styles", "hdr_styles"]:
        white, tinted = fields["white"][name], fields["tinted"][name]
        require(white is not None and tinted is not None and len(white) == len(tinted) == 86, "HDR tint red control requires every base and style receiver")
        require(all(left[0] * 2.0**left[3] == right[0] * 2.0**right[3] for left, right in zip(white, tinted, strict=True)), f"HDR tint changed decoded red {name} radiance")
        counts[name] = len(white)
    return {"decoded_red_exact": True, "checked_records": counts}

def tint_resume_inputs(args, report):
    require(report["success"] and report["tint"] == TINT and all(report[key] for key in ["source_history_preserved", "foreground_background_exact", "saved_reopened"]), "Tint source validation receipt is incomplete")
    require(len(report["checks"]) == 7 and all(all(check.values()) for check in report["checks"].values()), "Tint identity/color/style source checks failed")
    paths = [*args.output.glob("*.vmf"), *args.output.glob("*.bsp"), *Path(report["fixture_game"]).joinpath("materials").rglob("*"), args.probe_library]
    require((args.output / "detail-tint.vmf").is_file() and (args.output / "detail-tint-authored.vmf").is_file(), "Tint saved source fixtures are missing")
    directories = [*args.output.glob("native-*"), *([args.hdr_control] if args.hdr_control is not None else [])]
    paths.extend(path for directory in directories for path in directory.rglob("*") if path.is_file() and path.suffix in [".ppm", ".png", ".json", ".jsonl"])
    guarded = {str(path): probe.digest(path.read_bytes()) for path in paths if path.is_file()}
    if args.frozen_captures is not None:
        frozen = json.loads(probe.read_bounded(args.frozen_captures, 1024**2))
        require(frozen, "Frozen native capture hashes are empty")
        for name, digest in frozen.items():
            relative = Path(name)
            require(not relative.is_absolute() and ".." not in relative.parts, "Frozen capture path must stay inside the tint directory")
            path = args.output / relative
            require(path.is_file() and probe.digest(path.read_bytes()) == digest, f"Frozen native capture changed: {name}")
            guarded[str(path)] = digest
    return guarded


def tint_capture_command(args, fixture_game, control, output, style):
    command = [sys.executable, "-B", str(Path(__file__).with_name("validate_detail_props.py")), "--binary", str(args.binary), "--output", str(output), "--game-dir", str(args.game_dir), "--runtime", "--runtime-bsp", control["bsp"], "--fixture-game", fixture_game, "--expected-count", str(control["count"]), "--expected-sprites", str(control["sprites"]), "--sprite-orientation", str(control["orientation"]), "--baked-lighting", "--hdr-level", str(control["hdr_level"]), "--probe-library", str(args.probe_library), "--port", str(args.port)]
    if style is not None:
        command += ["--allow-detail-styles", "--detail-style-state", style]
    return command


def native_tint(args, report, resume=False, analyze_only=False):
    captures = {}
    frames = {}
    reused = []
    missing = []
    guarded = tint_resume_inputs(args, report) if resume else {}
    if args.hdr_control is not None:
        require(args.hdr_control.is_dir(), "An explicit --hdr-control must be a completed native capture directory")
        checked_tint_capture(args, report["controls"]["styled-direct_ldr_hdr-white"], args.hdr_control, "on")
    choices = [("fast-direct_ldr", None), ("slow-direct_ldr", None), ("styled-direct_ldr", "on"), ("styled-direct_ldr", "off"), ("styled-direct_ldr_hdr", "on")]
    for suite, style in choices:
        if analyze_only and any(not (args.output / f"native-{suite}-{style or 'static'}-{color}").exists() for color in ["white", "tinted"]):
            missing.extend(f"{suite}-{style or 'static'}-{color}" for color in ["white", "tinted"])
            continue
        pair = {}
        for color in ["white", "tinted"]:
            control = report["controls"][f"{suite}-{color}"]
            name = f"{suite}-{style or 'static'}-{color}"
            output = args.output / f"native-{name}"
            if resume and output.exists():
                reused.append(name)
            else:
                require(not analyze_only, "Analysis-only tint validation cannot launch a native process")
                subprocess.run(tint_capture_command(args, report["fixture_game"], control, output, style), check=True, timeout=150)
            result = checked_tint_capture(args, control, output, style)
            pair[color] = {"on": ppm(output / "runtime/details-on.ppm"), "off": ppm(output / "runtime/details-off.ppm")}
            captures[name] = result["native"]
        hdr_control_frames = None
        if suite == "styled-direct_ldr_hdr":
            control_output = args.hdr_control or args.output / "native-hdr-white-control"
            if not control_output.exists():
                require(args.hdr_control is None and not analyze_only, "Analysis-only tint validation needs a completed identical-white HDR control")
                subprocess.run(tint_capture_command(args, report["fixture_game"], report["controls"][f"{suite}-white"], control_output, style), check=True, timeout=150)
            require(control_output.is_dir(), "HDR tint needs a completed identical-white negative control in --hdr-control or native-hdr-white-control")
            independent = checked_tint_capture(args, report["controls"][f"{suite}-white"], control_output, style)
            pair_receipts = [captures[f"{suite}-{style}-{color}"] for color in ["white", "tinted"]]
            require(all(independent["native"]["hardware"]["native_pid"] != receipt["hardware"]["native_pid"] for receipt in pair_receipts), "HDR identical-white control must use an independent native process")
            require(all(independent["native"]["private_launch"]["controls"] == receipt["private_launch"]["controls"] for receipt in pair_receipts), "HDR identical-white launch controls differ from the tint pair")
            captures["hdr-identical-white-control"] = {"output": str(control_output), "native": independent["native"], "source_red_radiance": hdr_red_radiance(report)}
            hdr_control_frames = {"on": ppm(control_output / "runtime/details-on.ppm"), "off": ppm(control_output / "runtime/details-off.ppm")}
        measurements = tint_color_measurements(pair, hdr_control=hdr_control_frames)
        frames[f"{suite}-{style}"] = pair
        captures[f"{suite}-{style}-measurements"] = measurements
    switching = {}
    on, off = frames["styled-direct_ldr-on"], frames["styled-direct_ldr-off"]
    for kind, (left, top, right, bottom) in REGIONS.items():
        offsets = [(y * probe.WIDTH + x) * 3 for y in range(top, bottom) for x in range(left, right)]
        mask = [offset for offset in offsets if all(max(abs(a - b) for a, b in zip(pair[color]["on"][offset:offset + 3], pair[color]["off"][offset:offset + 3], strict=True)) > 12 for pair in [on, off] for color in ["white", "tinted"])]
        require(len(mask) > 150, f"{kind} lacks stable style-switching tint pixels")
        means = {state: {color: sum(sum(pair[color]["on"][offset:offset + 3]) for offset in mask) / len(mask) for color in ["white", "tinted"]} for state, pair in [("on", on), ("off", off)]}
        responses = {}
        for color in ["white", "tinted"]:
            delta = [[on[color]["on"][offset + axis] - off[color]["on"][offset + axis] for offset in mask] for axis in range(3)]
            require(all(value >= 0 for channel in delta for value in channel), f"{kind} styled light decreased foreground lighting")
            responses[color] = sum(value >= 3 for value in delta[0])
            require(responses[color] > 150, f"{kind} lacks actual named-light response on unattenuated red: {responses}")
        background = [abs(on[color]["off"][offset + axis] - off[color]["off"][offset + axis]) for color in ["white", "tinted"] for offset in offsets for axis in range(3)]
        require(background and max(background) == 0, "Tinted style controls changed the detail-disabled region background")
        switching[kind] = {"stable_detail_pixels": len(mask), "foreground_channel_sum": means, "red_style_response_pixels": responses}
    require(all(Path(path).is_file() and probe.digest(Path(path).read_bytes()) == digest for path, digest in guarded.items()), "Tint resume changed source fixtures or previously verified capture evidence")
    receipt = {"reused_captures": reused, "missing_captures": missing, "guarded_inputs": guarded, "captures": captures, "actual_style_switching": switching, "analysis_only": analyze_only, "success": True}
    if analyze_only:
        (args.output / "tint-native-analysis.json").write_text(json.dumps(receipt, indent=2) + "\n")
        return
    report["native_resume"] = receipt
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
    parser.add_argument("--resume-tint", action="store_true", help="Reuse checked completed tint captures and launch only missing native views")
    parser.add_argument("--analyze-only", action="store_true", help="With --resume-tint, verify existing captures without launching native views")
    parser.add_argument("--hdr-control", type=Path, help="Completed identical-white HDR capture; required for bounded HDR-only pixel comparisons")
    parser.add_argument("--frozen-captures", type=Path, help="Optional existing relative-path SHA256 manifest for immutable completed captures")
    parser.add_argument("--probe-library", type=Path, default=Path(__file__).resolve().parent.parent / "target/debug/hammerr_hdr_probe.so")
    parser.add_argument("--port", type=int, default=27719)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    args.output = args.output.resolve()
    if args.hdr_control is not None:
        args.hdr_control = args.hdr_control.resolve(strict=True)
    require(not args.analyze_only or args.resume_tint and args.tint and args.runtime, "Analysis-only requires --tint --runtime --resume-tint")
    require(args.frozen_captures is None or args.resume_tint, "Frozen capture hashes require --resume-tint")
    require(not args.resume_tint or args.tint and args.runtime, "Tint resume requires --tint --runtime")
    require(args.output.is_dir() if args.resume_tint else not args.output.exists(), "Use an existing tint directory for resume or a fresh absent output directory")
    require(1024 <= args.port <= 65530, "Invalid native port")
    if not args.resume_tint:
        args.output.mkdir(parents=True, mode=0o700)
    if args.tint and args.runtime:
        args.probe_library = args.probe_library.resolve(strict=True)
    result = json.loads((args.output / "results.json").read_text()) if args.resume_tint else validate_tint(args) if args.tint else validate(args)
    if args.runtime:
        native_tint(args, result, resume=args.resume_tint, analyze_only=args.analyze_only) if args.tint else native_colors(args, result)
    print(json.dumps({"success": True, "results": str(args.output / "results.json"), "native_exercised": result["native_exercised"]}))


if __name__ == "__main__":
    main()
