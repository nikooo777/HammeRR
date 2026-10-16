import argparse
import json
import subprocess
import sys
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_native_runtime as runtime
from validate_compile_jobs import stopped_on_exit
from validate_cubemap_reflections import ppm
from validate_detail_props import SMALL_BUSH, compile_pair, detail_records
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_material_details import set_world
from validate_visibility import ROOM

PLAIN = "hammerr_detail_lighting/plain"
GROUND = "hammerr_detail_lighting/ground"
TEXTURE = "dev/dev_measuregeneric01"
DICTIONARY = "scripts/hammerr_detail_lighting.vbsp"
CONTROLS = {"bright": "255 255 255 180", "dim": "255 255 255 40", "colored": "255 48 48 180"}
REGIONS = {"models": (230, 215, 590, 620), "sprites": (680, 215, 1160, 620)}


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
            for classname, origin, fields in [
                ("info_player_terrorist", [-192, -176, 8], {}),
                ("info_player_counterterrorist", [192, 176, 8], {}),
                ("logic_script", [0, -176, 64], {"targetname": "hammerr_detail_observer", "vscripts": "hammerr_detail_observer.nut", "thinkfunction": "HammeRRThink"}),
                ("prop_detail", [-96, 64, 20], {"model": SMALL_BUSH}),
                ("prop_detail", [-96, 128, 20], {"model": SMALL_BUSH}),
            ]:
                ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": fields})["id"])
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
            result = {"success": True, "controls": controls, "fixture_game": str(game), "light_id": light, "object_ids": ids, "fixture_source": str(args.output / "bright.vmf"), "source_history_preserved": True, "foreground_background_exact": True, "fullbright_white_default_preserved": True, "hdr_shared_base": True, "native_exercised": False}
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--fgd", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/SourceSDK/bin/source2009/bin/cstrike.fgd")
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--port", type=int, default=27719)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    args.output = args.output.resolve()
    require(not args.output.exists(), "Use a fresh absent output directory")
    require(1024 <= args.port <= 65530, "Invalid native port")
    args.output.mkdir(parents=True, mode=0o700)
    result = validate(args)
    if args.runtime:
        native_colors(args, result)
    print(json.dumps({"success": True, "results": str(args.output / "results.json"), "native_exercised": result["native_exercised"]}))


if __name__ == "__main__":
    main()
