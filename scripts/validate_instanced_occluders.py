import argparse
import hashlib
import json
import math
import os
import re
import struct
import sys
from pathlib import Path

import validate_instanced_visclusters as shared
from validate_authored_planes import records
from validate_collision_topology import document_state, identity
from validate_cubemap_capture_probe import game_lump_records
from validate_direct_lighting import FACE_FORMAT
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail
from validate_instance_contents import refresh
from validate_instance_props import cleanup_clients
from validate_material_albedo import payload, require
from validate_occluders import native_occlusion
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM

DEFAULT_GAME = Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source"
REASON = ("Child occluder helpers have no reference geometry or icons; "
          "helper bounds, picking and framing are unavailable.")
LEAF_BOUNDS = ([-36, 0, -2], [36, 192, 2])
LITERAL_BOUNDS = (([-2, -96, 0], [2, -24, 192]), ([-2, 24, 0], [2, 96, 192]))
GEOMETRY = ("vertices", "indices", "triangles", "line_vertices")
PHYSICAL = (2, 4, 5, 6, 7, 8, 10, 12, 13, 14, 15, 16, 17, 18, 19,
            20, 21, 22, 23, 26, 27, 31, 48, 51, 52, 53, 55, 56, 58)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def installation(root):
    selected = {root / "cstrike_linux64", root / "cstrike/gameinfo.txt"}
    for name in ("engine", "materialsystem", "shaderapidx9", "shaderapivk", "stdshader_dx9", "filesystem_stdio"):
        selected.add(root / f"bin/linux64/{name}.so")
    selected.update(root / f"cstrike/bin/linux64/{name}.so" for name in ("server", "client"))
    result = {}
    for folder in (root / "cstrike", root / "hl2", root / "platform"):
        archives = list(folder.glob("*.vpk"))
        require(len(archives) <= 256, "Installed VPK scope exceeds its bound")
        selected.update(archives)
        selected.add(folder)
        selected.update(folder / name for name in ("materials", "models", "download", "scripts"))
        for relative in ("models/props_junk", "materials/models/props_junk", "materials/dev", "materials/tools"):
            selected.add(folder / relative)
        for suffix in (".mdl", ".vvd", ".dx90.vtx", ".phy"):
            selected.add(folder / f"models/props_junk/wood_crate001a{suffix}")
    for path in sorted(selected):
        if not path.exists():
            result[str(path)] = None
            continue
        info = path.lstat()
        row = {"target": str(path.resolve(strict=True)), "mode": info.st_mode, "device": info.st_dev,
               "inode": info.st_ino, "bytes": info.st_size, "modified_ns": info.st_mtime_ns,
               "changed_ns": info.st_ctime_ns, "link": os.readlink(path) if path.is_symlink() else None}
        if path.is_file() and (path.suffix != ".vpk" or path.name.endswith("_dir.vpk")):
            row["sha256"] = digest(path)
        result[str(path)] = row
    require(all((root / name).is_file() for name in
                ("cstrike_linux64", "bin/linux64/engine.so", "cstrike/bin/linux64/server.so")),
            "Installed playable CS:S modules are required")
    return result


def mounted_game(path, root):
    path.mkdir()
    mounts = [path, root / "cstrike/cstrike_pak.vpk", root / "cstrike/cstrike_english.vpk",
              root / "cstrike", root / "hl2/hl2_textures.vpk", root / "hl2/hl2_misc.vpk",
              root / "hl2", root / "platform/platform_misc.vpk", root / "platform"]
    search = "\n".join(f'game "{mount}"' for mount in mounts)
    (path / "gameinfo.txt").write_text(f'GameInfo {{ FileSystem {{ SearchPaths {{ {search} }} }} }}\n')


def block(client, bounds, material="dev/dev_measuregeneric01"):
    return client.call("create_block", {"mins": bounds[0], "maxs": bounds[1], "material": material})["id"]


def helper(client, bounds, active=True):
    brush = block(client, bounds, "tools/toolsoccluder")
    owner = client.call("tie_to_entity", {"ids": [brush], "classname": "func_occluder"})["id"]
    definition = client.call("get_entity_class", {"classname": "func_occluder"})
    authored = dict(detail(client, [owner])[0]["properties"])
    require(authored == {"classname": "func_occluder", **definition["defaults"]}
            and authored.get("StartActive") == "1" and not authored.get("targetname", ""),
            "Normal installed-FGD occluder authoring changed defaults or synthesized a name")
    if not active:
        client.call("set_entity_properties", {"id": owner, "set": {"StartActive": "0"}})
    return {"owner": owner, "defaults": authored}


def leaf(client, path, custody, active=True, mixed=False):
    shared.new_map(client)
    authored = helper(client, LEAF_BOUNDS, active)
    if mixed:
        block(client, ([-8, -8, -8], [-4, -4, -4]))
        physical = block(client, ([4, 4, -8], [8, 8, -4]))
        client.call("tie_to_entity", {"ids": [physical], "classname": "func_detail"})
    shared.save(client, path, custody)
    return authored


def middle(client, path, child, custody):
    shared.new_map(client)
    shared.instance(client, child, [8, 16, 64], "0 90 0")
    shared.save(client, path, custody)


def scene(client, path, custody, active=True, child=None):
    shared.new_map(client)
    for bounds in ROOM:
        block(client, bounds)
    for classname, origin, properties in (
        ("info_player_terrorist", [-160, 0, 8], {}),
        ("info_player_counterterrorist", [192, -160, 8], {}),
        ("light", [0, 0, 160], {"_light": "255 255 255 200", "_constant_attn": "1", "_quadratic_attn": "0"}),
        ("prop_physics", [-208, -208, 64], {"model": "models/props_junk/wood_crate001a.mdl", "targetname": "physics_crate"}),
        ("logic_script", [-128, -128, 64], {"targetname": "hammerr_observer", "vscripts": "hammerr_observer.nut", "thinkfunction": "HammeRRThink"}),
    ):
        client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})
    for y in (0, -64, 64):
        client.call("create_entity", {"classname": "prop_static", "origin": [96, y, 8],
                                     "properties": {"model": "models/props_junk/wood_crate001a.mdl", "solid": "0"}})
    owners, authored = [], []
    if child is None:
        authored = [helper(client, bounds, active) for bounds in LITERAL_BOUNDS]
    else:
        for center in (-60, 60):
            owners.append(shared.instance(client, child, [-64, center - 16, 8], "90 0 0"))
    shared.save(client, path, custody)
    client.call("select", {"ids": owners})
    return {"source": path, "owners": owners, "authored": authored}


def preview(client, control, count, mixed=False):
    before, camera = document_state(client), client.call("get_camera")
    ready = refresh(client, background=True)
    require(ready["state"] == "ready" and ready["stats"].get("occluders", 0) == count
            and ("occluders" in ready["stats"]) == bool(count)
            and ("occluder_preview" in ready) == bool(count), "Expanded occluder usage/omission is wrong")
    if count:
        require(ready["occluder_preview"] == {"available": False, "reason": REASON},
                "Child occluder inspection fabricated helper geometry/picking/framing")
    require({row["owner"] for row in ready["owners"]} == set(control["owners"]), "Reference owners changed")
    if mixed:
        require(ready["available"] and ready["stats"]["world_brushes"] == ready["stats"]["detail_brushes"] == 2
                and ready["geometry"]["triangles"] > 0, "Mixed preview lost physical geometry or counted helper solids")
    else:
        require(not ready["available"] and not ready["displayed"]
                and ready["stats"]["world_brushes"] == ready["stats"]["detail_brushes"] == 0
                and all(ready["geometry"][key] == 0 for key in GEOMETRY)
                and ready["face_provenance_total"] == 0 and all(row["bounds"] is None for row in ready["owners"]),
                "Helper-only references acquired meshes, bounds or provenance")
        client.call("set_instance_contents", {"action": "frame", "token": str(ready["token"]), "ids": control["owners"]}, error=True)
    require(document_state(client) == before and client.call("get_camera") == camera
            and not client.call("get_map_info")["rendering"]["initialized"], "CPU inspection/frame changed source/history/camera or initialized graphics")
    return ready


def entities(data):
    rows = []
    for block_text in re.findall(r"\{([^{}]*)\}", payload(data, 0).decode()):
        tokens = [json.loads(token) for token in re.findall(r'"(?:\\.|[^"\\])*"', block_text)]
        require(len(tokens) % 2 == 0, "Malformed emitted entity key/value pairs")
        rows.append(dict(zip(tokens[::2], tokens[1::2], strict=True)))
    return rows


def decoded(data):
    native, polygons, indices = native_occlusion(data)
    vertices, planes = records(data, 3, "<3f"), records(data, 1, "<4fi")
    polygon_owners, index_owners = [], []
    result = []
    for flags, first, count, *bounds_area in native:
        shape = []
        polygon_owners.extend(range(first, first + count))
        for start, size, plane_index in polygons[first:first + count]:
            index_owners.extend(range(start, start + size))
            points = [tuple(vertices[indices[index][0]]) for index in range(start, start + size)]
            cycles = [tuple(points[index:] + points[:index]) for index in range(size)]
            shape.append((tuple(planes[plane_index][:4]), min(cycles)))
        result.append({"flags": flags, "bounds_area": bounds_area, "first_polygon": first, "polygons": shape})
    require(sorted(polygon_owners) == list(range(len(polygons)))
            and sorted(index_owners) == list(range(len(indices))), "Native polygon/reference records are omitted, duplicated or cross-owned")
    emitted = entities(data)
    require(not any(row.get("classname") == "func_instance" for row in emitted), "Compiler-only instances escaped into runtime entities")
    runtime = [row for row in emitted if row.get("classname") == "func_occluder"]
    require(len(native) == len(runtime) == 2
            and [row["occludernumber"] for row in runtime] == ["0", "1"]
            and all(not row.get("targetname", "") and "model" not in row for row in runtime),
            "Native helpers lack distinct dense indices or acquired names/models")
    for index, record in enumerate(result):
        require(record["bounds_area"][:6] == [*LITERAL_BOUNDS[index][0], *LITERAL_BOUNDS[index][1]]
                and record["bounds_area"][6] > 0 and len(record["polygons"]) == 6,
                "Native helper index does not bind its independent literal strip and positive area")
    return result, runtime


def allocation_bijection(actual, expected, label):
    require(len(actual) == len(expected), f"Native {label} record cardinalities differ")
    available = {}
    for index in reversed(range(len(expected))):
        available.setdefault(expected[index], []).append(index)
    mapping = []
    for value in actual:
        matches = available.get(value)
        require(matches, f"Native {label} has no exact coordinate/orientation match")
        mapping.append(matches.pop())
    require(all(not values for values in available.values()), f"Native {label} bijection omitted records")
    return mapping


def compare(actual, literal):
    a, a_entities = decoded(actual)
    b, b_entities = decoded(literal)
    polygon_mapping = []
    for index, (left, right) in enumerate(zip(a, b, strict=True)):
        require(left["flags"] == right["flags"] and left["bounds_area"] == right["bounds_area"],
                "Native helper index/state/bounds/area differs from its literal owner")
        mapping = allocation_bijection(left["polygons"], right["polygons"], f"helper{index} oriented polygons")
        polygon_mapping.extend({"child": left["first_polygon"] + source, "literal": right["first_polygon"] + target}
                               for source, target in enumerate(mapping))
    mapping = []
    for left, right in zip(a_entities, b_entities, strict=True):
        mapping.append({"child": left.pop("hammerid"), "literal": right.pop("hammerid")})
        require(left == right, "Runtime occluder fields differ beyond explicit generated source IDs")
    require(all(payload(actual, index) == payload(literal, index) for index in PHYSICAL),
            "Physical BSP/PVS/PAS/LDR output differs from literal controls")
    require(game_lump_records(actual)[:2] == game_lump_records(literal)[:2],
            "Static prop models/poses/solidity/leaf membership differ from literal controls")
    allocations = {}
    tables = {}
    for index, layout in ((1, "<4fi"), (3, "<3f")):
        actual_rows, literal_rows = records(actual, index, layout), records(literal, index, layout)
        require(all(math.isfinite(value) for row in [*actual_rows, *literal_rows] for value in row),
                "Native plane/vertex tables contain a nonfinite coordinate")
        tables[index] = actual_rows, literal_rows
        allocations[str(index)] = allocation_bijection(actual_rows, literal_rows, f"lump{index}")
    plane_ids = {row[0] for row in records(actual, 5, "<3i6h2H2h")}
    plane_ids.update(row[0] for row in records(actual, 19, "<HhhBB"))
    plane_ids.update(row[4] for row in records(actual, 21, "<4Hi"))
    for index in (7, 27, 58):
        plane_ids.update(row[0] for row in struct.iter_unpack(FACE_FORMAT, payload(actual, index)))
    vertex_ids = {value for row in records(actual, 12, "<2H") for value in row}
    for index, identifiers in ((1, plane_ids), (3, vertex_ids)):
        left, right = tables[index]
        require(all(0 <= identifier < len(left) and left[identifier] == right[identifier] for identifier in identifiers),
                "Native allocation changed a plane/vertex at a physical consumer index")
    actual_normals, literal_normals = records(actual, 30, "<3f"), records(literal, 30, "<3f")
    require(len(actual_normals) == len(tables[1][0]) and len(literal_normals) == len(tables[1][1])
            and all(math.isfinite(value) for row in [*actual_normals, *literal_normals] for value in row)
            and all(actual_normals[index] == tuple(tables[1][0][index][:3])
                    and actual_normals[index] == literal_normals[target] == tuple(tables[1][1][target][:3])
                    for index, target in enumerate(allocations["1"])),
            "Complete finite normal table does not follow its exact plane allocation bijection")
    normal_ids = {row[0] for row in records(actual, 31, "<H")}
    require(all(0 <= index < len(actual_normals) and actual_normals[index] == literal_normals[index] for index in normal_ids),
            "Plane allocation changed a normal at a physical normal-index consumer")
    return {"occluders": a, "runtime_source_ids": mapping, "physical_lumps": list(PHYSICAL),
            "native_allocation_bijections": allocations, "owned_polygon_bijection": polygon_mapping,
            "exact_physical_table_slots": {"planes": sorted(plane_ids), "vertices": sorted(vertex_ids), "normals": sorted(normal_ids)},
            "normal_table30": "Complete exact finite coverage through the same plane bijection; every physical normal31 consumer stays exact at its original index",
            "normalization": "Explicit helper source IDs and complete native allocation bijections with cyclic oriented winding starts only; no numeric rounding/tolerance/reversal/simplification"}


def configure(client):
    client.call("set_compile_settings", {"lighting": "direct_ldr", "visibility": "fast", "physics": "none",
                "bounces": 0, "generate_reflections": False, "bake_static_prop_vertices": False,
                "pack_static_prop_assets": False, "pack_sky_assets": False, "pack_water_assets": False})
    if not any(row["name"] == "Unnamed child occluders" for row in client.call("get_build_recipes")["recipes"]):
        client.call("save_build_recipe", {"name": "Unnamed child occluders"})


def compile_pair(client, args, game, label, control, sources):
    shared.reviewed_open(client, control["source"])
    configure(client)
    case = {}
    data = shared.pair(client, args, game, label, control, sources, case)
    require(case["foreground"]["stats"]["occluders"] == 2
            and case["foreground"]["stats"]["brushes"] == 6
            and case["foreground"]["stats"]["models"] == 1
            and case["foreground"]["stats"]["static_props"] == 3, "Native fixture lost its helpers, room or covered/gap props")
    if sources:
        for receipt in (case["foreground"], case["background"]["result"]):
            expanded = receipt["stats"]["instances"]
            require(expanded["occluders"] == 2 and expanded["instances"] == 4 and expanded["files"] == 2
                    and expanded["max_expansion_depth"] == 2 and expanded["world_brushes"] == expanded["detail_brushes"] == 0,
                    "Expanded helper count/depth or physical instance counts changed")
    decoded(data)
    case["bsp"] = str(args.output / "builds" / f"{label}-foreground.bsp")
    case["sha256"] = hashlib.sha256(data).hexdigest()
    return case, data


def validate(args):
    game = args.output / "game"
    mounted_game(game, args.game_dir)
    (args.output / "builds").mkdir()
    (args.output / "tmp").mkdir()
    args.custody = {str(args.binary): identity(args.binary)}
    installed_before = installation(args.game_dir)
    report = {"status": "failed", "binary": str(args.binary), "binary_sha256": digest(args.binary), "script_sha256": digest(Path(__file__)),
              "game_dir": str(args.game_dir), "transport": "private_stdio", "native_game_exercised": False,
              "desktop_input": False, "cases": {}, "preview": {}, "literal_transform": "middle yaw90+(8,16,64), root pitch90+(-64,center-16,8): (z,x+center,y); independent flat strips x[-2,2],y[-96,-24]/[24,96],z[0,192]"}
    clients, errors = [], []
    try:
        for role in ("compiler", "author"):
            output = args.output / role
            output.mkdir()
            env = {"TMPDIR": str(args.output / "tmp"), "HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
                   "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"), "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
                   "XDG_CONFIG_HOME": str(output / "config"), "XDG_STATE_HOME": str(output / "state")}
            client = Client(None, args.binary, output, game / "gameinfo.txt", environment=env, request_timeout=args.timeout)
            clients.append(client)
            definition = client.call("load_entity_definitions", {"path": str(args.fgd)})
            for row in definition["sources"]:
                path = Path(row["path"] if isinstance(row, dict) else row)
                args.custody[str(path)] = identity(path)
        compiler, author = clients
        for active in (True, False):
            label = "active" if active else "inactive"
            child, mid = args.output / f"{label}-leaf.vmf", args.output / f"{label}-middle.vmf"
            authored = leaf(author, child, args.custody, active)
            middle(author, mid, child, args.custody)
            control = scene(compiler, args.output / f"child-{label}.vmf", args.custody, active, mid)
            report["preview"][label] = {"child_authoring": authored, "ready": preview(compiler, control, 2)}
            child_case, child_bsp = compile_pair(compiler, args, game, f"child-{label}", control, [child, mid])
            flat = scene(author, args.output / f"literal-{label}.vmf", args.custody, active)
            flat_case, flat_bsp = compile_pair(compiler, args, game, f"literal-{label}", flat, [])
            child_case["literal_parity"] = compare(child_bsp, flat_bsp)
            report["cases"][f"child-{label}"], report["cases"][f"literal-{label}"] = child_case, flat_case
        mixed, physical, mid = (args.output / name for name in ("mixed-leaf.vmf", "physical-leaf.vmf", "mixed-middle.vmf"))
        leaf(author, mixed, args.custody, mixed=True)
        middle(author, mid, mixed, args.custody)
        control = scene(compiler, args.output / "mixed-parent.vmf", args.custody, child=mid)
        mixed_ready = preview(compiler, control, 2, mixed=True)
        shared.reviewed_open(author, mixed)
        owners = [row["id"] for row in author.call("find_objects", {"classname": "func_occluder"})["objects"]]
        author.call("delete_objects", {"ids": owners})
        shared.save(author, physical, args.custody)
        physical_mid = args.output / "physical-middle.vmf"
        middle(author, physical_mid, physical, args.custody)
        control = scene(compiler, args.output / "physical-parent.vmf", args.custody, child=physical_mid)
        physical_ready = preview(compiler, control, 0, mixed=True)
        require(all(mixed_ready["geometry"][key] == physical_ready["geometry"][key] for key in GEOMETRY)
                and mixed_ready["face_provenance_total"] == physical_ready["face_provenance_total"]
                and [row["bounds"] for row in mixed_ready["owners"]] == [row["bounds"] for row in physical_ready["owners"]],
                "Consumed helpers altered mixed physical preview meshes/provenance/bounds")
        report["preview"]["mixed"] = {"mixed": mixed_ready, "physical_only": physical_ready}
        for client in clients:
            require_clean_eof(client)
        require(installation(args.game_dir) == installed_before, "Installed source/module identities changed")
        require(all(identity(Path(path)) == expected for path, expected in args.custody.items()), "Source/binary/FGD/previous output identity changed")
        if args.compare_report:
            previous = json.loads(args.compare_report.read_text())
            require(previous["status"] == "passed" and previous["cases"].keys() == report["cases"].keys(), "Cross-binary case scope differs")
            for name, case in report["cases"].items():
                require(Path(case["bsp"]).read_bytes() == Path(previous["cases"][name]["bsp"]).read_bytes(), "Actual MCP binaries produced different native BSP bytes")
            report["cross_binary_exact_bsp_parity"] = True
        report.update(status="passed", installed_sources=installed_before)
    except BaseException as error:
        errors.append(error)
        report["error"] = {"type": type(error).__name__, "message": str(error)}
    finally:
        errors.extend(cleanup_clients(clients, report))
        if errors:
            report["status"] = "failed"
        report["compile_calls"] = args.compile_calls
        report["custody"] = args.custody
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        if errors:
            raise BaseExceptionGroup("Instanced unnamed occluder validation failed", errors)
    return report


def main():
    parser = argparse.ArgumentParser(description="Validate normal installed-FGD unnamed child occluders, helper-only and mixed CPU reference reports, independent literal native geometry and foreground/background output through either actual production MCP binary. No native engine, rendering or desktop input is used.",
        epilog="Requires Python3.11+, installed CS:S and SourceSDK cstrike.fgd, a matching production binary and caller-managed single8-job/24GiB pipeline. Creates fresh disk artifacts under target/validation. Native handle/culling qualification is a separate private packet; this script performs no builds or game launches.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--game-dir", type=Path, default=DEFAULT_GAME)
    parser.add_argument("--fgd", type=Path, default=shared.DEFAULT_FGD)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--compare-report", type=Path)
    args = parser.parse_args()
    args.binary, args.game_dir, args.fgd = (path.resolve(strict=True) for path in (args.binary, args.game_dir, args.fgd))
    require(args.timeout > 0 and not args.output.exists() and not args.output.is_symlink(), "Use a positive timeout and fresh output")
    args.output = args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Keep artifacts on disk under target/validation")
    args.output.mkdir(parents=True, mode=0o700)
    args.compile_calls = 0
    validate(args)
    print(json.dumps({"status": "passed", "output": str(args.output), "compile_calls": args.compile_calls}))


if __name__ == "__main__":
    sys.dont_write_bytecode = True
    main()
