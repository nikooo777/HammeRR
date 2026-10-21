import argparse
import hashlib
import json
import os
import struct
from contextlib import ExitStack, contextmanager
from pathlib import Path

from validate_build_recipes import protected_state
from validate_engine_physics import physics_records
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail
from validate_native_runtime import private_game
from validate_recovery_exit import require_clean_eof

SETTINGS = {"lighting": "fullbright", "physics": "engine", "bounces": 0,
            "generate_reflections": False, "bake_static_prop_vertices": False,
            "pack_sky_assets": False, "pack_water_assets": False,
            "pack_static_prop_assets": False}

FIXTURE = '''
world
{
    "id" "1"
    "classname" "worldspawn"
    solid
    {
        "id" "10"
        side { "id" "100" "plane" "(-544 544 0) (544 544 0) (544 -544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 -1 0 0] 0.25" }
        side { "id" "101" "plane" "(544 544 -32) (-544 544 -32) (-544 -544 -32)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 -1 0 0] 0.25" }
        side { "id" "102" "plane" "(-544 -544 0) (-544 -544 -32) (-544 544 -32)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "103" "plane" "(544 544 0) (544 544 -32) (544 -544 -32)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "104" "plane" "(-544 544 0) (-544 544 -32) (544 544 -32)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "105" "plane" "(544 -544 0) (544 -544 -32) (-544 -544 -32)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
    }
    solid
    {
        "id" "11"
        side { "id" "106" "plane" "(-544 544 544) (544 544 544) (544 -544 544)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 -1 0 0] 0.25" }
        side { "id" "107" "plane" "(544 544 512) (-544 544 512) (-544 -544 512)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 -1 0 0] 0.25" }
        side { "id" "108" "plane" "(-544 -544 544) (-544 -544 512) (-544 544 512)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "109" "plane" "(544 544 544) (544 544 512) (544 -544 512)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "110" "plane" "(-544 544 544) (-544 544 512) (544 544 512)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "111" "plane" "(544 -544 544) (544 -544 512) (-544 -544 512)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
    }
    solid
    {
        "id" "12"
        side { "id" "112" "plane" "(-544 544 512) (-512 544 512) (-512 -544 512)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 -1 0 0] 0.25" }
        side { "id" "113" "plane" "(-512 544 0) (-544 544 0) (-544 -544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 -1 0 0] 0.25" }
        side { "id" "114" "plane" "(-544 -544 512) (-544 -544 0) (-544 544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "115" "plane" "(-512 544 512) (-512 544 0) (-512 -544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "116" "plane" "(-544 544 512) (-544 544 0) (-512 544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "117" "plane" "(-512 -544 512) (-512 -544 0) (-544 -544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
    }
    solid
    {
        "id" "13"
        side { "id" "118" "plane" "(512 544 512) (544 544 512) (544 -544 512)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 -1 0 0] 0.25" }
        side { "id" "119" "plane" "(544 544 0) (512 544 0) (512 -544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 -1 0 0] 0.25" }
        side { "id" "120" "plane" "(512 -544 512) (512 -544 0) (512 544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "121" "plane" "(544 544 512) (544 544 0) (544 -544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "122" "plane" "(512 544 512) (512 544 0) (544 544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "123" "plane" "(544 -544 512) (544 -544 0) (512 -544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
    }
    solid
    {
        "id" "14"
        side { "id" "124" "plane" "(-512 -512 512) (512 -512 512) (512 -544 512)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 -1 0 0] 0.25" }
        side { "id" "125" "plane" "(512 -512 0) (-512 -512 0) (-512 -544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 -1 0 0] 0.25" }
        side { "id" "126" "plane" "(-512 -544 512) (-512 -544 0) (-512 -512 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "127" "plane" "(512 -512 512) (512 -512 0) (512 -544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "128" "plane" "(-512 -512 512) (-512 -512 0) (512 -512 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "129" "plane" "(512 -544 512) (512 -544 0) (-512 -544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
    }
    solid
    {
        "id" "15"
        side { "id" "130" "plane" "(-512 544 512) (512 544 512) (512 512 512)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 -1 0 0] 0.25" }
        side { "id" "131" "plane" "(512 544 0) (-512 544 0) (-512 512 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 -1 0 0] 0.25" }
        side { "id" "132" "plane" "(-512 512 512) (-512 512 0) (-512 544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "133" "plane" "(512 544 512) (512 544 0) (512 512 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[0 1 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "134" "plane" "(-512 544 512) (-512 544 0) (512 544 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
        side { "id" "135" "plane" "(512 512 512) (512 512 0) (-512 512 0)" "material" "DEV/DEV_MEASUREGENERIC01" "uaxis" "[1 0 0 0] 0.25" "vaxis" "[0 0 -1 0] 0.25" }
    }
}
entity { "id" "2" "classname" "info_player_terrorist" "origin" "-64 0 8" }
entity { "id" "3" "classname" "info_player_counterterrorist" "origin" "64 0 8" }
entity
{
    "id" "285"
    "classname" "func_detail"
    "targetname" "detail_misc"
    solid
    {
        "id" "202"
        side
        {
            "id" "1424"
            "plane" "(50.49610398338425 -124.45402565027105 195.4364950386774) (44.89961088252561 -167.5676290536579 148.46952295671846) (-17.600813310652967 -173.16412215451655 161.05428519701445)"
            "material" "DEV/DEV_MEASUREGENERIC01"
            "uaxis" "[1 0 0 0] 3"
            "vaxis" "[0 0 -1 0] 3"
        }
        side
        {
            "id" "1425"
            "plane" "(178.19333615040586 -242.193336150406 194.8388809802324) (189.38632235212313 -189.3863223521233 229.2210908218954) (136.5793085538404 -200.57930855384058 263.60330066355834)"
            "material" "DEV/DEV_MEASUREGENERIC01"
            "uaxis" "[1 0 0 0] 3"
            "vaxis" "[0 -1 0 0] 3"
        }
        side
        {
            "id" "1426"
            "plane" "(151.66581989144115 117.27105244120868 338.80458923801154) (153.55204595610203 53.93356986061436 347.79377449508047) (94.92224251666595 55.81979592527524 373.386513697559)"
            "material" "CS_ITALY/YELLOW"
            "uaxis" "[1 0 0 0] 0.25"
            "vaxis" "[0 -1 0 0] 0.25"
        }
        side
        {
            "id" "1427"
            "plane" "(329.5547724251598 -121.87635466144334 11.070249698994754) (340.4581561282255 -59.79120510516356 0) (338.5433056845053 -70.69458880822923 -63.03530416839365)"
            "material" "CS_ITALY/YELLOW"
            "uaxis" "[0 1 0 0] 0.25"
            "vaxis" "[0 0 -1 0] 0.25"
        }
    }
}
'''


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def identity(path):
    if not path.exists():
        return None
    stat = path.stat()
    return {"device": stat.st_dev, "inode": stat.st_ino, "bytes": stat.st_size,
            "mtime_ns": stat.st_mtime_ns, "sha256": digest(path)}


def source_identities(source):
    return {str(path): identity(path)
            for path in (source, source.with_suffix(".bsp"), source.with_suffix(".lin"))}


def document_state(client):
    objects, offset = [], 0
    while True:
        page = client.call("find_objects", {"offset": offset, "limit": 100})
        rows = page["objects"]
        require(rows or offset == page["total"], "Object pagination stopped before total")
        objects.extend(detail(client, [row["id"] for row in rows]))
        offset += len(rows)
        if offset == page["total"]:
            break
        require(offset < page["total"], "Object pagination exceeded total")
    return {"document": protected_state(client), "history": client.call("get_history"),
            "objects": objects, "settings": client.call("get_compile_settings")}


def check_schema(client):
    tools = {tool["name"]: tool["inputSchema"]
             for tool in client.rpc("tools/list", {})["tools"]}
    expected = {
        "open_map": ("path", "expected_review"),
        "compile_map": ("path", "expected_generation"),
        "set_compile_settings": tuple(SETTINGS),
        "find_objects": ("offset", "limit"),
        "get_objects": ("ids",),
        "get_brush_components": ("solids",),
    }
    for name, fields in expected.items():
        require(name in tools and all(field in tools[name]["properties"] for field in fields),
                f"Missing concrete MCP schema: {name} {fields}")
    require(all(name in tools for name in ("get_map_info", "get_history", "get_compile_settings")),
            "Missing shared Session inspection tools")
    return {name: tools[name] for name in expected}


def check_tiny_source(client):
    brushes = client.call("get_brush_components", {"solids": [202]})["brushes"]
    require(len(brushes) == 1, "Exact solid202 fixture missing")
    brush = brushes[0]
    require(len(brush["vertices"]) == 4 and len(brush["edges"]) == 6
            and len(brush["faces"]) == 4, "Exact solid202 tetrahedral topology differs")
    require({face["side"] for face in brush["faces"]} == {1424, 1425, 1426, 1427},
            "Exact solid202 source-side identities differ")
    vertices = [vertex["position"] for vertex in brush["vertices"]]
    bounds = [tuple(min(point[axis] for point in vertices) for axis in range(3)),
              tuple(max(point[axis] for point in vertices) for axis in range(3))]
    expected = [(354.05544963974535, 18.171157652242833, 264.57813850585126),
                (354.1877291737219, 18.386599059779208, 264.791323203739)]
    require(all(abs(actual - wanted) <= 1e-6 for row, reference in zip(bounds, expected, strict=True)
                for actual, wanted in zip(row, reference, strict=True)),
            f"Exact solid202 world bounds changed: {bounds}")
    return brush


def compile_case(client, source, destination, exact_tiny, report):
    protected = source_identities(source)
    report["source"] = str(source)
    report["source_before"] = protected
    require(not destination.exists() and not destination.with_suffix(".lin").exists(),
            "Compile destination must be fresh")
    review = client.call("get_map_info")["replacement_review"]
    client.call("open_map", {"path": str(source), "expected_review": review})
    if exact_tiny:
        report["exact_tiny_source"] = check_tiny_source(client)
    before = document_state(client)
    report["document_before_sha256"] = hashlib.sha256(
        json.dumps(before, sort_keys=True).encode()).hexdigest()
    try:
        compiled = client.call("compile_map", {
            "path": str(destination),
            "expected_generation": before["document"]["generation"],
        })
        report["compile"] = compiled
        require(compiled["success"], f"Shared native compile failed: {compiled}")
        require(compiled["bsp"] == str(destination) and compiled.get("game") is None,
                "Compile must publish only the requested private BSP without a game")
        diagnostics = compiled["diagnostics"]
        require(not any(row["level"] == "error" for row in diagnostics),
                f"Unexpected compile errors: {diagnostics}")
        require(not any(row.get("object") == "solid:202" for row in diagnostics),
                f"Unexpected solid202 diagnostic: {diagnostics}")
        data = destination.read_bytes()
        require(len(data) >= 1036 and data[:4] == b"VBSP", "Missing valid BSP header")
        models = physics_records(data)
        stats = compiled["stats"]
        physics_offset, physics_bytes = struct.unpack_from("<2i", data, 8 + 29 * 16)
        require(0 <= physics_offset < physics_offset + physics_bytes <= len(data)
                and physics_bytes == stats["physics_bytes"] > 16,
                "Native PHYSICS lump size disagrees with the published byte count")
        require(stats["physics_mode"] == "engine" and stats["lighting_mode"] == "fullbright",
                "Compile did not use the shared Fullbright/engine settings")
        require(stats["physics_models"] == len(models) > 0,
                "Native PHYSICS model headers disagree with compiler counts")
        require(stats["physics_solids"] == sum(len(solids) for _, solids, _ in models) > 0,
                "Native VPHY body headers disagree with compiler counts")
        require(stats["physics_convexes"] > 0,
                "Native compile omitted expected convex inputs")
        if exact_tiny:
            require((stats["physics_models"], stats["physics_solids"], stats["physics_convexes"])
                    == (1, 1, 7), "Sealed room plus exact202 must retain all seven convexes")
        report["physics"] = {"models": len(models), "solids": stats["physics_solids"],
                             "convexes": stats["physics_convexes"],
                             "bytes": physics_bytes,
                             "model_indices": [index for index, _, _ in models],
                             "solid_bytes": [[len(solid) for solid in solids]
                                             for _, solids, _ in models]}
        report["bsp"] = {"path": str(destination), "bytes": len(data), "sha256": digest(destination)}
    finally:
        after = document_state(client)
        report["document_after_sha256"] = hashlib.sha256(
            json.dumps(after, sort_keys=True).encode()).hexdigest()
        report["source_after"] = source_identities(source)
        report["document_preserved"] = after == before
        report["source_and_original_outputs_preserved"] = report["source_after"] == protected
        require(report["document_preserved"], "Compile changed the Session document, history or settings")
        require(report["source_and_original_outputs_preserved"],
                "Compile changed original VMF/BSP/LIN identity or created an original output")
        info = client.call("get_map_info")
        require(not info["rendering"]["initialized"] and not info["compiling"]
                and not info["launching"] and not info["game_running"],
                "Compile left graphics, build or game activity")
    report["status"] = "passed"


@contextmanager
def record_failure(errors):
    try:
        yield
    except BaseException as error:
        if not any(previous is error for previous in errors):
            errors.append(error)
        raise


def close_client(client, report):
    report["close_exit_code"] = require_clean_eof(client)


def check_custody(report, identities):
    changes = []
    report["sources_after_close"] = {}
    for name, case in report["cases"].items():
        if "source" in case:
            actual = source_identities(Path(case["source"]))
            report["sources_after_close"][name] = actual
            if actual != case["source_before"]:
                changes.append(AssertionError(f"Original source/output identity changed after EOF: {name}"))
    report["runtime_after"] = {path: identity(Path(path)) for path in identities}
    if report["runtime_after"] != identities:
        changes.append(AssertionError("MCP binary, physics worker or native library identity changed"))
    if changes:
        raise ExceptionGroup("Collision topology custody failed", changes)


def failure_details(error, seen=None):
    details = {"type": type(error).__name__, "message": str(error)}
    if seen is None:
        seen = set()
    if id(error) in seen:
        details["already_reported"] = True
        return details
    seen.add(id(error))
    if isinstance(error, BaseExceptionGroup):
        details["exceptions"] = [failure_details(child, seen) for child in error.exceptions]
    if error.__cause__ is not None:
        details["cause"] = failure_details(error.__cause__, seen)
    if error.__context__ is not None and error.__context__ is not error.__cause__:
        details["context"] = failure_details(error.__context__, seen)
    return details


def finish_report(output, report, errors):
    report["status"] = "failed" if errors else "passed"
    report["errors"] = [f"{type(error).__name__}: {error}" for error in errors]
    report["failure_details"] = [failure_details(error) for error in errors]
    (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "output": str(output),
                      "cases": list(report["cases"])}))
    if errors:
        raise BaseExceptionGroup("Collision topology validation failed", errors)


def main():
    parser = argparse.ArgumentParser(
        description="Compile a fixed sealed room plus exact tiny solid202 through production MCP using Fullbright/engine physics; optionally compile an original map too. No game or desktop is launched. Checks native PHYSICS/VPHY framing, compound counts, source identities and Session document/history preservation; does not establish game contact dynamics.",
        epilog="Requires Python 3.11+, a matching editor or hammerr-headless and hammerr-physics built from the same source, and the supported installed Linux64 CS:S vphysics.so/libtier0.so with mounted game assets. The physics worker must be beside the binary or selected by HAMMERR_PHYSICS_WORKER. Run under the caller's shared 24 GiB resource cap with disk-backed TMPDIR; this script performs no builds. Output must not exist and retained generated files stay there; originals are opened without saving.",
    )
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY,
                        help="Production editor or hammerr-headless, launched privately with --headless stdio MCP")
    parser.add_argument("--game-dir", type=Path,
                        default=Path(os.environ.get("CSS_DIR", str(Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source"))),
                        help="Installed CS:S root containing bin/linux64/vphysics.so and cstrike/gameinfo.txt")
    parser.add_argument("--source", type=Path,
                        help="Additionally compile this original VMF without saving; output goes only to the fresh private directory")
    parser.add_argument("--output", type=Path, default=ROOT / "target/validation/collision-topology",
                        help="Fresh private artifact directory on disk; refuses an existing path")
    parser.add_argument("--timeout", type=float, default=180,
                        help="Per-MCP-request timeout in seconds (default: 180)")
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    binary = args.binary.resolve(strict=True)
    game_root = args.game_dir.resolve(strict=True)
    source = args.source.resolve(strict=True) if args.source else None
    if source is not None and (not source.is_file() or source.suffix.lower() != ".vmf"):
        parser.error("--source must be an existing .vmf file")
    worker = Path(os.environ.get("HAMMERR_PHYSICS_WORKER", str(binary.parent / "hammerr-physics"))).resolve(strict=True)
    require(binary.is_file() and os.access(binary, os.X_OK), "MCP binary is not executable")
    require(worker.is_file() and os.access(worker, os.X_OK), "Matching hammerr-physics is not executable")
    libraries = [game_root / "bin/linux64" / name for name in ("vphysics.so", "libtier0.so")]
    require(all(path.is_file() for path in libraries), "Supported CS:S Linux64 physics libraries are missing")
    require(not args.output.exists() and not args.output.is_symlink(),
            "--output must be a fresh directory path")
    output = args.output.resolve()
    output.mkdir(parents=True, mode=0o700, exist_ok=False)
    identities = {}
    report = {"status": "running", "transport": "private_stdio", "binary": str(binary),
              "runtime_before": identities, "cases": {}, "game_contact_validated": False}
    errors = []
    with ExitStack() as lifecycle:
        lifecycle.callback(finish_report, output, report, errors)
        lifecycle.enter_context(record_failure(errors))
        lifecycle.callback(check_custody, report, identities)
        lifecycle.enter_context(record_failure(errors))
        for path in (binary, worker, *libraries):
            identities[str(path)] = identity(path)
        game = private_game(game_root, output / "source-game", engine_physics=True)
        environment = {"HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
                       "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"),
                       "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
                       "XDG_CONFIG_HOME": str(output / "config"),
                       "XDG_CACHE_HOME": str(output / "cache"),
                       "XDG_STATE_HOME": str(output / "state")}
        client = Client(None, binary, output, game / "gameinfo.txt",
                        environment=environment, request_timeout=args.timeout)
        lifecycle.callback(close_client, client, report)
        lifecycle.enter_context(record_failure(errors))
        report["schemas"] = check_schema(client)
        client.call("set_compile_settings", SETTINGS)
        fixture = output / "exact202.vmf"
        fixture.write_text(FIXTURE)
        report["cases"]["exact202"] = {}
        compile_case(client, fixture, output / "exact202-compiled.bsp", True, report["cases"]["exact202"])
        if source is not None:
            report["cases"]["source"] = {}
            compile_case(client, source, output / "original-source.bsp", False, report["cases"]["source"])


if __name__ == "__main__":
    main()
