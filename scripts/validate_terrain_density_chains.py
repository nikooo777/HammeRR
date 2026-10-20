import argparse
import json
import math
import os
import re
import struct
from collections import Counter
from pathlib import Path
from unittest.mock import patch

from validate_authored_planes import records
from validate_direct_lighting import FACE_FORMAT
from validate_face_lighting import DEFAULT_BINARY, ROOT, detail, face_ref
from validate_material_albedo import (
    digest,
    fixture_texture,
    open_client,
    payload,
    require,
    settings,
)
from validate_material_read_limits import (
    compile_pair,
    fingerprint,
    fingerprints,
    snapshot,
)
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM

MATERIAL = "hammerr695/terrain"
PATCHES = (
    ("A", (0, 0, 0), (64, 128, 16), 2, (0, 0, 16)),
    ("B", (64, 0, 0), (128, 64, 16), 2, (64, 0, 16)),
    ("C", (128, 0, 0), (160, 32, 16), 3, (128, 0, 16)),
    ("D", (160, 0, 0), (192, 32, 16), 3, (192, 32, 16)),
)
MASKS = (
    (0xFFFFFFFF,) * 10,
    (0xFFFF7FDF,) + (0xFFFFFFFF,) * 9,
    (0xC7F3F1FD, 0x7F3F1FDF, 0xFFFFFDFC) + (0xFFFFFFFF,) * 7,
    (0xFFFFFFFF,) * 10,
)
LINKS = {
    (0, 2, 0): (1, 0, 1, 0, 0),
    (1, 0, 0): (0, 0, 0, 1, 0),
    (1, 2, 0): (2, 0, 1, 0, 0),
    (2, 0, 0): (1, 0, 0, 1, 0),
    (2, 2, 0): (3, 2, 0, 0, 0),
    (3, 2, 0): (2, 2, 0, 0, 0),
}
ACTIVE_TRIANGLES = (32, 30, 98, 128)
INFO_FORMAT = "<3f4ifi2H2i48s40s10I"
GEOMETRY_LUMPS = (33, 48)
HELPERS = (
    "private_runtime_config.py", "validate_authored_planes.py", "validate_build_recipes.py",
    "validate_bump_lighting.py", "validate_compile_jobs.py",
    "validate_direct_lighting.py", "validate_face_lighting.py", "validate_material_albedo.py",
    "validate_material_read_limits.py", "validate_material_replacement.py", "validate_native_runtime.py",
    "validate_recovery_exit.py", "validate_visibility.py",
)
CONTRACT = {
    "scope": "Existing compiler data for one explicit three-cage-density chain; no Subdivide, rendering, contact or arbitrary-chain acceptance",
    "patches": PATCHES,
    "masks": MASKS,
    "links": [{"slot": key, "value": value} for key, value in sorted(LINKS.items())],
    "stored_vertices": 212,
    "stored_triangles": 320,
    "active_triangles": ACTIVE_TRIANGLES,
    "disabled_vertices": 21,
    "curved_snapped_vertices": 18,
    "height_offset": "y*y/1024",
    "alpha": "patch_index*32+row",
}


def source_point(index, row, col, curved):
    _, low, high, power, _ = PATCHES[index]
    cells = 1 << power
    u, v = col / cells, row / cells
    if index == 3:
        u, v = 1 - u, 1 - v
    x = low[0] + (high[0] - low[0]) * u
    y = low[1] + (high[1] - low[1]) * v
    return (x, y, 16 + (y * y / 1024 if curved else 0))


def displacement(index, curved):
    _, _, _, power, start = PATCHES[index]
    size = (1 << power) + 1
    normals, distances, alphas = [], [], []
    for row in range(size):
        normals.append(f'"row{row}" "' + " ".join(["0 0 1"] * size) + '"')
        distances.append(f'"row{row}" "' + " ".join(
            str(source_point(index, row, col, curved)[2] - 16) for col in range(size)) + '"')
        alphas.append(f'"row{row}" "' + " ".join([str(index * 32 + row)] * size) + '"')
    return (f'dispinfo {{ "power" "{power}" "startposition" "[{start[0]} {start[1]} 16]" '
            '"flags" "0" "elevation" "0" "subdiv" "0" normals { ' + "\n".join(normals)
            + " } distances { " + "\n".join(distances) + " } alphas { " + "\n".join(alphas) + " } }\n")


def assets(game):
    folder = game / "materials/hammerr695"
    folder.mkdir(parents=True)
    (game / "gameinfo.txt").write_text('GameInfo { FileSystem { SearchPaths { game "|gameinfo_path|." } } }\n')
    (folder / "terrain.vmt").write_text("LightmappedGeneric { $basetexture hammerr695/base }\n")
    (folder / "base.vtf").write_bytes(fixture_texture(2, 2, (0.5, 0.5, 0.5), (128, 160, 192, 255)))


def fixture(client, output):
    ids, faces = [], []
    for low, high in ROOM:
        identifier = client.call("create_block", {"mins": low, "maxs": high, "material": MATERIAL})["id"]
        ids.append(identifier)
        shape = detail(client, [identifier])[0]
        client.call("edit_faces", {"faces": [face_ref(shape, face) for face in shape["faces"]], "lightmap_scale": 64})
    for _, low, high, _, _ in PATCHES:
        identifier = client.call("create_block", {"mins": low, "maxs": high, "material": MATERIAL})["id"]
        ids.append(identifier)
        shape = detail(client, [identifier])[0]
        top = next(face for face in shape["faces"] if face["normal"][2] > 0.99)
        faces.append(face_ref(shape, top))
        client.call("edit_faces", {"faces": [face_ref(shape, face) for face in shape["faces"]], "lightmap_scale": 64})
    for classname, origin, properties in (
        ("info_player_terrorist", [-128, -128, 8], {}),
        ("info_player_counterterrorist", [128, -128, 8], {}),
        ("light", [100, 48, 180], {"_light": "255 64 0 200", "_lightHDR": "64 255 0 200",
                                  "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}),
    ):
        ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
    template = output / "ordinary.vmf"
    client.call("save_map", {"path": str(template)})
    bare = template.read_text()
    require("dispinfo" not in bare, "Fixture unexpectedly contains generated terrain")
    sources = {}
    for name, curved in (("flat", False), ("curved", True)):
        text = bare
        for index, face in enumerate(faces):
            pattern = rf'(\bside\s*\{{\s*"id"\s*"{face["side"]}"[^{{}}]*)(\}})'
            text, count = re.subn(pattern, lambda match: match[1] + displacement(index, curved) + match[2], text)
            require(count == 1, "Explicit terrain did not bind exactly one authored top side")
        path = output / f"{name}.vmf"
        path.write_text(text)
        sources[name] = path
    return ids, faces, sources


def queries(client, faces, curved):
    grids = client.call("get_terrain_grids", {"faces": faces})
    require(len(grids["faces"]) == 4, "Committed terrain grid count changed")
    for index, (face, grid) in enumerate(zip(faces, grids["faces"], strict=True)):
        size = (1 << PATCHES[index][3]) + 1
        require((grid["solid"], grid["side"], grid["power"], grid["grid_size"], grid["subdivided"])
                == (face["solid"], face["side"], PATCHES[index][3], size, False), "Imported grid identity/power/marker changed")
        expected = [list(source_point(index, row, col, curved)) for row in range(size) for col in range(size)]
        require(grid["positions"] == expected and grid["alphas"] == [index * 32 + row for row in range(size) for _ in range(size)],
                "Committed source grid differs from literal authored rows")
        require(grid["flags"] == 0 and grid["border_smoothing_error"] is None and grid["border_smoothing_angle"] is None,
                "Fixture acquired unsupported flags or smoothing")
    return grids


def active(index, vertex):
    return bool(MASKS[index][vertex // 32] & (1 << (vertex % 32)))


def triangles(index):
    size = (1 << PATCHES[index][3]) + 1
    result = []
    for row in range(size - 1):
        for col in range(size - 1):
            if index == 2 and col < 2:
                continue
            a = row * size + col
            b, c, d = a + 1, a + size, a + size + 1
            cell = ((a, c, d), (a, d, b)) if (row + col) % 2 == 0 else ((a, c, b), (c, d, b))
            removed = {5, 15} if index == 1 else {11, 29, 47, 65} if index == 2 else set()
            result.extend(triangle for triangle in cell if not removed.intersection(triangle))
    if index == 1:
        result.extend(((0, 10, 6), (10, 20, 16)))
    if index == 2:
        result.extend(((2, 20, 12), (20, 38, 30), (38, 56, 48), (56, 74, 66),
                       (2, 0, 20), (0, 36, 20), (36, 38, 20), (38, 36, 56), (36, 72, 56), (72, 74, 56)))
    require(len(result) == ACTIVE_TRIANGLES[index], "Independent literal triangle count changed")
    return result


def decoded_triangles(index, mask):
    size = (1 << PATCHES[index][3]) + 1
    result = []
    children = ((1, 1), (-1, 1), (-1, -1), (1, -1))
    ring = ((1, -1, 3), (0, -1, None), (-1, -1, 2), (-1, 0, None),
            (-1, 1, 1), (0, 1, None), (1, 1, 0), (1, 0, None), (1, -1, 3))

    def allowed(vertex):
        return bool(mask[vertex // 32] & (1 << (vertex % 32)))

    def node(x, y, step):
        selected = [False] * 4
        if step > 1:
            for child, (dx, dy) in enumerate(children):
                cx, cy = x + dx * (step // 2), y + dy * (step // 2)
                if allowed(cy * size + cx):
                    selected[child] = True
                    node(cx, cy, step // 2)
        previous = None
        for dx, dy, child in ring:
            if child is not None and selected[child]:
                previous = None
                continue
            vertex = (y + dy * step) * size + x + dx * step
            if allowed(vertex):
                if previous is not None:
                    result.append((previous, vertex, y * size + x))
                previous = vertex

    node(size // 2, size // 2, size // 2)
    return result


def canonical(triangles):
    result = []
    for triangle in triangles:
        first = triangle.index(min(triangle))
        result.append(triangle[first:] + triangle[:first])
    return sorted(result)


def height_correction(index, row, col):
    if index == 1 and row in (1, 3) and col == 0:
        return 1 / 4
    if index == 2:
        if row in (1, 3, 5, 7) and col <= 2:
            return (3, 1, 1)[col] / 64
        if row in (2, 6) and col <= 1:
            return (4, 2)[col] / 64
    return 0


def area(a, b, c):
    return ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) / 2


def height_on_surface(point, points, indices):
    heights = []
    for triangle in indices:
        a, b, c = (points[vertex] for vertex in triangle)
        denominator = area(a, b, c)
        weights = (area(point, b, c) / denominator, area(a, point, c) / denominator,
                   area(a, b, point) / denominator)
        if min(weights) >= -1e-9 and max(weights) <= 1 + 1e-9:
            heights.append(sum(weight * vertex[2] for weight, vertex in zip(weights, (a, b, c), strict=True)))
    require(heights and max(heights) - min(heights) < 1e-6, "Inactive source point has no unique covered surface height")
    return heights[0]


def quad(data, index, info):
    face = records(data, 7, FACE_FORMAT)[info[9]]
    require(face[4] == 4 and face[6] == index, "Native displacement lost its closed base quad binding")
    vertices, edges, surfedges = records(data, 3, "<3f"), records(data, 12, "<2H"), records(data, 13, "<i")
    directed = []
    for reference, in surfedges[face[3]:face[3] + 4]:
        edge = edges[abs(reference)]
        directed.append(edge if reference >= 0 else tuple(reversed(edge)))
    require(all(edge[1] == directed[(i + 1) % 4][0] for i, edge in enumerate(directed)), "Native quad edges are not closed")
    points = [vertices[edge[0]] for edge in directed]
    first = points.index(tuple(info[:3]))
    return points[first:] + points[:first]


def native_geometry(data, stats, curved):
    infos, vertices, tags = records(data, 26, INFO_FORMAT), records(data, 33, "<5f"), records(data, 48, "<H")
    require(len(infos) == 4 and len(vertices) == 212 and len(tags) == 320, "Frozen native displacement counts changed")
    require(all(tag == (7,) for tag in tags), "Ordinary terrain source triangle classifications changed")
    for key, expected in {"displacements": 4, "displacement_vertices": 212, "displacement_triangles": 320,
                          "displacement_neighbor_pairs": 3, "displacement_half_edge_pairs": 2,
                          "displacement_corner_neighbor_pairs": 0, "displacement_mixed_power_pairs": 1,
                          "displacement_lod_disabled_vertices": 21,
                          "displacement_lod_snapped_vertices": 18 if curved else 0}.items():
        require(stats[key] == expected, f"Unexpected {key}: {stats[key]} != {expected}")
    all_positions, summaries = [], []
    disabled, snapped, vertex_start, triangle_start = 0, 0, 0, 0
    for index, info in enumerate(infos):
        _, low, high, power, start = PATCHES[index]
        size = (1 << power) + 1
        require(tuple(info[:3]) == start and info[3:6] == (vertex_start, triangle_start, power), "Native start/range/power changed")
        require(tuple(info[15:]) == MASKS[index], f"Literal allowed mask changed on patch {index}")
        for edge in range(4):
            for slot in range(2):
                expected = LINKS.get((index, edge, slot), (65535, 0, 0, 0, 0))
                require(struct.unpack_from("<H4B", info[13], edge * 12 + slot * 6) == expected,
                        f"Literal neighbor slot changed: {(index, edge, slot)}")
        corners = quad(data, index, info)
        require(set(corners) == {(x, y, 16.0) for x in (low[0], high[0]) for y in (low[1], high[1])}, "Native base cage changed")
        positions, authored = [], []
        for row in range(size):
            for col in range(size):
                u, v = col / (size - 1), row / (size - 1)
                base = tuple((1 - u) * ((1 - v) * corners[0][axis] + v * corners[1][axis])
                             + u * ((1 - v) * corners[3][axis] + v * corners[2][axis]) for axis in range(3))
                raw = source_point(index, row, col, curved)
                require(base == (raw[0], raw[1], 16.0), "Native row/column orientation differs from literal source")
                vertex = vertices[vertex_start + row * size + col]
                require(all(math.isfinite(value) for value in vertex) and vertex[4] == index * 32 + row,
                        "Native vertex framing/alpha changed")
                positions.append(tuple(base[axis] + vertex[axis] * vertex[3] for axis in range(3)))
                authored.append(raw)
        indices = triangles(index)
        require(canonical(decoded_triangles(index, info[15:])) == canonical(indices),
                "Decoded native allowed triangles differ from the independent fixed replacements")
        incidence = Counter(tuple(sorted((triangle[i], triangle[(i + 1) % 3])))
                            for triangle in indices for i in range(3))
        require(set(incidence.values()) <= {1, 2}, "Active mesh has nonmanifold edge incidence")
        coverage = 0.0
        for triangle in indices:
            require(all(active(index, vertex) for vertex in triangle), "Active surface uses disabled vertex")
            signed = area(*(authored[vertex] for vertex in triangle))
            require(signed < 0, "Active native stencil contains a degenerate or flipped triangle")
            coverage += abs(signed)
        require(coverage == (high[0] - low[0]) * (high[1] - low[1]), "Active stencil does not cover its entire base rectangle")
        for (a, b), count in incidence.items():
            if count == 1:
                require(any(authored[a][axis] == authored[b][axis] == boundary
                            for axis in range(2) for boundary in (low[axis], high[axis])), "Active mesh contains an interior hole")
        for vertex, (actual, raw) in enumerate(zip(positions, authored, strict=True)):
            correction = height_correction(index, vertex // size, vertex % size) if curved else 0
            expected = (raw[0], raw[1], raw[2] + correction)
            require(abs(height_on_surface(raw, authored, indices) - expected[2]) < 1e-6,
                    "Literal point corrections disagree with the independent covered surface")
            require(math.dist(actual, expected) < 1e-6, f"Native emitted position changed on patch {index}, vertex {vertex}: {actual} != {expected}")
            disabled += not active(index, vertex)
            snapped += math.dist(actual, raw) > 1e-6
        all_positions.append(positions)
        summaries.append({"name": PATCHES[index][0], "active_triangles": len(indices), "coverage": coverage,
                          "positions_sha256": digest(json.dumps(positions).encode()), "mask": list(info[15:])})
        vertex_start += size * size
        triangle_start += 2 * (size - 1) ** 2
    require((disabled, snapped) == (21, 18 if curved else 0), "Actual disabled/snapped counts disagree with frozen arithmetic")
    for a, b in ((0, 0), (1, 2), (2, 4)):
        require(math.dist(all_positions[0][a * 5 + 4], all_positions[1][b * 5]) < 1e-6, "A/B common boundary point differs")
    for a, b in ((0, 0), (1, 4), (2, 8)):
        require(math.dist(all_positions[1][a * 5 + 4], all_positions[2][b * 9]) < 1e-6, "B/C common boundary point differs")
    for row in range(9):
        require(math.dist(all_positions[2][row * 9 + 8], all_positions[3][(8 - row) * 9 + 8]) < 1e-6,
                "C/D rotated full boundary differs")
    if curved:
        require([point[2] for point in all_positions[1][::5]] == [16, 16.5, 17, 18.5, 20], "Literal B-left snapped heights changed")
        require([point[2] for point in all_positions[2][::9]] == [16, 16.0625, 16.125, 16.1875, 16.25, 16.4375, 16.625, 16.8125, 17],
                "Literal C-left propagated heights changed")
    return {"patches": summaries, "disabled": disabled, "snapped": snapped,
            "geometry_sha256": {str(index): digest(payload(data, index)) for index in GEOMETRY_LUMPS}}


def lighting(data):
    infos = records(data, 26, INFO_FORMAT)
    result = {}
    for name, face_lump, color_lump, version, dominant in (("ldr", 7, 8, 0, 0), ("hdr", 58, 53, 1, 1)):
        faces, colors = records(data, face_lump, FACE_FORMAT, expected_version=version), payload(data, color_lump)
        counts = []
        for info in infos:
            face = faces[info[9]]
            count = (face[16] + 1) * (face[17] + 1)
            require(face[8:12] == (0, 255, 255, 255) and face[12] >= 0 and count > 0, "Terrain lacks its ordinary style0 lightmap")
            samples = list(struct.iter_unpack("<3Bb", colors[face[12]:face[12] + count * 4]))
            require(len(samples) == count and any(sample[dominant] > sample[1 - dominant] > 0 for sample in samples)
                    and all(sample[2] == 0 for sample in samples), f"{name} terrain samples lost the contrasting direct-light control")
            counts.append(count)
        result[name] = {"terrain_samples": counts, "lighting_sha256": digest(colors)}
    require(result["ldr"]["lighting_sha256"] != result["hdr"]["lighting_sha256"], "LDR/HDR lighting payloads unexpectedly match")
    return result


def property_draft(client):
    token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
    client.call("update_property_edit", {"token": token, "changes": [
        {"action": "set", "key": "message", "value": "Pending density-chain inspection"}]})
    return token


def preview_control(client, ids, faces, source, game, bsp_paths):
    before, grids = snapshot(client, ids, source, game), queries(client, faces, True)
    outputs = {str(path): fingerprint(path) for path in bsp_paths}
    draft = client.call("preview_terrain_sculpt", {"faces": faces, "mode": "raise", "center": [32, 32, 17],
                                                   "radius": 64, "amount": 2})
    require(draft["faces_changed"] > 0, "Terrain preview control changed no vertices")
    preview = client.call("get_terrain_grids", {"faces": faces, "preview": True})
    require(preview != grids and queries(client, faces, True) == grids, "Preview replaced committed source geometry")
    retained = snapshot(client, ids, source, game)
    client.call("preview_terrain_resolution", {"faces": faces, "power": 5}, error=True)
    require(snapshot(client, ids, source, game) == retained
            and client.call("get_terrain_grids", {"faces": faces, "preview": True}) == preview, "Rejected resolution replaced owned preview")
    client.call("cancel_face_preview", {"token": draft["state"]["preview_token"]})
    require(snapshot(client, ids, source, game) == before and queries(client, faces, True) == grids,
            "Cancel did not restore exact committed state/history")
    require({str(path): fingerprint(path) for path in bsp_paths} == outputs, "Preview/Cancel changed published output")
    return {"changed_faces": draft["faces_changed"], "invalid_request_preserved_owned_preview": True, "cancel_exact": True}


def validate(args):
    client = None
    result = {"status": "failed", "binary": str(args.binary), "binary_sha256": fingerprint(args.binary)["sha256"],
              "script_sha256": fingerprint(Path(__file__))["sha256"], "contract": CONTRACT, "cases": {},
              "helper_sha256": {name: fingerprint(Path(__file__).parent / name)["sha256"] for name in HELPERS},
              "native_worker": False, "game": False, "graphics": False}
    sources = {}
    try:
        game = args.output / "game"
        assets(game)
        temporary = args.output / "tmp"
        temporary.mkdir()
        with patch.dict(os.environ, {"TMPDIR": str(temporary)}):
            client = open_client(args.binary, args.output / "client", game)
        ids, faces, sources = fixture(client, args.output)
        frozen_sources, committed = {}, {}
        for name, curved in (("flat", False), ("curved", True)):
            result["active_case"] = name
            client.call("open_map", {"path": str(sources[name])})
            client.call("save_map", {"path": str(sources[name])})
            frozen_sources[name] = fingerprint(sources[name])
            settings(client, ("fullbright", 0))
            if name == "flat":
                client.call("save_build_recipe", {"name": "Explicit density chains"})
            client.call("select_faces", {"faces": faces})
            committed[name] = queries(client, faces, curved)
            token = property_draft(client)
            data, report = compile_pair(client, args.output, name, ids, sources[name], game)
            report["native"] = native_geometry(data, report["result"]["stats"], curved)
            require(queries(client, faces, curved) == committed[name], "Compile changed committed grid inspection")
            result["cases"][name] = report
            if curved:
                fullbright = data
                result["preview"] = preview_control(client, ids, faces, sources[name], game,
                                                     [Path(report["bsp"]), Path(report["background_bsp"])])
                settings(client, ("direct_ldr_hdr", 0))
                result["active_case"] = "curved-dual"
                data, report = compile_pair(client, args.output, "curved-dual", ids, sources[name], game)
                report["native"] = native_geometry(data, report["result"]["stats"], True)
                require(all(payload(data, index) == payload(fullbright, index) for index in GEOMETRY_LUMPS),
                        "Lighting changed native displacement vertex/tag bytes")
                report["lighting"] = lighting(data)
                result["cases"]["curved-dual"] = report
            client.call("cancel_property_edit", {"token": token})
        result["active_case"] = "curved-reopened"
        require(fingerprint(sources["curved"]) == frozen_sources["curved"], "Curved baseline source changed before reopen")
        client.call("open_map", {"path": str(sources["curved"])})
        settings(client, ("fullbright", 0))
        client.call("select_faces", {"faces": faces})
        require(queries(client, faces, True) == committed["curved"], "Exact saved-source reopen changed committed terrain")
        token = property_draft(client)
        data, report = compile_pair(client, args.output, "curved-reopened", ids, sources["curved"], game)
        require(data == fullbright, "Reopening exact retained source changed the complete BSP")
        report["native"] = native_geometry(data, report["result"]["stats"], True)
        result["cases"]["curved-reopened"] = report
        client.call("cancel_property_edit", {"token": token})
        result["active_case"] = "unsupported-flags"
        invalid = args.output / "unsupported-flags.vmf"
        text, count = re.subn(r'("flags"\s*")0(")', r'\g<1>1\2', sources["curved"].read_text(), count=1)
        require(count == 1, "Unsupported-flags control changed no declaration")
        invalid.write_text(text)
        sources["unsupported-flags"] = invalid
        frozen_sources["unsupported-flags"] = fingerprint(invalid)
        client.call("open_map", {"path": str(invalid)})
        client.call("select_faces", {"faces": faces})
        token = property_draft(client)
        _, result["unsupported_flags"] = compile_pair(client, args.output, "unsupported-flags", ids, invalid, game,
                                                      fullbright, ("Bumped/unknown displacement flags",))
        client.call("cancel_property_edit", {"token": token})
        require({name: fingerprint(path) for name, path in sources.items()} == frozen_sources, "Inspection/compile/reopen rewrote saved sources")
        if args.reference_output is not None:
            reference = json.loads((args.reference_output / "results.json").read_text())
            require(reference["status"] == "accepted" and reference["contract"] == json.loads(json.dumps(CONTRACT))
                    and reference["script_sha256"] == result["script_sha256"]
                    and reference["helper_sha256"] == result["helper_sha256"]
                    and reference["binary_sha256"] != result["binary_sha256"], "Cross-binary reference is not the same accepted frozen oracle")
            require(set(reference["cases"]) == set(result["cases"]), "Cross-binary case set changed")
            for name, report in result["cases"].items():
                previous = Path(reference["cases"][name]["bsp"])
                require(fingerprint(previous)["sha256"] == reference["cases"][name]["bsp_sha256"]
                        and previous.read_bytes() == Path(report["bsp"]).read_bytes(), f"Cross-binary whole BSP differs: {name}")
            result["cross_binary"] = {"reference_output": str(args.reference_output), "whole_bsp_equal": True}
        recipe_path = Path(client.call("get_build_recipes")["path"])
        before_eof = {"sources": frozen_sources, "assets": fingerprints(game), "recipe": fingerprint(recipe_path)}
        result["clean_eof"] = require_clean_eof(client)
        require({name: fingerprint(path) for name, path in sources.items()} == before_eof["sources"]
                and fingerprints(game) == before_eof["assets"], "EOF changed authored sources/assets")
        require(fingerprint(recipe_path) == before_eof["recipe"], "EOF changed saved recipes")
        result["status"] = "accepted"
        result.pop("active_case", None)
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        try:
            if client is not None and client.process is not None:
                require_clean_eof(client)
        except Exception as error:
            result["cleanup_error"] = f"{type(error).__name__}: {error}"
            result["status"] = "failed"
            raise
        finally:
            (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description="Validate the frozen explicit terrain density chain through both public MCP compile paths without Subdivide, graphics or native/game execution.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--reference-output", type=Path)
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(), args.output.resolve()
    if args.reference_output is not None:
        args.reference_output = args.reference_output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Artifacts must stay on disk under target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    result = validate(args)
    print(json.dumps({"status": result["status"], "cases": len(result["cases"]), "output": str(args.output)}))


if __name__ == "__main__":
    main()
