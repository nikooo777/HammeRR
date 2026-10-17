import argparse
import json
import subprocess
import sys
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_detail_lighting as base
from reflection_validation import ppm
from validate_compile_jobs import stopped_on_exit
from validate_detail_props import compile_pair, detail_records
from validate_face_lighting import DEFAULT_BINARY, Client, detail


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def layout(rows):
    return {"models": rows["models"], "sprites": rows["sprites"], "props": [{key: value for key, value in row.items() if key not in ["lighting", "light_styles", "light_style_count"]} for row in rows["props"]]}


def validate(args):
    setup_output = args.output / "static-controls"
    setup_output.mkdir()
    setup = base.validate(argparse.Namespace(**{**vars(args), "output": setup_output}))
    game = Path(setup["fixture_game"])
    editor_output = args.output / "style-authoring"
    editor_output.mkdir()
    client = Client(None, args.binary, editor_output, game / "gameinfo.txt")
    try:
        with stopped_on_exit(client):
            client.call("load_entity_definitions", {"path": str(args.fgd)})
            client.call("open_map", {"path": setup["fixture_source"]})
            light = setup["light_id"]
            ids = list(setup["object_ids"])
            client.call("set_entity_properties", {"id": light, "set": {"targetname": "hammerr_detail_keylight", "_light": "255 255 255 180", "_lightHDR": "0 0 255 100"}})
            ids.append(client.call("create_entity", {"classname": "light", "origin": [0, 96, 180], "properties": {"_light": "255 255 255 20", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}})["id"])
            client.call("set_compile_settings", {"lighting": "direct_ldr", "bounces": 0})
            source = args.output / "detail-light-styles.vmf"
            client.call("save_map", {"path": str(source)})
            saved = source.read_bytes()
            authored = detail(client, ids)
            client.call("open_map", {"path": str(source)})
            require(detail(client, ids) == authored, "Style source did not survive reopen")
            bsp, data, result = compile_pair(client, args.output, "detail-light-styles", ids)
            rows = detail_records(data, allow_sprites=True, allow_baked=True, allow_styles=True)
            require(len(rows["props"]) == 86 and sum(row.get("kind") == 1 for row in rows["props"]) == 84, rows)
            require(all(row["light_style_count"] == 1 for row in rows["props"]), "Named style did not reach every receiver")
            require([row["light_styles"] for row in rows["props"]] == list(range(86)), "Style offsets are not array indices")
            require(len(rows["style_lighting_ldr"]) == 86 and all(value[4] == 32 and max(value[:3]) > 0 for value in rows["style_lighting_ldr"]), rows)
            require(rows["style_lighting_hdr"] is None, "LDR-only fixture unexpectedly emitted HDR styles")
            try:
                detail_records(data, allow_sprites=True, allow_baked=True)
            except AssertionError:
                pass
            else:
                raise AssertionError("Default style-disabled reader accepted styled records")
            client.call("set_compile_settings", {"lighting": "direct_ldr_hdr"})
            _, dual, _ = compile_pair(client, args.output, "detail-styles-dual", ids)
            dual_rows = detail_records(dual, allow_sprites=True, allow_baked=True, allow_styles=True)
            require(layout(dual_rows) == layout(rows), "HDR styling changed receiver placement")
            require([value[4] for value in dual_rows["style_lighting_ldr"]] == [value[4] for value in dual_rows["style_lighting_hdr"]], "Dual style arrays differ in layout")
            require(all(value[0] == value[1] == 0 and value[2] > 0 for value in dual_rows["style_lighting_hdr"]), "HDR style did not retain its authored blue channel")
            client.call("set_compile_settings", {"lighting": "fullbright"})
            _, white, _ = compile_pair(client, args.output, "detail-styles-fullbright", ids)
            detail_records(white, allow_sprites=True)
            white_rows = detail_records(white, allow_sprites=True, allow_baked=True, allow_styles=True)
            require(layout(white_rows) == layout(rows) and white_rows["style_lighting_ldr"] is white_rows["style_lighting_hdr"] is None, "Fullbright altered placement or emitted style arrays")
            require(source.read_bytes() == saved and detail(client, ids) == authored, "Style baking changed source")
            report = {"success": True, "bsp": str(bsp), "bsp_sha256": probe.digest(data), "fixture_game": str(game), "records": rows, "stats": result["stats"], "source_history_preserved": True, "foreground_background_exact": True, "dual_style_layout": True, "native_exercised": False}
            (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
            return report
    finally:
        client.close()


def native(args, report):
    frames = {}
    for index, (name, state) in enumerate([("on", "on"), ("off", "off"), ("restored", "on")]):
        output = args.output / f"native-{name}"
        subprocess.run([sys.executable, "-B", str(Path(__file__).with_name("validate_detail_props.py")),
            "--binary", str(args.binary), "--output", str(output), "--game-dir", str(args.game_dir),
            "--runtime", "--runtime-bsp", report["bsp"], "--fixture-game", report["fixture_game"],
            "--expected-count", "86", "--expected-sprites", "84", "--baked-lighting", "--allow-detail-styles",
            "--detail-style-state", state, "--port", str(args.port + index)], check=True, timeout=150)
        result = json.loads((output / "results.json").read_text())
        require(result["native_exercised"] and result["native"]["success"] and result["native"]["detail_style_state"] == state, result)
        require(result["bsp_sha256"] == report["bsp_sha256"], "Style controls used different BSPs")
        frames[name] = {"on": ppm(output / "runtime/details-on.ppm"), "off": ppm(output / "runtime/details-off.ppm")}
    measurements = {}
    for kind, (left, top, right, bottom) in base.REGIONS.items():
        mask = []
        background = []
        for y in range(top, bottom):
            for x in range(left, right):
                offset = (y * probe.WIDTH + x) * 3
                pixels = {name: {state: frame[state][offset:offset + 3] for state in ["on", "off"]} for name, frame in frames.items()}
                background.extend(abs(a - b) for a, b in zip(pixels["on"]["off"], pixels["off"]["off"], strict=True))
                if all(max(abs(a - b) for a, b in zip(value["on"], value["off"], strict=True)) > 12 for value in pixels.values()):
                    mask.append(offset)
        require(sum(background) / len(background) < 0.25, f"{kind} visible background changed with the light style")
        require(len(mask) > 150, f"{kind} lacks a stable ON-minus-OFF detail mask")
        means = {name: [sum(frame["on"][offset + axis] for offset in mask) / len(mask) for axis in range(3)] for name, frame in frames.items()}
        require(sum(means["on"]) > sum(means["off"]) * 1.2 + 3, f"{kind} did not respond to actual TurnOn/TurnOff inputs: {means}")
        require(max(abs(a - b) for a, b in zip(means["on"], means["restored"], strict=True)) < 1.5, f"{kind} restored style differs: {means}")
        measurements[kind] = {"stable_detail_pixels": len(mask), "foreground_rgb": means, "background_mean_difference": sum(background) / len(background)}
    report.update({"native_exercised": True, "actual_named_light_inputs": True, "native_detail_style_pixels": measurements})
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--fgd", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/SourceSDK/bin/source2009/bin/cstrike.fgd")
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--port", type=int, default=27739)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    args.output = args.output.resolve()
    require(not args.output.exists() and 1024 <= args.port <= 65530, "Use a fresh output directory and valid native port")
    args.output.mkdir(parents=True, mode=0o700)
    report = validate(args)
    if args.runtime:
        native(args, report)
    print(json.dumps({"success": True, "results": str(args.output / "results.json"), "native_exercised": report["native_exercised"]}))


if __name__ == "__main__":
    main()
