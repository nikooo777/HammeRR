import argparse
import json
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_native_runtime as runtime
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_detail_props import SMALL_BUSH, compile_pair, detail_records
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_material_details import state
from validate_visibility import ROOM


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def colors(data):
    rows = detail_records(data, allow_baked=True)["props"]
    require(len(rows) == 2, "Expected two identical-placement explicit models")
    require(rows[0]["origin"] == rows[1]["origin"] and rows[0]["angles"] == rows[1]["angles"], "Fixture placements differ")
    return [[value * 2.0 ** row["lighting"][3] for value in row["lighting"][:3]] for row in rows]


def validate(args):
    game = runtime.private_game(args.game_dir, args.output / "source-game")
    material = game / "materials/hammerr_detail_origins/black.vmt"
    material.parent.mkdir(parents=True)
    material.write_text("UnlitGeneric { }\n")
    client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    try:
        with stopped_on_exit(client):
            client.call("load_entity_definitions", {"path": str(args.fgd)})
            ids = [client.call("create_block", {"mins": low, "maxs": high, "material": "hammerr_detail_origins/black"})["id"] for low, high in ROOM]
            props = []
            for _ in range(2):
                props.append(client.call("create_entity", {"classname": "prop_detail", "origin": [0, 0, 16], "properties": {"model": SMALL_BUSH}})["id"])
            ids += props
            helpers = []
            for name, point in [("red_sample", [-96, 0, 64]), ("blue_sample", [96, 0, 64])]:
                helpers.append(client.call("create_entity", {"classname": "info_lighting", "origin": point, "properties": {"targetname": name}})["id"])
            ids += helpers
            for point, value in [([-96, 0, 128], "255 0 0 80"), ([96, 0, 128], "0 0 255 80")]:
                ids.append(client.call("create_entity", {"classname": "light", "origin": point, "properties": {"_light": value, "_constant_attn": "0", "_linear_attn": "0", "_quadratic_attn": "1"}})["id"])
            client.call("set_compile_settings", {"lighting": "fullbright", "physics": "none", "bounces": 0, "sky_quality": "fast"})
            _, plain, _ = compile_pair(client, args.output, "no-reference", ids)
            for prop, name in zip(props, ["red_sample", "blue_sample"], strict=True):
                client.call("set_entity_properties", {"id": prop, "set": {"lightingorigin": name}})
                inspector = client.call("get_entity_inspector", {"id": prop})
                field = next(field for field in inspector["properties"] if field["key"] == "lightingorigin")
                require(field["advanced"] and field["value"] == name, "Shared Advanced properties did not inspect lightingorigin")
            _, fullbright, _ = compile_pair(client, args.output, "named-fullbright", ids)
            require(fullbright == plain, "Fullbright references changed native placement or output bytes")
            source = args.output / "named-origins.vmf"
            client.call("save_map", {"path": str(source)})
            saved = source.read_bytes()
            authored = detail(client, ids)
            client.call("open_map", {"path": str(source)})
            require(detail(client, ids) == authored, "Save/reopen changed named-origin properties")
            results = {}
            for mode in ["direct_ldr", "direct_ldr_hdr"]:
                client.call("set_compile_settings", {"lighting": mode})
                _, original, _ = compile_pair(client, args.output, mode, ids)
                first = colors(original)
                require(first[0][0] > first[0][2] * 4 and first[1][2] > first[1][0] * 4, f"Named receivers lost distinct colors: {first}")
                for prop, name in zip(props, ["blue_sample", "red_sample"], strict=True):
                    client.call("set_entity_properties", {"id": prop, "set": {"lightingorigin": name}})
                _, swapped, _ = compile_pair(client, args.output, f"{mode}-swapped", ids)
                require(colors(swapped) == list(reversed(first)), "Identical native placements lost source-to-receiver alignment")
                for prop, name in zip(props, ["red_sample", "blue_sample"], strict=True):
                    client.call("set_entity_properties", {"id": prop, "set": {"lightingorigin": name}})
                results[mode] = {"bsp_sha256": probe.digest(original), "colors": first, "swapped_colors": colors(swapped)}
            client.call("set_compile_settings", {"lighting": "fullbright"})
            previous = args.output / "previous.bsp"
            previous.write_bytes(fullbright)
            rejected = {}
            for name in ["missing", "hidden", "duplicate"]:
                if name == "missing":
                    client.call("set_entity_properties", {"id": props[0], "set": {"lightingorigin": "missing_sample"}})
                elif name == "hidden":
                    client.call("set_object_visibility", {"action": "hide", "ids": [helpers[0]]})
                else:
                    duplicate = client.call("create_entity", {"classname": "info_lighting", "origin": [-96, 0, 64], "properties": {"targetname": "red_sample"}})["id"]
                before = state(client, ids, source)
                response = client.call("compile_map", {"path": str(previous)}, error=True)
                failure = json.loads(response["content"][0]["text"])
                require(not failure["success"] and any("info_lighting" in diagnostic["message"] for diagnostic in failure["diagnostics"]), failure)
                client.call("start_compile", {"path": str(previous)})
                require(wait_job(client)["status"] == "failed", "Invalid reference background build succeeded")
                require(previous.read_bytes() == fullbright and state(client, ids, source) == before, "Failed named reference changed prior BSP, source or history")
                rejected[name] = failure["diagnostics"]
                if name == "missing":
                    client.call("set_entity_properties", {"id": props[0], "set": {"lightingorigin": "red_sample"}})
                elif name == "hidden":
                    client.call("set_object_visibility", {"action": "show", "ids": [helpers[0]]})
                else:
                    client.call("delete_objects", {"ids": [duplicate]})
            require(source.read_bytes() == saved and detail(client, ids) == authored, "Named-origin controls changed saved or authored source")
            result = {"success": True, "transport": "private stdio", "same_position_receivers": results, "fullbright_bytes_preserved": True, "saved_reopened": True, "foreground_background_exact": True, "source_history_preserved": True, "rejected": rejected, "native_exercised": False}
            (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps({"success": True, "results": str(args.output / "results.json"), "native_exercised": False}))
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser(description="Validate named explicit detail model lighting receivers through isolated MCP property edits.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--fgd", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/SourceSDK/bin/source2009/bin/cstrike.fgd")
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    args.output = args.output.resolve()
    require(not args.output.exists(), "Use a fresh absent output directory")
    args.output.mkdir(parents=True, mode=0o700)
    validate(args)


if __name__ == "__main__":
    main()
