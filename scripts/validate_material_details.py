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
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_visibility import ROOM

MODEL = "models/props/de_inferno/bushgreensmall.mdl"
MATERIAL = "hammerr_material_details/ground"
TEXTURE = "dev/dev_measuregeneric01"
DICTIONARY = "scripts/hammerr_material_details.vbsp"


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def set_world(client, key, value):
    token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
    client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": key, "value": value}]})
    client.call("commit_property_edit", {"token": token})


def dictionary(density=64, fields=None):
    fields = fields or f'model "{MODEL}" upright 1 minAngle 0 maxAngle 0'
    return f'detail {{ hammerr_tufts {{ density {density} group {{ alpha 1 item {{ {fields} }} }} }} }}\n'


def state(client, ids, source):
    return detail(client, ids), client.call("get_map_info")["undo"], source.read_bytes()


def validate(args):
    game = runtime.private_game(args.game_dir, args.output / "source-game")
    material = game / f"materials/{MATERIAL}.vmt"
    material.parent.mkdir(parents=True)
    material.write_text(f'LightmappedGeneric {{ $basetexture "{TEXTURE}" }}\n')
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
            source = args.output / "material-details.vmf"
            client.call("save_map", {"path": str(source)})
            _, empty, empty_result = compile_pair(client, args.output, "empty", ids)
            empty_records = detail_records(empty)
            require(not empty_records["props"] and not empty_records["models"] and empty_result["stats"]["detail_props"] == 0, "Plain material control emitted detail props")
            material.write_text(f'LightmappedGeneric {{ $basetexture "{TEXTURE}" %detailtype hammerr_tufts }}\n')
            client.call("reload_material_assets")
            authored = detail(client, ids)
            saved = source.read_bytes()
            client.call("open_map", {"path": str(source)})
            require(detail(client, ids) == authored, "VMF reopen changed authored surfaces")
            bsp, data, result = compile_pair(client, args.output, "material-details", ids)
            parsed = detail_records(data)
            require(len(parsed["props"]) == result["stats"]["detail_props"] == 16, parsed)
            require(parsed["models"] == [MODEL] and result["stats"]["detail_prop_models"] == 1, parsed)
            require(result["stats"]["static_props"] == 0, result["stats"])
            leaves = records(data, 10, "<ihh6h4HhH", expected_version=1)
            require(all(row["origin"][2] == 0 and all(-256 <= value <= 256 for value in row["origin"][:2]) for row in parsed["props"]), "Generated models escaped the exposed floor")
            require(all(row["angles"][0] == row["angles"][2] == 0 and 0 <= row["angles"][1] < 360 for row in parsed["props"]), "Upright generated orientations are invalid")
            require(all(row["leaf"] < len(leaves) and leaves[row["leaf"]][0] & 1 == 0 and leaves[row["leaf"]][1] >= 0 for row in parsed["props"]), "Generated models did not bind to open visible leaves")
            require(len({tuple(row["origin"]) for row in parsed["props"]}) == len(parsed["props"]), "Generated records contain duplicate placements")
            before_lumps, after_lumps = probe.bsp_lumps(empty), probe.bsp_lumps(data)
            for index in range(64):
                if index != 35:
                    require(before_lumps[index] == after_lumps[index], f"Scattering changed unrelated lump {index}")
            require(source.read_bytes() == saved, "Compilation changed saved VMF")
            rejected = {}
            previous = args.output / "rejected.bsp"
            previous.write_bytes(data)
            for name, text, expected in [
                ("unknown-type", "detail { other { density 64 group { item { model missing.mdl } } } }", "%detailtype"),
                ("sprite", dictionary(fields='sprite "0 0 32 32 256"'), "models only"),
                ("missing-model", dictionary(fields='model models/hammerr_missing_detail.mdl upright 1 minAngle 0 maxAngle 0'), "not found"),
                ("lit-model", dictionary(fields='model models/props_junk/wood_crate001a.mdl upright 1 minAngle 0 maxAngle 0'), "unlit"),
                ("density", dictionary(density="NaN"), "density"),
                ("sample-budget", dictionary(density=1000000, fields=f'model "{MODEL}" amount 0'), "sample build budget"),
                ("prop-budget", dictionary(density=100000, fields=f'model "{MODEL}" upright 1 minAngle 0 maxAngle 0'), "16,384-object"),
                ("orientation", dictionary(fields=f'model "{MODEL}" detailOrientation 1'), "detailOrientation=0"),
            ]:
                dictionary_path.write_text(text)
                before = state(client, ids, source)
                response = client.call("compile_map", {"path": str(previous)}, error=True)
                failure = json.loads(response["content"][0]["text"])
                require(not failure["success"] and expected.lower() in json.dumps(failure["diagnostics"]).lower(), failure)
                require(previous.read_bytes() == data and state(client, ids, source) == before, "Rejected compile replaced output or changed source/history")
                client.call("start_compile", {"path": str(previous)})
                job = wait_job(client)
                require(job["status"] == "failed" and previous.read_bytes() == data and state(client, ids, source) == before, job)
                rejected[name] = failure["diagnostics"]
            dictionary_path.write_text(dictionary())
            _, repeated, _ = compile_pair(client, args.output, "material-details-repeated", ids)
            require(repeated == data, "Dictionary restore changed the deterministic distribution")
            require(source.read_bytes() == saved, "Validation changed authored VMF bytes")
            report = {"success": True, "stats": result["stats"], "records": parsed, "rejected": rejected, "saved_reopened": True, "foreground_background_exact": True, "source_history_preserved": True, "empty_control_geometry_exact": True, "native_exercised": False, "fixture_game": str(game), "bsp": str(bsp), "bsp_sha256": probe.digest(data)}
            (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
            return report
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--fgd", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/SourceSDK/bin/source2009/bin/cstrike.fgd")
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--port", type=int, default=27699)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    args.output = args.output.resolve()
    require(not args.output.exists(), "Use a fresh absent output directory")
    args.output.mkdir(parents=True, mode=0o700)
    result = validate(args)
    if args.runtime:
        native_output = args.output / "native"
        subprocess.run([sys.executable, "-B", str(Path(__file__).with_name("validate_detail_props.py")),
            "--binary", str(args.binary), "--output", str(native_output),
            "--game-dir", str(args.game_dir), "--runtime", "--runtime-bsp", result["bsp"],
            "--fixture-game", result["fixture_game"], "--expected-count", "16", "--port", str(args.port)],
            check=True, timeout=150)
        native = json.loads((native_output / "results.json").read_text())
        require(native["native_exercised"] and native["native"]["success"], native)
        result.update({"native_exercised": True, "native": native["native"]})
        (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"success": result["success"], "results": str(args.output / "results.json"), "bsp": result["bsp"], "native_exercised": result["native_exercised"]}))


if __name__ == "__main__":
    main()
