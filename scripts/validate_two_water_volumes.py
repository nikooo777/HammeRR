import argparse
import hashlib
import json
import math
import os
import re
import stat
import struct
import sys
from pathlib import Path

SCHEMA = 748
ROOT = Path(__file__).resolve().parents[1]
MATERIAL = "tools/toolsnodraw"
TARGETS = ("hammerr_748_water_west", "hammerr_748_water_east")
ORIGINS = ((-640, -256, 256), (640, -256, 256))
MINS, MAXS = (-512, -512, -128), (512, 512, 128)
ROOM = (
    ((-2064, -1552, -16), (2064, 1552, 0)),
    ((-2064, -1552, 768), (2064, 1552, 784)),
    ((-2064, -1552, 0), (-2048, 1552, 768)),
    ((2048, -1552, 0), (2064, 1552, 768)),
    ((-2048, -1552, 0), (2048, -1536, 768)),
    ((-2048, 1536, 0), (2048, 1552, 768)),
)
SETTINGS = {"lighting": "direct_ldr", "physics": "engine", "bounces": 0, "visibility": "fast", "sky_quality": "fast", "generate_reflections": False, "pack_sky_assets": False, "pack_water_assets": False, "pack_static_prop_assets": False, "bake_static_prop_vertices": False}
STATE_FIELDS = ("path", "unsaved_changes", "world_brushes", "point_entities", "brush_entities", "worldspawn", "undo", "redo", "selection", "face_selection", "camera", "view")
FORMATS = {1: ("<4fi", 0), 5: ("<3i6h2H2h", 0), 10: ("<ihh6h4Hhh", 1), 14: ("<9f3i", 0), 17: ("<H", 0), 18: ("<3i", 0), 19: ("<HhhBB", 0)}
MAX_BSP = 64 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    result, size = hashlib.sha256(), 0
    with os.fdopen(descriptor, "rb") as stream:
        before = os.fstat(stream.fileno())
        require(stat.S_ISREG(before.st_mode) and 0 <= before.st_size <= 4 * 1024 * 1024 * 1024, "Bounded regular digest input")
        while chunk := stream.read(1024 * 1024):
            size += len(chunk)
            require(size <= before.st_size, "Digest input grew")
            result.update(chunk)
        after = os.fstat(stream.fileno())
    fields = ("st_dev", "st_ino", "st_mode", "st_size", "st_mtime_ns", "st_ctime_ns")
    require(size == before.st_size and all(getattr(before, field) == getattr(after, field) for field in fields), "Digest input custody")
    return result.hexdigest()


def read_regular(path, maximum=MAX_BSP):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        before = os.fstat(stream.fileno())
        require(stat.S_ISREG(before.st_mode) and 0 < before.st_size <= maximum, "Bounded regular input: " + str(path))
        raw = stream.read(maximum + 1)
        after = os.fstat(stream.fileno())
    fields = ("st_dev", "st_ino", "st_mode", "st_size", "st_mtime_ns", "st_ctime_ns")
    require(len(raw) == before.st_size and all(getattr(before, field) == getattr(after, field) for field in fields), "Input changed while reading: " + str(path))
    return raw


def write_json(path, value):
    with path.open("x") as stream:
        json.dump(value, stream, indent=2)
        stream.write("\n")


def build_pins(path, binary):
    raw = read_regular(path, 1024 * 1024)
    pins = json.loads(raw)
    require(set(pins) == {"schema", "binary", "physics_worker", "sources"} and pins["schema"] == SCHEMA, "Exact748 root-owned build pin schema")
    worker = binary.with_name("hammerr-physics").resolve(strict=True)
    for name, actual in (("binary", binary), ("physics_worker", worker)):
        entry = pins[name]
        require(set(entry) == {"path", "sha256"} and Path(entry["path"]).resolve(strict=True) == actual and digest(actual) == entry["sha256"], "Actual executable/worker build identity")
    require(0 < len(pins["sources"]) <= 4096, "Bounded nonempty production source pins")
    for name, expected in pins["sources"].items():
        source = Path(name)
        require(source.is_absolute() and source.resolve(strict=True).is_relative_to(ROOT) and digest(source) == expected, "Production source custody: " + name)
    required = {ROOT / name for name in ("crates/compiler/src/water_analog.rs", "crates/compiler/src/map.rs", "crates/compiler/src/emit.rs", "crates/compiler/src/physics.rs", "crates/editor-core/src/session.rs", "Cargo.toml", "Cargo.lock")}
    require(required <= {Path(name) for name in pins["sources"]}, "Build pins cover compiler roles, shared Session and dependency graph")
    return {"path": str(path), "sha256": hashlib.sha256(raw).hexdigest(), "pins": pins}


def helper_pins():
    names = ("validate_two_water_volumes.py", "validate_face_lighting.py", "contact_runtime_observer.py", "private_runtime_config.py")
    return {str(ROOT / "scripts" / name): digest(ROOT / "scripts" / name) for name in names}


def directory(raw):
    require(1036 <= len(raw) <= MAX_BSP and raw[:4] == b"VBSP" and struct.unpack_from("<i", raw, 4)[0] == 20, "Bounded BSP20")
    result, ranges = [], []
    for index in range(64):
        entry = struct.unpack_from("<4i", raw, 8 + index * 16)
        offset, size, version, compression = entry
        require(0 <= offset <= len(raw) and 0 <= size <= len(raw) - offset and version >= 0 and compression == 0, "Uncompressed lump framing")
        if size:
            require(offset >= 1036, "Payload after BSP header")
            ranges.append((offset, offset + size))
        result.append(entry)
    ranges.sort()
    require(all(before[1] <= after[0] for before, after in zip(ranges, ranges[1:])), "Disjoint lump payloads")
    return result


def payload(raw, entries, index):
    offset, size, _, _ = entries[index]
    return raw[offset:offset + size]


def entities(raw):
    require(raw.endswith(b"\0") and b"\0" not in raw[:-1] and len(raw) <= 65536, "Entity termination/budget")
    text = raw[:-1].decode("ascii")
    pattern = re.compile(r'\s+|[{}]|"[^"\\\x00-\x1f]*"')
    tokens, cursor = [], 0
    for match in pattern.finditer(text):
        require(match.start() == cursor, "Entity syntax")
        cursor = match.end()
        if not match.group().isspace():
            tokens.append(match.group())
    require(cursor == len(text), "Complete entity text")
    output, cursor = [], 0
    while cursor < len(tokens):
        require(tokens[cursor] == "{", "Entity start")
        cursor += 1
        entity = {}
        while cursor < len(tokens) and tokens[cursor] != "}":
            require(cursor + 1 < len(tokens) and tokens[cursor].startswith('"') and tokens[cursor + 1].startswith('"'), "Quoted key/value")
            key, value = tokens[cursor][1:-1].casefold(), tokens[cursor + 1][1:-1]
            require(key and key not in entity, "Unique nonempty entity key")
            entity[key] = value
            cursor += 2
        require(cursor < len(tokens), "Entity end")
        cursor += 1
        output.append(entity)
    return output


def vector(value):
    result = tuple(float(part) for part in value.split())
    require(len(result) == 3 and all(math.isfinite(part) for part in result), "Finite three-vector")
    return result


def ownership(rows):
    nodes, leaves, brushes, references = (rows[index] for index in (5, 10, 18, 17))
    require(0 < len(nodes) + len(leaves) <= 262144, "Graph record budget")
    require(all(0 <= node[0] < len(rows[1]) and all(0 <= child < len(nodes) if child >= 0 else 0 <= -1 - child < len(leaves) for child in node[1:3]) for node in nodes), "Node indices")
    require(all(leaf[11] <= len(references) and leaf[12] <= len(references) - leaf[11] for leaf in leaves), "Leaf reference spans")
    require(all(0 <= item[0] < len(brushes) for item in references), "Referenced brush indices")
    require(all(0 <= first <= len(rows[19]) and 0 < count <= len(rows[19]) - first for first, count, _ in brushes), "Brush side spans")
    require(all(side[0] < len(rows[1]) for side in rows[19]), "Brush plane indices")
    owners = []
    for model_index, model in enumerate(rows[14]):
        seen_nodes, seen_leaves, seen_brushes = set(), set(), set()
        stack = [model[9]]
        while stack:
            current = stack.pop()
            if current >= 0:
                require(current < len(nodes) and current not in seen_nodes, "Cycle/shared node")
                seen_nodes.add(current)
                stack.extend(nodes[current][1:3])
            else:
                leaf = -1 - current
                require(0 <= leaf < len(leaves) and leaf not in seen_leaves, "Repeated/out-of-range leaf")
                seen_leaves.add(leaf)
                first, count = leaves[leaf][11:13]
                refs = [item[0] for item in references[first:first + count]]
                require(len(refs) == len(set(refs)), "Repeated leaf brush reference")
                seen_brushes.update(refs)
        owners.append({"model": model_index, "headnode": model[9], "nodes": sorted(seen_nodes), "leaves": sorted(seen_leaves), "brushes": sorted(seen_brushes)})
    require(len(owners) == 3, "Exactly world and two inline models")
    for field, count in (("nodes", len(nodes)), ("leaves", len(leaves)), ("brushes", len(brushes))):
        accumulated = set()
        for owner in owners:
            selected = set(owner[field])
            require(not accumulated & selected, "Distinct complete model " + field)
            accumulated.update(selected)
        require(accumulated == set(range(1 if field == "leaves" else 0, count)), "No unowned/reserved model " + field)
    return owners


def physics(raw, entries):
    data = payload(raw, entries, 29)
    require(entries[29][2] == 0 and len(data) >= 16, "Ordinary PHY framing")
    cursor, models = 0, []
    while cursor + 16 <= len(data):
        index, body_size, key_size, count = struct.unpack_from("<4i", data, cursor)
        cursor += 16
        if index == -1:
            require((body_size, key_size, count) == (-1, 0, 0) and cursor == len(data), "Exact PHY sentinel")
            break
        require(index == len(models) and index < 3 and count == 1, "Three ordered ordinary nonempty model bodies")
        require(0 < body_size <= len(data) - cursor and 0 < key_size <= 65535, "PHY body/key bounds")
        end = cursor + body_size
        require(cursor + 4 <= end, "PHY solid framing")
        size = struct.unpack_from("<i", data, cursor)[0]
        cursor += 4
        require(28 <= size <= 16 * 1024 * 1024 and cursor + size == end, "One exact ordinary solid")
        require(struct.unpack_from("<4sHHI", data, cursor) == (b"VPHY", 0x100, 0, size - 28), "Ordinary VPHY body")
        body_hash = hashlib.sha256(data[cursor:end]).hexdigest()
        cursor = end
        require(key_size <= len(data) - cursor, "PHY key size")
        keys = data[cursor:cursor + key_size]
        require(keys.endswith(b"\0") and b"\0" not in keys[:-1], "PHY key termination")
        item = {"model": index, "solid_count": 1, "body_sha256": body_hash, "keys_sha256": hashlib.sha256(keys).hexdigest()}
        if index:
            text = keys[:-1].decode("ascii")
            match = re.fullmatch(r'\s*solid\s*\{\s*((?:"[^"\\]*"\s*"[^"\\]*"\s*)+)\}\s*', text)
            require(match is not None, "One flat inline solid block")
            pairs = re.findall(r'"([^"\\]*)"\s*"([^"\\]*)"', match[1])
            fields = dict(pairs)
            require(len(pairs) == len(fields) == 4 and set(fields) == {"index", "mass", "surfaceprop", "volume"}, "Exact ordinary solid keys")
            mass, volume_text = float(fields["mass"]), float(fields["volume"])
            volume = struct.unpack("<f", struct.pack("<f", volume_text))[0]
            require(fields["index"] == "0" and fields["surfaceprop"] and math.isfinite(mass) and mass > 0 and math.isfinite(volume_text) and volume_text > 0, "Positive ordinary index-zero body")
            require(math.isfinite(volume) and abs(volume - 268435456) <= 1, "739 native f32 cuboid volume tolerance")
            item["volume"] = {"raw_text": fields["volume"], "decimal_f64": volume_text, "native_f32": volume, "tolerance": 1}
        models.append(item)
        cursor += key_size
    else:
        raise ValueError("Missing PHY sentinel")
    require(len(models) == 3, "Complete world and both inline PHY bodies")
    return {"sha256": hashlib.sha256(data).hexdigest(), "models": models}


def point_leaf(rows, model, point):
    current = rows[14][model][9]
    for _ in range(len(rows[5]) + 1):
        if current < 0:
            return -1 - current
        node = rows[5][current]
        plane = rows[1][node[0]]
        distance = sum(a * b for a, b in zip(point, plane[:3], strict=True)) - plane[3]
        current = node[1 if distance > 0 else 2]
    raise ValueError("Point traversal budget")


def query_points(owner):
    center = ORIGINS[owner]
    result = [{"name": "center", "position": center, "owner": owner}]
    for axis, label in enumerate(("x", "y", "z")):
        for sign, side in ((-1, "negative"), (1, "positive")):
            for shift, suffix, inside in ((-1 / 32, "inside", True), (1 / 32, "outside", False), (2, "exterior", False)):
                point = list(center)
                point[axis] += sign * (MAXS[axis] + shift)
                result.append({"name": f"{side}_{label}_{suffix}", "position": point, "owner": owner if inside else -1})
    result.extend(({"name": "other_center", "position": ORIGINS[1 - owner], "owner": 1 - owner}, {"name": "dry_gap", "position": (0, -256, 256), "owner": -1}))
    return result


def emitted_nodraw(raw, entries, rows, inline):
    infos_raw, textures_raw, table_raw, strings = (payload(raw, entries, index) for index in (6, 2, 44, 43))
    require(all(entries[index][2] == 0 for index in (6, 2, 44, 43)), "Texture lump versions")
    require(len(infos_raw) % 72 == len(textures_raw) % 32 == len(table_raw) % 4 == 0, "Texture record framing")
    infos = list(struct.iter_unpack("<16f2i", infos_raw))
    textures = list(struct.iter_unpack("<3f5i", textures_raw))
    offsets = list(struct.iter_unpack("<i", table_raw))
    first, count, _ = rows[18][inline["brushes"][0]]
    for plane, info_index, displacement, bevel, thin in rows[19][first:first + count]:
        require(0 <= plane < len(rows[1]) and 0 <= info_index < len(infos) and (displacement, bevel, thin) == (-1, 0, 0), "Ordinary nodraw side texinfo")
        info = infos[info_index]
        require(info[16] & 0x80 and 0 <= info[17] < len(textures), "Actual emitted NODRAW flag")
        name = textures[info[17]][3]
        require(0 <= name < len(offsets) and 0 <= offsets[name][0] < len(strings), "Actual emitted material name index")
        start = offsets[name][0]
        end = strings.find(b"\0", start)
        require(end > start and strings[start:end] == b"TOOLS/TOOLSNODRAW", "Exact compiler-canonical nodraw without fallback")


def geometry(raw, water):
    entries = directory(raw)
    rows = {}
    for index, (fmt, version) in FORMATS.items():
        data = payload(raw, entries, index)
        require(entries[index][2] == version and len(data) % struct.calcsize(fmt) == 0, "Exact record framing")
        rows[index] = list(struct.iter_unpack(fmt, data))
    authored = entities(payload(raw, entries, 0))
    classname, contents = ("func_water_analog", 32) if water else ("func_movelinear", 1)
    require(len(authored) == 5 and sorted(entity.get("classname", "") for entity in authored) == sorted(("worldspawn", "info_player_terrorist", "light", classname, classname)), "Exact two-owner entity corpus")
    require(len(rows[14]) == 3 and all(all(math.isfinite(part) for part in plane[:4]) for plane in rows[1]), "Three models and finite planes")
    owners = ownership(rows)
    targets = []
    for owner, (name, origin) in enumerate(zip(TARGETS, ORIGINS, strict=True)):
        selected = [index for index, entity in enumerate(authored) if entity.get("targetname") == name]
        require(len(selected) == 1, "One exact named owner")
        entity_index = selected[0]
        entity = authored[entity_index]
        model = owner + 1
        require(set(entity) == {"classname", "targetname", "origin", "angles", "model", "startposition", "spawnflags", "hammerid"}, "Exact stationary unparented owner properties")
        require(entity["classname"] == classname and entity["model"] == f"*{model}" and vector(entity["origin"]) == origin and vector(entity["angles"]) == (0, 0, 0) and entity["startposition"] == entity["spawnflags"] == "0", "Exact owner role/model/neutral pose")
        inline = owners[model]
        require(tuple(rows[14][model][:9]) == (*MINS, *MAXS, 0, 0, 0) and rows[14][model][11] == 0, "Exact local bounds and zero rendered faces")
        require(len(inline["brushes"]) == 1 and 0 not in inline["leaves"], "One exclusively owned nonreserved brush")
        brush = inline["brushes"][0]
        first, count, actual_contents = rows[18][brush]
        expected_planes = {(-1., 0., 0., 512.), (1., 0., 0., 512.), (0., -1., 0., 512.), (0., 1., 0., 512.), (0., 0., -1., 128.), (0., 0., 1., 128.)}
        require(count == 6 and actual_contents == contents and {rows[1][side[0]][:4] for side in rows[19][first:first + count]} == expected_planes, "Six ordinary local cuboid planes/contents")
        require(all(rows[5][index][10] == 0 for index in inline["nodes"]) and all(rows[10][index][10] == 0 for index in inline["leaves"]), "No inline node/leaf rendered faces")
        interior = [index for index in inline["leaves"] if rows[10][index][0] == contents]
        outside = [index for index in inline["leaves"] if rows[10][index][0] == 0]
        require(len(interior) == 1 and len(interior) + len(outside) == len(inline["leaves"]), "One occupied inline leaf")
        for leaf in inline["leaves"]:
            start, length = rows[10][leaf][11:13]
            require([item[0] for item in rows[17][start:start + length]] == ([brush] if leaf in interior else []), "Only occupied own leaf references own brush")
        emitted_nodraw(raw, entries, rows, inline)
        targets.append({"targetname": name, "origin": origin, "model": model, "entity": entity_index, "brush": brush, "interior": interior, "outside": outside, "headnode": inline["headnode"]})
    require(sum("model" in entity for entity in authored) == 2 and rows[10][0][0] == 1 and all(rows[18][index][2] == 1 for index in owners[0]["brushes"]), "Distinct entities, reserved leaf0 and ordinary dry world")
    for owner in range(2):
        low, high = ([a + b for a, b in zip(ORIGINS[owner], bound, strict=True)] for bound in (MINS, MAXS))
        for brush in owners[0]["brushes"]:
            first, count, _ = rows[18][brush]
            planes = [rows[1][side[0]] for side in rows[19][first:first + count]]
            require(any(sum(normal * (low[axis] if normal >= 0 else high[axis]) for axis, normal in enumerate(plane[:3])) > plane[3] for plane in planes), "Whole pool strictly separated from every world brush")
        for point in query_points(owner):
            position = point["position"]
            require(rows[10][point_leaf(rows, 0, position)][0] == 0, "All query points in open dry world")
            for selected in range(2):
                local = [a - b for a, b in zip(position, ORIGINS[selected], strict=True)]
                actual = rows[10][point_leaf(rows, selected + 1, local)][0]
                require(actual == (contents if selected == point["owner"] else 0), "Independent point/owner tree classification")
    return {"entries": entries, "rows": rows, "entities": authored, "owners": owners, "targets": targets, "physics": physics(raw, entries), "world_headnode": rows[14][0][9]}


def normalized_game_lump(raw, entries):
    data = bytearray(payload(raw, entries, 35))
    require(entries[35][2] == 0 and len(data) >= 4, "Game-lump framing")
    count = struct.unpack_from("<i", data)[0]
    require(0 <= count <= 4096 and 4 + count * 16 <= len(data), "Game-lump directory budget")
    ranges, identities = [], set()
    for index in range(count):
        position = 4 + index * 16
        identity, flags, _, absolute, size = struct.unpack_from("<IHHii", data, position)
        relative = absolute - entries[35][0]
        require(identity not in identities and flags == 0 and size >= 0 and 4 + count * 16 <= relative <= len(data) - size, "Unique uncompressed in-payload game lump")
        identities.add(identity)
        if size:
            ranges.append((relative, relative + size))
        struct.pack_into("<i", data, position + 8, relative)
    ranges.sort()
    require(all(before[1] <= after[0] for before, after in zip(ranges, ranges[1:])), "Disjoint game-lump entries")
    return bytes(data)


def compare_pair(dry_path, water_path):
    dry_raw, water_raw = read_regular(dry_path), read_regular(water_path)
    dry, water = geometry(dry_raw, False), geometry(water_raw, True)
    require(dry_raw[:8] == water_raw[:8] and dry_raw[1032:1036] == water_raw[1032:1036] and dry["owners"] == water["owners"] and dry["targets"] == water["targets"], "Header/recomputed complete per-model ownership identity")
    expected_entities = [dict(entity) for entity in dry["entities"]]
    for target in dry["targets"]:
        expected_entities[target["entity"]]["classname"] = "func_water_analog"
    require(water["entities"] == expected_entities, "Only both owned classnames change semantically")
    comparisons = []
    for index, (before_entry, after_entry) in enumerate(zip(dry["entries"], water["entries"], strict=True)):
        require(before_entry[2:] == after_entry[2:], "Lump metadata equality")
        before, after = payload(dry_raw, dry["entries"], index), payload(water_raw, water["entries"], index)
        if index == 0:
            comparison = "parsed entities; only two owned classnames"
        elif index in (10, 18):
            expected = bytearray(before)
            for target in dry["targets"]:
                offset = target["brush"] * 12 + 8 if index == 18 else target["interior"][0] * struct.calcsize(FORMATS[10][0])
                require(struct.unpack_from("<i", expected, offset)[0] == 1, "Selected dry SOLID contents")
                struct.pack_into("<i", expected, offset, 32)
            require(after == expected, "Only two exact owned WATER contents words")
            comparison = "complete payload except two owned contents words"
        elif index == 35:
            require(normalized_game_lump(dry_raw, dry["entries"]) == normalized_game_lump(water_raw, water["entries"]), "Complete normalized game-lump payload equality")
            comparison = "complete payload with own-base absolute directory relocation"
        else:
            require(before == after, "Unexpected complete payload difference: " + str(index))
            comparison = "complete byte equality"
        comparisons.append({"lump": index, "comparison": comparison, "dry_sha256": hashlib.sha256(before).hexdigest(), "water_sha256": hashlib.sha256(after).hexdigest()})
    require(dry["physics"] == water["physics"], "Complete PHY, framing, keys and native f32 volume equality")
    require(read_regular(dry_path) == dry_raw and read_regular(water_path) == water_raw, "Pair input custody")
    return {"schema": SCHEMA, "success": True, "dry": str(dry_path), "dry_sha256": hashlib.sha256(dry_raw).hexdigest(), "water": str(water_path), "water_sha256": hashlib.sha256(water_raw).hexdigest(), "owners": dry["owners"], "targets": dry["targets"], "world_headnode": dry["world_headnode"], "physics": dry["physics"], "lumps": comparisons, "complete_phy_identical": True, "native_qualified": False}


def source_state(client, ids):
    from validate_face_lighting import detail

    return {"map_info": client.call("get_map_info"), "history": client.call("get_history"), "settings": client.call("get_compile_settings"), "objects": detail(client, ids)}


def same_state(before, after):
    require(before["objects"] == after["objects"] and before["history"] == after["history"] and before["settings"] == after["settings"], "Source objects/history/settings custody")
    require(all(before["map_info"][key] == after["map_info"][key] for key in STATE_FIELDS), "Source map/selection/camera/view custody")


def create_fixture(binary, pins_path, gameinfo, output, water):
    from contact_runtime_observer import MODULES
    from validate_face_lighting import Client, detail

    sources = helper_pins()
    build = build_pins(pins_path, binary)
    worker = binary.with_name("hammerr-physics").resolve(strict=True)
    require(not output.exists(), "Fresh fixture output")
    output.mkdir(parents=True)
    stores = output / "stores"
    for name in ("recent", "prefabs", "recovery", "config", "state", "cache", "data", "tmp"):
        (stores / name).mkdir(parents=True)
    environment = {"HAMMERR_RECENT_MAPS_DIR": str(stores / "recent"), "HAMMERR_BUILD_RECIPES": str(stores / "recipes.json"), "HAMMERR_PREFABS_DIR": str(stores / "prefabs"), "HAMMERR_RECOVERY_DIR": str(stores / "recovery"), "XDG_CONFIG_HOME": str(stores / "config"), "XDG_STATE_HOME": str(stores / "state"), "XDG_CACHE_HOME": str(stores / "cache"), "XDG_DATA_HOME": str(stores / "data"), "TMPDIR": str(stores / "tmp"), "HAMMERR_PHYSICS_WORKER": str(worker)}
    modules = {}
    for name, (relative, expected, _) in MODULES.items():
        path = (gameinfo.parent.parent / relative).resolve(strict=True)
        require(digest(path) == expected, "Installed cook/runtime module identity")
        modules[name] = {"path": str(path), "sha256": expected}
    role = "water" if water else "dry"
    recipe = {"schema": SCHEMA, "role": role, "build": build, "binary": str(binary), "binary_sha256": digest(binary), "worker": str(worker), "worker_sha256": digest(worker), "gameinfo": str(gameinfo), "gameinfo_sha256": digest(gameinfo), "modules": modules, "helpers": sources, "environment": environment, "settings": SETTINGS, "room": ROOM, "targets": TARGETS, "origins": ORIGINS, "mins": MINS, "maxs": MAXS, "material": MATERIAL}
    write_json(output / "recipe.json", recipe)
    result = {"schema": SCHEMA, "success": False, "recipe": recipe}
    client = None
    vmf, bsp = output / f"{role}.vmf", output / f"{role}.bsp"
    try:
        client = Client(None, binary, output, gameinfo=gameinfo, environment=environment)
        client.call("new_map", {"discard_changes": True})
        result["material_sources"] = {}
        for material in ("dev/dev_measuregeneric01", MATERIAL):
            mounted = client.call("get_material_source", {"material": material})
            require(mounted["error"] is None and mounted["resolved"] is not None and mounted["raw_source"] is not None, "Mounted actual material without fallback")
            result["material_sources"][material] = mounted
        settings = client.call("set_compile_settings", SETTINGS)
        require(all(settings[key] == value for key, value in SETTINGS.items()), "Explicit Engine compile settings")
        ids = [client.call("create_block", {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"})["id"] for low, high in ROOM]
        ids.append(client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-1536, 768, 8]})["id"])
        ids.append(client.call("create_entity", {"classname": "light", "origin": [0, 0, 640], "properties": {"_light": "255 255 255 1500", "_quadratic_attn": "1"}})["id"])
        owner_ids, creation = [], []
        classname = "func_water_analog" if water else "func_movelinear"
        for name, origin in zip(TARGETS, ORIGINS, strict=True):
            low, high = ([a + b for a, b in zip(origin, bound, strict=True)] for bound in (MINS, MAXS))
            block = client.call("create_block", {"mins": low, "maxs": high, "material": MATERIAL})["id"]
            properties = {"targetname": name} if water else {"targetname": name, "origin": " ".join(map(str, origin)), "angles": "0 0 0", "StartPosition": "0", "spawnflags": "0"}
            entity = client.call("tie_to_entity", {"ids": [block], "classname": classname, "properties": properties})["id"]
            item = detail(client, [entity])[0]
            expected = {"classname": classname, "targetname": name, "origin": " ".join(map(str, origin)), "angles": "0 0 0", "startposition": "0", "spawnflags": "0"}
            if not water:
                inherited = [key for key, _ in item["properties"] if key.casefold() not in expected]
                if inherited:
                    client.call("set_entity_properties", {"id": entity, "remove": inherited})
                    item = detail(client, [entity])[0]
            actual = {key.casefold(): value for key, value in item["properties"]}
            require(actual == expected and len(actual) == len(item["properties"]) and not item["connections"] and len(item["brushes"]) == 1, "Actual shared defaults and exact neutral owner properties")
            faces = item["brushes"][0]["faces"]
            require(len(faces) == 6 and all(face["material"] == MATERIAL and face["displacement"] is None for face in faces), "Six ordinary authored nodraw faces")
            creation.append(item)
            owner_ids.append(entity)
            ids.append(entity)
        result["creation"] = creation
        client.call("save_map", {"path": str(vmf)})
        client.call("open_map", {"path": str(vmf)})
        saved = read_regular(vmf)
        before = source_state(client, ids)
        write_json(output / "source-before.json", before)
        compiled = client.call("compile_map", {"path": str(bsp)})
        write_json(output / "compile-result.json", compiled)
        require(compiled["success"] is True and compiled["leak"] is None, "Sealed two-owner Engine compile")
        stats = compiled["stats"]
        require(stats["physics_mode"] == "engine" and stats["lighting_mode"] == "direct_ldr" and (stats["physics_models"], stats["physics_solids"], stats["physics_convexes"]) == (3, 3, 8), "Six world convexes and two ordinary inline solids")
        framing = geometry(read_regular(bsp), water)
        after = source_state(client, ids)
        same_state(before, after)
        require(read_regular(vmf) == saved, "Saved VMF custody")
        write_json(output / "source-after.json", after)
        result.update({"compiled": compiled, "source_before": before, "source_after": after, "vmf": str(vmf), "vmf_sha256": digest(vmf), "bsp": str(bsp), "bsp_sha256": digest(bsp), "targets": framing["targets"], "world_headnode": framing["world_headnode"], "physics": framing["physics"]})
        if water:
            prior_bsp = read_regular(bsp)
            lin = bsp.with_suffix(".lin")
            lin.write_bytes(b"748 prior LIN preservation sentinel\n")
            prior_lin = read_regular(lin)
            client.call("set_compile_settings", {"physics": "none"})
            refused_before = source_state(client, ids)
            failure = json.loads(client.call("compile_map", {"path": str(bsp)}, error=True)["content"][0]["text"])
            errors = [item for item in failure["diagnostics"] if item["level"] == "error"]
            require(failure["success"] is False and {item["object"] for item in errors if "func_water_analog" in item["message"] and "CS:S engine physics is required" in item["message"]} == set(owner_ids), "Explicit source-owned Engine physics prerequisite refusal for both pools")
            same_state(refused_before, source_state(client, ids))
            require(read_regular(bsp) == prior_bsp and read_regular(lin) == prior_lin and read_regular(vmf) == saved, "Refusal preserves prior BSP/LIN/source")
            client.call("set_compile_settings", SETTINGS)
            same_state(before, source_state(client, ids))
            result["physics_none_refusal"] = failure
        result["success"] = True
    except BaseException as error:
        result.update({"success": False, "error": str(error), "error_type": type(error).__name__})
        raise
    finally:
        try:
            if client is not None:
                code, trailing = client.close_with_status(timeout=15)
                result.update({"mcp_exit_code": code, "mcp_trailing_stdout": trailing})
                require(code == 0 and not trailing.strip(), "Actual MCP clean EOF drain/reap")
            expected = {str(binary): recipe["binary_sha256"], str(worker): recipe["worker_sha256"], str(gameinfo): recipe["gameinfo_sha256"], **{entry["path"]: entry["sha256"] for entry in modules.values()}}
            if vmf.is_file() and "vmf_sha256" in result:
                expected[str(vmf)] = result["vmf_sha256"]
            result["custody_after_eof"] = {name: digest(Path(name)) for name in expected}
            require(result["custody_after_eof"] == expected and helper_pins() == sources and build_pins(pins_path, binary) == build, "Executable/worker/source/module/helper custody after EOF")
        except BaseException:
            result["success"] = False
            raise
        finally:
            write_json(output / "fixture-receipt.json", result)
    return result


def verify_receipt(path, water):
    result = json.loads(read_regular(path, 8 * 1024 * 1024))
    require(result["schema"] == SCHEMA and result["success"] is True and result["mcp_exit_code"] == 0 and not result["mcp_trailing_stdout"].strip(), "Successful actual748 MCP receipt and EOF")
    recipe = result["recipe"]
    require(json.loads(read_regular(path.parent / "recipe.json", 8 * 1024 * 1024)) == recipe and json.loads(read_regular(path.parent / "compile-result.json", 8 * 1024 * 1024)) == result["compiled"], "Actual recipe and compiler result artifacts")
    require(json.loads(read_regular(path.parent / "source-before.json", 8 * 1024 * 1024)) == result["source_before"] and json.loads(read_regular(path.parent / "source-after.json", 8 * 1024 * 1024)) == result["source_after"], "Actual source/history/settings/camera artifacts")
    require(recipe["schema"] == SCHEMA and recipe["role"] == ("water" if water else "dry") and recipe["settings"] == SETTINGS and recipe["helpers"] == helper_pins(), "Exact fresh recipe/helper custody")
    require(recipe["targets"] == list(TARGETS) and recipe["origins"] == [list(origin) for origin in ORIGINS] and recipe["mins"] == list(MINS) and recipe["maxs"] == list(MAXS) and recipe["room"] == [[list(low), list(high)] for low, high in ROOM] and recipe["material"] == MATERIAL, "Exact independent fixed authoring recipe")
    binary = Path(recipe["binary"]).resolve(strict=True)
    require(build_pins(Path(recipe["build"]["path"]), binary) == recipe["build"], "Actual current production build custody")
    expected_eof = {recipe["binary"]: recipe["binary_sha256"], recipe["worker"]: recipe["worker_sha256"], recipe["gameinfo"]: recipe["gameinfo_sha256"], result["vmf"]: result["vmf_sha256"], **{entry["path"]: entry["sha256"] for entry in recipe["modules"].values()}}
    require(result["custody_after_eof"] == expected_eof, "Complete exact post-EOF identity set")
    for name, expected in expected_eof.items():
        require(digest(Path(name)) == expected, "Actual post-EOF readback")
    require(digest(Path(result["vmf"])) == result["vmf_sha256"] and digest(Path(result["bsp"])) == result["bsp_sha256"], "Actual source/BSP bytes")
    same_state(result["source_before"], result["source_after"])
    stats = result["compiled"]["stats"]
    require(result["compiled"]["success"] is True and result["compiled"]["leak"] is None and stats["physics_mode"] == "engine" and stats["lighting_mode"] == "direct_ldr" and (stats["physics_models"], stats["physics_solids"], stats["physics_convexes"]) == (3, 3, 8), "Actual sealed Engine cook receipt")
    framing = geometry(read_regular(result["bsp"]), water)
    require(json.loads(json.dumps(framing["targets"])) == result["targets"] and framing["physics"] == result["physics"] and framing["world_headnode"] == result["world_headnode"], "Recomputed independent ownership/PHY")
    if water:
        require(result["physics_none_refusal"]["success"] is False, "Retained actual unsupported physics refusal")
    return result


def main():
    parser = argparse.ArgumentParser(description="Author independent dry and two stationary invisible Water fixtures through both actual MCP executables, verify exact per-owner BSP/complete PHY differences and Engine-physics refusal. Does not build binaries or launch games. Requires matching748 build manifests, installed CS:S and fresh lowercase on-disk target output.")
    parser.add_argument("--headless-binary", type=Path, required=True)
    parser.add_argument("--editor-binary", type=Path, required=True, help="Actual editor executable started in headless MCP mode by the existing Client")
    parser.add_argument("--headless-build-pins", type=Path, required=True, help="Root-owned schema748 binary/physics_worker/sources manifest")
    parser.add_argument("--editor-build-pins", type=Path, required=True)
    parser.add_argument("--game-dir", type=Path, required=True, help="Installed CS:S root containing cstrike/gameinfo.txt")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    require(not sys.flags.optimize and not os.environ.get("LD_PRELOAD"), "Unoptimized Python and no inherited LD_PRELOAD")
    output = args.output.resolve()
    require(not output.exists() and output.is_relative_to(ROOT / "target") and str(output).lower() == str(output), "Fresh lowercase output under target")
    binaries = (args.headless_binary.resolve(strict=True), args.editor_binary.resolve(strict=True))
    require(binaries[0] != binaries[1] and tuple(binary.name for binary in binaries) == ("hammerr-headless", "editor"), "Both distinct actual MCP executable programs")
    output.mkdir(parents=True)
    report = {"schema": SCHEMA, "success": False}
    try:
        pairs = {}
        for label, binary, pins in zip(("headless", "editor"), binaries, (args.headless_build_pins.resolve(strict=True), args.editor_build_pins.resolve(strict=True)), strict=True):
            fixtures = [create_fixture(binary, pins, (args.game_dir / "cstrike/gameinfo.txt").resolve(strict=True), output / label / role, water) for role, water in (("dry", False), ("water", True))]
            for role, water in (("dry", False), ("water", True)):
                verify_receipt(output / label / role / "fixture-receipt.json", water)
            pair = compare_pair(Path(fixtures[0]["bsp"]), Path(fixtures[1]["bsp"]))
            write_json(output / label / "pair-verification.json", pair)
            pairs[label] = pair
        report.update({"success": True, "pairs": pairs, "limits": "Actual MCP authoring/native cook and offline BSP ownership/complete PHY comparison. Native simultaneous body/controller/client/query/player qualification requires separately reviewed observers."})
    except BaseException as error:
        report.update({"error": str(error), "error_type": type(error).__name__})
        raise
    finally:
        write_json(output / "results.json", report)


if __name__ == "__main__":
    main()
