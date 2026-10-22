import argparse
import hashlib
import json
import math
import os
import re
import sys
from pathlib import Path

import validate_instanced_visclusters as shared
from validate_authored_planes import records
from validate_collision_topology import document_state, identity
from validate_direct_lighting import FACE_FORMAT
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail, face_ref
from validate_instance_contents import refresh
from validate_instance_props import cleanup_clients
from validate_material_albedo import payload, require
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM

DEFAULT_GAME = Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source"
MATERIAL = "decals/decalgraffiti001a"
REASON = ("Child overlay projections and icons are not shown in the reference preview; "
          "overlay bounds, picking, framing and editing drafts are unavailable. Underlying brush preview is unchanged.")
BOUNDS = ([-2, -36, 0], [2, 36, 192])
CENTERS = (-60, 60)
GEOMETRY = ("vertices", "indices", "triangles", "line_vertices")
FRAME = ("BasisOrigin", "BasisU", "BasisV", "BasisNormal")
FIT_FRAME = {"BasisOrigin": (-2, 0, 96), "BasisU": (0, 0, 1), "BasisV": (0, 1, 0), "BasisNormal": (-1, 0, 0),
             "uv0": (-96, -36, 0), "uv1": (-96, 36, 0), "uv2": (96, 36, 0), "uv3": (96, -36, 0)}
LEAF_FRAME = {"BasisOrigin": (-2, 0, 96), "BasisU": (0, 0, 1), "BasisV": (0, 1, 0), "BasisNormal": (-1, 0, 0),
              "uv0": (96, -36, 0), "uv1": (-96, -36, 0), "uv2": (-96, 36, 0), "uv3": (96, 36, 0)}
SOURCE_PLANES = (
    ((-2, 36, 192), (2, 36, 192), (2, -36, 192)),
    ((2, 36, 0), (-2, 36, 0), (-2, -36, 0)),
    ((-2, -36, 192), (-2, -36, 0), (-2, 36, 0)),
    ((2, 36, 192), (2, 36, 0), (2, -36, 0)),
    ((-2, 36, 192), (-2, 36, 0), (2, 36, 0)),
    ((2, -36, 192), (2, -36, 0), (-2, -36, 0)),
)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def installation(root):
    selected = {root / "cstrike_linux64", root / "cstrike/gameinfo.txt"}
    selected.update(root / f"bin/linux64/{name}.so" for name in
                    ("engine", "materialsystem", "shaderapidx9", "shaderapivk", "stdshader_dx9", "filesystem_stdio", "vphysics", "libtier0", "libvstdlib"))
    selected.update(root / f"cstrike/bin/linux64/{name}.so" for name in ("server", "client"))
    result = {}
    for folder in (root / "cstrike", root / "hl2", root / "platform"):
        archives = list(folder.glob("*.vpk"))
        require(len(archives) <= 256, "Installed VPK scope exceeds its bound")
        selected.update(archives)
        selected.add(folder)
        selected.update(folder / name for name in ("materials", "models", "download", "scripts"))
        for relative in ("materials/decals", "materials/dev", "models/props_junk", "materials/models/props_junk"):
            selected.add(folder / relative)
        for relative in ("materials/decals/decalgraffiti001a.vmt", "materials/decals/decalgraffiti001a.vtf",
                         "materials/dev/dev_measuregeneric01.vmt", "materials/dev/dev_measuregeneric01.vtf"):
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
                ("cstrike_linux64", "bin/linux64/engine.so", "cstrike/bin/linux64/server.so")), "Installed playable CS:S is required")
    return result


def mounted_game(path, root):
    require(all((root / "bin/linux64" / name).is_file() for name in ("vphysics.so", "libtier0.so", "libvstdlib.so")),
            "Installed Linux64 CS:S vphysics, tier0 and vstdlib libraries are required")
    path.mkdir()
    (path.parent / "bin").symlink_to(root / "bin", target_is_directory=True)
    mounts = [path, root / "cstrike/cstrike_pak.vpk", root / "cstrike/cstrike_english.vpk", root / "cstrike",
              root / "hl2/hl2_textures.vpk", root / "hl2/hl2_misc.vpk", root / "hl2", root / "platform/platform_misc.vpk", root / "platform"]
    search = "\n".join(f'game "{mount}"' for mount in mounts)
    (path / "gameinfo.txt").write_text(f'GameInfo {{ FileSystem {{ SearchPaths {{ {search} }} }} }}\n')


def library_mount(path, root):
    require(path.is_symlink() and path.resolve(strict=True) == (root / "bin").resolve(strict=True),
            "Private native library mount does not name the installed CS:S bin directory")
    info = path.lstat()
    return (info.st_dev, info.st_ino, info.st_mode, info.st_size, info.st_mtime_ns, info.st_ctime_ns, os.readlink(path))


def block(client, bounds):
    return client.call("create_block", {"mins": bounds[0], "maxs": bounds[1], "material": "dev/dev_measuregeneric01"})["id"]


def vec(raw):
    values = tuple(map(float, raw.split()))
    require(len(values) == 3 and all(math.isfinite(value) for value in values), "Invalid literal source vector")
    return values


def text_vec(values):
    return " ".join(format(value, ".17g") for value in values)


def literal_point(point, center=0):
    x, y, z = point
    return (x, y + center, z)


def quote(value):
    return json.dumps(str(value))


def literal_solid(center, index):
    sides = []
    for number, plane in enumerate(SOURCE_PLANES):
        points = " ".join("(" + text_vec(literal_point(point, center)) + ")" for point in plane)
        u, v = ((1, 0, 0), (0, -1, 0)) if number < 2 else (((0, 1, 0), (0, 0, -1)) if number < 4 else ((1, 0, 0), (0, 0, -1)))
        axes = [(axis, -axis[1] * center / 0.25) for axis in (u, v)]
        fields = {"id": 2000 + index * 6 + number, "plane": points, "material": "DEV/DEV_MEASUREGENERIC01",
                  "uaxis": f'[{text_vec(axes[0][0])} {axes[0][1]}] 0.25',
                  "vaxis": f'[{text_vec(axes[1][0])} {axes[1][1]}] 0.25',
                  "rotation": "0", "lightmapscale": "16", "smoothing_groups": "0"}
        sides.append("side\n{\n" + "\n".join(f'{quote(key)} {quote(value)}' for key, value in fields.items()) + "\n}\n")
    return f'solid\n{{\n"id" "{1000 + index}"\n' + "".join(sides) + "}\n"


def world_end(text):
    match = re.search(r"(?m)^world\s*\{", text)
    require(match is not None, "Literal source lacks its world block")
    depth, quoted, escaped = 1, False, False
    for index in range(match.end(), len(text)):
        char = text[index]
        if quoted:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quoted = False
        elif char == '"':
            quoted = True
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return index
    raise RuntimeError("Literal world block is incomplete")


def leaf(client, path, custody):
    shared.new_map(client)
    plate = block(client, BOUNDS)
    source = detail(client, [plate])[0]
    face = next(face for face in source["faces"] if face["normal"] == [-1, 0, 0])
    selected = face_ref(source, face)
    client.call("select_faces", {"faces": [selected]})
    client.call("save_map", {"path": str(path)})
    saved, before, camera = path.read_bytes(), document_state(client), client.call("get_camera")
    state = client.call("preview_overlay", {"snapshot": client.call("get_overlay_state")["snapshot"],
                    "source": selected, "material": MATERIAL, "render_order": 0,
                    "u": [0, 1], "v": [0, 1], "fade_min": -1, "fade_max": 0})
    require(state["draft"]["valid"] and state["draft"]["owner"] == "agent"
            and document_state(client) == before and client.call("get_camera") == camera and path.read_bytes() == saved,
            "Normal unnamed overlay preview changed source/history/camera")
    reviewed = client.call("review_overlay", {"token": state["draft"]["token"]})
    created = client.call("apply_overlay", {"token": reviewed["draft"]["token"]})["created"]
    authored = dict(detail(client, [created])[0]["properties"])
    expected = {"classname", "origin", "material", "sides", *FRAME, "uv0", "uv1", "uv2", "uv3",
                "RenderOrder", "StartU", "EndU", "StartV", "EndV", "fademindist", "fademaxdist"}
    require(set(authored) == expected and authored["classname"] == "info_overlay" and authored["material"] == MATERIAL
            and authored["sides"] == str(selected["side"]) and all(vec(authored[key]) == value for key, value in FIT_FRAME.items()),
            "Normal shared overlay writer changed its independent fixed unnamed frame/envelope")
    client.call("undo")
    require(detail(client, [plate])[0] == source, "Overlay Undo changed its source quad")
    client.call("redo")
    require(dict(detail(client, [created])[0]["properties"]) == authored, "Overlay Redo changed the normal authored frame")
    corner_before = document_state(client)
    corners = [list(LEAF_FRAME[f"uv{index}"][:2]) for index in range(4)]
    state = client.call("preview_overlay_corners", {"snapshot": client.call("get_overlay_state")["snapshot"],
                        "id": created, "corners": corners})
    require(state["draft"]["valid"] and state["draft"]["operation"] == "corners"
            and state["draft"]["local_corners"] == corners and state["draft"]["source"] == selected
            and document_state(client) == corner_before and client.call("get_camera") == camera and path.read_bytes() == saved,
            "Corner preview changed source/history/camera or its fixed source binding")
    corner_review = client.call("review_overlay", {"token": state["draft"]["token"]})
    edited = client.call("apply_overlay", {"token": corner_review["draft"]["token"]})
    require(edited["operation"] == "corners" and edited["updated"] == created and "created" not in edited,
            "Corner Apply did not update the same normal authored helper")
    final = dict(detail(client, [created])[0]["properties"])
    points = {f"uv{index}" for index in range(4)}
    require(set(final) == expected and all(vec(final[key]) == value for key, value in LEAF_FRAME.items())
            and {key: value for key, value in final.items() if key not in points}
                == {key: value for key, value in authored.items() if key not in points}
            and detail(client, [plate])[0] == source and client.call("get_camera") == camera and path.read_bytes() == saved,
            "Shared corner writer changed the fixed frame, binding, settings, source or camera")
    client.call("undo")
    require(dict(detail(client, [created])[0]["properties"]) == authored, "Corner Undo changed the original fitted helper")
    client.call("redo")
    require(dict(detail(client, [created])[0]["properties"]) == final, "Corner Redo changed the final independent corner association")
    shared.save(client, path, custody)
    require(dict(detail(client, [created])[0]["properties"]) == final, "Save/reopen changed the authored child overlay")
    return {"entity": created, "source": selected, "properties": final, "review": reviewed["draft"]["operation"],
            "corner_review": corner_review["draft"]["operation"]}


def middle(client, path, child, custody):
    shared.new_map(client)
    client.call("save_map", {"path": str(path)})
    owner, authored = shared.authored_instance(client, child, [8, 16, 64], "0 0 0")
    shared.save(client, path, custody)
    return {"owner": owner, "authoring": authored}


def scene(client, path, custody, child=None, overlay=None):
    shared.new_map(client)
    for bounds in ROOM:
        block(client, bounds)
    for classname, origin, properties in (
        ("info_player_terrorist", [-160, 0, 8], {}), ("info_player_counterterrorist", [192, -160, 8], {}),
        ("light", [-128, 0, 160], {"_light": "255 255 255 200", "_constant_attn": "1", "_quadratic_attn": "0"}),
        ("prop_physics", [-208, -208, 64], {"model": "models/props_junk/wood_crate001a.mdl", "targetname": "physics_crate"}),
        ("logic_script", [-128, -128, 64], {"targetname": "hammerr_observer", "vscripts": "hammerr_observer.nut", "thinkfunction": "HammeRRThink"}),
    ):
        client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})
    client.call("save_map", {"path": str(path)})
    owners, authored = [], []
    if child:
        for center in CENTERS:
            owner, receipt = shared.authored_instance(client, child, [-8, center - 16, -64], "0 0 0")
            owners.append(owner)
            authored.append(receipt)
    else:
        text = path.read_text()
        index = world_end(text)
        text = text[:index] + "".join(literal_solid(center, i) for i, center in enumerate(CENTERS)) + text[index:]
        if overlay:
            for i, center in enumerate(CENTERS):
                fields = dict(overlay)
                fields["id"] = str(1000001 + i)
                fields["sides"] = str(2000 + i * 6 + 2)
                fields.update(BasisOrigin=f"-2 {center} 96", BasisU="0 0 1", BasisV="0 1 0", BasisNormal="-1 0 0")
                if "origin" in fields:
                    require(vec(overlay["origin"]) == (-2, 0, 96), "Normal authored marker differs from fixed child frame")
                    fields["origin"] = f"-2 {center} 96"
                text += "\nentity\n{\n" + "\n".join(f'{quote(key)} {quote(value)}' for key, value in fields.items()) + "\n}\n"
        path.write_text(text)
        shared.reviewed_open(client, path)
    shared.save(client, path, custody)
    client.call("select", {"ids": owners})
    return {"source": path, "owners": owners, "authoring": authored}


def metadata(path):
    text = path.read_text()
    result = {}
    for name in ("versioninfo", "world"):
        match = re.search(r"(?m)^" + name + r"\s*\{", text)
        require(match is not None, "Authored control lacks normal metadata")
        prefix = text[match.end():]
        prefix = prefix[:min(index for index in (prefix.find("{"), prefix.find("}")) if index >= 0)]
        pairs = re.findall(r'("(?:\\.|[^"\\])*")\s+("(?:\\.|[^"\\])*")', prefix)
        result[name] = [(json.loads(key), json.loads(value)) for key, value in pairs]
    return result


def inspect(client, control, count):
    before, camera = document_state(client), client.call("get_camera")
    ready = refresh(client, background=True)
    require(ready["state"] == "ready" and ready["stats"].get("overlays", 0) == count
            and ("overlays" in ready["stats"]) == bool(count) and ("overlay_preview" in ready) == bool(count), "Expanded overlay count/omission differs")
    if count:
        require(ready["overlay_preview"] == {"available": False, "reason": REASON}, "Parent inspection fabricated overlay projection/editing")
    require(ready["available"] and ready["stats"]["world_brushes"] == 2 and ready["stats"]["detail_brushes"] == 0
            and ready["geometry"]["triangles"] > 0 and {row["owner"] for row in ready["owners"]} == set(control["owners"]),
            "Overlay helper changed underlying physical plate reference geometry")
    require(document_state(client) == before and client.call("get_camera") == camera
            and not client.call("get_map_info")["rendering"]["initialized"], "Reference inspection changed source/history/camera or initialized rendering")
    return ready


def entities(data):
    result = []
    for block_text in re.findall(r"\{([^{}]*)\}", payload(data, 0).decode()):
        tokens = [json.loads(token) for token in re.findall(r'"(?:\\.|[^"\\])*"', block_text)]
        require(len(tokens) % 2 == 0, "Malformed emitted entity fields")
        result.append(dict(zip(tokens[::2], tokens[1::2], strict=True)))
    return result


def decoded(data, count):
    rows, fades = records(data, 45, "<ihH64i22f"), records(data, 60, "<2f")
    faces, planes = records(data, 7, FACE_FORMAT), records(data, 1, "<4fi")
    vertices, edges, surfedges = records(data, 3, "<3f"), records(data, 12, "<HH"), records(data, 13, "<i")
    infos, textures, names = records(data, 6, "<16f2i"), records(data, 2, "<3f5i"), records(data, 44, "<i")
    strings = payload(data, 43)
    require(len(rows) == len(fades) == count and all(fade == (-1, 0) for fade in fades), "Ordinary overlay/fade count differs")
    require(not payload(data, 50) and not any(row.get("classname") in ("info_overlay_accessor", "info_overlay", "func_instance") for row in entities(data)),
            "Unnamed helper/accessor/transition escaped into native runtime")
    for index, row in enumerate(rows):
        center = CENTERS[index]
        require(row[0] == index and row[2] >> 14 == 0 and 0 < row[2] & 0x3FFF <= 64
                and row[67:71] == (0, 1, 0, 1) and row[-6:] == (-2, center, 96, -1, 0, 0)
                and row[71:83] == (96, -36, 0, -96, -36, 0, -96, 36, 1, 96, 36, 0), "Native overlay ID/settings/packed frame differs")
        require(0 <= row[1] < len(infos), "Overlay material info index is invalid")
        offset = names[textures[infos[row[1]][-1]][3]][0]
        require(strings[offset:].split(b"\0", 1)[0].decode().lower() == MATERIAL, "Overlay material changed")
        points = set()
        for target in row[3:3 + (row[2] & 0x3FFF)]:
            require(0 <= target < len(faces), "Invalid emitted target face")
            plane = planes[faces[target][0]][:4]
            require(faces[target][1] == (faces[target][0] & 1), "Emitted target plane parity differs")
            normal, distance = plane[:3], plane[3]
            require(normal == (-1, 0, 0) and distance == 2, "Overlay target is not the independent literal front plane")
            for edge_index in range(faces[target][3], faces[target][3] + faces[target][4]):
                edge = surfedges[edge_index][0]
                point = vertices[edges[abs(edge)][0 if edge >= 0 else 1]]
                require(point[0] == -2 and center - 36 <= point[1] <= center + 36 and 0 <= point[2] <= 192,
                        "Overlay binding crossed the independent occurrence's front patch")
                points.add(point)
        require({(-2, center + y, z) for y in (-36, 36) for z in (0, 192)} <= points,
                "Overlay bindings omit an independent literal patch corner")
    return {"records": [list(row) for row in rows], "fades": [list(row) for row in fades], "accessors": 0}


def configure(client):
    client.call("set_compile_settings", {"lighting": "direct_ldr", "visibility": "fast", "physics": "engine", "bounces": 0,
                "generate_reflections": False, "bake_static_prop_vertices": False, "pack_static_prop_assets": False,
                "pack_water_assets": False, "pack_sky_assets": False})
    if not any(row["name"] == "Unnamed child overlays" for row in client.call("get_build_recipes")["recipes"]):
        client.call("save_build_recipe", {"name": "Unnamed child overlays"})


def compile_pair(client, args, game, name, control, sources, count):
    shared.reviewed_open(client, control["source"])
    configure(client)
    case = {}
    camera = client.call("get_camera")
    data = shared.pair(client, args, game, name, control, sources, case)
    require(client.call("get_camera") == camera, "Foreground/background compilation changed the shared camera")
    case["native"] = decoded(data, count)
    for receipt in (case["foreground"], case["background"]["result"]):
        require(receipt["stats"]["overlays"] == count and receipt["stats"]["models"] == 1, "Retained overlay/model counts differ")
        if sources:
            expanded = receipt["stats"]["instances"]
            require(expanded["overlays"] == 2 and expanded["world_brushes"] == 2 and expanded["instances"] == 4
                    and expanded["files"] == 2 and expanded["max_expansion_depth"] == 2, "Expanded-before-region overlay usage differs")
    case["bsp"] = str(args.output / "builds" / f"{name}-foreground.bsp")
    case["sha256"] = digest(Path(case["bsp"]))
    return case, data


def validate(args):
    args.custody = {str(args.binary): identity(args.binary), str(args.fgd): identity(args.fgd), str(Path(__file__).resolve()): identity(Path(__file__).resolve())}
    for name in ("tmp", "builds"):
        (args.output / name).mkdir()
    game = args.output / "game"
    mounted_game(game, args.game_dir)
    native_mount = library_mount(args.output / "bin", args.game_dir)
    installed = installation(args.game_dir)
    report = {"schema": 754, "status": "failed", "binary": str(args.binary), "binary_sha256": digest(args.binary),
              "script_sha256": digest(Path(__file__)), "fixture": "nested_translations_vertical_leaf", "game_dir": str(args.game_dir), "desktop_input": False, "native_library_mount": native_mount, "cases": {}}
    clients, failures = [], []
    try:
        for role in ("compiler", "author"):
            output = args.output / role
            output.mkdir()
            environment = {"TMPDIR": str(args.output / "tmp"), "HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
                           "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"), "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
                           "XDG_CONFIG_HOME": str(output / "config"), "XDG_STATE_HOME": str(output / "state")}
            client = Client(None, args.binary, output, game / "gameinfo.txt", environment=environment, request_timeout=args.timeout)
            clients.append(client)
            definitions = client.call("load_entity_definitions", {"path": str(args.fgd)})
            for row in definitions["sources"]:
                path = Path(row["path"] if isinstance(row, dict) else row)
                args.custody[str(path)] = identity(path)
        compiler, author = clients
        child, mid = args.output / "leaf.vmf", args.output / "middle.vmf"
        authored = leaf(author, child, args.custody)
        report["authored_leaf"] = authored
        report["authored_middle"] = middle(author, mid, child, args.custody)
        control = scene(compiler, args.output / "child.vmf", args.custody, child=mid)
        report["preview"] = {"child": inspect(compiler, control, 2)}
        report["cases"]["child"], child_data = compile_pair(compiler, args, game, "child", control, [child, mid], 2)
        literal = scene(author, args.output / "literal.vmf", args.custody, overlay=authored["properties"])
        require(metadata(control["source"]) == metadata(literal["source"]), "Balanced normal Saves produced different authored versioninfo/world metadata")
        report["cases"]["literal"], literal_data = compile_pair(compiler, args, game, "literal", literal, [], 2)
        require(child_data == literal_data, "Independent literal and child complete BSP bytes differ; preserve failed premise without rounding or discarded records")
        absent = scene(author, args.output / "absent.vmf", args.custody)
        require(metadata(absent["source"]) == metadata(literal["source"]), "Helper-absent source metadata differs")
        report["authored_metadata"] = metadata(literal["source"])
        report["cases"]["absent"], absent_data = compile_pair(compiler, args, game, "absent", absent, [], 0)
        physical = (1, 3, 4, 5, 7, 8, 10, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 26, 27, 29, 30, 31, 48, 51, 52, 53, 55, 56, 58)
        require(all(payload(absent_data, slot) == payload(literal_data, slot) for slot in physical), "Helper-absent control changed physical/lighting/physics/visibility payloads")
        bad_leaf, bad_mid = args.output / "named-leaf.vmf", args.output / "named-middle.vmf"
        shared.reviewed_open(author, child)
        author.call("set_entity_properties", {"id": authored["entity"], "set": {"targetname": " "}})
        shared.save(author, bad_leaf, args.custody)
        middle(author, bad_mid, bad_leaf, args.custody)
        refused = scene(compiler, args.output / "named-parent.vmf", args.custody, child=bad_mid)
        shared.reviewed_open(compiler, refused["source"])
        configure(compiler)
        failure = report["refused_whitespace_name"] = {}
        shared.pair(compiler, args, game, "named-refusal", refused, [bad_leaf, bad_mid], failure,
                    failure="Child overlay requires exactly empty targetname")
        for receipt in (failure["foreground"], failure["background"]["result"]):
            require(any(row["level"] == "error" and row["object"] in refused["owners"] and bad_leaf.name in row["message"]
                        and authored["entity"].split(":")[-1] in row["message"] for row in receipt["diagnostics"]),
                    "Child refusal lost its root owner, true leaf or original helper identity")
        shared.reviewed_open(author, child)
        author.call("delete_objects", {"ids": [authored["entity"]]})
        empty_leaf, empty_mid = args.output / "physical-leaf.vmf", args.output / "physical-middle.vmf"
        shared.save(author, empty_leaf, args.custody)
        middle(author, empty_mid, empty_leaf, args.custody)
        bare = scene(compiler, args.output / "physical-parent.vmf", args.custody, child=empty_mid)
        ready = report["preview"]["physical_only"] = inspect(compiler, bare, 0)
        original = report["preview"]["child"]
        require(all(original["geometry"][key] == ready["geometry"][key] for key in GEOMETRY)
                and original["face_provenance_total"] == ready["face_provenance_total"]
                and [row["bounds"] for row in original["owners"]] == [row["bounds"] for row in ready["owners"]], "Overlay helpers changed physical preview geometry/provenance/bounds")
        for client in clients:
            require_clean_eof(client)
        require(library_mount(args.output / "bin", args.game_dir) == native_mount, "Private native library mount changed")
        require(installation(args.game_dir) == installed and all(identity(Path(path)) == expected for path, expected in args.custody.items()), "Source/binary/FGD/output/installation custody changed")
        if args.compare_report:
            previous = json.loads(args.compare_report.read_text())
            require(previous["status"] == "passed" and set(previous["cases"]) == set(report["cases"]), "Cross-binary case scope differs")
            require(all(Path(case["bsp"]).read_bytes() == Path(previous["cases"][name]["bsp"]).read_bytes() for name, case in report["cases"].items()), "Actual MCP binaries produced different BSP bytes")
            report["cross_binary_exact_bsp_parity"] = True
        report.update(status="passed", installed_sources=installed, exact_literal_child_bsp=True)
    except BaseException as error:
        failures.append(error)
        report["error"] = {"type": type(error).__name__, "message": str(error)}
    finally:
        failures.extend(cleanup_clients(clients, report))
        if failures:
            report["status"] = "failed"
        report.update(custody=args.custody, compile_calls=args.compile_calls)
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        if failures:
            raise BaseExceptionGroup("Instanced unnamed overlay validation failed", failures)


def main():
    parser = argparse.ArgumentParser(description="Validate normal unnamed vertical child overlay and corner authoring through nested translations, reference inspection, independent literal frames and exact foreground/background BSP output through an actual production MCP binary. No game launch or rotated native claim.",
        epilog="Requires Python3.11+, installed CS:S/SourceSDK FGD, a matching production binary and engine-physics support. Root owns builds and aggregate24GiB/single8-job pipeline. Keep fresh artifacts under target/validation; native qualification uses the separate private754 packet.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--game-dir", type=Path, default=DEFAULT_GAME)
    parser.add_argument("--fgd", type=Path, default=shared.DEFAULT_FGD)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--compare-report", type=Path)
    args = parser.parse_args()
    args.binary, args.game_dir, args.fgd = (path.resolve(strict=True) for path in (args.binary, args.game_dir, args.fgd))
    require(args.timeout > 0 and not args.output.exists() and not args.output.is_symlink(), "Use a fresh output and positive timeout")
    args.output = args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Keep artifacts on disk under target/validation")
    args.output.mkdir(parents=True, mode=0o700)
    args.compile_calls = 0
    validate(args)
    print(json.dumps({"status": "passed", "output": str(args.output), "compile_calls": args.compile_calls}))


if __name__ == "__main__":
    sys.dont_write_bytecode = True
    main()
