import argparse
import json
import re
from pathlib import Path
from unittest.mock import patch

import validate_native_runtime as runtime
from probe_compile_region_shell import HIGH, LOW, OBSERVER, SOURCE
from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail


def preview(client, enabled=True, low=LOW, high=HIGH, clear=False):
    args = {"snapshot": client.call("get_compile_region")["snapshot"]}
    if clear:
        args["clear"] = True
    else:
        args.update(enabled=enabled, mins=low, maxs=high)
    return client.call("preview_compile_region", args)


def apply(client, **kwargs):
    draft = preview(client, **kwargs)
    return client.call("apply_compile_region", {"token": draft["draft"]["token"]})


def validate(args):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    with patch.dict("os.environ", {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json")}):
        client = Client(None, args.binary, args.output)
    try:
        ids = [client.call("create_block", {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"})["id"] for low, high in SOURCE]
        for classname, origin, properties in [
            ("info_player_terrorist", [-64, -64, 8], {}),
            ("info_player_counterterrorist", [64, -64, 8], {}),
            ("light", [0, 0, 192], {"_light": "255 255 255 200", "_constant_attn": "1"}),
            ("prop_physics", [64, 64, 128], {"targetname": "physics_crate", "model": "models/props_junk/wood_crate001a.mdl"}),
            ("logic_script", [-64, -64, 64], {"targetname": "hammerr_observer", "vscripts": "hammerr_observer.nut", "thinkfunction": "HammeRRThink"}),
            ("info_target", [400, 400, 64], {"targetname": "outside_region"}),
            ("prop_static", [400, 300, 16], {"model": "models/props_junk/wood_crate001a.mdl"}),
            ("light", [400, 400, 192], {"_light": "255 0 0 5000"}),
        ]:
            ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
        outside = ids[-3:]
        global_ids = []
        if args.global_markers:
            for classname, properties in [
                ("light_environment", {"_light": "255 255 255 64", "_ambient": "0 0 0 0"}),
                ("water_lod_control", {"cheapwaterstartdistance": "128", "cheapwaterenddistance": "512"}),
            ]:
                marker = client.call("create_entity", {"classname": classname, "origin": [400, 400, 192], "properties": properties})["id"]
                global_ids.append(marker)
                ids.append(marker)
        if args.water:
            pool = client.call("create_block", {"mins": [-128, 16, 0], "maxs": [128, 128, 64], "material": "tools/toolsnodraw"})["id"]
            brush = detail(client, [pool])[0]
            top = next(face for face in brush["faces"] if face["normal"] == [0, 0, 1])
            client.call("select_faces", {"faces": [{"solid": brush["solid"], "side": top["side"]}]})
            client.call("edit_faces", {"material": "liquids/aztecwater"})
            ids.append(pool)
        if args.terrain:
            terrain = client.call("create_block", {"mins": [32, 32, 0], "maxs": [112, 112, 32], "material": "dev/dev_measuregeneric01"})["id"]
            brush = detail(client, [terrain])[0]
            top = next(face for face in brush["faces"] if face["normal"] == [0, 0, 1])
            client.call("select_faces", {"faces": [{"solid": brush["solid"], "side": top["side"]}]})
            client.call("create_terrain", {"power": 2, "start_corner": 0})
            hint = client.call("get_compile_region")["suggested_bounds"]
            check(hint["mins"][2] <= 0 and hint["maxs"][2] >= 32, "Suggested terrain bounds include its complete base cage")
            ids.append(terrain)
        client.call("set_compile_settings", {"lighting": "fullbright", "physics": "none"})
        source = args.output / "source.vmf"
        client.call("save_map", {"path": str(source)})
        full_path = args.output / "full-map.bsp"
        full = client.call("compile_map", {"path": str(full_path)})
        check(full["success"] and full["stats"]["static_props"] == 1, "Full-map baseline includes the outside static prop")
        original_objects = detail(client, ids)
        original_state = client.call("get_map_info")
        draft = preview(client)
        token = draft["draft"]["token"]
        check(client.call("get_map_info")["undo"] == original_state["undo"] and detail(client, ids) == original_objects, "A region draft changes no source object or history")
        untouched = args.output / "draft-full-map.bsp"
        client.call("compile_map", {"path": str(untouched)})
        check(untouched.read_bytes() == full_path.read_bytes(), "Unapplied region bounds do not affect compilation")
        client.call("cancel_compile_region", {"token": token})
        check(client.call("get_compile_region")["committed"] is None, "Cancel leaves native metadata absent")
        draft = preview(client)
        old_token = draft["draft"]["token"]
        replacement = preview(client, enabled=False)
        client.call("cancel_compile_region", {"token": old_token}, error=True)
        check(client.call("get_compile_region")["draft"]["token"] == replacement["draft"]["token"], "A stale cancellation preserves the replacement")
        client.call("cancel_compile_region", {"token": replacement["draft"]["token"]})
        apply(client)
        check(client.call("get_compile_region")["committed"]["enabled"], "Apply enables the native region")
        check(client.call("get_map_info")["undo"] == ["Set compile region", *original_state["undo"][:9]], "Region Apply adds one metadata step to the visible history")
        client.call("undo")
        check(client.call("get_compile_region")["committed"] is None, "Undo removes the authored native region")
        client.call("redo")
        check(client.call("get_compile_region")["committed"]["enabled"], "Redo restores the region")
        client.call("frame_compile_region")
        check(detail(client, ids) == original_objects, "Framing changes no source geometry")
        if args.render:
            client.render(args.output / "region-preview.png", width=1000, height=650, camera={"position": [400, -500, 400], "look_at": [0, 0, 96]}, surface="wireframe")
        client.call("save_map", {"path": str(source)})
        saved = source.read_bytes()
        apply(client, enabled=False)
        client.call("compile_map", {"path": str(full_path)})
        client.call("undo")
        history = client.call("get_map_info")["undo"]
        selection = client.call("get_map_info")["selection"]
        region_path = args.output / "region.bsp"
        built = client.call("compile_map", {"path": str(region_path)})
        scope = built["stats"]["compile_region"]
        check(scope["mins"] == LOW and scope["maxs"] == HIGH and scope["shell_thickness"] == 16, "The result reports exact region and boundary dimensions")
        check(scope["boundary_material"] == "tools/toolsblack", "The boundary has a checked rendered black material")
        check(built["stats"]["brushes"] == 9 + int(args.water) and len(scope["generated_brushes"]) == 6, "Retained source brushes share six generated sealing slabs")
        check(client.call("get_map_info")["compile_region"] == client.call("get_compile_region")["build_scope"], "The map-info summary reports effective region scope without form bounds")
        check(all(item in scope["omitted"] for item in outside) and built["stats"]["static_props"] == 0, "Map, lights and independent static-prop baking share the filtered VMF")
        check(b"outside_region" not in region_path.read_bytes(), "The omitted point entity does not reappear in emitted entities")
        check(detail(client, ids) == original_objects and client.call("get_map_info")["undo"] == history and client.call("get_map_info")["selection"] == selection and source.read_bytes() == saved, "Compilation preserves source geometry, history, selection and saved bytes")
        background = args.output / "region-background.bsp"
        client.call("start_compile", {"path": str(background)})
        apply(client, enabled=False)
        job = wait_job(client)
        check(job["status"] == "completed" and job["compile_region"]["enabled"] and job["compile_region"]["mins"] == LOW, "The background job retains its committed start scope after region edits")
        check(background.read_bytes() == region_path.read_bytes(), "Synchronous and background region builds produce identical BSPs")
        check(job["stale"], "Changing region metadata marks the earlier build stale")
        disabled = args.output / "disabled.bsp"
        client.call("compile_map", {"path": str(disabled)})
        check(disabled.read_bytes() == full_path.read_bytes(), "Disabled region metadata preserves exact full-map BSP output")
        client.call("open_map", {"path": str(source), "discard_changes": True})
        check(client.call("get_compile_region")["committed"]["enabled"], "Native region metadata survives save and reopen")
        clear = preview(client, clear=True)
        client.call("cancel_compile_region", {"token": clear["draft"]["token"]})
        check(client.call("get_compile_region")["committed"]["enabled"], "Cancel of a Clear draft preserves the region")
        valid = preview(client)
        invalid = {"snapshot": client.call("get_compile_region")["snapshot"], "enabled": True, "mins": [0, 0, 0], "maxs": [0, 128, 256]}
        client.call("preview_compile_region", invalid, error=True)
        check(client.call("get_compile_region")["draft"]["token"] == valid["draft"]["token"], "Invalid bounds preserve a valid draft and ownership")
        client.call("cancel_compile_region", {"token": valid["draft"]["token"]})
        if args.water:
            previous = region_path.read_bytes()
            crossing = args.output / "crossing-water.bsp"
            crossing.write_bytes(previous)
            apply(client, high=[127, 128, 256])
            rejected = client.call("compile_map", {"path": str(crossing)}, error=True)
            check("crosses a compile-region boundary" in json.dumps(rejected) and crossing.read_bytes() == previous, "Crossing water rejects before replacing the previous region BSP")
            client.call("start_compile", {"path": str(crossing)})
            check(wait_job(client)["status"] == "failed" and crossing.read_bytes() == previous, "Background crossing-water rejection preserves the same output")
            client.call("undo")
        if args.terrain:
            previous = region_path.read_bytes()
            crossing = args.output / "crossing-terrain.bsp"
            crossing.write_bytes(previous)
            client.call("move_objects", {"ids": [terrain], "offset": [0, 0, 240]})
            rejected = client.call("compile_map", {"path": str(crossing)}, error=True)
            check("Terrain crosses a compile-region boundary" in json.dumps(rejected) and crossing.read_bytes() == previous, "Crossing terrain rejects without replacing the previous BSP")
            client.call("start_compile", {"path": str(crossing)})
            check(wait_job(client)["status"] == "failed" and crossing.read_bytes() == previous, "Background crossing-terrain rejection preserves prior output")
            client.call("undo")
            check(detail(client, [terrain]) == [row for row in original_objects if row["id"] == terrain], "Undo restores the exact terrain cage and grid after the control")
        helper = client.call("create_entity", {"classname": "info_overlay_transition", "origin": [400, 400, 64]})["id"]
        region_path.write_bytes(b"previous region output")
        rejected = client.call("compile_map", {"path": str(region_path)}, error=True)
        check("info_overlay_transition" in json.dumps(rejected) and region_path.read_bytes() == b"previous region output", "Unsupported consumed helpers fail explicitly and preserve the previous BSP")
        client.call("delete_objects", {"ids": [helper]})
        client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"})
        native_path = args.output / "native-region.bsp"
        native = client.call("compile_map", {"path": str(native_path)})
        check(native["stats"]["world_lights"] == 1 + int(args.global_markers), "Direct lighting retains the inside white light and any global sun")
        if args.global_markers:
            data = native_path.read_bytes()
            check(all(marker in scope["included"] for marker in global_ids) and data.count(b'"origin" "400 400 192"') == 2, "Outside nonspatial markers retain their original positions without leaking")
            check(data.count(b'"classname" "water_lod_control"') == 1 and b'"cheapwaterstartdistance" "128"' in data and b'"cheapwaterenddistance" "512"' in data, "Authored global water distances survive without generated defaults")
        native_background = args.output / "native-region-background.bsp"
        client.call("start_compile", {"path": str(native_background)})
        check(wait_job(client)["status"] == "completed" and native_path.read_bytes() == native_background.read_bytes(), "Lit engine-physics region builds agree through both paths")
        check(native["stats"]["physics_models"] == 1, "The generated boundary participates in engine collision")
        if args.water:
            check(native["stats"]["water_regions"] == native["stats"]["physics_fluids"] == 1 and native["stats"]["water_leaves"] > 0, "Contained world water retains native leaves and one engine fluid body")
        if args.terrain:
            check(native["stats"]["displacements"] == 1 and native["stats"]["displacement_triangles"] == native["stats"]["physics_mesh_triangles"] == 32, "Contained terrain retains its native grid and collision triangles")
        native_source = args.output / "native-source.vmf"
        client.call("save_map", {"path": str(native_source)})
        legacy = args.output / "legacy-import.bsp"
        client.call("compile_map", {"path": str(legacy)})
        metadata = f'cordons {{ "active" "1" cordon {{ "name" "Imported region" "active" "1" box {{ "mins" "({" ".join(map(str, LOW))})" "maxs" "({" ".join(map(str, HIGH))})" }} }} cordon {{ "name" "Disabled region" "active" "0" }} }}'
        imported, replaced = re.subn(r"(?ms)^cordon\s*\{.*?^\}", lambda _: metadata, native_source.read_text())
        assert replaced == 1
        plural = args.output / "plural-source.vmf"
        plural.write_text(imported)
        client.call("open_map", {"path": str(plural), "discard_changes": True})
        imported_state = client.call("get_compile_region")
        check(imported_state["committed"] == imported_state["build_scope"] and imported_state["committed"]["mins"] == LOW, "A plural import with disabled siblings exposes its effective active box")
        imported_history = client.call("get_map_info")["undo"]
        client.call("frame_compile_region", {"aspect": 1.5})
        check(client.call("get_map_info")["undo"] == imported_history and plural.read_text() == imported, "Framing imported box sets preserves history and every saved named entry")
        if args.render:
            client.render(args.output / "imported-region-preview.png", width=1000, height=700)
        plural_bsp = args.output / "plural-import.bsp"
        client.call("compile_map", {"path": str(plural_bsp)})
        synchronous = plural_bsp.read_bytes()
        client.call("start_compile", {"path": str(plural_bsp)})
        check(wait_job(client)["status"] == "completed" and plural_bsp.read_bytes() == synchronous == legacy.read_bytes(), "Imported plural regions match legacy BSP output through both build paths")
        replaced = client.call("preview_compile_region", {"snapshot": client.call("get_compile_region")["snapshot"], "enabled": False, "mins": LOW, "maxs": HIGH}, error=True)
        check("clear" in json.dumps(replaced).lower() and client.call("get_compile_region")["committed"]["enabled"], "Imported plural records require explicit Clear before replacement")
        clear = preview(client, clear=True)
        client.call("cancel_compile_region", {"token": clear["draft"]["token"]})
        check(plural.read_text() == imported and client.call("get_compile_region")["committed"]["enabled"], "Cancelling Clear preserves imported named boxes and source bytes")
        apply(client, clear=True)
        check(client.call("get_compile_region")["committed"] is None, "Applying Clear removes imported plural metadata")
        client.call("undo")
        check(client.call("get_compile_region")["committed"]["enabled"] and plural.read_text() == imported, "Undo restores imported region metadata without touching its saved file")
        (args.output / "validation.json").write_text(json.dumps({"passed": len(checks), "checks": checks, "region": scope, "desktop_input": False}, indent=2) + "\n")
        print(json.dumps({"passed": len(checks), "output": str(args.output)}))
        return native_path
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--render", action="store_true")
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--water", action="store_true", help="Include contained world water and check crossing-volume rejection")
    parser.add_argument("--global-markers", action="store_true", help="Keep outside sun and authored water controls without leaking or replacing settings")
    parser.add_argument("--terrain", action="store_true", help="Include whole contained terrain, suggested cage bounds and crossing-terrain publication guards")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    if args.worker:
        runtime.OBSERVER = OBSERVER.replace(", 64)", ", 192)") if args.water else OBSERVER
        args.observe_seconds = 60 if args.water else 30
        runtime.worker(args)
        return
    args.output.mkdir(parents=True)
    bsp = validate(args)
    if args.runtime:
        flags = (["--water"] if args.water else []) + (["--terrain"] if args.terrain else [])
        native = runtime.private_display(Path(__file__), args, bsp, extra_flags=flags)
        result = json.loads((native / "runtime.json").read_text())
        log = (native / "cstrike/hammerr-runtime.log").read_text(errors="replace")
        traces = [[float(value) for value in row] for row in re.findall(r"HAMMERR_REGION_TRACE ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+)", log)]
        assert '"sv_lan" = "1"' in log and len(traces) >= 3
        assert all(abs(value - 128 / 300) < 0.001 for row in traces for value in row), traces
        positions = result["prop_positions"]
        if args.water:
            assert (54.5 if args.terrain else 40) < positions[-1][2] < 96
            assert all(-128 < point[0] < 128 and 16 < point[1] < 128 for point in positions[-3:])
            if not args.terrain:
                assert any(b[2] - a[2] > 8 for a, b in zip(positions[1:], positions[2:]))
        elif args.terrain:
            assert 48 < positions[-1][2] < 64
            assert all(32 < point[0] < 112 and 32 < point[1] < 112 for point in positions[-3:])
        else:
            assert 0 < positions[-1][2] < 64
        assert max(abs(a - b) for a, b in zip(positions[-1], positions[-2], strict=True)) < (2 if args.water else 0.1)
        (args.output / "native-validation.json").write_text(json.dumps({"boundary_traces": traces[-3:], "grounded_player_and_stable_prop": True, "terrain_contact": args.terrain and not args.water, "buoyant_prop": args.water, "insecure": True, "sv_lan": True}, indent=2) + "\n")


if __name__ == "__main__":
    main()
