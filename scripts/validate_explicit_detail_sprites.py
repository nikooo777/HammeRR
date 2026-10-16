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
            for key, value, message in [("position_lr", "-20 0", "geometry"), ("tex_size", "0 128", "texture rectangle"), ("tex_total_size", "NaN", "tex_total_size"), ("scale", "2", "scale=1"), ("detailOrientation", "0.5", "integer"), ("lightingorigin", "missing", "model-only"), ("renderamt", "128", "renderamt"), ("sway", "0.1", "sway=0")]:
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
            result = {"success": True, "bsp": str(bsp), "bsp_sha256": probe.digest(data), "count": 24, "records": parsed, "rejected": rejected, "saved_reopened": True, "foreground_background_exact": True, "source_history_preserved": True, "unrelated_geometry_exact": True, "fixture_game": str(game), "native_exercised": False}
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
    parser.add_argument("--port", type=int, default=27729)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    args.output = args.output.resolve()
    require(not args.output.exists(), "Use a fresh absent output directory")
    args.output.mkdir(parents=True, mode=0o700)
    result = validate(args)
    if args.runtime:
        output = args.output / "native"
        subprocess.run([sys.executable, "-B", str(Path(__file__).with_name("validate_detail_props.py")), "--binary", str(args.binary), "--output", str(output), "--game-dir", str(args.game_dir), "--runtime", "--runtime-bsp", result["bsp"], "--fixture-game", result["fixture_game"], "--expected-count", "24", "--expected-sprites", "24", "--sprite-orientation", "2", "--baked-lighting", "--port", str(args.port)], check=True, timeout=150)
        native = json.loads((output / "results.json").read_text())
        require(native["native_exercised"] and native["native"]["success"], native)
        result.update({"native_exercised": True, "native": native["native"]})
        (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"success": True, "results": str(args.output / "results.json"), "native_exercised": result["native_exercised"]}))


if __name__ == "__main__":
    main()
