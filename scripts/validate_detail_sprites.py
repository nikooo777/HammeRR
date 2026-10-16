import argparse
import json
import subprocess
import sys
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_cubemap_reflections import ppm
from validate_detail_props import compile_pair, detail_records
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_material_details import set_world, state
from validate_visibility import ROOM

MATERIAL = "hammerr_detail_sprites/ground"
TEXTURE = "dev/dev_measuregeneric01"
DICTIONARY = "scripts/hammerr_detail_sprites.vbsp"
FIELDS = 'sprite "0 0 128 128 512" spritesize "0.5 0 40 45" detailOrientation 2 upright 1 spriterandomscale 0.2'


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def dictionary(fields=FIELDS, density=512):
    return f'detail {{ hammerr_tufts {{ density {density} group {{ alpha 1 item {{ {fields} }} }} }} }}\n'


def validate(args):
    game = runtime.private_game(args.game_dir, args.output / "source-game")
    material = game / f"materials/{MATERIAL}.vmt"
    material.parent.mkdir(parents=True)
    plain = f'LightmappedGeneric {{ $basetexture "{TEXTURE}" }}\n'
    material.write_text(plain)
    dictionary_path = game / DICTIONARY
    dictionary_path.write_text(dictionary())
    client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    try:
        with stopped_on_exit(client):
            client.call("load_entity_definitions", {"path": str(args.fgd)})
            ids = [client.call("create_block", {"mins": low, "maxs": high, "material": TEXTURE})["id"] for low, high in ROOM]
            floor = detail(client, [ids[0]])[0]
            top = next(face for face in floor["faces"] if face["normal"] == [0, 0, 1])
            client.call("edit_faces", {"faces": [face_ref(floor, top)], "material": MATERIAL})
            for classname, origin, properties in [
                ("info_player_terrorist", [-192, -176, 8], {}),
                ("info_player_counterterrorist", [192, 176, 8], {}),
                ("light", [0, 0, 180], {"_light": "255 255 255 180", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}),
                ("logic_script", [0, -176, 64], {"targetname": "hammerr_detail_observer", "vscripts": "hammerr_detail_observer.nut", "thinkfunction": "HammeRRThink"}),
            ]:
                ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
            set_world(client, "detailvbsp", DICTIONARY)
            client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "none", "bounces": 0})
            source = args.output / "detail-sprites.vmf"
            client.call("save_map", {"path": str(source)})
            _, empty, empty_result = compile_pair(client, args.output, "empty", ids)
            require(not detail_records(empty)["props"] and empty_result["stats"]["detail_props"] == 0, "Plain material control emitted details")
            material.write_text(f'LightmappedGeneric {{ $basetexture "{TEXTURE}" %detailtype hammerr_tufts }}\n')
            client.call("reload_material_assets")
            authored = detail(client, ids)
            saved = source.read_bytes()
            client.call("open_map", {"path": str(source)})
            require(detail(client, ids) == authored, "VMF reopen changed authored surfaces")
            bsp, data, result = compile_pair(client, args.output, "detail-sprites", ids)
            parsed = detail_records(data, allow_sprites=True, allow_baked=True)
            require(len(parsed["props"]) == result["stats"]["detail_props"] == 134, result)
            require(parsed["models"] == [] and result["stats"]["detail_prop_models"] == 0, parsed)
            require(len(parsed["sprites"]) == result["stats"]["detail_prop_sprites"] == 1, parsed)
            require(parsed["sprites"][0] == [-20, 45, 20, 0, 0.5 / 512, 0.5 / 512, 127.5 / 512, 127.5 / 512], parsed)
            require(all(row.get("kind") == 1 and row["sprite"] == 0 for row in parsed["props"]), "Fixture emitted unexpected native detail types")
            require(any(row["scale"] < 1 for row in parsed["props"]) and any(row["scale"] > 1 for row in parsed["props"]), "Gaussian scaling did not vary")
            require(all(row["origin"][2] == 0 and all(-256 <= value <= 256 for value in row["origin"][:2]) for row in parsed["props"]), "Generated sprites escaped the exposed floor")
            leaves = records(data, 10, "<ihh6h4HhH", expected_version=1)
            require(all(leaves[row["leaf"]][0] & 1 == 0 and leaves[row["leaf"]][1] >= 0 for row in parsed["props"]), "Sprite leaves are not open and visible")
            for index, (before, after) in enumerate(zip(probe.bsp_lumps(empty), probe.bsp_lumps(data), strict=True)):
                if index != 35:
                    require(before == after, f"Sprite scattering changed unrelated lump {index}")
            rejected = {}
            previous = args.output / "rejected.bsp"
            previous.write_bytes(data)
            for name, fields, expected in [
                ("orientation", FIELDS.replace("detailOrientation 2", "detailOrientation 1.5"), "integer"),
                ("fixed-random-scale", FIELDS.replace("detailOrientation 2", "detailOrientation 0").replace("spriterandomscale 0.2", "spriterandomscale 1.1"), "spriterandomscale"),
                ("full-random-scale", FIELDS.replace("detailOrientation 2", "detailOrientation 1").replace("spriterandomscale 0.2", "spriterandomscale 1.1"), "spriterandomscale"),
                ("shape", FIELDS + " spriteshape 1", "unsupported"),
                ("logical-atlas", FIELDS.replace("0 0 128 128 512", "500 0 128 128 512"), "rectangle"),
                ("random-scale", FIELDS.replace("spriterandomscale 0.2", "spriterandomscale 1.1"), "spriterandomscale"),
            ]:
                dictionary_path.write_text(dictionary(fields))
                before = state(client, ids, source)
                response = client.call("compile_map", {"path": str(previous)}, error=True)
                failure = json.loads(response["content"][0]["text"])
                require(not failure["success"] and expected.lower() in json.dumps(failure["diagnostics"]).lower(), failure)
                require(previous.read_bytes() == data and state(client, ids, source) == before, "Rejected sprite build replaced output or changed source/history")
                client.call("start_compile", {"path": str(previous)})
                job = wait_job(client)
                require(job["status"] == "failed" and previous.read_bytes() == data and state(client, ids, source) == before, job)
                rejected[name] = failure["diagnostics"]
            orientations = {}
            unit = FIELDS.replace(" spriterandomscale 0.2", "")
            reference = None
            for orientation in [2, 0, 1]:
                dictionary_path.write_text(dictionary(unit.replace("detailOrientation 2", f"detailOrientation {orientation}")))
                variant_bsp, variant_data, variant_result = compile_pair(client, args.output, f"orientation-{orientation}", ids)
                variant = detail_records(variant_data, allow_sprites=True, allow_baked=True, sprite_orientation=orientation)
                require(len(variant["props"]) == variant_result["stats"]["detail_props"] == len(parsed["props"]), "Orientation changed the sampled count")
                require(variant["sprites"] == parsed["sprites"] and variant["models"] == [], "Orientation changed dictionary identity")
                normalized = [{**row, "orientation": 2} for row in variant["props"]]
                if reference is None:
                    reference = normalized
                require(normalized == reference, "Orientation changed placement, angles, scale, leaves or identity")
                require(all(row["scale"] == 1 for row in variant["props"]), "Fixed/full fixture must use unit native scale")
                for index, (before, after) in enumerate(zip(probe.bsp_lumps(data), probe.bsp_lumps(variant_data), strict=True)):
                    if index != 35:
                        require(before == after, f"Orientation changed unrelated lump {index}")
                orientations[str(orientation)] = {"bsp": str(variant_bsp), "bsp_sha256": probe.digest(variant_data), "count": len(variant["props"]), "sprite_entries": len(variant["sprites"])}
            scaled_orientations = {}
            for orientation in [0, 1]:
                dictionary_path.write_text(dictionary(FIELDS.replace("detailOrientation 2", f"detailOrientation {orientation}")))
                variant_bsp, variant_data, variant_result = compile_pair(client, args.output, f"scaled-orientation-{orientation}", ids)
                variant = detail_records(variant_data, allow_sprites=True, allow_baked=True, sprite_orientation=orientation)
                require(variant["sprites"] == parsed["sprites"] and variant_result["stats"]["detail_props"] == len(parsed["props"]), "Nonunit orientation changed dictionary/count")
                require([{**row, "orientation": 2} for row in variant["props"]] == parsed["props"], "Nonunit orientation changed raw scale, sampling or SDK lighting")
                scales = [row["scale"] for row in variant["props"]]
                require(2 ** -24 <= min(scales) < 1 < max(scales) <= 6, "Nonunit scales did not vary within native bounds")
                for index, (before, after) in enumerate(zip(probe.bsp_lumps(data), probe.bsp_lumps(variant_data), strict=True)):
                    if index != 35:
                        require(before == after, f"Nonunit orientation changed unrelated lump {index}")
                scaled_orientations[str(orientation)] = {"bsp": str(variant_bsp), "bsp_sha256": probe.digest(variant_data), "count": len(scales), "sprite_entries": len(variant["sprites"]), "scale_range": [min(scales), max(scales)]}
            dictionary_path.write_text(dictionary())
            _, repeated, _ = compile_pair(client, args.output, "detail-sprites-repeated", ids)
            require(repeated == data and source.read_bytes() == saved and detail(client, ids) == authored, "Sprite compilation changed deterministic bytes or source")
            report = {"success": True, "stats": result["stats"], "records": parsed, "orientations": orientations, "scaled_orientations": scaled_orientations, "rejected": rejected, "saved_reopened": True, "foreground_background_exact": True, "source_history_preserved": True, "empty_control_geometry_exact": True, "native_exercised": False, "atlas": "installed detail/detailsprites", "fixture_game": str(game), "bsp": str(bsp), "bsp_sha256": probe.digest(data)}
            (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
            return report
    finally:
        client.close()


def silhouette(directory):
    on = ppm(directory / "details-on.ppm")
    off = ppm(directory / "details-off.ppm")
    mask = []
    for y in range(180, 650):
        for x in range(160, 1120):
            start = (y * probe.WIDTH + x) * 3
            mask.append(max(abs(a - b) for a, b in zip(on[start:start + 3], off[start:start + 3], strict=True)) > 12)
    return mask


def orientation_views(args, result):
    captures = {}
    for orientation in [0, 1]:
        variant = result["scaled_orientations"][str(orientation)]
        for view in ["level", "elevated"]:
            output = args.output / f"native-orientation-{orientation}-{view}"
            subprocess.run([sys.executable, "-B", str(Path(__file__).with_name("validate_detail_props.py")),
                "--binary", str(args.binary), "--output", str(output), "--game-dir", str(args.game_dir),
                "--runtime", "--runtime-bsp", variant["bsp"], "--fixture-game", result["fixture_game"],
                "--expected-count", str(variant["count"]), "--expected-sprites", str(variant["count"]),
                "--sprite-orientation", str(orientation), "--view", view, "--baked-lighting", "--port", str(args.port)], check=True, timeout=150)
            native = json.loads((output / "results.json").read_text())
            require(native["native_exercised"] and native["native"]["success"], native)
            captures[(orientation, view)] = (native["native"], silhouette(output / "runtime"))
    comparisons = {}
    for view in ["level", "elevated"]:
        fixed = captures[(0, view)][1]
        full = captures[(1, view)][1]
        changed = sum(a != b for a, b in zip(fixed, full, strict=True))
        require(changed > len(fixed) * 0.002, f"Fixed/full billboard silhouettes did not differ in {view} view")
        comparisons[view] = {"different_pixels": changed, "fixed_pixels": sum(fixed), "full_billboard_pixels": sum(full)}
    scale_controls = {}
    for orientation in [0, 1]:
        variant = result["orientations"][str(orientation)]
        output = args.output / f"native-unit-orientation-{orientation}"
        subprocess.run([sys.executable, "-B", str(Path(__file__).with_name("validate_detail_props.py")),
            "--binary", str(args.binary), "--output", str(output), "--game-dir", str(args.game_dir),
            "--runtime", "--runtime-bsp", variant["bsp"], "--fixture-game", result["fixture_game"],
            "--expected-count", str(variant["count"]), "--expected-sprites", str(variant["count"]),
            "--sprite-orientation", str(orientation), "--view", "elevated", "--baked-lighting", "--port", str(args.port)], check=True, timeout=150)
        unit_native = json.loads((output / "results.json").read_text())
        require(unit_native["native_exercised"] and unit_native["native"]["success"], unit_native)
        unit_mask = silhouette(output / "runtime")
        scaled_mask = captures[(orientation, "elevated")][1]
        changed = sum(a != b for a, b in zip(unit_mask, scaled_mask, strict=True))
        require(changed > 0, "Sprite scale silhouette did not change")
        scale_controls[str(orientation)] = {"native_unit": unit_native["native"], "different_pixels": changed, "unit_pixels": sum(unit_mask), "scaled_pixels": sum(scaled_mask)}
    return {"captures": {f"{orientation}-{view}": report for (orientation, view), (report, _) in captures.items()}, "silhouettes": comparisons, "scale_silhouettes": scale_controls}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--fgd", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/SourceSDK/bin/source2009/bin/cstrike.fgd")
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--orientation-views", action="store_true", help="Validate nonunit fixed/full sprites at level and elevated native views")
    parser.add_argument("--port", type=int, default=27709)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    args.output = args.output.resolve()
    require(not args.orientation_views or args.runtime, "--orientation-views requires --runtime")
    require(not args.output.exists(), "Use a fresh absent output directory")
    args.output.mkdir(parents=True, mode=0o700)
    result = validate(args)
    if args.runtime:
        native_output = args.output / "native"
        subprocess.run([sys.executable, "-B", str(Path(__file__).with_name("validate_detail_props.py")),
            "--binary", str(args.binary), "--output", str(native_output), "--game-dir", str(args.game_dir),
            "--runtime", "--runtime-bsp", result["bsp"], "--fixture-game", result["fixture_game"],
            "--expected-count", "134", "--expected-sprites", "134", "--baked-lighting", "--port", str(args.port)], check=True, timeout=150)
        native = json.loads((native_output / "results.json").read_text())
        require(native["native_exercised"] and native["native"]["success"], native)
        result.update({"native_exercised": True, "native": native["native"]})
        if args.orientation_views:
            result["orientation_views"] = orientation_views(args, result)
        (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"success": True, "results": str(args.output / "results.json"), "bsp": result["bsp"], "native_exercised": result["native_exercised"]}))


if __name__ == "__main__":
    main()
