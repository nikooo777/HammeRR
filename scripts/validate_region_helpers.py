import argparse
import json
import re
from pathlib import Path
from unittest.mock import patch

import validate_areaportals as areas
import validate_native_runtime as runtime
import validate_window_portals as windows
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_compile_regions import apply
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_occluders import native_occlusion, occluder
from validate_visibility import ROOM


def fixture(client):
    for low, high in ROOM + [
        ([-8, -256, 0], [8, -64, 256]),
        ([-8, 64, 0], [8, 256, 256]),
        ([-8, -64, 192], [8, 64, 256]),
    ]:
        client.call("create_block", {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"})
    for classname, origin, properties in [
        ("info_player_terrorist", [-64, -64, 8], {}),
        ("info_player_counterterrorist", [128, 64, 8], {}),
        ("light", [-160, 0, 180], {"_light": "255 100 100 200", "_constant_attn": "1", "_quadratic_attn": "0"}),
        ("light", [160, 0, 180], {"_light": "100 100 255 200", "_constant_attn": "1", "_quadratic_attn": "0"}),
        ("logic_script", [0, 0, 64], {"targetname": "hammerr_observer", "vscripts": "hammerr_observer.nut", "thinkfunction": "HammeRRThink"}),
        ("prop_physics", [160, -128, 128], {"targetname": "physics_crate", "model": "models/props_junk/wood_crate001a.mdl"}),
    ]:
        client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})
    brush = client.call("create_block", {"mins": [-4, -64, 0], "maxs": [4, 64, 192], "material": "tools/toolsareaportal"})["id"]
    gate = client.call("tie_to_entity", {"ids": [brush], "classname": "func_areaportal", "properties": {"targetname": "partition_gate", "StartOpen": "1"}})["id"]
    screen = occluder(client, [-160, -32, 16], [-144, 32, 160], "0")
    outside = occluder(client, [600, -32, 16], [616, 32, 160])
    for helper in [gate, screen]:
        client.call("set_entity_properties", {"id": helper, "set": {"origin": "1000 0 64"}})
    client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"})
    return gate, screen, outside


def validate(args):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    with patch.dict("os.environ", {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json")}):
        client = Client(None, args.binary, args.output)
    try:
        gate, screen, outside = fixture(client)
        apply(client, low=[-256, -256, 0], high=[256, 256, 256])
        source = args.output / "helpers.vmf"
        client.call("save_map", {"path": str(source)})
        original = source.read_bytes()
        objects = detail(client, [gate, screen, outside])
        history = client.call("get_map_info")["undo"]
        path = args.output / "portal-region.bsp"
        built = client.call("compile_map", {"path": str(path)})
        data = path.read_bytes()
        areas.graph(data, built["stats"])
        native, polygons, indices = native_occlusion(data)
        check(built["success"] and len(native) == 1 and len(polygons) == 6 and len(indices) == 24, "A region emits a complete native occluder beside a reciprocal two-area portal")
        check(native[0][0] == 1 and native[0][3:9] == (-160, -32, 16, -144, 32, 160), "Contained helper geometry wins over outside editor origins and preserves activation state")
        scope = built["stats"]["compile_region"]
        check(gate in scope["included"] and screen in scope["included"] and outside in scope["omitted"], "Region accounting includes contained helpers and omits disjoint occluders")
        background = args.output / "portal-background.bsp"
        client.call("start_compile", {"path": str(background)})
        check(wait_job(client)["status"] == "completed" and background.read_bytes() == data, "Both shared build paths emit identical helper records")
        check(objects == detail(client, [gate, screen, outside]) and client.call("get_map_info")["undo"] == history and source.read_bytes() == original, "Compilation preserves helper properties, all source geometry, history and saved bytes")
        for helper, offset in [(screen, [0, 240, 0]), (gate, [0, 240, 0])]:
            client.call("move_objects", {"ids": [helper], "offset": offset})
            rejected = client.call("compile_map", {"path": str(path)}, error=True)
            check("cross" in json.dumps(rejected).lower() and path.read_bytes() == data, "A boundary-crossing helper fails before replacing the previous BSP")
            client.call("start_compile", {"path": str(path)})
            check(wait_job(client)["status"] == "failed" and path.read_bytes() == data, "Background helper rejection preserves the same previous output")
            client.call("undo")
        models = []
        for name, material in [("rendered_window", "dev/dev_measuregeneric01"), ("window_foreground", "glass/glasswindow001b")]:
            brush = client.call("create_block", {"mins": [-8, -64, 0], "maxs": [8, 64, 192], "material": material})["id"]
            models.append(client.call("tie_to_entity", {"ids": [brush], "classname": "func_brush", "properties": {"targetname": name, "origin": "0 0 96", "Solidity": "1"}})["id"])
        client.call("set_entity_properties", {"id": gate, "set": {"classname": "func_areaportalwindow", "target": "rendered_window", "BackgroundBModel": "window_foreground", "FadeStartDist": "64", "FadeDist": "128", "TranslucencyLimit": "0.2"}})
        window_path = args.output / "window-region.bsp"
        window = client.call("compile_map", {"path": str(window_path)})
        window_data = window_path.read_bytes()
        portals = records(window_data, 21, "<4Hi")
        assert window["stats"]["areas"] == 2 and len(portals) == 3 and portals[1][4] ^ 1 == portals[2][4]
        check(window["stats"]["areaportal_windows"] == 1 and window["stats"]["models"] == 3 and len(native_occlusion(window_data)[0]) == 1, "Region windows retain both rendered brush-model bindings and native occluder areas")
        client.call("start_compile", {"path": str(background)})
        check(wait_job(client)["status"] == "completed" and background.read_bytes() == window_data, "Shared window builds agree byte for byte")
        client.call("set_object_visibility", {"ids": [models[0]], "action": "hide"})
        rejected = client.call("compile_map", {"path": str(window_path)}, error=True)
        check("visible rendered brush model" in json.dumps(rejected) and window_path.read_bytes() == window_data, "Missing window render targets fail explicitly without losing the previous BSP")
        client.call("undo")
        (args.output / "validation.json").write_text(json.dumps({"passed": len(checks), "checks": checks, "portal": built["stats"], "window": window["stats"]}, indent=2) + "\n")
        print(json.dumps({"passed": len(checks), "output": str(args.output)}))
        return path, window_path
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True, help="Fresh artifact directory")
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--window", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.binary = args.binary.resolve()
    if args.worker:
        if args.window:
            windows.worker(args)
        else:
            runtime.OBSERVER = areas.OBSERVER
            runtime.worker(args)
        return
    args.output.mkdir(parents=True)
    portal, window = validate(args)
    if args.runtime:
        for name, bsp, flags in [("portal", portal, []), ("window", window, ["--window"])]:
            native = runtime.private_display(Path(__file__), args, bsp, extra_flags=flags, output=args.output / f"native-{name}")
            log = (native / "cstrike/hammerr-runtime.log").read_text(errors="replace")
            check_lan = '"sv_lan" = "1"' in log
            assert check_lan, log[-3000:]
            if name == "portal":
                masks = [tuple(map(int, values)) for values in re.findall(r"HAMMERR_AREA_STATE (\d+) (\d+)", log)]
                latest = {stage: [mask & 6 for phase, mask in masks if phase == stage] for stage in [0, 1, 2]}
                assert all(latest.values()) and latest[0][-1] == latest[2][-1] == 6 and latest[1][-1] in [2, 4], latest
                bits = [tuple(map(int, values)) for values in re.findall(r"HAMMERR_PORTAL_BITS (\d+) (\d+)", log)]
                latest_bits = {stage: [mask & 2 for phase, mask in bits if phase == stage] for stage in [0, 1, 2]}
                assert all(latest_bits.values()) and [latest_bits[stage][-1] for stage in [0, 1, 2]] == [2, 0, 2], latest_bits
        print(json.dumps({"native_portal_and_window": True, "insecure": True, "sv_lan": True}))


if __name__ == "__main__":
    main()
