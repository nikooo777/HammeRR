import argparse
import hashlib
import itertools
import json
import math
import os
import re
from pathlib import Path
from unittest.mock import patch

from validate_authored_planes import records
from validate_brush_instances import source_state
from validate_compile_jobs import stopped_on_exit
from validate_direct_lighting import FACE_FORMAT
from validate_face_lighting import DEFAULT_BINARY, ROOT, detail
from validate_instance_contents import refresh
from validate_material_albedo import (
    compile_pair,
    digest,
    fixture_texture,
    floor_base,
    identities,
    open_client,
    payload,
    require,
    settings,
    state,
)
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM

WORLD = ([-8, -6, -4], [4, 10, 12])
DETAIL = ([12, -4, 0], [20, 4, 8])
WORLD_B = ([-8, -4, -4], [4, 4, 12])
DETAIL_B = ([12, -3, 0], [20, 3, 8])
MATERIALS = ("hammerr690/world", "hammerr690/detail")
FULL = ("fullbright", 0)
LIT = ("direct_ldr_hdr", 1)
IDENTITY = ((1, 0, 0), (0, 1, 0), (0, 0, 1))
QUARTER = ((0, -1, 0), (0, 0, 1), (-1, 0, 0))
QUARTER_OFFSET = (86.375, 20.875, 20.1875)
U = (1, 0.5, 0)
V = (0, 1, 0.25)
SCALES = (-0.25, 0.5)
SHIFTS = (7.5, -3.25)
DETAIL_CONTENTS = 0x08000000
NORMAL_ERROR = 0.00001
POSITION_ERROR = 0.002
UV_ERROR = 0.004
OLD_BINARY_SHA = "b2fd516891fc649aa36fb1bd6a81850318c48ae87b31a172a317b2fb573b3d8e"
ERROR_TOKENS = {"depth5": "4 helper levels", "occurrence257": "256", "ancestor-hardlink": "cycle",
                "symlink": "symlink", "traversal": "relative", "missing": "missing.vmf",
                "malformed-nested": "angles", "runtime-child": "info_target", "intermediate-bounds": "bounds", "root-physical-cycle": "cycle"}


def dot(a, b):
    return sum(x * y for x, y in zip(a, b, strict=True))


def vector(matrix, point):
    return tuple(dot(row, point) for row in matrix)


def point(matrix, offset, value):
    return tuple(a + b for a, b in zip(vector(matrix, value), offset, strict=True))


def multiply(a, b):
    return tuple(tuple(dot(row, column) for column in zip(*b, strict=True)) for row in a)


def corners(bounds):
    return list(itertools.product(*zip(*bounds, strict=True)))


def assets(game):
    folder = game / "materials/hammerr690"
    folder.mkdir(parents=True)
    (game / "maps").mkdir()
    (game / "gameinfo.txt").write_text('GameInfo { FileSystem { SearchPaths { Game "|gameinfo_path|." } } }\n')
    (folder / "base.vtf").write_bytes(fixture_texture(2, 2, (0.5, 0.25, 0.125), (160, 96, 48, 255)))
    for name in ("world", "detail", "world-b", "detail-b"):
        (folder / f"{name}.vmt").write_text('LightmappedGeneric { $basetexture "hammerr690/base" }\n')
    (game.parent / "cstrike_linux64").write_text("Private non-executable installation placeholder; no game is present.\n")


def block(client, bounds, material, projection=True):
    identifier = client.call("create_block", {"mins": bounds[0], "maxs": bounds[1], "material": material})["id"]
    if projection:
        brush = detail(client, [identifier])[0]
        client.call("edit_faces", {"faces": [{"solid": brush["solid"], "side": face["side"]} for face in brush["faces"]],
                                  "u_axis": U, "v_axis": V, "scale_u": SCALES[0], "scale_v": SCALES[1],
                                  "shift_u": SHIFTS[0], "shift_v": SHIFTS[1], "lightmap_scale": 64})
    return identifier


def leaf(client, path, variant=False):
    client.call("new_map", {"discard_changes": True})
    materials = tuple(name + ("-b" if variant else "") for name in MATERIALS)
    first = block(client, WORLD_B if variant else WORLD, materials[0])
    second = block(client, DETAIL_B if variant else DETAIL, materials[1])
    before = detail(client, [first, second])
    owner = client.call("tie_to_entity", {"ids": [second], "classname": "func_detail"})["id"]
    client.call("save_map", {"path": str(path)})
    result = {"world": {"solid": before[0]["solid"], "sides": [face["side"] for face in before[0]["faces"]]},
              "detail": {"solid": before[1]["solid"], "sides": [face["side"] for face in before[1]["faces"]]}, "owner": owner}
    raw_solids = fixture_blocks(path.read_text(), "solid")
    raw = [{"solid": int(re.search(r'"id"\s+"(\d+)"', solid)[1]),
            "sides": [int(re.search(r'"id"\s+"(\d+)"', side)[1]) for side in fixture_blocks(solid, "side")]}
           for solid in raw_solids]
    require(raw == [result["world"], result["detail"]], "Saved literal leaf IDs differ from authored brush/side IDs")
    return result


def helpers_file(path, helpers=(), hidden=(), extra=""):
    def entity(index, values):
        fields = {"id": str(index), "classname": "func_instance", **values}
        return "entity\n{\n" + "".join(f'"{key}" "{value}"\n' for key, value in fields.items()) + "}\n"

    content = 'world\n{\n"id" "1"\n"classname" "worldspawn"\n}\n'
    content += "".join(entity(index, values) for index, values in enumerate(helpers, 2))
    content += "".join("hidden\n{\n" + entity(index, values) + "}\n" for index, values in enumerate(hidden, len(helpers) + 2))
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content + extra)


def room(client, material=MATERIALS[0]):
    ids = [block(client, bounds, material, False) for bounds in ROOM]
    for identifier in ids:
        brush = detail(client, [identifier])[0]
        client.call("edit_faces", {"faces": [{"solid": brush["solid"], "side": face["side"]} for face in brush["faces"]], "lightmap_scale": 64})
    ids.append(client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-192, -192, 8]})["id"])
    ids.append(client.call("create_entity", {"classname": "light", "origin": [-64, -96, 192], "properties": {
        "_light": "255 192 96 128", "_lightHDR": "96 192 255 192", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0",
    }})["id"])
    return ids


def root_map(client, path, file=None, origin=(0, 0, 0), angles="0 0 0", sealed=True, material=MATERIALS[0]):
    client.call("new_map", {"discard_changes": True})
    ids = room(client, material) if sealed else []
    owner = None
    if file is not None:
        owner = client.call("create_entity", {"classname": "func_instance", "origin": origin, "properties": {"file": file, "angles": angles}})["id"]
        ids.append(owner)
    client.call("select", {"ids": [owner] if owner else ids[:1]})
    client.call("save_map", {"path": str(path)})
    return ids, owner


def transform_source(text, matrix, offset):
    def plane(match):
        values = [tuple(float(value) for value in row.split()) for row in re.findall(r"\(([^)]*)\)", match[1])]
        require(len(values) == 3 and all(len(value) == 3 for value in values), "Independent flat source has an invalid plane")
        return '"plane" "' + " ".join("(" + " ".join(format(value, ".17g") for value in point(matrix, offset, original)) + ")" for original in values) + '"'

    def axis(match):
        values = [float(value) for value in match[2].split()]
        require(len(values) == 4, "Independent flat source has an invalid UV row")
        rotated = vector(matrix, values[:3])
        scale = float(match[3])
        shift = values[3] - dot(offset, rotated) / scale
        return f'"{match[1]}" "[' + " ".join(format(value, ".17g") for value in (*rotated, shift)) + f'] {scale:.17g}"'

    text, planes = re.subn(r'"plane"\s+"([^"]+)"', plane, text)
    text, axes = re.subn(r'"(uaxis|vaxis)"\s+"\[([^]]+)\]\s+([^"]+)"', axis, text)
    require(planes == 12 and axes == 24, "Independent leaf transform did not cover both complete solids")
    return text


def flat_map(client, path, source, matrix, offset, material=MATERIALS[0]):
    transformed = transform_source(source, matrix, offset)
    world = fixture_blocks(transformed, "world")
    require(len(world) == 1, "Independent leaf must have one world")
    solids = fixture_blocks(world[0], "solid")
    entities = fixture_blocks(transformed, "entity")
    require(len(solids) == len(entities) == 1 and '"func_detail"' in entities[0], "Independent leaf must contain one world solid and one detail")
    remapped = [re.sub(r'"id"\s+"(\d+)"', lambda match: f'"id" "{int(match[1]) + 100000}"', text) for text in (*solids, *entities)]
    client.call("new_map", {"discard_changes": True})
    room(client, material)
    client.call("save_map", {"path": str(path)})
    original = path.read_text()
    root_world = fixture_blocks(original, "world")
    require(len(root_world) == 1 and original.count(root_world[0]) == 1, "Independent flat root world is ambiguous")
    replacement = root_world[0][:-1] + "\n" + remapped[0] + "\n}"
    path.write_text(original.replace(root_world[0], replacement) + "\n" + remapped[1] + "\n")
    client.call("open_map", {"path": str(path), "discard_changes": True})
    ids = [row["id"] for row in client.call("find_objects", {"limit": 500})["objects"]]
    client.call("select", {"ids": ids[:1]})
    return ids


def fixture_blocks(text, name):
    result = []
    for match in re.finditer(r"(?m)^\s*" + re.escape(name) + r"\s*\n\s*\{", text):
        depth, quoted = 0, False
        for index in range(text.index("{", match.start()), len(text)):
            character = text[index]
            if character == '"' and (index == 0 or text[index - 1] != "\\"):
                quoted = not quoted
            if quoted:
                continue
            if character == "{":
                depth += 1
            elif character == "}":
                depth -= 1
                if depth == 0:
                    result.append(text[match.start():index + 1])
                    break
        else:
            raise AssertionError("Unbalanced generated fixture block")
    return result


def setup(client, source):
    settings(client, FULL)
    if not any(row["name"] == "Nested brush instance oracle" for row in client.call("get_build_recipes")["recipes"]):
        client.call("save_build_recipe", {"name": "Nested brush instance oracle"})
    require(not client.call("get_map_info")["rendering"]["initialized"], "Fixture initialized graphics")
    return source.read_bytes()


def pair(client, output, label, ids, source, game, mode=FULL, failure=False, retained=None):
    settings(client, mode)
    lin = b"13 17 19\n23 29 31\n"
    for suffix in ("", "-background"):
        (output / f"{label}{suffix}.lin").write_bytes(lin)
    with stopped_on_exit(client):
        data, receipt = compile_pair(client, output, label, ids, source, game, "" if failure else None, retained)
    require(all((output / f"{label}{suffix}.lin").read_bytes() == lin for suffix in ("", "-background")), f"{label}: replaced prior LIN")
    receipt["source_sha256"] = digest(source.read_bytes())
    if failure:
        require(all(any(row["level"] == "error" and row["object"] in ids and ERROR_TOKENS[label] in row["message"].lower() for row in result["diagnostics"])
                    for result in (receipt["result"], receipt["background"]["result"])), f"{label}: missing selectable root diagnostic")
    return data, receipt


def stats(receipt, expected, base, depth):
    for result in (receipt["result"], receipt["background"]["result"]):
        actual = result["stats"]["instances"]
        require({key: actual[key] for key in expected} == expected, f"Instance counts differ: {actual}")
        require(actual["max_expansion_depth"] == depth, f"Wrong expansion depth: {actual}")
        paths = [row["path"] for row in actual["dependencies"]]
        require(paths == sorted(paths) and len(paths) == expected["files"], "Dependencies do not count normalized requested paths")
        require(actual["dependencies"] == [{"path": path, "bytes": len((base / path).read_bytes()), "sha256": digest((base / path).read_bytes())} for path in paths], "Dependency identity differs from actual input bytes")


def native_brushes(data):
    planes = records(data, 1, "<4fi")
    sides = records(data, 19, "<Hhhh")
    texinfos = records(data, 6, "<16f2i")
    result = []
    for first, count, contents in records(data, 18, "<3i"):
        result.append((contents, sorted((tuple(planes[side[0]][:4]), tuple(texinfos[side[1]][:8])) for side in sides[first:first + count] if side[1] >= 0)))
    return sorted(result)


def native_equivalent(actual, reference, exact=False):
    left, right = native_brushes(actual), native_brushes(reference)
    require(len(left) == len(right), "Flattened native brush count differs")
    if exact:
        require(left == right, "Translation-only native planes/UV/DETAIL differ from manual flat control")
    else:
        unmatched = list(right)
        for contents, sides in left:
            matches = []
            for index, (other_contents, other_sides) in enumerate(unmatched):
                if contents != other_contents or len(sides) != len(other_sides):
                    continue
                remaining = list(other_sides)
                for plane, uv in sides:
                    candidates = [slot for slot, (other_plane, other_uv) in enumerate(remaining)
                                  if all(abs(a - b) <= NORMAL_ERROR for a, b in zip(plane[:3], other_plane[:3], strict=True))
                                  and abs(plane[3] - other_plane[3]) <= POSITION_ERROR
                                  and all(abs(a - b) <= UV_ERROR for a, b in zip(uv, other_uv, strict=True))]
                    if not candidates:
                        break
                    remaining.pop(candidates[0])
                else:
                    matches.append(index)
            require(len(matches) == 1, "Manual flat plane/UV/DETAIL brush match is missing or ambiguous")
            unmatched.pop(matches[0])
    require(sum(bool(contents & DETAIL_CONTENTS) for contents, _sides in left) == 1, "Ordinary detail ownership changed")
    return {"brushes": len(left), "detail_brushes": 1, "native_planes_uv_match": True, "exact": exact}


def literal_surface_check(data, matrix, offset):
    vertices = records(data, 3, "<3f")
    expected = [point(matrix, offset, value) for bounds in (WORLD, DETAIL) for value in corners(bounds)]
    observed = []
    for wanted in expected:
        nearest = min(vertices, key=lambda vertex: max(abs(a - b) for a, b in zip(vertex, wanted, strict=True)))
        require(max(abs(a - b) for a, b in zip(nearest, wanted, strict=True)) <= POSITION_ERROR, "An independently transformed corner is absent from native vertices")
        observed.append(nearest)
    u, v = vector(matrix, U), vector(matrix, V)
    rows = (tuple(value / SCALES[0] for value in u) + (SHIFTS[0] - dot(offset, u) / SCALES[0],),
            tuple(value / SCALES[1] for value in v) + (SHIFTS[1] - dot(offset, v) / SCALES[1],))
    texinfos = records(data, 6, "<16f2i")
    candidates = [info for info in texinfos if all(abs(a - b) <= UV_ERROR for a, b in zip(info[:8], (*rows[0], *rows[1]), strict=True))]
    require(candidates, "Independent attached native UV rows are absent")
    original_points = [value for bounds in (WORLD, DETAIL) for value in corners(bounds)]
    require(any(all(abs(dot(original, axis) / scale + shift - dot(stored, row[:3]) - row[3]) <= UV_ERROR
                        for original, stored in zip(original_points, observed, strict=True)
                        for axis, scale, shift, row in zip((U, V), SCALES, SHIFTS, (info[:4], info[4:8]), strict=True))
                for info in candidates), "Actual native texel UVs are detached from original source corners")
    return {"corners": expected, "observed_corners": observed, "native_texel_rows": rows, "position_error_hu": POSITION_ERROR, "uv_error_texels": UV_ERROR}


def face_material(data, index):
    face = records(data, 7, FACE_FORMAT)[index]
    info = records(data, 6, "<16f2i")[face[5]]
    texture = records(data, 2, "<3f5i")[info[-1]]
    start = records(data, 44, "<i")[texture[3]][0]
    strings = payload(data, 43)
    end = strings.find(b"\0", start)
    require(0 <= start < end <= len(strings), "Compiled source material name is invalid")
    return strings[start:end].decode("ascii").lower()


def run_controls(client, output, game):
    source = output / "controls-parent.vmf"
    ids, _owner = root_map(client, source)
    setup(client, source)
    no_instance, first = pair(client, output, "control-no-instance", ids, source, game)
    require("instances" not in first["result"]["stats"], "No-instance build has optional instance metadata")
    no_instance_source = output / "control-no-instance.vmf"
    no_instance_source.write_bytes(source.read_bytes())
    ids, owner = root_map(client, source, "leaf.vmf", (24, 16, 64))
    setup(client, source)
    one_level, second = pair(client, output, "control-one-level", ids, source, game)
    counts = second["result"]["stats"]["instances"]
    require([counts[key] for key in ("instances", "files", "world_brushes", "detail_brushes")] == [1, 1, 1, 1], "One-level baseline differs")
    one_level_source = output / "control-one-level.vmf"
    one_level_source.write_bytes(source.read_bytes())
    return {"no_instance": {"source": str(no_instance_source), "source_sha256": digest(no_instance_source.read_bytes()), "bsp_sha256": digest(no_instance), "receipt": first},
            "one_level": {"source": str(one_level_source), "source_sha256": digest(one_level_source.read_bytes()), "bsp_sha256": digest(one_level), "receipt": second, "owner": owner}}


def retained_controls(client, output, game, path):
    old = json.loads(path.read_text())
    require(old["status"] == "frozen_old_controls" and old["binary_sha256"] == OLD_BINARY_SHA, "Controls are not an accepted pre-nesting receipt")
    old_leaf = Path(old["controls"]["one_level"]["source"]).parent / "leaf.vmf"
    require(digest(old_leaf.read_bytes()) == old["leaf_sha256"], "Retained one-level leaf bytes changed")
    result = {}
    for name, control in old["controls"].items():
        source = Path(control["source"])
        require(digest(source.read_bytes()) == control["source_sha256"], "Retained control source changed")
        client.call("open_map", {"path": str(source), "discard_changes": True})
        ids = [row["id"] for row in client.call("find_objects", {"limit": 500})["objects"]]
        client.call("select", {"ids": ids[:1]})
        setup(client, source)
        data, receipt = pair(client, output, "retained-" + name, ids, source, game)
        require(digest(data) == control["bsp_sha256"], f"{name}: existing BSP bytes changed")
        if name == "no_instance":
            require("instances" not in receipt["result"]["stats"] and "instance_dependencies" not in client.call("get_compile_status")["job"], "No-instance metadata changed")
        else:
            require(receipt["result"]["stats"]["instances"]["max_expansion_depth"] == 1, "One-level depth differs")
        result[name] = receipt
    require(set(result) == {"no_instance", "one_level"}, "Old controls are incomplete")
    return result


def inputs(client, output):
    first = leaf(client, output / "leaf.vmf")
    second = leaf(client, output / "leaf-b.vmf", True)
    require({key: first[key] for key in ("world", "detail")} == {key: second[key] for key in ("world", "detail")}, "Distinct leaves must genuinely reuse original solid/side IDs")
    helpers_file(output / "modules/middle.vmf", [{"file": "leaf.vmf", "origin": "4.5 -6.25 12.125", "angles": "90 0 0"}])
    (output / "modules/leaf.vmf").write_text((output / "leaf-b.vmf").read_text())
    helpers_file(output / "modules/left.vmf", [{"file": "leaf.vmf"}])
    helpers_file(output / "modules/right.vmf", [{"file": "leaf.vmf"}])
    helpers_file(output / "modules/diamond.vmf", [
        {"file": "modules/left.vmf", "origin": "-96 0 0"},
        {"file": "modules/right.vmf"}, {"file": "leaf-b.vmf", "origin": "96 0 0"},
    ])
    os.link(output / "leaf.vmf", output / "leaf-alias.vmf")
    helpers_file(output / "aliases.vmf", [{"file": "leaf.vmf", "origin": "-64 0 64"}, {"file": "leaf-alias.vmf", "origin": "64 0 64"}])
    helpers_file(output / "empty.vmf")
    helpers_file(output / "fan256.vmf", [{"file": "empty.vmf"}] * 255)
    helpers_file(output / "fan257.vmf", [{"file": "empty.vmf"}] * 256)
    for level in (1, 2, 3):
        helpers_file(output / f"level{level}.vmf", [{"file": f"level{level + 1}.vmf" if level < 3 else "leaf.vmf"}])
    helpers_file(output / "level0.vmf", [{"file": "level1.vmf"}])
    helpers_file(output / "cycle.vmf", [{"file": "cycle-alias.vmf"}])
    os.link(output / "cycle.vmf", output / "cycle-alias.vmf")
    (output / "leaf-link.vmf").symlink_to("leaf.vmf")
    helpers_file(output / "bad-transform.vmf", [{"file": "leaf.vmf", "angles": "bad"}])
    helpers_file(output / "intermediate.vmf", [{"file": "leaf.vmf", "origin": "16380 0 0"}])
    helpers_file(output / "runtime.vmf", extra='entity { "id" "2" "classname" "info_target" "origin" "0 0 64" }\n')
    helpers_file(output / "hidden.vmf", [{"file": "leaf.vmf", "origin": "0 0 64"}], [{"file": "missing-hidden.vmf", "angles": "bad"}])
    return first


def preview(client, output, source, leaf_ids):
    ids, owner = root_map(client, source, "modules\\diamond.vmf", (0, 0, 64), sealed=False)
    original = source_state(client, source)
    camera = client.call("get_camera")
    initial = client.call("inspect_instance_contents")
    require(not initial["available"] and not client.call("get_map_info")["rendering"]["initialized"], "Preview loaded implicitly")
    ready = refresh(client, background=True)
    require(ready["limits"]["depth"] == 32 and ready["limits"]["expansion_depth"] == 4, "Syntax and expansion limits were conflated")
    require(ready["stats"]["max_expansion_depth"] == 3 and ready["stats"]["instances"] == 6, "Preview graph counts differ")
    require(ready["total"] == 1 and ready["owners"][0]["owner"] == owner and ready["owners"][0]["file"] == "modules/diamond.vmf", "Preview root reference was overwritten by a leaf")
    require(ready["owners"][0]["bounds"] == [[-104, -6, 60], [116, 10, 76]], "Whole nested owner bounds differ")
    require(ready["brushes_total"] == 6 and ready["face_provenance_total"] == 36 and not ready["brushes_truncated"] and not ready["face_provenance_truncated"], "Preview provenance was lost/truncated")
    for row in ready["brushes"]:
        role = "detail" if row["detail"] else "world"
        require(row["owner"] == owner and row["file"] in ("leaf.vmf", "leaf-b.vmf") and row["child_brush"] == leaf_ids[role]["solid"], "Preview brush source identity differs")
    require(sum(row["file"] == "leaf.vmf" for row in ready["brushes"]) == 4 and sum(row["file"] == "leaf-b.vmf" for row in ready["brushes"]) == 2, "Shared-leaf diamond identity differs")
    expected_brushes = [
        ("leaf.vmf", False, [[-104, -6, 60], [-92, 10, 76]]),
        ("leaf.vmf", True, [[-84, -4, 64], [-76, 4, 72]]),
        ("leaf.vmf", False, [[-8, -6, 60], [4, 10, 76]]),
        ("leaf.vmf", True, [[12, -4, 64], [20, 4, 72]]),
        ("leaf-b.vmf", False, [[88, -4, 60], [100, 4, 76]]),
        ("leaf-b.vmf", True, [[108, -3, 64], [116, 3, 72]]),
    ]
    require(sorted((row["file"], row["detail"], row["bounds"]) for row in ready["brushes"]) == sorted(expected_brushes), "Expanded preview leaf bounds differ from literal placements")
    for row in ready["face_provenance"]:
        require(row["owner"] == owner and row["file"] in ("leaf.vmf", "leaf-b.vmf")
                and any(row["child_brush"] == leaf_ids[role]["solid"] and row["child_side"] in leaf_ids[role]["sides"] for role in ("world", "detail")), "Preview face provenance differs")
    hit = client.call("pick_at_pixel", {"x": 256, "y": 256, "width": 513, "height": 513,
                                       "camera": {"projection": "top", "center": [-98, 0, 0], "ortho_height": 64}, "instance_contents": True})
    require(hit["object"] == owner and hit["solid"] is None and hit["side"] is None and abs(hit["point"][2] - 76) <= POSITION_ERROR, "Nested picking did not expose only the root owner")
    client.call("set_instance_contents", {"action": "frame", "token": str(ready["token"]), "ids": [owner], "aspect": 1.5})
    require(client.call("get_camera") != camera, "Explicit root framing did not move the camera")
    client.call("set_camera", camera)
    require(source_state(client, source) == original, "Refresh/pick/frame changed source/history/selection")
    return {"ready": ready, "pick": hit, "source_preserved": True}, owner, ids


def preview_stale(client, output, source, owner):
    original = source_state(client, source)
    camera = client.call("get_camera")
    target = output / "leaf.vmf"
    saved = target.read_bytes()
    ready = client.call("inspect_instance_contents")
    try:
        target.write_bytes(saved.replace(b"\n", b" ", 1))
        stale = client.call("inspect_instance_contents")
        require(stale["state"] == "stale" and not stale["available"] and not stale["displayed"], "Grandchild mutation kept preview available")
        hit = client.call("pick_at_pixel", {"x": 256, "y": 256, "width": 513, "height": 513,
                                           "camera": {"projection": "top", "center": [-98, 0, 0], "ortho_height": 64}, "instance_contents": True})
        require(hit == {"hit": None}, "Stale child geometry remained pickable")
        refused = client.call("set_instance_contents", {"action": "frame", "token": str(ready["token"]), "ids": [owner]}, error=True)
        require(client.call("get_camera") == camera, "Failed stale framing changed the camera")
        target.write_bytes(b"world {\n")
        failed = client.call("set_instance_contents", {"action": "refresh"})
        require(failed["state"] == "failed" and not failed["available"] and not failed["displayed"], "Malformed grandchild Refresh kept a partial diamond")
        require(source_state(client, source) == original, "Stale picking/framing/failed Refresh changed root state")
        return {"stale": stale, "stale_pick": hit, "frame_refusal": refused, "failed_refresh": failed}
    finally:
        target.write_bytes(saved)


def transitive_stale(client, output, source, ids, game):
    results = {}
    for name in ("same-length-leaf", "same-bytes-inode", "missing-middle", "path-rebound"):
        data, built = pair(client, output, "before-" + name, ids, source, game)
        ready = refresh(client)
        target = output / ("modules/left.vmf" if name == "missing-middle" else "leaf.vmf")
        original = target.read_bytes()
        backup = output / ("saved-" + name + ".vmf")
        if name == "same-length-leaf":
            backup.write_bytes(original)
            replacement = original.replace(b"\n", b" ", 1)
            require(replacement != original and len(replacement) == len(original), "Leaf mutation is not same-length")
            target.write_bytes(replacement)
        else:
            target.rename(backup)
            if name == "same-bytes-inode":
                target.write_bytes(original)
            elif name == "path-rebound":
                target.symlink_to(backup.name)
        try:
            before = state(client, ids, source, game)
            status = client.call("get_compile_status")
            baked = client.call("inspect_baked_lightmaps")
            stale = client.call("inspect_instance_contents")
            require(status["job"]["stale"] and not status["job"]["instance_dependencies"]["current"] and not status["launch"]["ready"], "Changed grandchild did not stale completed/publication state")
            require(baked["build"]["status"] == "stale" and not baked["build"]["current"], "Changed grandchild did not stale baked origins")
            require(stale["state"] == "stale" and not stale["available"] and not stale["displayed"], "Changed grandchild left a partial reference layer")
            rejected = client.call("launch_game", error=True)
            require("Instance source/dependencies changed" in json.dumps(rejected), "Stale launch did not refuse at dependency gate")
            require(state(client, ids, source, game) == before and not client.call("get_map_info")["game_running"], "Stale inspection/launch changed protected state")
            require((output / ("before-" + name + ".bsp")).read_bytes() == data, "Staleness replaced an existing BSP")
            results[name] = {"build": built, "preview_token": ready["token"], "status": status, "baked": baked, "preview": stale, "launch_refusal": rejected}
        finally:
            if name == "same-length-leaf":
                target.write_bytes(original)
                backup.unlink()
            else:
                if target.is_symlink() or target.exists():
                    target.unlink()
                backup.rename(target)
    return results


def discover(client):
    definitions = {tool["name"]: tool for tool in client.rpc("tools/list", {})["tools"]}
    expected = {"compile_map": ("nested", "four helper levels", "root-relative"),
                "start_compile": ("nested", "four helper levels", "root-relative"),
                "inspect_instance_contents": ("nested", "leaf source")}
    for name, tokens in expected.items():
        description = definitions[name].get("description", "").lower()
        require(all(token in description for token in tokens), f"Actual {name} discovery does not describe nested admission")
    return {name: definitions[name] for name in expected}


def validate(args):
    client = None
    physics_client = None
    game = args.output / "game"
    source = args.output / "parent.vmf"
    result = {"status": "failed", "binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()),
              "script_sha256": digest(Path(__file__).read_bytes()), "helpers_sha256": digest((ROOT / "scripts/validate_material_albedo.py").read_bytes()),
              "physics_exercised": False, "native_game_exercised": False, "positive": {}, "negative": {}}
    try:
        assets(game)
        temporary = args.output / "tmp"
        temporary.mkdir()
        with patch.dict(os.environ, {"TMPDIR": str(temporary)}):
            client = open_client(args.binary, args.output / "client", game)
        result["discovery"] = discover(client)
        leaf_ids = inputs(client, args.output)
        result["leaf_ids"] = leaf_ids
        leaf_source = (args.output / "leaf.vmf").read_text()
        result["retained_controls"] = retained_controls(client, args.output, game, args.controls)
        cases = [
            ("quarter", "modules/middle.vmf", (80.125, 16.375, 8.0625), "0 90 0", QUARTER, QUARTER_OFFSET),
            ("translation", "translated.vmf", (16, 8, 32), "0 0 0", IDENTITY, (24, 16, 64)),
        ]
        helpers_file(args.output / "translated.vmf", [{"file": "leaf.vmf", "origin": "8 8 32"}])
        h = math.sqrt(0.5)
        inner = ((0.5, -0.5 - h / 2, 0.5 - h / 2), (0.5, 0.5 - h / 2, -0.5 - h / 2), (h, 0.5, 0.5))
        outer = ((0.5, h / 2 - 0.5, h / 2 + 0.5), (0.5, h / 2 + 0.5, h / 2 - 0.5), (-h, 0.5, 0.5))
        mixed = multiply(outer, inner)
        mixed_offset = point(outer, (40.125, 16.375, 64.0625), (4.5, -6.25, 12.125))
        helpers_file(args.output / "mixed.vmf", [{"file": "leaf.vmf", "origin": "4.5 -6.25 12.125", "angles": "-45 45 45"}])
        cases.append(("mixed", "mixed.vmf", (40.125, 16.375, 64.0625), "45 45 45", mixed, mixed_offset))
        retained = None
        for label, file, origin, angles, matrix, offset in cases:
            source = args.output / f"{label}-parent.vmf"
            ids, owner = root_map(client, source, file, origin, angles)
            setup(client, source)
            data, receipt = pair(client, args.output, label, ids, source, game)
            stats(receipt, {"instances": 2, "files": 2, "world_brushes": 1, "detail_brushes": 1}, args.output, 2)
            require("modules/leaf.vmf" not in [entry["path"] for entry in receipt["result"]["stats"]["instances"]["dependencies"]], "Nested lookup used the child's directory")
            receipt["literal_surfaces"] = literal_surface_check(data, matrix, offset)
            flat = args.output / f"{label}-flat.vmf"
            flat_ids = flat_map(client, flat, leaf_source, matrix, offset)
            setup(client, flat)
            flat_data, flat_receipt = pair(client, args.output, label + "-flat", flat_ids, flat, game)
            receipt["flat_equivalence"] = native_equivalent(data, flat_data, label == "translation")
            if label == "translation":
                require(digest(data) == result["retained_controls"]["one_level"]["bsp_sha256"], "Nested identity translations changed the retained one-level BSP bytes")
                receipt["retained_one_level_bsp_exact"] = True
            receipt["flat"] = flat_receipt
            result["positive"][label] = receipt
            retained = data
        source = args.output / "diamond-parent.vmf"
        ids, owner = root_map(client, source, "modules/diamond.vmf", (0, 0, 64))
        setup(client, source)
        data, receipt = pair(client, args.output, "diamond", ids, source, game)
        stats(receipt, {"instances": 6, "files": 5, "world_brushes": 3, "detail_brushes": 3}, args.output, 3)
        require(sum(bool(row[2] & DETAIL_CONTENTS) for row in records(data, 18, "<3i")) == 3, "Diamond lost detail ownership")
        queries = {}
        for file, minimum in (("leaf.vmf", 2), ("leaf-b.vmf", 1)):
            query = {"kind": "instance", "instance": int(owner.split(":")[1]), "file": file, "solid": leaf_ids["world"]["solid"], "side": leaf_ids["world"]["sides"][0]}
            inspection = client.call("inspect_baked_lightmaps", {"source": query, "limit": 32})
            require(inspection["mapping"]["match_page"]["total"] >= minimum and inspection["mapping"]["source"] == query, "Grouped leaf origin matches were aliased or lost")
            require(query in inspection["mapping"]["contributors"], "Selected compiled face lacks its exact root-owner/leaf/source-ID contributor")
            expected_material = MATERIALS[0] + ("-b" if file == "leaf-b.vmf" else "")
            require(all(face_material(data, row["face"]) == expected_material for row in inspection["mapping"]["matches"]), "Same-ID leaf query selected another file's compiled surface")
            queries[file] = inspection
        receipt["origins"] = queries
        result["positive"]["diamond"] = receipt
        result["transitive_stale"] = transitive_stale(client, args.output, source, ids, game)
        source = args.output / "aliases-parent.vmf"
        ids, owner = root_map(client, source, "aliases.vmf")
        setup(client, source)
        _actual, alias_receipt = pair(client, args.output, "physical-alias-siblings", ids, source, game)
        stats(alias_receipt, {"instances": 3, "files": 3, "world_brushes": 2, "detail_brushes": 2}, args.output, 2)
        require([row["path"] for row in alias_receipt["result"]["stats"]["instances"]["dependencies"]] == ["aliases.vmf", "leaf-alias.vmf", "leaf.vmf"], "Requested hard-link aliases were collapsed")
        result["positive"]["physical-alias-siblings"] = alias_receipt
        physical = args.output / "physical"
        physical.mkdir()
        physical_root = physical / "root.vmf"
        ids, owner = root_map(client, physical_root, "modules/middle.vmf", (80.125, 16.375, 8.0625), "0 90 0")
        referring = args.output / "referring-parent.vmf"
        referring.symlink_to("physical/root.vmf")
        client.call("open_map", {"path": str(referring), "discard_changes": True})
        client.call("select", {"ids": [owner]})
        setup(client, referring)
        alias_data, alias_receipt = pair(client, args.output, "saved-root-symlink", ids, referring, game)
        stats(alias_receipt, {"instances": 2, "files": 2, "world_brushes": 1, "detail_brushes": 1}, args.output, 2)
        alias_receipt["literal_surfaces"] = literal_surface_check(alias_data, QUARTER, QUARTER_OFFSET)
        result["positive"]["saved-root-symlink"] = alias_receipt
        for label, file, depth, occurrences, files in (("depth4", "level1.vmf", 4, 4, 4), ("occurrence256", "fan256.vmf", 2, 256, 2), ("hidden-unread", "hidden.vmf", 2, 2, 2)):
            source = args.output / f"{label}-parent.vmf"
            ids, owner = root_map(client, source, file)
            setup(client, source)
            actual, receipt = pair(client, args.output, label, ids, source, game)
            brushes = 0 if label == "occurrence256" else 1
            stats(receipt, {"instances": occurrences, "files": files, "world_brushes": brushes, "detail_brushes": brushes}, args.output, depth)
            result["positive"][label] = receipt
        source = args.output / "lit-parent.vmf"
        ids, owner = root_map(client, source, "modules/middle.vmf", (80.125, 16.375, 8.0625), "0 90 0")
        setup(client, source)
        lit, receipt = pair(client, args.output, "lit-quarter", ids, source, game, LIT)
        flat = args.output / "lit-flat.vmf"
        flat_ids = flat_map(client, flat, leaf_source, QUARTER, QUARTER_OFFSET)
        setup(client, flat)
        flat_lit, flat_receipt = pair(client, args.output, "lit-flat", flat_ids, flat, game, LIT)
        receipt["flat_equivalence"] = native_equivalent(lit, flat_lit)
        receipt["channels"] = {}
        for channel in ("ldr", "hdr"):
            samples, comparison = floor_base(lit, channel), floor_base(flat_lit, channel)
            mean = [sum(row[index] for row in samples) / len(samples) for index in range(3)]
            other = [sum(row[index] for row in comparison) / len(comparison) for index in range(3)]
            require(all(abs(a - b) <= 1 + 0.05 * max(a, b) for a, b in zip(mean, other, strict=True)), "Flattened floor lighting exceeds the predeclared quantized mean bound")
            receipt["channels"][channel] = {"mean": mean, "manual_mean": other, "samples": len(samples), "manual_samples": len(comparison)}
        receipt["flat"] = flat_receipt
        result["positive"]["lit"] = receipt
        for label, file, origin in (("depth5", "level0.vmf", (0, 0, 0)), ("occurrence257", "fan257.vmf", (0, 0, 0)),
                                    ("ancestor-hardlink", "cycle.vmf", (0, 0, 0)), ("symlink", "leaf-link.vmf", (0, 0, 0)),
                                    ("traversal", "../leaf.vmf", (0, 0, 0)), ("missing", "missing.vmf", (0, 0, 0)),
                                    ("malformed-nested", "bad-transform.vmf", (0, 0, 0)), ("runtime-child", "runtime.vmf", (0, 0, 0)),
                                    ("intermediate-bounds", "intermediate.vmf", (-16380, 0, 64))):
            source = args.output / f"{label}-parent.vmf"
            ids, owner = root_map(client, source, file, origin)
            setup(client, source)
            _, receipt = pair(client, args.output, label, ids, source, game, failure=True, retained=retained)
            before = source_state(client, source)
            refresh_result = client.rpc("tools/call", {"name": "set_instance_contents", "arguments": {"action": "refresh"}})
            failed = client.call("inspect_instance_contents")
            require(failed["state"] == "failed" and not failed["available"] and not failed["displayed"] and source_state(client, source) == before, "Failed nested Refresh retained a partial layer or changed the parent")
            receipt["failed_preview"] = failed
            receipt["refresh_response"] = refresh_result
            result["negative"][label] = receipt
        source = args.output / "root-cycle-parent.vmf"
        ids, owner = root_map(client, source, "root-cycle-alias.vmf")
        os.link(source, args.output / "root-cycle-alias.vmf")
        setup(client, source)
        _, result["negative"]["root-physical-cycle"] = pair(client, args.output, "root-physical-cycle", ids, source, game, failure=True, retained=retained)
        source = args.output / "preview-parent.vmf"
        result["preview"], preview_owner, _ids = preview(client, args.output, source, leaf_ids)
        result["preview_stale"] = preview_stale(client, args.output, source, preview_owner)
        if args.physics_gameinfo:
            physics_output = args.output / "physics"
            physics_output.mkdir()
            physics_game = physics_output / "installation/game"
            assets(physics_game)
            native = args.physics_gameinfo.parent.parent / "bin/linux64"
            require(native.is_dir(), "Physics installation has no Linux64 library directory")
            private_native = physics_game.parent / "bin/linux64"
            private_native.parent.mkdir()
            private_native.symlink_to(native, target_is_directory=True)
            native_inputs = (args.physics_gameinfo, native / "vphysics.so", native / "libtier0.so", args.binary.parent / "hammerr-physics")
            protected_inputs = {}
            for path in native_inputs:
                limit = (512 if path == args.binary.parent / "hammerr-physics" else 64) * 1024 * 1024
                protected_inputs[str(path)] = physics_input(path, limit)
            with patch.dict(os.environ, {"TMPDIR": str(temporary)}):
                physics_client = open_client(args.binary, physics_output / "client", physics_game)
            result["physics"] = physics_pair(physics_client, physics_output, physics_game, args.physics_gameinfo, protected_inputs)
            require(private_native.is_symlink() and private_native.resolve(strict=True) == native.resolve(strict=True), "Private native-library binding changed")
            result["physics_exercised"] = True
        ending_assets = identities(game)
        ending_physics_assets = identities(physics_game) if physics_client is not None else None
        ending_sources = {str(path): digest(path.read_bytes()) for path in args.output.rglob("*.vmf")}
        result["input_sources_sha256"] = ending_sources
        recipe_path = Path(client.call("get_build_recipes")["path"])
        recipe_bytes = recipe_path.read_bytes()
        result["clean_eof"] = require_clean_eof(client)
        if physics_client is not None:
            result["physics_clean_eof"] = require_clean_eof(physics_client)
        require(identities(game) == ending_assets and all(digest(Path(path).read_bytes()) == identity for path, identity in ending_sources.items())
                and recipe_path.read_bytes() == recipe_bytes, "EOF changed private assets, sources or recipes")
        if physics_client is not None:
            require(identities(physics_game) == ending_physics_assets, "EOF changed private Engine assets")
            require(all(physics_input(Path(path), identity["limit_bytes"]) == identity for path, identity in protected_inputs.items()),
                    "EOF changed a native-installation input")
        result["status"] = "accepted"
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        try:
            for active in (client, physics_client):
                if active is not None and active.process is not None:
                    require_clean_eof(active)
        except Exception as error:
            result["cleanup_error"] = f"{type(error).__name__}: {error}"
            result["status"] = "failed"
            raise
        finally:
            (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def physics_input(path, limit):
    require(path.is_file() and path.stat().st_size <= limit, f"Missing or oversized physics input: {path}")
    before = path.stat()
    resolved = str(path.resolve(strict=True))
    fields = ("st_dev", "st_ino", "st_mode", "st_size", "st_mtime_ns", "st_ctime_ns")
    with path.open("rb") as source:
        opened = os.fstat(source.fileno())
        require(all(getattr(opened, key) == getattr(before, key) for key in fields),
                f"Physics input changed before hashing: {path}")
        identity = hashlib.file_digest(source, "sha256").hexdigest()
        finished = os.fstat(source.fileno())
        require(all(getattr(finished, key) == getattr(opened, key) for key in fields),
                f"Physics input changed during hashing: {path}")
    after = path.stat()
    require(all(getattr(after, key) == getattr(before, key) for key in fields)
            and str(path.resolve(strict=True)) == resolved, f"Physics input identity changed during hashing: {path}")
    return {"resolved": resolved, "bytes": before.st_size, "sha256": identity, "limit_bytes": limit}


def physics_pair(client, output, game, gameinfo, protected_inputs):
    leaf(client, output / "leaf.vmf")
    original = (output / "leaf.vmf").read_text()
    helpers_file(output / "middle.vmf", [{"file": "leaf.vmf", "origin": "8 8 32"}])
    source = output / "parent.vmf"
    ids, owner = root_map(client, source, "middle.vmf", (16, 8, 32))
    setup(client, source)
    client.call("set_compile_settings", {"physics": "engine"})
    with stopped_on_exit(client):
        nested, receipt = compile_pair(client, output, "engine-nested", ids, source, game)
    flat = output / "flat.vmf"
    flat_ids = flat_map(client, flat, original, IDENTITY, (24, 16, 64))
    setup(client, flat)
    client.call("set_compile_settings", {"physics": "engine"})
    with stopped_on_exit(client):
        manual, flat_receipt = compile_pair(client, output, "engine-flat", flat_ids, flat, game)
    require(receipt["result"]["stats"]["physics_solids"] > 0 and payload(nested, 29) == payload(manual, 29), "Nested ordinary physics differs from the manual flat control")
    require(all(physics_input(Path(path), identity["limit_bytes"]) == identity for path, identity in protected_inputs.items()),
            "Physics changed or lost a native-installation input")
    return {"native_installation_gameinfo": str(gameinfo), "private_gameinfo": str(game / "gameinfo.txt"),
            "material_scope": "complete private fixture; installed stock materials and surface manifests are not mounted",
            "nested": receipt, "manual": flat_receipt,
            "read_only_inputs": protected_inputs, "physics_lump_sha256": digest(payload(nested, 29)), "equivalent": native_equivalent(nested, manual, True)}


def main():
    parser = argparse.ArgumentParser(description="Validate bounded nested world/detail instances, root-relative leaf provenance and guarded MCP compilation without graphics or a game launch.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--controls", type=Path, required=True)
    parser.add_argument("--physics-gameinfo", type=Path, help="Locate installed Linux64 physics libraries; the Engine pair uses an isolated fully hashed fixture game")
    args = parser.parse_args()
    args.binary, args.output, args.controls = args.binary.resolve(), args.output.resolve(), args.controls.resolve()
    if args.physics_gameinfo:
        args.physics_gameinfo = args.physics_gameinfo.resolve(strict=True)
    require(args.output.is_relative_to(ROOT / "target/validation"), "Artifacts must stay under target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    result = validate(args)
    print(json.dumps({"status": result["status"], "positive": len(result["positive"]), "negative": len(result["negative"]), "physics_exercised": result["physics_exercised"]}))


if __name__ == "__main__":
    main()
