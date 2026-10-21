import argparse
import json
import os
import re
from pathlib import Path
from unittest.mock import patch

from validate_authored_planes import records
from validate_compile_jobs import stopped_on_exit
from validate_direct_lighting import FACE_FORMAT
from validate_face_lighting import DEFAULT_BINARY, ROOT, detail, face_ref
from validate_instance_contents import refresh
from validate_material_albedo import (
    compile_pair,
    digest,
    floor_base,
    identities,
    payload,
    require,
    settings,
)
from validate_nested_brush_instances import (
    FULL,
    LIT,
    MATERIALS,
    SCALES,
    SHIFTS,
    U,
    V,
    assets,
    fixture_blocks,
    open_client,
    physics_input,
    room,
    root_map,
    source_state,
)
from validate_recovery_exit import require_clean_eof
from validate_sky_preview import rgba_png
from validate_terrain_density_chains import INFO_FORMAT, quad

PATCHES = (
    {"power": 2, "detail": False, "low": (-96, -48, 0), "high": (-32, 16, 16),
     "moves": ((6, (3, 1, 12), 83), (18, (-1, 2, 20), 197))},
    {"power": 3, "detail": True, "low": (32, -32, 0), "high": (96, 32, 16),
     "moves": ((11, (2, -1, 10), 101), (50, (-2, 3, 18), 211))},
)
PLACEMENTS = ("translation", "quarter")
POINTFILE = b"13 17 19\n23 29 31\n"
POSITION_ERROR = 0.0001
UV_ERROR = 0.004


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def private_client(binary, output, game, temporary):
    with patch.dict(os.environ, {"TMPDIR": str(temporary)}):
        return open_client(binary, output, game)


def property_state(client):
    draft = client.call("get_map_info")["property_draft"]
    return client.call("get_property_edit") if draft is not None else None


def placed(point, placement):
    x, y, z = point
    if placement == "translation":
        return (x + 24, y + 8, z + 56)
    if placement == "quarter":
        return (60 - y, 44 + x, 40 + z)
    require(placement == "source", "Unknown literal placement")
    return (x, y, z)


def direction(vector, placement):
    x, y, z = vector
    return (-y, x, z) if placement == "quarter" else (x, y, z)


def projection(placement):
    shifts = {"source": SHIFTS, "translation": (119.5, -47.25), "quarter": (63.5, 96.75)}[placement]
    return {"u_axis": direction(U, placement), "v_axis": direction(V, placement),
            "scale_u": SCALES[0], "scale_v": SCALES[1], "shift_u": shifts[0], "shift_v": shifts[1], "lightmap_scale": 64}


def grid_points(spec, placement, deformed=True):
    cells = 1 << spec["power"]
    moves = {vertex: offset for vertex, offset, _alpha in spec["moves"]} if deformed else {}
    result = []
    for row in range(cells + 1):
        for col in range(cells + 1):
            original = (spec["low"][0] + col * (spec["high"][0] - spec["low"][0]) / cells,
                        spec["low"][1] + row * (spec["high"][1] - spec["low"][1]) / cells, 16)
            offset = moves.get(row * (cells + 1) + col, (0, 0, 0))
            result.append(placed(tuple(value + delta for value, delta in zip(original, offset, strict=True)), placement))
    return result


def bounds(points):
    return [[min(point[axis] for point in points) for axis in range(3)],
            [max(point[axis] for point in points) for axis in range(3)]]


def same_bounds(actual, expected):
    return actual is not None and all(abs(a - b) <= POSITION_ERROR for left, right in zip(actual, expected, strict=True)
                                      for a, b in zip(left, right, strict=True))


def commit(client, draft):
    client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})


def vertex_mask(client, face, vertices):
    highlight = client.call("highlight_terrain_vertices", {"targets": [{"face": face, "vertices": vertices}]})
    return client.call("get_terrain_vertex_mask", {"faces": [face], "token": highlight["token"]})["token"]


def author_patch(client, spec, placement):
    corners = [placed((x, y, z), placement) for x in (spec["low"][0], spec["high"][0])
               for y in (spec["low"][1], spec["high"][1]) for z in (0, 16)]
    low, high = bounds(corners)
    material = MATERIALS[int(spec["detail"])]
    block = client.call("create_block", {"mins": low, "maxs": high, "material": material})["id"]
    shape = detail(client, [block])[0]
    client.call("edit_faces", {"faces": [face_ref(shape, face) for face in shape["faces"]], **projection(placement)})
    top = face_ref(shape, next(face for face in shape["faces"] if face["normal"] == [0, 0, 1]))
    creation = client.call("get_terrain_creation", {"faces": [top]})["faces"][0]
    start = list(placed((spec["low"][0], spec["low"][1], 16), placement))
    require(creation["corners"].count(start) == 1, "Literal terrain start is not one unique authored cage corner")
    client.call("create_terrain", {"faces": [top], "power": spec["power"], "start_corner": creation["corners"].index(start)})
    initial = client.call("get_terrain_grids", {"faces": [top]})["faces"][0]
    require(initial["positions"] == [list(point) for point in grid_points(spec, placement, False)],
            "Authored terrain row orientation differs from the independent literal grid")
    for vertex, offset, alpha in spec["moves"]:
        token = vertex_mask(client, top, [vertex])
        commit(client, client.call("preview_terrain_vertex_move", {"faces": [top], "mask_token": token,
                                                                  "offset": direction(offset, placement)}))
        token = vertex_mask(client, top, [vertex])
        commit(client, client.call("preview_terrain_blend", {"faces": [top], "mode": "fill", "value": alpha, "mask_token": token}))
    commit(client, client.call("preview_terrain_triangle_tags", {
        "targets": [{"face": top, "triangles": list(range(2 * (1 << (2 * spec["power"]))))}],
        "walkable": "enabled", "buildable": "enabled"}))
    for triangle, walkable, buildable in ((3, "disabled", "enabled"), (17, "enabled", "disabled")):
        commit(client, client.call("preview_terrain_triangle_tags", {"targets": [{"face": top, "triangles": [triangle]}],
                                                                    "walkable": walkable, "buildable": buildable}))
    grid = client.call("get_terrain_grids", {"faces": [top]})["faces"][0]
    expected_alpha = {vertex: alpha for vertex, _offset, alpha in spec["moves"]}
    require(grid["positions"] == [list(point) for point in grid_points(spec, placement)]
            and grid["alphas"] == [expected_alpha.get(vertex, 0) for vertex in range(len(grid["positions"]))],
            "Shared terrain edits differ from the literal offsets or alpha values")
    triangles = client.call("get_terrain_triangles", {"faces": [top]})["faces"][0]["triangles"]
    require(not triangles[3]["walkable"] and triangles[3]["buildable"]
            and triangles[17]["walkable"] and not triangles[17]["buildable"], "Triangle classifications were not authored")
    if spec["detail"]:
        client.call("tie_to_entity", {"ids": [block], "classname": "func_detail"})
    return {"solid": shape["solid"], "side": top["side"], "power": spec["power"], "detail": spec["detail"]}


def author_leaf(client, path):
    client.call("new_map", {"discard_changes": True})
    result = [author_patch(client, spec, "source") for spec in PATCHES]
    client.call("set_selection_mode", {"mode": "objects"})
    client.call("save_map", {"path": str(path)})
    return result


def author_middle(client, path, placement, leaf="leaf.vmf", shifted=False):
    client.call("new_map", {"discard_changes": True})
    origin = [14000, 0, 0] if shifted else ([8, 16, 24] if placement == "translation" else [12, 20, 16])
    client.call("create_entity", {"classname": "func_instance", "origin": origin, "properties": {"file": leaf, "angles": "0 0 0"}})
    client.call("save_map", {"path": str(path)})


def author_flat(client, path, placement):
    client.call("new_map", {"discard_changes": True})
    room(client)
    for spec in PATCHES:
        author_patch(client, spec, placement)
    client.call("set_selection_mode", {"mode": "objects"})
    ids = [row["id"] for row in client.call("find_objects", {"limit": 500})["objects"]]
    client.call("select", {"ids": ids[:1]})
    client.call("save_map", {"path": str(path)})
    return ids


def setup(client, mode=FULL, engine=False):
    settings(client, mode)
    if engine:
        client.call("set_compile_settings", {"physics": "engine"})
    if not any(row["name"] == "Instance terrain oracle" for row in client.call("get_build_recipes")["recipes"]):
        client.call("save_build_recipe", {"name": "Instance terrain oracle"})


def pair(client, output, label, source, ids, game, sources, failure=None, retained=None):
    before = {str(path): digest(path.read_bytes()) for path in sources}
    draft = property_state(client)
    for suffix in ("", "-background"):
        (output / f"{label}{suffix}.lin").write_bytes(POINTFILE)
    with stopped_on_exit(client):
        data, receipt = compile_pair(client, output, label, ids, source, game, failure, retained)
    require(len(data) <= 16 * 1024 * 1024, "Bounded terrain fixture produced an oversized BSP")
    require(all((output / f"{label}{suffix}.lin").read_bytes() == POINTFILE for suffix in ("", "-background")),
            "Terrain compilation changed the retained pointfile")
    require(property_state(client) == draft and all(digest(Path(path).read_bytes()) == value for path, value in before.items()),
            "Terrain compilation changed an owned property draft or instance source")
    receipt["instance_sources"] = before
    receipt["property_draft_present"] = draft is not None
    receipt["property_draft_preserved"] = True
    return data, receipt


def verify_native(data, stats, placement):
    infos = records(data, 26, INFO_FORMAT)
    vertices, tags = records(data, 33, "<5f"), records(data, 48, "<H")
    require(len(infos) == 2 and {info[5] for info in infos} == {2, 3} and len(vertices) == 106 and len(tags) == 160,
            "Native terrain was omitted or replaced by its cage")
    require([stats[key] for key in ("displacements", "displacement_vertices", "displacement_triangles")] == [2, 106, 160],
            "Compiler terrain statistics differ from the independent grids")
    result = []
    for index, info in enumerate(infos):
        spec = next(spec for spec in PATCHES if spec["power"] == info[5])
        size = (1 << spec["power"]) + 1
        corners = quad(data, index, info)
        expected = grid_points(spec, placement)
        observed = []
        alpha = {vertex: value for vertex, _offset, value in spec["moves"]}
        for vertex in range(size * size):
            row, col = divmod(vertex, size)
            u, v = col / (size - 1), row / (size - 1)
            base = tuple((1 - u) * ((1 - v) * corners[0][axis] + v * corners[1][axis])
                         + u * ((1 - v) * corners[3][axis] + v * corners[2][axis]) for axis in range(3))
            raw = vertices[info[3] + vertex]
            point = tuple(base[axis] + raw[axis] * raw[3] for axis in range(3))
            require(max(abs(a - b) for a, b in zip(point, expected[vertex], strict=True)) <= POSITION_ERROR
                    and raw[4] == alpha.get(vertex, 0), "Native displacement position/alpha differs from the literal transformed grid")
            observed.append(point)
        count = 2 * (size - 1) ** 2
        require([tag[0] for tag in tags[info[4]:info[4] + count]] == [5 if triangle == 3 else 3 if triangle == 17 else 7 for triangle in range(count)],
                "Native terrain triangle classifications changed")
        face = records(data, 7, FACE_FORMAT)[info[9]]
        texinfo = records(data, 6, "<16f2i")[face[5]]
        mapping = projection(placement)
        uv = tuple(value / mapping["scale_u"] for value in mapping["u_axis"]) + (mapping["shift_u"],)
        uv += tuple(value / mapping["scale_v"] for value in mapping["v_axis"]) + (mapping["shift_v"],)
        require(all(abs(a - b) <= UV_ERROR for a, b in zip(texinfo[:8], uv, strict=True)), "Attached terrain texture projection changed")
        result.append({"power": spec["power"], "detail": spec["detail"], "vertices": len(observed),
                       "bounds": bounds(observed), "positions_sha256": digest(json.dumps(observed).encode())})
    return result


def verify_instances(receipt, base):
    for compiled in (receipt["result"], receipt["background"]["result"]):
        stats = compiled["stats"]["instances"]
        require([stats[key] for key in ("instances", "files", "world_brushes", "detail_brushes", "max_expansion_depth", "terrain_patches", "terrain_vertices")]
                == [2, 2, 1, 1, 2, 2, 106], "Expanded instance terrain accounting changed")
        paths = sorted(Path(path).relative_to(base).as_posix() for path in receipt["instance_sources"])
        require(stats["dependencies"] == [{"path": path, "bytes": len((base / path).read_bytes()),
                                            "sha256": digest((base / path).read_bytes())} for path in paths],
                "Instance dependency hashes differ from the saved terrain sources")


def preview(client, source, owner, leaf_rows, placement):
    before = source_state(client, source)
    draft = property_state(client)
    ready = refresh(client, background=True)
    require(ready["geometry"]["vertices"] == 106 and ready["geometry"]["indices"] == 480
            and ready["geometry"]["triangles"] == 160 and ready["geometry"]["line_vertices"] == 528,
            "Instance preview used cage faces or omitted displaced triangles")
    require(ready["face_provenance_total"] == 2 and ready["brushes_total"] == 2
            and not ready["face_provenance_truncated"] and not ready["brushes_truncated"], "Terrain provenance is missing or truncated")
    for spec, leaf in zip(PATCHES, leaf_rows, strict=True):
        brush = next(row for row in ready["brushes"] if row["child_brush"] == leaf["solid"])
        face = next(row for row in ready["face_provenance"] if row["child_side"] == leaf["side"])
        require(brush["owner"] == owner and brush["file"] == "leaf.vmf" and brush["detail"] == spec["detail"]
                and same_bounds(brush["bounds"], bounds(grid_points(spec, placement))), "Preview bounds or true leaf/detail identity changed")
        require(face["owner"] == owner and face["file"] == "leaf.vmf" and face["child_brush"] == leaf["solid"]
                and face["terrain"] is True and face["power"] == spec["power"]
                and face["vertices"] == ((1 << spec["power"]) + 1) ** 2
                and face["triangles"] == 2 * (1 << (2 * spec["power"])), "Preview terrain face provenance changed")
    require(ready["owners"][0]["owner"] == owner and same_bounds(ready["owners"][0]["bounds"],
            bounds([point for spec in PATCHES for point in grid_points(spec, placement)])), "Owner bounds use the undeformed cage")
    require([ready["limits"][key] for key in ("terrain_source_vertices", "terrain_expanded_vertices", "terrain_grid_work")]
            == [65536, 262144, 2097152], "Terrain source, expansion and grid-work caps are not discoverable")
    require(ready["geometry"]["cpu_bytes"] <= ready["limits"]["cpu_bytes"]
            and ready["geometry"]["gpu_bytes"] <= ready["limits"]["gpu_bytes"], "Terrain preview exceeded its retained geometry budget")
    require(source_state(client, source) == before and property_state(client) == draft
            and not client.call("get_map_info")["rendering"]["initialized"], "CPU preview changed the document/draft or initialized graphics")
    return ready


def active_ids(client):
    return [row["id"] for row in client.call("find_objects", {"limit": 500})["objects"]]


def run_cases(client, args, game, leaf, leaf_rows, report):
    retained = None
    controls = {}
    for placement in PLACEMENTS:
        middle = args.output / f"{placement}-middle.vmf"
        author_middle(client, middle, placement)
        source = args.output / f"{placement}-parent.vmf"
        origin, angles = ([16, -8, 32], "0 0 0") if placement == "translation" else ([80, 32, 24], "0 90 0")
        ids, owner = root_map(client, source, middle.name, origin, angles)
        flat = args.output / f"{placement}-flat.vmf"
        author_flat(client, flat, placement)
        for label, mode in (("fullbright", FULL), ("lit", LIT)):
            client.call("open_map", {"path": str(source), "discard_changes": True})
            ids = active_ids(client)
            client.call("select", {"ids": [owner]})
            setup(client, mode)
            token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
            client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": "message", "value": "Unapplied instance terrain review"}]})
            ready = preview(client, source, owner, leaf_rows, placement)
            nested, receipt = pair(client, args.output, f"{placement}-{label}", source, ids, game, [leaf, middle])
            verify_instances(receipt, args.output)
            receipt["native_grid"] = verify_native(nested, receipt["result"]["stats"], placement)
            receipt["preview"] = ready
            client.call("cancel_property_edit", {"token": token})
            client.call("open_map", {"path": str(flat), "discard_changes": True})
            flat_ids = active_ids(client)
            setup(client, mode)
            manual, flat_receipt = pair(client, args.output, f"{placement}-{label}-flat", flat, flat_ids, game, [leaf, middle])
            flat_receipt["native_grid"] = verify_native(manual, flat_receipt["result"]["stats"], placement)
            require("instances" not in flat_receipt["result"]["stats"], "Literal flattened map acquired instance metadata")
            require(payload(nested, 48) == payload(manual, 48), "Flattened native terrain tags differ")
            if mode == LIT:
                receipt["lighting"] = {}
                for channel in ("ldr", "hdr"):
                    actual, expected = floor_base(nested, channel), floor_base(manual, channel)
                    means = [[sum(row[index] for row in samples) / len(samples) for index in range(3)] for samples in (actual, expected)]
                    require(all(abs(a - b) <= 1 + 0.05 * max(a, b) for a, b in zip(*means, strict=True)),
                            "Flattened floor lighting exceeds the existing quantized mean bound")
                    receipt["lighting"][channel] = {"nested_mean": means[0], "flat_mean": means[1]}
            receipt["flat"] = flat_receipt
            report["positive"][f"{placement}-{label}"] = receipt
            if placement == "quarter" and mode == FULL:
                retained = nested
                controls = {"source": source, "flat": flat, "owner": owner, "middle": middle, "ids": ids}
    return retained, controls


def negative_cases(client, args, game, leaf, controls, retained, report):
    source, middle = controls["source"], controls["middle"]
    client.call("open_map", {"path": str(source), "discard_changes": True})
    client.call("select", {"ids": [controls["owner"]]})
    setup(client)
    token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
    client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": "message", "value": "Keep this draft through invalid child grids"}]})
    original = leaf.read_bytes()
    text = original.decode()
    normals = fixture_blocks(text, "normals")[0]
    malformed, count = re.subn(r'("row0"\s+")[^"]*(")', r'\g<1>0 0\2', normals, count=1)
    require(count == 1 and malformed != normals, "Malformed-grid fixture did not alter one original normals row")
    try:
        leaf.write_text(text.replace(normals, malformed, 1))
        _, report["negative"]["malformed-grid"] = pair(client, args.output, "malformed-grid", source, controls["ids"], game,
                                                        [leaf, middle], "normals rows", retained)
        before = source_state(client, source), client.call("get_property_edit")
        response = client.call("set_instance_contents", {"action": "refresh"})
        require(response["state"] == "failed" and not response["available"]
                and (source_state(client, source), client.call("get_property_edit")) == before,
                "Malformed child grid retained a partial preview or changed its parent/draft")
    finally:
        leaf.write_bytes(original)
    client.call("cancel_property_edit", {"token": token})
    shifted = args.output / "shifted-leaf.vmf"
    client.call("open_map", {"path": str(leaf), "discard_changes": True})
    face = {"solid": report["leaf_rows"][0]["solid"], "side": report["leaf_rows"][0]["side"]}
    mask = vertex_mask(client, face, list(range(25)))
    commit(client, client.call("preview_terrain_vertex_move", {"faces": [face], "mask_token": mask, "offset": [4096, 0, 0]}))
    client.call("save_map", {"path": str(shifted)})
    intermediate = args.output / "intermediate.vmf"
    author_middle(client, intermediate, "source", shifted.name, True)
    failed_source = args.output / "intermediate-parent.vmf"
    ids, owner = root_map(client, failed_source, intermediate.name, [-14000, 0, 64])
    setup(client)
    token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
    client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": "message", "value": "Keep this intermediate-grid review"}]})
    _, receipt = pair(client, args.output, "intermediate-grid-bounds", failed_source, ids, game, [shifted, intermediate],
                      "Displacement positions must fit Source compile bounds", retained)
    require(all(any(row["level"] == "error" and row["object"] == owner for row in result["diagnostics"])
                for result in (receipt["result"], receipt["background"]["result"])), "Intermediate grid refusal lost root-helper provenance")
    report["negative"]["intermediate-grid-bounds"] = receipt
    before = source_state(client, failed_source), client.call("get_property_edit")
    failed = client.call("set_instance_contents", {"action": "refresh"})
    require(failed["state"] == "failed" and not failed["available"]
            and (source_state(client, failed_source), client.call("get_property_edit")) == before,
            "Intermediate terrain bounds refusal changed its parent/draft or retained a partial preview")
    client.call("cancel_property_edit", {"token": token})


def identity_restart(client, args, game, leaf, leaf_rows, controls, retained, report):
    source, middle, owner = controls["source"], controls["middle"], controls["owner"]
    client.call("open_map", {"path": str(source), "discard_changes": True})
    client.call("select", {"ids": [owner]})
    setup(client)
    before, receipt = pair(client, args.output, "identity-before", source, controls["ids"], game, [leaf, middle])
    require(before == retained, "Identity control no longer matches the original fullbright BSP")
    ready = preview(client, source, owner, leaf_rows, "quarter")
    stable, camera = source_state(client, source), client.call("get_camera")
    old = leaf.with_name("retained-leaf.vmf")
    leaf.rename(old)
    leaf.write_bytes(old.read_bytes())
    stale = client.call("get_compile_status")
    contents = client.call("inspect_instance_contents")
    require(stale["job"]["stale"] and not stale["job"]["instance_dependencies"]["current"] and not stale["launch"]["ready"]
            and contents["state"] == "stale" and not contents["available"], "Same-byte replacement did not invalidate retained source identity")
    client.call("set_instance_contents", {"action": "frame", "token": str(ready["token"]), "ids": [owner]}, error=True)
    require(source_state(client, source) == stable and client.call("get_camera") == camera, "Stale terrain refusal changed document or camera")
    preview(client, source, owner, leaf_rows, "quarter")
    refreshed, rebuilt = pair(client, args.output, "identity-refreshed", source, controls["ids"], game, [leaf, middle])
    require(refreshed == retained, "Fresh source identity changed accepted terrain BSP bytes")
    report["identity"] = {"before": receipt, "stale": stale, "stale_preview": contents, "refreshed": rebuilt}


def physics(args, controls, leaf):
    output = args.output / "physics"
    output.mkdir()
    private = output / "installation/game"
    assets(private)
    native = args.physics_gameinfo.parent.parent / "bin/linux64"
    require(native.is_dir(), "Physics installation has no Linux64 library directory")
    mounted = private.parent / "bin/linux64"
    mounted.parent.mkdir()
    mounted.symlink_to(native, target_is_directory=True)
    protected = {}
    for path in (args.physics_gameinfo, native / "vphysics.so", native / "libtier0.so", args.binary.parent / "hammerr-physics"):
        protected[str(path)] = physics_input(path, (512 if path.name == "hammerr-physics" else 64) * 1024 * 1024)
    client = private_client(args.binary, output / "client", private, args.output / "tmp")
    try:
        receipts, data = {}, {}
        for label, source in (("nested", controls["source"]), ("flat", controls["flat"])):
            client.call("open_map", {"path": str(source)})
            setup(client, engine=True)
            data[label], receipts[label] = pair(client, output, f"engine-{label}", source, active_ids(client), private,
                                               [leaf, controls["middle"]])
        require(payload(data["nested"], 29) == payload(data["flat"], 29)
                and receipts["nested"]["result"]["stats"]["physics_mesh_triangles"] == 160,
                "Nested terrain Engine cooking differs from the independent flattened control")
        require(all(physics_input(Path(path), identity["limit_bytes"]) == identity for path, identity in protected.items()),
                "Engine cooking changed an installed library or physics helper")
        require(mounted.is_symlink() and mounted.resolve(strict=True) == native.resolve(strict=True), "Private native library binding changed")
        result = {"receipts": receipts, "read_only_inputs": protected, "physics_lump_sha256": digest(payload(data["nested"], 29)),
                  "private_gameinfo": str(private / "gameinfo.txt"), "clean_eof": require_clean_eof(client)}
        require(all(physics_input(Path(path), identity["limit_bytes"]) == identity for path, identity in protected.items()),
                "Engine EOF changed an installed library or physics helper")
        return result
    finally:
        if client.process is not None:
            require_clean_eof(client)


def render_preview(client, args, controls, report):
    source = controls["source"]
    ready = client.call("inspect_instance_contents")
    require(ready["state"] == "ready" and ready["geometry"]["vertices"] == 106
            and ready["geometry"]["triangles"] == 160, "Restart lost the checked terrain snapshot before rendering")
    token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
    client.call("update_property_edit", {"token": token, "changes": [
        {"action": "set", "key": "message", "value": "Keep this terrain render review unapplied"}]})
    before = source_state(client, source)
    draft = client.call("get_property_edit")
    build_settings = client.call("get_compile_settings")
    spatial = client.call("get_map_info")["spatial"]
    files = {str(path): digest(path.read_bytes()) for path in args.output.iterdir()
             if path.is_file() and path.suffix in (".vmf", ".bsp", ".lin")}
    assets_before = identities(args.output / "game")
    camera = {"projection": "perspective", "position": [-130, -135, 172],
              "look_at": [66, 42, 59], "fov": 44}
    caption = ("Nested world and func_detail terrain, shown as flat reference surfaces after a quarter turn. "
               "The uploaded displaced sheets retain power-2 and power-3 grids: 106 vertices, 160 triangles, "
               "two terrain provenance rows and deformed bounds; cage side faces are excluded. "
               "The same private restarted Session produced both captures with temporary camera/view overrides.")
    rendering = {"scope": "Private production MCP render_view; offscreen flat terrain reference preview only; no game or desktop input",
                 "caption": caption, "images": [], "cpu_snapshot": ready}
    report["render_preview"] = rendering
    paths = []
    for enabled, name in ((False, "instance-terrain-off"), (True, "instance-terrain-perspective")):
        png, metadata = client.render_result(width=1201, height=801, camera=camera, surface="flat",
            instance_contents=enabled, selection_outlines=False, fog_preview=False,
            lighting_preview=False, normal_maps=False, sky_preview=False, sky3d_preview=False,
            rope_preview=False, entity_images=False, entity_model_guides=False, leak_path=False)
        path = args.output / f"{name}.png"
        path.write_bytes(png)
        write_json(args.output / f"{name}.metadata.json", metadata)
        paths.append(path)
        rendering["images"].append({"file": path.name, "sha256": digest(png), "instance_contents": enabled})
        report["graphics_initialized"] = client.call("get_map_info")["rendering"]["initialized"]
        uploaded = metadata["instance_contents"]
        require(report["graphics_initialized"] and metadata["view"]["surface"] == "flat"
                and metadata["view"]["instance_contents"] is enabled
                and uploaded["state"] == "ready" and uploaded["requested"] is enabled
                and uploaded["displayed"] is enabled, "Offscreen render did not apply the temporary terrain display override")
        require(all(uploaded[key] == ready[key] for key in (
            "token", "source", "stats", "owners", "brushes", "brushes_total", "brushes_truncated",
            "face_provenance", "face_provenance_total", "face_provenance_truncated", "geometry", "policy")),
            "Uploaded terrain geometry/provenance differs from the independently checked CPU snapshot")
        require(uploaded["policy"] == "read_only_world_detail_terrain_root_owner_picking_no_material_loading"
                and uploaded["geometry"]["indices"] == 480 and uploaded["geometry"]["line_vertices"] == 528
                and [uploaded["stats"][key] for key in ("world_brushes", "detail_brushes", "terrain_patches", "terrain_vertices")]
                == [1, 1, 2, 106]
                and sorted((row["power"], row["vertices"], row["triangles"]) for row in uploaded["face_provenance"])
                == [(2, 25, 32), (3, 81, 128)] and all(row["terrain"] for row in uploaded["face_provenance"]),
                "Rendered reference metadata omitted the displaced grids or substituted cage geometry")
    require(rgba_png(paths[0]) != rgba_png(paths[1]), "Enabling retained instance terrain did not change the offscreen image")
    require(source_state(client, source) == before and client.call("get_property_edit") == draft
            and client.call("get_compile_settings") == build_settings
            and client.call("get_map_info")["spatial"] == spatial,
            "Temporary terrain rendering changed source, draft, history, settings, selection, view or camera")
    require(all(digest(Path(path).read_bytes()) == identity for path, identity in files.items())
            and identities(args.output / "game") == assets_before,
            "Offscreen terrain rendering changed authored source, BSP/LIN outputs or mounted materials")
    require(client.call("inspect_instance_contents") == ready, "Rendering replaced the retained terrain snapshot")
    (args.output / "instance-terrain-perspective.caption.txt").write_text(caption + "\n")
    rendering["nonidentical_images"] = True
    rendering["document_draft_history_settings_selection_view_camera_files_preserved"] = True


def validate(args):
    client, restarted = None, None
    game = args.output / "game"
    leaf = args.output / "leaf.vmf"
    report = {"status": "failed", "binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()),
              "script_sha256": digest(Path(__file__).read_bytes()), "native_game_exercised": False,
              "graphics_initialized": False, "desktop_input": False, "positive": {}, "negative": {}}
    try:
        (args.output / "tmp").mkdir()
        assets(game)
        client = private_client(args.binary, args.output / "client", game, args.output / "tmp")
        leaf_rows = author_leaf(client, leaf)
        report["leaf_rows"] = leaf_rows
        original = leaf.read_bytes()
        retained, controls = run_cases(client, args, game, leaf, leaf_rows, report)
        negative_cases(client, args, game, leaf, controls, retained, report)
        require(leaf.read_bytes() == original, "Negative fixtures changed the original leaf source")
        identity_restart(client, args, game, leaf, leaf_rows, controls, retained, report)
        saved_sources = {str(path): digest(path.read_bytes()) for path in args.output.glob("*.vmf")}
        saved_assets = identities(game)
        report["clean_eof"] = require_clean_eof(client)
        restarted = private_client(args.binary, args.output / "restart", game, args.output / "tmp")
        restarted.call("open_map", {"path": str(controls["source"])})
        setup(restarted)
        preview(restarted, controls["source"], controls["owner"], leaf_rows, "quarter")
        data, report["restart"] = pair(restarted, args.output, "restart", controls["source"], active_ids(restarted), game,
                                        [leaf, controls["middle"]])
        require(data == retained, "Fresh process failed to recapture the current terrain sources deterministically")
        if args.render_preview:
            render_preview(restarted, args, controls, report)
        report["restart_clean_eof"] = require_clean_eof(restarted)
        if args.physics_gameinfo:
            report["physics"] = physics(args, controls, leaf)
        report["source_sha256"] = digest(leaf.read_bytes())
        report["assets"] = identities(game)
        report["source_files"] = saved_sources
        require(leaf.read_bytes() == original and identities(game) == saved_assets
                and all(digest(Path(path).read_bytes()) == identity for path, identity in saved_sources.items()),
                "EOF, restart or physics changed authored source files or mounted materials")
        report["status"] = "accepted"
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        try:
            for active in (client, restarted):
                if active is not None and active.process is not None:
                    with stopped_on_exit(active):
                        pass
                    require_clean_eof(active)
        finally:
            write_json(args.output / "results.json", report)
    return report


def main():
    parser = argparse.ArgumentParser(description="Validate nested world/detail terrain through private production MCP, literal flattened grids, CPU instance preview and guarded publication; optional offscreen rendering, no game launch.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY, help="Production hammerr-headless or editor binary; requires sibling hammerr-physics for optional Engine checks")
    parser.add_argument("--output", type=Path, required=True, help="New artifact directory below target/validation")
    parser.add_argument("--physics-gameinfo", type=Path, help="Optional installed gameinfo locating read-only bin/linux64/vphysics.so and libtier0.so; stock assets are not mounted")
    parser.add_argument("--render-preview", action="store_true", help="After CPU/restart checks, initialize offscreen graphics to compare flat instance terrain off/on and save a perspective PNG with retained geometry/provenance metadata; requires a working GPU or software graphics adapter, never opens a desktop window")
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(), args.output.resolve()
    if args.physics_gameinfo:
        args.physics_gameinfo = args.physics_gameinfo.resolve(strict=True)
    require(args.output.is_relative_to(ROOT / "target/validation"), "Artifacts must stay under target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    report = validate(args)
    print(json.dumps({"status": report["status"], "compile_calls": 26 + (4 if args.physics_gameinfo else 0), "output": str(args.output)}))


if __name__ == "__main__":
    main()
