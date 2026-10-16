import argparse
import json
import subprocess
import sys
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_detail_props import compile_pair, detail_records
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_material_details import state
from validate_visibility import ROOM

TEXTURE = "dev/dev_measuregeneric01"
FIELDS = {"position_ul": "-20 45", "position_lr": "20 0", "tex_ul": "0 0", "tex_size": "128 128", "tex_total_size": "512", "detailOrientation": "2", "angles": "0 0 0"}


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def validate_regions(client, args, ids, sprites, source, saved, data, expected):
    authored = detail(client, ids)
    draft = client.call("preview_compile_region", {"snapshot": client.call("get_compile_region")["snapshot"], "enabled": True, "mins": [-224, -224, 0], "maxs": [128, 128, 224]})
    client.call("apply_compile_region", {"token": draft["draft"]["token"]})
    outside = client.call("create_entity", {"classname": "prop_detail_sprite", "origin": [240, 0, 32], "properties": {"scale": "2", "tex_size": "invalid"}})["id"]
    scoped = [*ids, outside]
    region_bsp, region_data, region_result = compile_pair(client, args.output, "explicit-region", scoped)
    parsed = detail_records(region_data, allow_sprites=True, allow_baked=True)
    retained = [origin for origin in expected if all(-224 <= value <= 128 for value in origin[:2])]
    require(sorted(row["origin"] for row in parsed["props"]) == sorted(retained) and len(retained) == 15, "Region did not retain explicit sprite origins inclusively")
    scope = region_result["stats"]["compile_region"]
    require(scope is not None and outside in scope["omitted"] and len(parsed["sprites"]) == 1, region_result)
    previous = args.output / "region-rejected.bsp"
    previous.write_bytes(region_data)
    inside = next(sprite for sprite, origin in zip(sprites, expected, strict=True) if origin in retained)
    failures = {}
    for name, target, changes, message in [("inside-malformed", inside, {"tex_size": "0 128"}, "texture rectangle"), ("inclusive-boundary", outside, {"origin": "128 0 32", **FIELDS, "scale": "1"}, "open visible world leaf")]:
        client.call("set_entity_properties", {"id": target, "set": changes})
        before = state(client, scoped, source)
        failure = json.loads(client.call("compile_map", {"path": str(previous)}, error=True)["content"][0]["text"])
        require(not failure["success"] and message in json.dumps(failure["diagnostics"]), failure)
        require(previous.read_bytes() == region_data and state(client, scoped, source) == before, "Region rejection changed source/history or prior BSP")
        client.call("start_compile", {"path": str(previous)})
        job = wait_job(client)
        require(job["status"] == "failed" and previous.read_bytes() == region_data and state(client, scoped, source) == before, job)
        failures[name] = failure["diagnostics"]
        client.call("undo")
    client.call("undo")
    client.call("undo")
    require(detail(client, ids) == authored and source.read_bytes() == saved, "Region controls changed authored explicit sprites or saved source")
    _, restored, _ = compile_pair(client, args.output, "explicit-region-restored", ids)
    require(restored == data, "Region disable changed full-map bytes")
    return {"bsp": str(region_bsp), "bsp_sha256": probe.digest(region_data), "count": len(retained), "outside_invalid_omitted": True, "rejected": failures, "full_map_restored_exact": True}


def validate_controls(client, args, ids, sprites, source, saved, data, expected):
    authored = detail(client, ids)
    variants = {}
    choices = {
        "fixed-unit": (0, "1", "", "0", "0", "0", 1, 0, 0),
        "fixed-scaled": (0, "1.5", "", "0", "0", "0", 1, 0, 0),
        "full-scaled": (1, "0.75", "", "0", "0", "0.5", 1, 0, 127),
        "vertical-scaled": (2, "1.25", "", "0", "0", "0.5", 1, 0, 127),
        "cross-scaled": (0, "1.25", "cross", "0", "0", "0.5", 2, 0, 127),
        "tri-fanned": (0, "0.75", "tri", "35", "0.5", "1", 3, 127, 255),
    }
    for name, (orientation, scale, shape, angle, size, sway, kind, encoded_size, encoded_sway) in choices.items():
        properties = {"detailOrientation": str(orientation), "angles": "0 90 0", "scale": scale, "sprite_shape": shape, "shape_angle": angle, "shape_size": size, "sway": sway}
        for sprite in sprites:
            client.call("set_entity_properties", {"id": sprite, "set": properties})
        variant_source = args.output / f"{name}.vmf"
        client.call("save_map", {"path": str(variant_source)})
        variant_saved = variant_source.read_bytes()
        variant_authored = detail(client, ids)
        client.call("open_map", {"path": str(variant_source)})
        require(detail(client, ids) == variant_authored, "Saved explicit sprite controls changed on reopen")
        client.call("set_object_visibility", {"action": "hide", "ids": sprites})
        _, variant_empty, _ = compile_pair(client, args.output, f"{name}-empty", ids)
        client.call("undo")
        require(detail(client, ids) == variant_authored, "Hidden control Undo changed authored sprite controls")
        bsp, compiled, result = compile_pair(client, args.output, name, ids)
        parsed = detail_records(compiled, allow_sprites=True, allow_baked=True, sprite_orientation=orientation, allow_shapes=kind != 1, allow_sway=True)
        require(len(parsed["props"]) == result["stats"]["detail_props"] == len(expected) and parsed["models"] == [], result)
        require(parsed["sprites"] == detail_records(data, allow_sprites=True, allow_baked=True)["sprites"], "Explicit controls changed the shared atlas dictionary")
        require(sorted(row["origin"] for row in parsed["props"]) == sorted(expected), "Explicit controls changed authored origins")
        require(all(row["kind"] == kind and row["orientation"] == orientation and row["scale"] == float(scale) and row["sway"] == encoded_sway and row["angles"] == [0, 90, 0] for row in parsed["props"]), "Explicit control native fields differ")
        if kind != 1:
            require(all(row["shape_angle"] == int(angle) and row["shape_size"] == encoded_size for row in parsed["props"]), "Explicit shape parameter bytes differ")
        for index, (before, after) in enumerate(zip(probe.bsp_lumps(variant_empty), probe.bsp_lumps(compiled), strict=True)):
            if index != 35:
                require(before == after, f"Explicit controls changed unrelated lump {index}")
        require(b'"prop_detail_sprite"' not in probe.bsp_lumps(compiled)[0][2], "Controlled sprite entities leaked into runtime")
        if encoded_sway:
            for sprite in sprites:
                client.call("set_entity_properties", {"id": sprite, "set": {"sway": "0"}})
            _, stationary, _ = compile_pair(client, args.output, f"{name}-zero", ids)
            zero = detail_records(stationary, allow_sprites=True, allow_baked=True, sprite_orientation=orientation, allow_shapes=kind != 1, allow_sway=True)
            require({**parsed, "payload_sha256": zero["payload_sha256"], "props": [{**row, "sway": 0} for row in parsed["props"]]} == zero, "Explicit sway changed scale, geometry, placement, leaf or SDK lighting")
            for _ in sprites:
                client.call("undo")
            require(detail(client, ids) == variant_authored, "Sway control Undo changed authored fields")
        _, repeated, _ = compile_pair(client, args.output, f"{name}-repeated", ids)
        require(repeated == compiled and variant_source.read_bytes() == variant_saved, "Explicit controls changed deterministic output or saved source")
        variants[name] = {"bsp": str(bsp), "bsp_sha256": probe.digest(compiled), "count": len(expected), "orientation": orientation, "kind": kind, "scale": float(scale), "sway": encoded_sway, "saved_reopened": True}
        client.call("open_map", {"path": str(source)})
        require(detail(client, ids) == authored, "Explicit control reset changed original source")
    previous = args.output / "control-rejected.bsp"
    previous.write_bytes(data)
    failures = {}
    for name, changes, message in [
        ("slow-underflow", {"detailOrientation": "0", "scale": "0.00000001"}, "half precision"),
        ("slow-saturation", {"detailOrientation": "0", "scale": "65505", "position_ul": "-0.0001 0.0001", "position_lr": "0.0001 -0.0001"}, "half precision"),
        ("sway-underflow", {"scale": "0.00000001", "sway": "0.003921568627"}, "half precision"),
        ("fast-width-overflow", {"scale": "1e-38", "position_ul": "-3e38 45", "position_lr": "3e38 0"}, "geometry"),
        ("fast-height-overflow", {"scale": "1e-38", "position_ul": "-20 3e38", "position_lr": "20 -3e38"}, "geometry"),
        ("shape-orientation", {"sprite_shape": "tri"}, "detailOrientation"),
        ("ignored-cross-angle", {"detailOrientation": "0", "sprite_shape": "cross", "shape_angle": "1"}, "ignores"),
        ("fractional-tri-angle", {"detailOrientation": "0", "sprite_shape": "tri", "shape_angle": "0.5"}, "integer"),
        ("shape-size", {"detailOrientation": "0", "sprite_shape": "tri", "shape_size": "NaN"}, "shape_size"),
        ("modelscale", {"modelscale": "2"}, "modelscale=1"),
    ]:
        client.call("set_entity_properties", {"id": sprites[0], "set": changes})
        before = state(client, ids, source)
        failure = json.loads(client.call("compile_map", {"path": str(previous)}, error=True)["content"][0]["text"])
        require(not failure["success"] and message in json.dumps(failure["diagnostics"]), failure)
        require(previous.read_bytes() == data and state(client, ids, source) == before, "Failed explicit control compile changed source, history or prior output")
        client.call("start_compile", {"path": str(previous)})
        job = wait_job(client)
        require(job["status"] == "failed" and previous.read_bytes() == data and state(client, ids, source) == before, job)
        failures[name] = failure["diagnostics"]
        client.call("undo")
        require(detail(client, ids) == authored, "Explicit control rejection Undo changed the original source")
    _, restored, _ = compile_pair(client, args.output, "controls-restored", ids)
    require(restored == data and source.read_bytes() == saved, "Explicit control reset changed baseline bytes")
    return {"variants": variants, "rejected": failures, "baseline_restored_exact": True}


def native_controls(args, result):
    from validate_detail_shapes import sway_frames
    from validate_detail_sprites import silhouette

    captures = {}
    for name, variant in result["controls"]["variants"].items():
        output = args.output / f"native-{name}"
        command = [sys.executable, "-B", str(Path(__file__).with_name("validate_detail_props.py")), "--binary", str(args.binary), "--output", str(output), "--game-dir", str(args.game_dir), "--runtime", "--runtime-bsp", variant["bsp"], "--fixture-game", result["fixture_game"], "--expected-count", str(variant["count"]), "--expected-sprites", str(variant["count"]), "--sprite-orientation", str(variant["orientation"]), "--view", "elevated", "--baked-lighting", "--port", str(args.port)]
        if variant["kind"] != 1:
            command.append("--allow-shapes")
        if variant["sway"]:
            command += ["--sway-amplitude", "8"]
        subprocess.run(command, check=True, timeout=150)
        report = json.loads((output / "results.json").read_text())
        require(report["native_exercised"] and report["native"]["success"] and report["bsp_sha256"] == variant["bsp_sha256"], report)
        captures[name] = {"native": report["native"]}
        if variant["sway"]:
            captures[name]["motion"] = sway_frames(output / "runtime", variant["sway"])
    unit = silhouette(args.output / "native-fixed-unit/runtime")
    scaled = silhouette(args.output / "native-fixed-scaled/runtime")
    changed = sum(a != b for a, b in zip(unit, scaled, strict=True))
    require(changed > len(unit) * 0.002, "Authored scale did not change the fixed sprite silhouette")
    return {"captures": captures, "scale_silhouette": {"different_pixels": changed, "unit_pixels": sum(unit), "scaled_pixels": sum(scaled)}}


def validate(args):
    game = runtime.private_game(args.game_dir, args.output / "source-game")
    (game / "materials").mkdir(exist_ok=True)
    client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    try:
        with stopped_on_exit(client):
            client.call("load_entity_definitions", {"path": str(args.fgd)})
            ids = [client.call("create_block", {"mins": low, "maxs": high, "material": TEXTURE})["id"] for low, high in ROOM]
            for classname, origin, properties in [
                ("info_player_terrorist", [-192, -176, 8], {}),
                ("info_player_counterterrorist", [192, 176, 8], {}),
                ("light", [0, 0, 180], {"_light": "255 255 255 180", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}),
                ("logic_script", [0, -176, 64], {"targetname": "hammerr_detail_observer", "vscripts": "hammerr_detail_observer.nut", "thinkfunction": "HammeRRThink"}),
            ]:
                ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
            client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "none", "bounces": 0})
            expected = [[x, y, 1] for x in [-180, -108, -36, 36, 108, 180] for y in [-144, -48, 48, 144]]
            sprites = [client.call("create_entity", {"classname": "prop_detail_sprite", "origin": origin, "properties": FIELDS})["id"] for origin in expected]
            ids.extend(sprites)
            source = args.output / "explicit-detail-sprites.vmf"
            client.call("save_map", {"path": str(source)})
            saved = source.read_bytes()
            authored = detail(client, ids)
            client.call("open_map", {"path": str(source)})
            require(detail(client, ids) == authored, "Saved custom sprite entities changed on reopen")
            client.call("set_object_visibility", {"action": "hide", "ids": sprites})
            _, empty, _ = compile_pair(client, args.output, "empty", ids)
            client.call("undo")
            require(detail(client, ids) == authored, "Undo failed to restore visible sprite sources")
            bsp, data, result = compile_pair(client, args.output, "explicit-detail-sprites", ids)
            parsed = detail_records(data, allow_sprites=True, allow_baked=True)
            require(result["stats"]["detail_props"] == len(parsed["props"]) == 24, result)
            require(parsed["models"] == [] and len(parsed["sprites"]) == result["stats"]["detail_prop_sprites"] == 1, parsed)
            require(parsed["sprites"][0] == [-20, 45, 20, 0, 0.5 / 512, 0.5 / 512, 127.5 / 512, 127.5 / 512], parsed)
            require(sorted(row["origin"] for row in parsed["props"]) == sorted(expected), "Explicit authored positions changed")
            require(all(row["kind"] == 1 and row["orientation"] == 2 and row["scale"] == 1 and row["angles"] == [0, 0, 0] for row in parsed["props"]), parsed)
            leaves = records(data, 10, "<ihh6h4HhH", expected_version=1)
            require(all(leaves[row["leaf"]][0] & 1 == 0 and leaves[row["leaf"]][1] >= 0 for row in parsed["props"]), "Explicit sprites did not use visible open leaves")
            require(b'"prop_detail_sprite"' not in probe.bsp_lumps(data)[0][2], "Compile-only sprite entities leaked into runtime")
            for index, (before, after) in enumerate(zip(probe.bsp_lumps(empty), probe.bsp_lumps(data), strict=True)):
                if index != 35:
                    require(before == after, f"Explicit sprites changed unrelated lump {index}")
            previous = args.output / "rejected.bsp"
            previous.write_bytes(data)
            rejected = {}
            for key, value, message in [("position_lr", "-20 0", "geometry"), ("tex_size", "0 128", "texture rectangle"), ("tex_total_size", "NaN", "tex_total_size"), ("scale", "0", "scale"), ("detailOrientation", "0.5", "integer"), ("lightingorigin", "missing", "model-only"), ("renderamt", "128", "renderamt"), ("sway", "1.1", "sway")]:
                client.call("set_entity_properties", {"id": sprites[0], "set": {key: value}})
                before = state(client, ids, source)
                failure = json.loads(client.call("compile_map", {"path": str(previous)}, error=True)["content"][0]["text"])
                require(not failure["success"] and message in json.dumps(failure["diagnostics"]), failure)
                require(previous.read_bytes() == data and state(client, ids, source) == before, "Failed explicit sprite compile mutated source or output")
                client.call("start_compile", {"path": str(previous)})
                job = wait_job(client)
                require(job["status"] == "failed" and previous.read_bytes() == data and state(client, ids, source) == before, job)
                rejected[key] = failure["diagnostics"]
                client.call("undo")
                require(detail(client, ids) == authored, "Undo failed to restore explicit sprite fields")
            _, repeated, _ = compile_pair(client, args.output, "explicit-restored", ids)
            require(repeated == data and source.read_bytes() == saved, "Explicit sprite controls changed deterministic output or saved source")
            region = validate_regions(client, args, ids, sprites, source, saved, data, expected) if args.regions else None
            controls = validate_controls(client, args, ids, sprites, source, saved, data, expected) if args.controls else {}
            result = {"controls": controls, "regions": region, "success": True, "bsp": str(bsp), "bsp_sha256": probe.digest(data), "count": 24, "records": parsed, "rejected": rejected, "saved_reopened": True, "foreground_background_exact": True, "source_history_preserved": True, "unrelated_geometry_exact": True, "fixture_game": str(game), "native_exercised": False}
            (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
            return result
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--fgd", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/SourceSDK/bin/source2009/bin/cstrike.fgd")
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--regions", action="store_true")
    parser.add_argument("--controls", action="store_true", help="Validate authored scale, cross/tri shape and sway controls with native precision guards")
    parser.add_argument("--port", type=int, default=27729)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    args.output = args.output.resolve()
    require(not args.output.exists(), "Use a fresh absent output directory")
    args.output.mkdir(parents=True, mode=0o700)
    result = validate(args)
    if args.runtime and args.controls:
        result.update({"native_exercised": True, "native_controls": native_controls(args, result)})
        (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    elif args.runtime:
        output = args.output / "native"
        fixture = result["regions"] if args.regions else result
        count = str(fixture["count"])
        subprocess.run([sys.executable, "-B", str(Path(__file__).with_name("validate_detail_props.py")), "--binary", str(args.binary), "--output", str(output), "--game-dir", str(args.game_dir), "--runtime", "--runtime-bsp", fixture["bsp"], "--fixture-game", result["fixture_game"], "--expected-count", count, "--expected-sprites", count, "--sprite-orientation", "2", "--baked-lighting", "--port", str(args.port)], check=True, timeout=150)
        native = json.loads((output / "results.json").read_text())
        require(native["native_exercised"] and native["native"]["success"], native)
        result.update({"native_exercised": True, "native": native["native"]})
        (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"success": True, "results": str(args.output / "results.json"), "native_exercised": result["native_exercised"]}))


if __name__ == "__main__":
    main()
