import argparse
import json
import re
from pathlib import Path
from unittest.mock import patch

import validate_native_runtime as runtime
from validate_face_lighting import ROOT, Client, detail
from validate_terrain_lod import build

LOW = [-128, -128, 0]
HIGH = [128, 128, 256]
OBSERVER = runtime.OBSERVER.replace(
    "function HammeRRThink() {",
    '''function HammeRRThink() {
    printl("HAMMERR_REGION_TRACE " +
        TraceLine(Vector(0, 0, 64), Vector(300, 0, 64), null) + " " +
        TraceLine(Vector(0, 0, 64), Vector(-300, 0, 64), null) + " " +
        TraceLine(Vector(0, 0, 64), Vector(0, 300, 64), null) + " " +
        TraceLine(Vector(0, 0, 64), Vector(0, -300, 64), null));''',
).replace("sv_cheats 1; joystick 0;", "sv_cheats 1; joystick 0; bot_quota 0; bot_kick;")
SOURCE = [
    ([-528, -528, -16], [528, 528, 0]),
    ([-528, -528, 256], [528, 528, 272]),
    ([-528, -528, 0], [-512, 528, 256]),
    ([512, -528, 0], [528, 528, 256]),
    ([-512, -528, 0], [512, -512, 256]),
    ([-512, 512, 0], [512, 528, 256]),
    ([-600, -16, 0], [600, 16, 32]),
]


def shell(low, high, thickness):
    outer_low = [value - thickness for value in low]
    outer_high = [value + thickness for value in high]
    for axis in range(3):
        for upper in [False, True]:
            a = outer_low.copy()
            b = outer_high.copy()
            for previous in range(axis):
                a[previous] = low[previous]
                b[previous] = high[previous]
            if upper:
                a[axis] = high[axis]
            else:
                b[axis] = low[axis]
            yield a, b


def fixture(args):
    with patch.dict("os.environ", {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json")}):
        client = Client(None, args.binary, args.output)
    try:
        kept = []
        excluded = []
        for a, b in SOURCE:
            brush = client.call("create_block", {"mins": a, "maxs": b, "material": "dev/dev_measuregeneric01"})["id"]
            (kept if all(x <= high and y >= low for x, y, low, high in zip(a, b, LOW, HIGH, strict=True)) else excluded).append(brush)
        for classname, origin, properties in [
            ("info_player_terrorist", [-64, -64, 8], {}),
            ("info_player_counterterrorist", [64, -64, 8], {}),
            ("light", [0, 0, 192], {"_light": "255 255 255 200", "_constant_attn": "1"}),
            ("prop_physics", [64, 64, 128], {"targetname": "physics_crate", "model": "models/props_junk/wood_crate001a.mdl"}),
            ("info_target", [400, 400, 64], {"targetname": "outside_region"}),
        ]:
            entity = client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"]
            (kept if all(low <= value <= high for low, value, high in zip(LOW, origin, HIGH, strict=True)) else excluded).append(entity)
        original = args.output / "source.vmf"
        client.call("save_map", {"path": str(original)})
        source = original.read_bytes()
        authored = detail(client, kept + excluded)
        history = client.call("get_map_info")["undo"]
        copied = client.call("copy_objects", {"ids": kept, "snapshot": client.call("get_clipboard")["snapshot"]})
        payload = client.call("export_clipboard", {"content_revision": copied["payload"]["content_revision"]})["text"]
        assert detail(client, kept + excluded) == authored and client.call("get_map_info")["undo"] == history
        client.call("new_map", {"discard_changes": True})
        imported = client.call("import_clipboard", {"text": payload, "snapshot": client.call("get_clipboard")["snapshot"]})
        preview = client.call("preview_paste", {"snapshot": imported["snapshot"], "offset": [0, 0, 0]})
        applied = client.call("apply_paste", {"token": preview["preview"]["token"]})
        placed = detail(client, applied["objects"])
        assert any(row["bounds"] == {"mins": SOURCE[-1][0], "maxs": SOURCE[-1][1]} for row in placed)
        for row in placed:
            if row.get("classname") == "prop_physics":
                client.call("set_entity_properties", {"id": row["id"], "set": {"targetname": "physics_crate"}})
        client.call("create_entity", {"classname": "logic_script", "origin": [-64, -64, 64], "properties": {"targetname": "hammerr_observer", "vscripts": "hammerr_observer.nut", "thinkfunction": "HammeRRThink"}})
        added = [client.call("create_block", {"mins": a, "maxs": b, "material": "tools/toolsnodraw"})["id"] for a, b in shell(LOW, HIGH, 16)]
        client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"})
        client.call("save_map", {"path": str(args.output / "prototype.vmf")})
        bsp, stats = build(client, args.output, "sealed")
        assert stats["brushes"] == 9 and stats["physics_models"] == 1, stats
        assert b"outside_region" not in bsp.read_bytes()
        client.call("delete_objects", {"ids": [added[1]]})
        broken = client.call("compile_map", {"path": str(args.output / "open-shell.bsp")}, error=True)
        failed = json.loads(broken["content"][0]["text"])
        assert failed.get("leak") and not (args.output / "open-shell.bsp").exists(), failed
        assert original.read_bytes() == source
        report = {"prototype_only": True, "region": [LOW, HIGH], "whole_crossing_brush_retained": True, "kept_sources": len(kept), "excluded_sources": len(excluded), "shell_brushes": 6, "sealed_stats": stats, "removed_boundary_wall_leaks": True, "original_file_unchanged": True, "shared_builds_identical": True, "desktop_input": False}
        (args.output / "shell-probe.json").write_text(json.dumps(report, indent=2) + "\n")
        return bsp
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser(description="Disposable MCP shell experiment; does not implement compile regions")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        runtime.OBSERVER = OBSERVER
        runtime.worker(args)
        return
    bsp = fixture(args)
    if args.runtime:
        output = runtime.private_display(Path(__file__), args, bsp)
        result = json.loads((output / "runtime.json").read_text())
        log = (output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
        assert '"sv_lan" = "1"' in log
        traces = [[float(value) for value in row] for row in re.findall(r"HAMMERR_REGION_TRACE ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+)", log)]
        assert len(traces) >= 3 and all(abs(value - 128 / 300) < 0.001 for row in traces for value in row), traces
        assert 0 < result["prop_positions"][-1][2] < 64, result
        assert max(abs(a - b) for a, b in zip(result["prop_positions"][-1], result["prop_positions"][-2], strict=True)) < 0.1
        report = {"prototype_only": True, "native_ground_and_prop_contact": True, "boundary_trace_fractions": traces[-3:], "insecure": True, "sv_lan": True}
        (args.output / "native-shell-probe.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report))
    else:
        print((args.output / "shell-probe.json").read_text())


if __name__ == "__main__":
    main()
