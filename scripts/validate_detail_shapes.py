import argparse
import json
import subprocess
import sys
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_native_runtime as runtime
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_cubemap_reflections import ppm
from validate_detail_props import compile_pair, detail_records
from validate_detail_sprites import dictionary, silhouette
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_material_details import set_world, state
from validate_visibility import ROOM

MATERIAL = "hammerr_detail_shapes/ground"
TEXTURE = "dev/dev_measuregeneric01"
DICTIONARY = "scripts/hammerr_detail_shapes.vbsp"
FIELDS = 'sprite "0 0 128 128 512" spritesize "0.5 0 40 45" detailOrientation 0 upright 1'
VARIANTS = {
    "cross": ("sprite_shape cross", 2, 0, 0),
    "tri": ("sprite_shape tri", 3, 0, 0),
    "tri-fanned": ("sprite_shape tri shape_angle 35 shape_size 0.5", 3, 35, 127),
    "cross-scaled": ("sprite_shape cross spriterandomscale 0.2", 2, 0, 0),
    "tri-scaled": ("sprite_shape tri shape_angle 35 shape_size 0.5 spriterandomscale 0.2", 3, 35, 127),
}


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def validate(args):
    game = runtime.private_game(args.game_dir, args.output / "source-game")
    material = game / f"materials/{MATERIAL}.vmt"
    material.parent.mkdir(parents=True)
    material.write_text(f'LightmappedGeneric {{ $basetexture "{TEXTURE}" %detailtype hammerr_tufts }}\n')
    dictionary_path = game / DICTIONARY
    dictionary_path.write_text(dictionary(FIELDS))
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
            source = args.output / "detail-shapes.vmf"
            client.call("save_map", {"path": str(source)})
            authored = detail(client, ids)
            saved = source.read_bytes()
            client.call("open_map", {"path": str(source)})
            require(detail(client, ids) == authored, "VMF reopen changed authored shape surfaces")
            _, plain_data, plain_result = compile_pair(client, args.output, "plain-control", ids)
            plain = detail_records(plain_data, allow_sprites=True, allow_baked=True, sprite_orientation=0)
            require(len(plain["props"]) == plain_result["stats"]["detail_props"] > 0, "Plain control emitted no sprites")
            variants = {}
            reference = None
            choices = {"cross-zero": ("sprite_shape cross sway 0", 2, 0, 0), "cross-half": ("sprite_shape cross sway 0.5", 2, 0, 0), "tri-full": ("sprite_shape tri shape_angle 35 shape_size 0.5 sway 1", 3, 35, 127)} if args.sway else VARIANTS
            for name, (fields, kind, angle, size) in choices.items():
                dictionary_path.write_text(dictionary(f"{FIELDS} {fields}"))
                bsp, data, result = compile_pair(client, args.output, name, ids)
                parsed = detail_records(data, allow_sprites=True, allow_baked=True, sprite_orientation=0, allow_shapes=True, allow_sway=args.sway)
                require(len(parsed["props"]) == result["stats"]["detail_props"] == len(plain["props"]), "Shape changed sample count")
                require(parsed["models"] == [] and parsed["sprites"] == plain["sprites"] and len(parsed["sprites"]) == 1, "Shape changed sprite dictionary identity")
                sway = 127 if name == "cross-half" else 255 if name == "tri-full" else 0
                require(all(row["kind"] == kind and row["shape_angle"] == angle and row["shape_size"] == size and row["sway"] == sway for row in parsed["props"]), "Native shape fields differ")
                if sway:
                    try:
                        detail_records(data, allow_sprites=True, allow_baked=True, sprite_orientation=0, allow_shapes=True)
                    except AssertionError:
                        pass
                    else:
                        raise AssertionError("Default reader accepted authored shape sway without its opt-in")
                scales = [row["scale"] for row in parsed["props"]]
                if name.endswith("-scaled"):
                    require(2 ** -24 <= min(scales) < 1 < max(scales) <= 6, "Nonunit shape scales did not vary within native bounds")
                else:
                    require(all(scale == 1 for scale in scales), "Unit shape bytes changed")
                identity = [(row["origin"], row["angles"], row["leaf"], row["sprite"]) for row in parsed["props"]]
                require(identity == [(row["origin"], row["angles"], row["leaf"], row["sprite"]) for row in plain["props"]], "Shape changed placement identity")
                colors = [row["lighting"] for row in parsed["props"]]
                if reference is None:
                    reference = colors
                require(colors == reference, "Shape parameters changed SDK origin-based lighting")
                for index, (before, after) in enumerate(zip(probe.bsp_lumps(plain_data), probe.bsp_lumps(data), strict=True)):
                    if index != 35:
                        require(before == after, f"Shape changed unrelated lump {index}")
                _, repeated, _ = compile_pair(client, args.output, f"{name}-repeated", ids)
                require(repeated == data, "Shape output is nondeterministic")
                variants[name] = {"bsp": str(bsp), "bsp_sha256": probe.digest(data), "kind": kind, "shape_angle": angle, "shape_size": size, "count": len(parsed["props"]), "sprite_entries": len(parsed["sprites"]), "scale_range": [min(scales), max(scales)], "sway": sway}
            previous = args.output / "rejected.bsp"
            previous.write_bytes(plain_data)
            rejected = {}
            for name, fields, expected in [
                ("unknown-shape", "sprite_shape unknown", "cross or tri"),
                ("cross-ignored-angle", "sprite_shape cross shape_angle 1", "ignores"),
                ("cross-ignored-size", "sprite_shape cross shape_size 0.5", "ignores"),
                ("fractional-angle", "sprite_shape tri shape_angle 0.5", "integer"),
                ("oversized-angle", "sprite_shape tri shape_angle 256", "shape_angle"),
                ("oversized-size", "sprite_shape tri shape_size 1.1", "shape_size"),
                ("sway", "sprite_shape tri sway NaN", "sway"),
                ("scale", "sprite_shape tri spriterandomscale 1.1", "spriterandomscale"),
            ]:
                dictionary_path.write_text(dictionary(f"{FIELDS} {fields}"))
                before = state(client, ids, source)
                response = client.call("compile_map", {"path": str(previous)}, error=True)
                failure = json.loads(response["content"][0]["text"])
                require(not failure["success"] and expected.lower() in json.dumps(failure["diagnostics"]).lower(), failure)
                require(previous.read_bytes() == plain_data and state(client, ids, source) == before, "Shape failure changed source/history or previous output")
                client.call("start_compile", {"path": str(previous)})
                job = wait_job(client)
                require(job["status"] == "failed" and previous.read_bytes() == plain_data and state(client, ids, source) == before, job)
                rejected[name] = failure["diagnostics"]
            dictionary_path.write_text(dictionary(FIELDS))
            _, restored, _ = compile_pair(client, args.output, "plain-restored", ids)
            require(restored == plain_data and source.read_bytes() == saved and detail(client, ids) == authored, "Shape controls changed plain output or saved source")
            result = {"success": True, "variants": variants, "rejected": rejected, "saved_reopened": True, "foreground_background_exact": True, "source_history_preserved": True, "plain_bytes_preserved": True, "fixture_game": str(game), "native_exercised": False}
            (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
            return result
    finally:
        client.close()


def native_capture(args, result, name, variant, default_avoidance=False):
    output = args.output / f"native-{name}"
    command = [sys.executable, "-B", str(Path(__file__).with_name("validate_detail_props.py")),
            "--binary", str(args.binary), "--output", str(output), "--game-dir", str(args.game_dir), "--runtime",
            "--runtime-bsp", variant["bsp"], "--fixture-game", result["fixture_game"], "--expected-count", str(variant["count"]),
            "--expected-sprites", str(variant["count"]), "--sprite-orientation", "0", "--allow-shapes", "--baked-lighting",
            "--view", "elevated", "--port", str(args.port)]
    if args.sway:
        command += ["--sway-amplitude", "8"]
    if default_avoidance:
        command.append("--default-shape-avoidance")
    subprocess.run(command, check=True, timeout=150)
    report = json.loads((output / "results.json").read_text())
    require(report["native_exercised"] and report["native"]["success"], report)
    return report["native"]


def native(args, result):
    captures = {name: native_capture(args, result, name, variant) for name, variant in result["variants"].items()}
    if args.sway:
        return {"captures": captures, "sway_controls": {name: sway_frames(args.output / f"native-{name}/runtime", variant["sway"]) for name, variant in result["variants"].items()}}
    captures["cross-default-avoidance"] = native_capture(args, result, "cross-default-avoidance", result["variants"]["cross"], default_avoidance=True)
    first = silhouette(args.output / "native-tri/runtime")
    second = silhouette(args.output / "native-tri-fanned/runtime")
    changed = sum(a != b for a, b in zip(first, second, strict=True))
    require(changed > len(first) * 0.002, "Tri shape parameter silhouette control did not change")
    scale_controls = {}
    for unit, scaled in [("cross", "cross-scaled"), ("tri-fanned", "tri-scaled")]:
        unit_mask = silhouette(args.output / f"native-{unit}/runtime")
        scaled_mask = silhouette(args.output / f"native-{scaled}/runtime")
        difference = sum(a != b for a, b in zip(unit_mask, scaled_mask, strict=True))
        require(difference > 0, "Shape scale silhouette did not change")
        scale_controls[scaled] = {"different_pixels": difference, "unit_pixels": sum(unit_mask), "scaled_pixels": sum(scaled_mask)}
    return {"captures": captures, "scale_silhouettes": scale_controls, "tri_parameter_silhouette": {"different_pixels": changed, "flat_pixels": sum(first), "fanned_pixels": sum(second)}}

def sway_frames(directory, encoded):
    frames = {name: ppm(directory / f"{name}.ppm") for name in ["details-on", "details-sway", "details-zero", "details-zero-repeat", "details-off"]}
    mask = []
    background = []
    for y in range(24, 80):
        for x in range(320, 960):
            offset = (y * probe.WIDTH + x) * 3
            for name in ["details-on", "details-sway", "details-zero", "details-zero-repeat"]:
                background.extend(abs(a - b) for a, b in zip(frames[name][offset:offset + 3], frames["details-off"][offset:offset + 3], strict=True))
    moving = controlled = stationary = 0
    for y in range(180, 650):
        for x in range(160, 1120):
            offset = (y * probe.WIDTH + x) * 3
            pixels = {name: data[offset:offset + 3] for name, data in frames.items()}
            foreground = any(max(abs(a - b) for a, b in zip(pixels[name], pixels["details-off"], strict=True)) > 12 for name in ["details-on", "details-sway", "details-zero", "details-zero-repeat"])
            positive = max(abs(a - b) for a, b in zip(pixels["details-on"], pixels["details-sway"], strict=True))
            zero = max(abs(a - b) for a, b in zip(pixels["details-zero"], pixels["details-zero-repeat"], strict=True))
            if foreground:
                mask.append(offset)
                moving += positive > 12
                stationary += zero > 12
                controlled += max(abs(a - b) for a, b in zip(pixels["details-on"], pixels["details-zero"], strict=True)) > 12
    require(len(mask) > 150 and background, "Sway fixture lacks foreground/background controls")
    require(stationary < 10 and max(background) == 0, f"Zero-amplitude or fixed wall captures moved: {stationary}, {max(background)}")
    if encoded:
        require(moving > 30 and controlled > 30, f"Encoded sway did not produce native temporal/amplitude geometry changes: {moving}, {controlled}")
    else:
        require(moving < 10 and controlled < 10, f"Zero encoded sway unexpectedly moved: {moving}, {controlled}")
    return {"encoded_sway": encoded, "same_process": True, "amplitudes": [8, 0], "foreground_pixels": len(mask), "positive_amplitude_changed_pixels": moving, "amplitude_control_changed_pixels": controlled, "zero_amplitude_changed_pixels": stationary, "background_mean_difference": sum(background) / len(background)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--fgd", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/SourceSDK/bin/source2009/bin/cstrike.fgd")
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--sway", action="store_true", help="Compile byte0/127/255 shape sway fixtures and compare controlled native temporal captures")
    parser.add_argument("--port", type=int, default=27719)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    args.output = args.output.resolve()
    require(not args.output.exists(), "Use a fresh absent output directory")
    args.output.mkdir(parents=True, mode=0o700)
    result = validate(args)
    if args.runtime:
        result.update({"native_exercised": True, "native": native(args, result)})
        (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"success": True, "results": str(args.output / "results.json"), "native_exercised": result["native_exercised"]}))


if __name__ == "__main__":
    main()
