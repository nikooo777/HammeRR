import argparse
import json
import shutil
import struct
import sys
from pathlib import Path
from types import SimpleNamespace

import validate_water_bottom_native as native
from validate_authored_planes import records
from validate_collision_topology import document_state, identity, source_identities
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_face_lighting import ROOT, Client, detail, face_ref
from validate_material_albedo import require
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM
from validate_water_bottom import native_faces, raw_lump

POINTFILE = b"13 17 19\n23 29 31\n"


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def imported_sources():
    paths = {Path(module.__file__).resolve() for module in tuple(sys.modules.values())
             if getattr(module, "__file__", None)
             and Path(module.__file__).resolve().parent == ROOT / "scripts"
             and Path(module.__file__).suffix == ".py"}
    return {str(path): identity(path) for path in sorted(paths)}


def snapshot(client, source):
    return {"session": document_state(client), "camera": client.call("get_camera"),
            "selection": client.call("get_selection"), "source": source_identities(source)}


def projection(face):
    axes = [[float(value) for value in face[name].replace("[", "").replace("]", "").split()]
            for name in ("uaxis", "vaxis")]
    texture = [value for axis in axes for value in
               [*(component / axis[4] for component in axis[:3]), axis[3]]]
    lightmap = [value for axis in axes for value in
                [*(component / face["lightmap_scale"] for component in axis[:3]),
                 axis[3] * axis[4] / face["lightmap_scale"]]]
    return [struct.unpack("<f", struct.pack("<f", value))[0] for value in texture + lightmap]


def author(client, output):
    tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
    required = {"create_block", "create_entity", "edit_faces", "select_faces", "undo", "redo",
                "save_map", "open_map", "compile_map", "start_compile", "get_compile_status",
                "set_compile_settings", "get_history", "get_camera", "set_camera"}
    require(required <= tools, f"Missing required shared Session tools: {sorted(required - tools)}")
    client.call("new_map", {"discard_changes": True})
    for mins, maxs in ROOM:
        client.call("create_block", {"mins": mins, "maxs": maxs,
                                     "material": "hammerr/water_receiver"})
    for classname, origin, properties in (
        ("info_player_terrorist", [-224, -192, 8], {}),
        ("info_player_counterterrorist", [224, -192, 8], {}),
        ("light", [0, 0, 224], {"_light": "255 255 255 200", "_constant_attn": "1",
                                "_quadratic_attn": "0"}),
        ("logic_script", [-224, 0, 32], {"targetname": "hammerr_observer",
                                        "vscripts": "hammerr_observer.nut",
                                        "thinkfunction": "HammeRRThink"}),
    ):
        client.call("create_entity", {"classname": classname, "origin": origin,
                                       "properties": properties})
    pool = client.call("create_block", {"mins": [-160, -160, native.BOTTOM_Z],
                                        "maxs": [160, 160, native.SURFACE],
                                        "material": "tools/toolsnodraw"})["id"]
    brush = detail(client, [pool])[0]
    top = next(face for face in brush["faces"] if face["normal"] == [0, 0, 1])
    bottom = next(face for face in brush["faces"] if face["normal"] == [0, 0, -1])
    client.call("edit_faces", {"faces": [face_ref(brush, top)], "material": native.TOP})
    client.call("select_faces", {"faces": [face_ref(brush, bottom)]})
    before = detail(client, [pool])
    client.call("edit_faces", {"material": native.BOTTOM_SURFACE, "shift_u": 13.5,
                                "shift_v": -21, "scale_u": 0.5, "scale_v": 0.25,
                                "rotation": 0, "lightmap_scale": 16})
    edited = detail(client, [pool])
    client.call("undo")
    require(detail(client, [pool]) == before, "Bottom-face edit did not undo exactly")
    client.call("redo")
    require(detail(client, [pool]) == edited, "Bottom-face edit did not redo exactly")
    client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "engine",
                                         "visibility": "fast"})
    source = output / "source.vmf"
    client.call("save_map", {"path": str(source)})
    saved = identity(source)
    review = client.call("get_map_info")["replacement_review"]
    client.call("open_map", {"path": str(source), "expected_review": review})
    require(detail(client, [pool]) == edited and identity(source) == saved,
            "Saved/reopened authored bottom changed geometry, material or UVs")
    client.call("select_faces", {"faces": [face_ref(brush, bottom)]})
    client.call("set_camera", {"projection": "perspective", "position": [0, 0, 112],
                                "look_at": [21, 0, native.BOTTOM_Z]})
    for suffix, data in ((".bsp", b"private previous source BSP\n"), (".lin", POINTFILE)):
        source.with_suffix(suffix).write_bytes(data)
    return source, pool, next(face for face in edited[0]["faces"]
                              if face["normal"] == [0, 0, -1])


def compile_pair(client, output, label, source, pool, prior=None, failure=False):
    before = snapshot(client, source)
    paths = [output / f"{label}{suffix}.bsp" for suffix in ("", "-background")]
    for path in paths:
        path.with_suffix(".lin").write_bytes(POINTFILE)
        if prior is not None:
            path.write_bytes(prior)
    previous = {str(path): identity(path) for path in paths if failure}
    response = client.call("compile_map", {"path": str(paths[0])}, error=failure)
    result = json.loads(response["content"][0]["text"]) if failure else response
    foreground = client.call("get_compile_status")["job"]
    require(foreground["status"] == ("failed" if failure else "completed")
            and not foreground["stale"]
            and foreground["result"]["success"] is result["success"],
            f"{label}: foreground job does not match its synchronous result")
    require(snapshot(client, source) == before, f"{label}: foreground changed source/session custody")
    started = client.call("start_compile", {"path": str(paths[1])})["job"]
    job = wait_job(client)
    require(job["token"] == started["token"] and not job["stale"],
            f"{label}: background snapshot/token became stale")
    require(snapshot(client, source) == before, f"{label}: background changed source/session custody")
    if failure:
        for compiled in (foreground["result"], job["result"]):
            require(compiled["success"] is False and compiled["published"] is False
                    and any(row["level"] == "error" and "$bottommaterial" in row["message"]
                            and row["object"] == pool for row in compiled["diagnostics"]),
                    f"{label}: explicit bad bottom did not refuse at its authored solid: {compiled}")
        require(job["status"] == "failed" and
                {path: identity(Path(path)) for path in previous} == previous,
                f"{label}: refusal replaced the prior BSP")
    else:
        require(job["status"] == "completed", f"{label}: background compilation failed: {job}")
        for compiled in (foreground["result"], job["result"]):
            require(compiled["success"] is True and compiled["published"] is True
                    and compiled["leak"] is None and compiled["stats"]["water_regions"] == 1
                    and compiled["stats"]["physics_fluids"] == 1,
                    f"{label}: requested Engine-water build unsupported or failed: {compiled}")
    require(paths[0].read_bytes() == paths[1].read_bytes()
            and all(path.with_suffix(".lin").read_bytes() == POINTFILE for path in paths),
            f"{label}: foreground/background BSP or prior LIN custody differs")
    return {"bsp": str(paths[0]), "background_bsp": str(paths[1]), "stats": result.get("stats"),
            "foreground": result, "foreground_job": foreground, "background": job, "source_history_camera_preserved": True,
            "prior_lin_preserved": True}


def validate_program(args, binary, output):
    output.mkdir()
    options = SimpleNamespace(output=output, game_dir=args.game_dir, vertical_sides=False,
                              negative_z_bottom=True)
    game = native.assets(options)
    top_file = game / f"materials/{native.TOP}.vmt"
    top_file.write_text(native.top_material(native.NEUTRAL_BOTTOM))
    bottom_file = game / f"materials/{native.BOTTOM_SURFACE}.vmt"
    bottom_file.write_text(native.top_material(native.BOTTOMS["red"]))
    environment = {"HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
                   "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"),
                   "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
                   "XDG_CONFIG_HOME": str(output / "config"),
                   "XDG_STATE_HOME": str(output / "state")}
    client = Client(None, binary, output, game / "gameinfo.txt", environment=environment)
    report = {"binary": str(binary), "negative_z_bottom": True, "cases": {}}
    try:
        with stopped_on_exit(client):
            source, pool, bottom = author(client, output)
            protected = snapshot(client, source)
            for name, target in native.BOTTOMS.items():
                bottom_file.write_text(native.top_material(target))
                case = compile_pair(client, output, name, source, pool)
                data = Path(case["bsp"]).read_bytes()
                case["native"] = native.check_negative_z_native(data, target)
                require(case["native"]["source_projections"][native.BOTTOM_SURFACE][:16] == projection(bottom),
                        "Native authored/inward bottom UVs differ from saved source-side projection")
                fixture = output / "material-snapshots" / name
                shutil.copytree(game / "materials", fixture / "materials")
                case.update({"fixture_game": str(fixture),
                             "materials_sha256": native.material_hashes(fixture)})
                report["cases"][name] = case
            report["binding_only"] = native.check_side_binding_only(report["cases"])
            material_trees = [{path: value for path, value in case["materials_sha256"].items()
                               if path != f"materials/{native.BOTTOM_SURFACE}.vmt"}
                              for case in report["cases"].values()]
            require(material_trees[0] == material_trees[1],
                    "Compile A/B changed assets beyond the authored bottom target declaration")
            bottom_file.write_text("Water { $abovewater 1 }\n")
            one_sided = compile_pair(client, output, "missing-key", source, pool)
            data = Path(one_sided["bsp"]).read_bytes()
            require(all(target not in native_faces(data)[2] for target in native.BOTTOMS.values()),
                    "Missing bottom key emitted a fallback inward-bottom target")
            require(any("no $bottommaterial" in row["message"]
                        for row in one_sided["foreground"]["diagnostics"]),
                    "Missing bottom key did not report its one-sided boundary")
            baseline = Path(report["cases"]["red"]["bsp"]).read_bytes()
            require(all(raw_lump(data, index) == raw_lump(baseline, index)
                        for index in (1, 17, 18, 19, 29, 36, 46)),
                    "Adding an inward bottom changed planes, brush collision, depth, distance or complete PHY")
            leaf_geometry = [[row[:9] + row[11:] for row in
                              records(bsp, 10, "<ihh6h4Hhh", expected_version=1)]
                             for bsp in (data, baseline)]
            require(leaf_geometry[0] == leaf_geometry[1],
                    "Adding an inward bottom changed wet/dry leaf geometry or region ownership")
            report["missing_key"] = one_sided
            report["refusals"] = {}
            for label, target in (("missing", "hammerr/absent_bottom"),
                                   ("malformed", "hammerr/broken_bottom")):
                if label == "malformed":
                    (game / "materials/hammerr/broken_bottom.vmt").write_text("Water { $abovewater }\n")
                bottom_file.write_text(native.top_material(target))
                report["refusals"][label] = compile_pair(client, output, f"refused-{label}", source,
                                                         pool, prior=baseline, failure=True)
            bottom_file.write_text(native.top_material(native.BOTTOMS["red"]))
            report["recovered"] = compile_pair(client, output, "recovered", source, pool)
            require(Path(report["recovered"]["bsp"]).read_bytes() == baseline,
                    "Repaired bottom target did not restore exact original output")
            require(snapshot(client, source) == protected, "Build/refusal matrix changed authored custody")
            report.update({"source": str(source), "source_sha256": native.digest(source.read_bytes()),
                           "source_side": {"object": pool, "side": bottom["side"],
                                           "normal": bottom["normal"], "projection": projection(bottom)},
                           "compile_calls": 12, "native_games": 0,
                           "runtime_checked": False, "source_history_camera_preserved": True})
            mounted = {str(path): identity(path) for path in game.rglob("*") if path.is_file()}
            require(client.call("get_map_info")["rendering"]["initialized"] is False,
                    "Headless author/build matrix unexpectedly initialized graphics")
    finally:
        report["eof_exit_code"] = require_clean_eof(client)
        write_json(output / "partial.json", report)
    require(source_identities(source) == protected["source"]
            and {path: identity(Path(path)) for path in mounted} == mounted,
            "Clean EOF changed original source/BSP/LIN or mounted asset/configuration identities")
    report["custody"] = {str(path): identity(path) for path in
                         [binary, *[path for path in output.rglob("*") if path.is_file()
                                    and (path.is_relative_to(game)
                                         or path.suffix in {".vmf", ".bsp", ".lin", ".vmt", ".vtf"})]]}
    report["custody"].update(imported_sources())
    write_json(output / "compile.json", report)
    return report


def main():
    parser = argparse.ArgumentParser(
        description="Exercise authored negative-Z Water bottoms through both actual headless MCP programs; no native game is launched",
        epilog="Requires installed CS:S Engine physics assets. Run with both --binary target/debug/hammerr-headless and --binary target/debug/editor. Native pixels require the separate validate_water_bottom_native.py --negative-z-bottom --runtime --reuse-fixture PROGRAM/compile.json command. Use a short target/validation output path for private Wayland sockets. Private dynamic imports require PYTHONPATH=ROOT/scripts.",
    )
    parser.add_argument("--binary", action="append", type=Path, required=True)
    parser.add_argument("--game-dir", type=Path, default=Path.home() /
                        ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output = args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target"), "Retain validation evidence below target/")
    args.output.mkdir(parents=True)
    binaries = [path.resolve(strict=True) for path in args.binary]
    require(len(binaries) == 2 and {path.name for path in binaries} == {"hammerr-headless", "editor"},
            "Provide each actual headless/editor executable exactly once")
    before = {str(path): identity(path) for path in binaries}
    before.update(imported_sources())
    report = {"status": "running", "programs": {}, "native_games": 0, "runtime_checked": False,
              "desktop_input": False, "custody_before": before}
    try:
        for binary in binaries:
            report["programs"][binary.name] = validate_program(args, binary, args.output / binary.name)
        for name in native.BOTTOMS:
            data = [Path(program["cases"][name]["bsp"]).read_bytes()
                    for program in report["programs"].values()]
            require(data[0] == data[1], f"{name}: actual MCP executables emitted different BSP bytes")
        report["custody_after"] = {path: identity(Path(path)) for path in before}
        require(report["custody_after"] == before, "Actual binaries or validator source identities changed")
        report.update({"status": "passed", "compile_calls": 24,
                       "actual_program_bsp_bytes_identical": True})
    except BaseException as error:
        report.update({"status": "failed", "error": {"type": type(error).__name__, "message": str(error)}})
        raise
    finally:
        write_json(args.output / "results.json", report)
    print(json.dumps({"output": str(args.output), "compile_calls": 24, "native_games": 0}))


if __name__ == "__main__":
    main()
