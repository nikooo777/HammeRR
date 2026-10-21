import argparse
import json
import struct
from pathlib import Path

import validate_cubemap_capture_probe as probe
from prop_indirect_fixtures import model as fixture_model
from validate_compile_jobs import stopped_on_exit
from validate_compile_regions import apply as apply_region
from validate_face_lighting import DEFAULT_BINARY, ROOT
from validate_instance_lighting import entity_rows, stage_property
from validate_instance_terrain import private_client, property_state, write_json
from validate_material_albedo import (
    compile_pair,
    digest,
    fixture_texture,
    floor_base,
    identities,
    require,
    settings,
)
from validate_nested_brush_instances import FULL, assets, room, source_state
from validate_prop_vertex_lighting import MANIFEST, VHV_NAME, pak_and_flags, read_vhv
from validate_recovery_exit import require_clean_eof
from validate_static_props import static_records

MODELS = ("models/hammerr719/first.mdl", "models/hammerr719/second.mdl")
INVALID = "models/hammerr719/omitted.mdl"
SOURCE = (((-24, 16, 8), (0, 0, 0)), ((40, -8, 24), (0, 0, -90)))
LITERALS = {
    "translation": (((-8, 8, 40), (0, 0, 0)), ((56, -16, 56), (0, 0, -90))),
    "noncommuting": (((32, -96, 160), (0, 90, 90)), ((48, -32, 136), (0, 90, 0))),
}
DUAL = ("direct_ldr_hdr", 0)
POINTFILE = b"13 17 19\n23 29 31\n"
SPRP = int.from_bytes(b"sprp", "big")
POSE_ERROR = 0.00001


def small_files(directory):
    entries = list(directory.rglob("*"))
    paths = [path for path in entries if path.is_file()]
    require(all(not path.is_symlink() for path in entries) and len(paths) <= 256
            and all(path.stat().st_size <= 16 * 1024 * 1024 for path in paths)
            and sum(path.stat().st_size for path in paths) <= 64 * 1024 * 1024,
            "Use a bounded standalone baked fixture, at most256 files/64MiB; no installed game tree")
    return identities(directory)


def game_assets(args):
    game = args.output / "game"
    assets(game)
    (game.parent / "cstrike_linux64").unlink()
    for index, model in enumerate(MODELS):
        low, high = ((-16, -8, 0), (16, 8, 32)) if index == 0 else ((-12, -6, -4), (20, 10, 28))
        points = [(low[0] + 2, low[1] + 2, 4), (high[0] - 2, low[1] + 2, 4),
                  (high[0] - 2, high[1] - 2, 4), (low[0] + 2, high[1] - 2, 4)]
        stem = Path(model).stem
        fixture_model(game, stem, points)
        path = game / model
        path.parent.mkdir(parents=True, exist_ok=True)
        for suffix, checksum_offset in ((".mdl", 8), (".vvd", 8), (".dx90.vtx", 16)):
            generated = game / f"models/oracle/{stem}{suffix}"
            data = bytearray(generated.read_bytes())
            struct.pack_into("<i", data, checksum_offset, 719 + index)
            if suffix == ".mdl":
                require(len(data) == 1024, "Complete tiny fixture must retain its declared MDL length")
                for offset in (104, 128):
                    struct.pack_into("<6f", data, offset, *low, *high)
                data[752:784] = b"models/hammerr719\0".ljust(32, b"\0")
            (path.parent / f"{stem}{suffix}").write_bytes(data)
            generated.unlink()
    (game / "models/oracle").rmdir()
    material = game / "materials/models/hammerr719/receiver.vmt"
    material.parent.mkdir(parents=True)
    material.write_text('VertexLitGeneric { $model 1 $basetexture "hammerr719/white" }\n')
    base = game / "materials/hammerr719/white.vtf"
    base.parent.mkdir(parents=True)
    base.write_bytes(fixture_texture(2, 2, (0.5, 0.5, 0.5), (255, 255, 255, 255)))
    (game / INVALID).write_bytes(b"Private omitted invalid MDL\n")
    tools = game / "materials/tools"
    tools.mkdir()
    (tools / "toolsblack.vtf").write_bytes(fixture_texture(2, 2, (0, 0, 0), (0, 0, 0, 255)))
    (tools / "toolsblack.vmt").write_text('LightmappedGeneric { $basetexture "tools/toolsblack" }\n')
    if args.baked_fixture_game:
        fixture = str(args.baked_fixture_game)
        require('"' not in fixture, "Baked fixture path cannot contain a quote")
        (game / "gameinfo.txt").write_text(
            'GameInfo { FileSystem { SearchPaths { Game "|gameinfo_path|." '
            f'Game "{fixture}" }} }} }}\n')
    return game


def prop(client, model, origin, angles, extra=None, classname="prop_static"):
    values = {"model": model, "angles": " ".join(map(str, angles)), "solid": "0", "body": "0", "skin": "0",
              "modelscale": "1", "rendermode": "0", "renderfx": "0", "renderamt": "255", "rendercolor": "255 255 255"}
    values.update(extra or {})
    return client.call("create_entity", {"classname": classname, "origin": origin, "properties": values})["id"]


def leaf(client, path, models=MODELS, omitted=None):
    client.call("new_map", {"discard_changes": True})
    ids = [prop(client, model, origin, angles) for model, (origin, angles) in zip(models, SOURCE, strict=True)]
    if omitted:
        extra = prop(client, INVALID, [512, 0, 32] if omitted == "region" else [0, 0, 16], [0, 0, 0])
        if omitted == "hidden":
            client.call("set_object_visibility", {"action": "hide", "ids": [extra]})
    client.call("save_map", {"path": str(path)})
    return ids


def middle(client, path, child, origin=(8, 16, 64), angles="90 0 0"):
    client.call("new_map", {"discard_changes": True})
    client.call("create_entity", {"classname": "func_instance", "origin": origin,
        "properties": {"file": child.name, "angles": angles}})
    client.call("save_map", {"path": str(path)})


def parent(client, path, placement, child=None, models=MODELS, region=False, bounds=False):
    client.call("new_map", {"discard_changes": True})
    ids = room(client)
    owner = None
    if child is None:
        ids.extend(prop(client, model, origin, angles) for model, (origin, angles) in zip(models, LITERALS[placement], strict=True))
    else:
        origin, angles = ((16, -8, 32), "0 0 0") if placement == "translation" else ((16, -8, 128), "0 0 90")
        if bounds:
            origin, angles = (-16376, 0, 0), "0 0 0"
        owner = client.call("create_entity", {"classname": "func_instance", "origin": origin,
            "properties": {"file": child.name, "angles": angles}})["id"]
        ids.append(owner)
    client.call("select", {"ids": [owner] if owner else ids[:1]})
    if region:
        apply_region(client, low=[-128, -128, 0], high=[128, 128, 224])
    client.call("save_map", {"path": str(path)})
    return {"source": path, "ids": ids, "owner": owner}


def setup(client, mode=FULL, bake=False):
    settings(client, mode)
    client.call("set_compile_settings", {"pack_water_assets": False, "bake_static_prop_vertices": bake})
    if not any(row["name"] == "Instance prop oracle" for row in client.call("get_build_recipes")["recipes"]):
        client.call("save_build_recipe", {"name": "Instance prop oracle"})


def pair(client, args, game, label, control, sources, failure=None, prior=None, leaf_name=None):
    output = args.output / "builds"
    inputs = {str(path): digest(path.read_bytes()) for path in sources}
    draft = property_state(client)
    for suffix in ("", "-background"):
        (output / f"{label}{suffix}.lin").write_bytes(POINTFILE)
    with stopped_on_exit(client):
        data, receipt = compile_pair(client, output, label, control["ids"], control["source"], game, failure, prior)
    require(len(data) <= 16 * 1024 * 1024 and property_state(client) == draft
            and all(digest(Path(path).read_bytes()) == value for path, value in inputs.items()),
            "Child-prop compile exceeded its BSP bound or changed its draft/dependencies")
    require(all((output / f"{label}{suffix}.lin").read_bytes() == POINTFILE for suffix in ("", "-background")),
            "Child-prop publication/refusal changed prior LIN bytes")
    require(receipt["result"]["game"] is None and receipt["background"]["native_runtime"] is None,
            "Compiler-only acceptance admitted native game activity")
    if failure:
        require(all(any(row["level"] == "error" and row["object"] == control["owner"]
                        and failure in row["message"] and leaf_name in row["message"] for row in result["diagnostics"])
                    for result in (receipt["result"], receipt["background"]["result"])),
                "Child-prop refusal lost the root helper, true leaf path or contextual reason")
    receipt["instance_sources"] = inputs
    receipt["property_draft_present"] = draft is not None
    receipt["property_draft_preserved"] = True
    return data, receipt


def counts(receipt, sources, depth, expanded=2):
    dependencies = [{"path": path.name, "bytes": path.stat().st_size, "sha256": digest(path.read_bytes())} for path in sorted(sources)]
    for result in (receipt["result"], receipt["background"]["result"]):
        stats = result["stats"]["instances"]
        require([stats[key] for key in ("instances", "files", "max_expansion_depth", "world_brushes", "detail_brushes", "static_props")]
                == [depth, depth, depth, 0, 0, expanded] and result["stats"]["static_props"] == 2,
                "Instance prop counts changed or were filtered before expansion accounting")
        require(stats["dependencies"] == dependencies, "Instance prop dependency hashes differ from saved child files")


def unread_model(client, args, game, source):
    before = source_state(client, source), property_state(client), client.call("get_compile_settings")
    outputs = identities(args.output / "builds")
    invalid = game / INVALID
    original = invalid.read_bytes()
    try:
        invalid.write_bytes(b"Another private invalid unused MDL\n")
        status = client.call("get_compile_status")
        require(not status["job"]["stale"] and status["job"]["static_prop_source_issue"] is None
                and status["job"]["instance_dependencies"]["current"],
                "Hidden/region-omitted invalid MDL was consumed or guarded")
        require((source_state(client, source), property_state(client), client.call("get_compile_settings")) == before
                and identities(args.output / "builds") == outputs,
                "Unused model inspection changed source/history/settings/draft or prior BSP/LIN")
        return status
    finally:
        invalid.write_bytes(original)


def native(data, placement, models):
    result = static_records(data)
    dictionary = list(dict.fromkeys(models))
    require(result["models"] == dictionary and len(result["props"]) == 2, "Native sprp6 dictionary/ordinal count differs")
    for ordinal, (row, (origin, angles), model) in enumerate(zip(result["props"], LITERALS[placement], models, strict=True)):
        require(max(abs(a - b) for a, b in zip(row[:3], origin, strict=True)) <= POSE_ERROR
                and max(abs((a - b + 180) % 360 - 180) for a, b in zip(row[3:6], angles, strict=True)) <= POSE_ERROR,
                f"Native prop ordinal{ordinal} differs from independently authored literal origin/angles")
        require(row[6] == dictionary.index(model) and row[8] > 0 and row[9] == row[11] == 0
                and row[7] + row[8] <= len(result["leaves"]), "Native prop model index/leaf span/solid0/skin0 differs")
    require(not any(row.get("classname") in ("func_instance", "prop_static") for row in entity_rows(data)),
            "Compiler retained a helper or prop_static as a runtime entity")
    return result


def compare(actual, literal, placement, models, baked=False):
    first, second = native(actual, placement, models), native(literal, placement, models)
    require(first["models"] == second["models"] and first["leaves"] == second["leaves"]
            and all(a[6:] == b[6:] for a, b in zip(first["props"], second["props"], strict=True)),
            "Original sprp6 dictionary, ordinals, leaf lists or non-pose metadata differs from the literal control")
    left, right = probe.game_lump_records(actual)[1], probe.game_lump_records(literal)[1]
    require([(row[:4]) for row in left] == [(row[:4]) for row in right], "Native game-lump directory identity changed")
    for a, b in zip(left, right, strict=True):
        if a[0] == SPRP:
            prefix = 12 + 128 * len(first["models"]) + 2 * len(first["leaves"])
            require(a[4][:prefix] == b[4][:prefix] and all(
                a[4][offset + 24:offset + 64] == b[4][offset + 24:offset + 64]
                for offset in range(prefix, len(a[4]), 64)), "Original sprp6 bytes outside literal-checked poses differ")
        else:
            require(a == b, "A non-prop native game-lump payload differs")
    require(entity_rows(actual) == entity_rows(literal), "Flattening changed a non-prop runtime entity")
    a, b = probe.bsp_lumps(actual), probe.bsp_lumps(literal)
    excluded = {0, 35, 40} if baked else {0, 35}
    require(all(a[index] == b[index] for index in range(64) if index not in excluded),
            "Flattening changed geometry, face lightmaps or an unrelated BSP lump")
    return {"nested_native": first, "literal_native": second, "pose_error_limit": POSE_ERROR,
            "scope": "Original sprp6 model dictionary and leaf/ordinal/non-pose bytes match exactly; poses independently match literals. BSP bytes are retained without rewriting; game-lump absolute offsets may relocate with entity text."}


def baked(data, stats, args, model):
    entries, flags = pak_and_flags(data)
    require(flags & 3 == 3 and MANIFEST in entries, "Optional real dual-channel prop bake was not published")
    manifest = json.loads(entries[MANIFEST])
    require(manifest["version"] == 1 and manifest["channels"] == ["ldr", "hdr"]
            and [row["ordinal"] for row in manifest["receivers"]] == [0, 1],
            "Optional bake lost expanded prop ordinals or one lighting channel")
    mdl = probe.read_bounded(args.baked_fixture_game / model, 16 * 1024 * 1024)
    vtx = probe.read_bounded(args.baked_fixture_game / (model[:-4] + ".dx90.vtx"), 16 * 1024 * 1024)
    checksum = struct.unpack_from("<I", mdl, 8)[0]
    require(len(vtx) >= 36 and struct.unpack_from("<I", vtx, 16)[0] == checksum,
            "Optional real VTX checksum differs from the original MDL")
    lods = struct.unpack_from("<i", vtx, 20)[0]
    require(1 <= lods <= 8, "Optional real fixture draw-LOD count is unsupported")
    record = next(row for row in probe.game_lump_records(data)[1] if row[0] == SPRP)
    require(manifest["static_props_version"] == 6 and manifest["static_props_sha256"] == digest(record[4]),
            "Optional VHV manifest does not identify the actual original sprp6 bytes")
    colors, reports, illuminated = {}, {}, False
    for receiver in manifest["receivers"]:
        require(receiver["model"] == model and receiver["checksum"] == checksum and receiver["meshes"]
                and {mesh["lod"] for mesh in receiver["meshes"]} == set(range(lods)),
                "Optional bake omitted a real model/checksum/hardware draw LOD")
        for channel in ("ldr", "hdr"):
            name = f"sp_{'hdr_' if channel == 'hdr' else ''}{receiver['ordinal']}.vhv"
            asset = next(row for row in manifest["assets"] if row["path"] == name)
            measured, rgba = read_vhv(entries[name], checksum, receiver["meshes"])
            require(asset["bytes"] == measured["bytes"] and asset["sha256"] == measured["sha256"],
                    "Optional real VHV is missing or corrupt")
            illuminated |= any(any(color[:3]) for color in rgba)
            reports[name], colors[name] = measured, entries[name]
    require(set(colors) == {name for name in entries if VHV_NAME.fullmatch(name)} and len(manifest["assets"]) == 4
            and stats["prop_vertex_lighting"]["props"] == 2 and illuminated
            and any(colors[f"sp_{ordinal}.vhv"] != colors[f"sp_hdr_{ordinal}.vhv"] for ordinal in (0, 1)),
            "Optional bake has missing/extra files/counts or no discriminating authored channel illumination")
    for source in manifest["sources"]:
        require(not Path(source["path"]).is_absolute() and ".." not in Path(source["path"]).parts
                and "\\" not in source["path"], "Optional bake emitted an unsafe source path")
        path = args.baked_fixture_game / source["path"]
        private = args.output / "game" / source["path"]
        raw = probe.read_bounded(private if private.is_file() else path, 16 * 1024 * 1024)
        require(len(raw) == source["bytes"] and digest(raw) == source["sha256"], "Optional bake read receipt differs from its mounted fixture")
    return {"manifest": manifest, "vhv": reports}, entries


def control_pair(client, args, game, label, placement, child, sources, models, mode, bake=False, region=False):
    nested = parent(client, args.output / f"{label}-parent.vmf", placement, child, models, region)
    flat = parent(client, args.output / f"{label}-flat.vmf", placement, models=models, region=region)
    client.call("open_map", {"path": str(nested["source"]), "discard_changes": True})
    setup(client, mode, bake)
    token = stage_property(client)
    data, receipt = pair(client, args, game, label, nested, sources)
    counts(receipt, sources, 2 if placement == "noncommuting" else 1, 3 if region else 2)
    if region:
        receipt["unused_model_status"] = unread_model(client, args, game, nested["source"])
    client.call("cancel_property_edit", {"token": token})
    client.call("open_map", {"path": str(flat["source"]), "discard_changes": True})
    setup(client, mode, bake)
    manual, comparison = pair(client, args, game, f"{label}-flat", flat, sources)
    require("instances" not in comparison["result"]["stats"], "Literal control acquired instance metadata")
    receipt["comparison"] = compare(data, manual, placement, models, bake)
    if mode == DUAL:
        receipt["floor"] = {}
        for channel in ("ldr", "hdr"):
            actual, expected = floor_base(data, channel), floor_base(manual, channel)
            require(actual == expected, "Nested/flat baked floor samples differ")
            receipt["floor"][channel] = {"samples": len(actual), "sha256": digest(json.dumps(actual).encode())}
    if bake:
        receipt["bake"], a = baked(data, receipt["result"]["stats"], args, models[0])
        comparison["bake"], b = baked(manual, comparison["result"]["stats"], args, models[0])
        require({key: value for key, value in a.items() if key != MANIFEST}
                == {key: value for key, value in b.items() if key != MANIFEST}, "Optional nested/flat packed VHV bytes differ")
        require({key: value for key, value in receipt["bake"]["manifest"].items() if key != "static_props_sha256"}
                == {key: value for key, value in comparison["bake"]["manifest"].items() if key != "static_props_sha256"},
                "Optional bake manifests differ outside their independently verified original native-table hashes")
    receipt["flat"] = comparison
    return data, receipt, nested


def refusal_cases(client, author, args, game, child, ids, control, prior, report):
    client.call("open_map", {"path": str(control["source"]), "discard_changes": True})
    setup(client)
    token = stage_property(client)
    original = child.read_bytes()
    for label, changes, reason in (
        ("unsupported-class", None, "unsupported"),
        ("legacy-pose", {"angle": "90"}, "requires angle 0"),
        ("unsupported-property", {"hammerr719_unsupported": "1"}, "property hammerr719_unsupported is unsupported"),
        ("missing-model", {"model": "models/hammerr719/missing.mdl"}, "was not found"),
    ):
        author.call("open_map", {"path": str(child), "discard_changes": True})
        if changes is None:
            prop(author, MODELS[0], [0, 0, 16], [0, 0, 0], classname="prop_dynamic")
        else:
            author.call("set_entity_properties", {"id": ids[0], "set": changes})
        author.call("save_map")
        try:
            _, report[label] = pair(client, args, game, label, control, [child], reason, prior, child.name)
        finally:
            child.write_bytes(original)
    client.call("cancel_property_edit", {"token": token})
    bad_leaf, bad_middle = args.output / "bounds-leaf.vmf", args.output / "bounds-middle.vmf"
    author.call("new_map", {"discard_changes": True})
    prop(author, MODELS[0], [0, 0, 8], [0, 0, 0])
    author.call("save_map", {"path": str(bad_leaf)})
    middle(author, bad_middle, bad_leaf, origin=[16376, 0, 0], angles="0 0 0")
    bad = parent(author, args.output / "bounds-parent.vmf", "translation", bad_middle, bounds=True)
    client.call("open_map", {"path": str(bad["source"]), "discard_changes": True})
    setup(client)
    stage_property(client)
    _, report["intermediate-model-box"] = pair(client, args, game, "intermediate-model-box", bad,
        [bad_leaf, bad_middle], "intermediate pose 1", prior, bad_leaf.name)


def source_drift(client, args, game, control, child, prior):
    client.call("open_map", {"path": str(control["source"]), "discard_changes": True})
    setup(client)
    stage_property(client)
    data, before = pair(client, args, game, "source-before", control, [child])
    require(data == prior, "Original child-prop control changed before source ownership check")
    state = source_state(client, control["source"]), property_state(client), client.call("get_compile_settings")
    outputs = identities(args.output / "builds")
    model = game / MODELS[0]
    original = model.read_bytes()
    changed = bytearray(original)
    changed[20] ^= 1
    try:
        model.write_bytes(changed)
        stale = client.call("get_compile_status")
        issue = stale["job"]["static_prop_source_issue"]
        require(stale["job"]["stale"] and stale["job"]["instance_dependencies"]["current"]
                and isinstance(issue, str) and MODELS[0] in issue
                and not stale["launch"]["ready"] and issue in stale["launch"]["reason"],
                "Nested model drift lost original717 source ownership or was confused with child VMF drift")
        require((source_state(client, control["source"]), property_state(client), client.call("get_compile_settings")) == state
                and identities(args.output / "builds") == outputs, "Stale source inspection changed document/draft/settings or prior BSP/LIN")
    finally:
        model.write_bytes(original)
    repaired, receipt = pair(client, args, game, "source-repaired", control, [child])
    require(repaired == prior and receipt["background"]["static_prop_source_issue"] is None,
            "Repair did not restore original child-prop BSP bytes and source freshness")
    return {"before": before, "stale": stale, "repaired": receipt}


def cleanup_clients(clients, report):
    errors = []
    for index, active in enumerate(clients):
        if active.process is None:
            continue
        try:
            with stopped_on_exit(active):
                pass
        except BaseException as error:
            errors.append(error)
            report.setdefault("cleanup_errors", []).append({"client": index, "stage": "stop", "error": repr(error)})
        try:
            require_clean_eof(active)
        except BaseException as error:
            errors.append(error)
            report.setdefault("cleanup_errors", []).append({"client": index, "stage": "eof", "error": repr(error)})
    return errors


def validate(args):
    clients = []
    failure = None
    report = {"status": "failed", "binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()),
              "script_sha256": digest(Path(__file__).read_bytes()), "compile_calls": 36 + (4 if args.baked_fixture_game else 0),
              "graphics_initialized": False, "native_game_exercised": False, "desktop_input": False,
              "positive": {}, "negative": {}, "omitted": {},
              "scope": "Compiler-only child prop_static solid0; complete tiny default model meshes support real shadow admission and literal native sprp6/face-lightmap controls. No native-placement or collision claim; VHV controls require the optional baked fixture."}
    try:
        (args.output / "tmp").mkdir()
        (args.output / "builds").mkdir()
        external = small_files(args.baked_fixture_game) if args.baked_fixture_game else None
        game = game_assets(args)
        mounted = identities(game)
        client = private_client(args.binary, args.output / "client", game, args.output / "tmp")
        clients.append(client)
        report["default_models"] = [client.call("inspect_model", {"path": model, "skin": 0, "body": 0}) for model in MODELS]
        require(all(model["static_prop"] and model["vertices"] == 4 and model["triangles"] == 2
                    and model["skins"] == 1 and model["skin"] == model["body"] == 0
                    and model["material_error"] is None and len(model["materials"]) == 1
                    and "models/hammerr719/receiver" in model["materials"][0] for model in report["default_models"])
                and not client.call("get_map_info")["rendering"]["initialized"],
                "Complete tiny models failed actual CPU mesh/material/static-prop inspection")
        child = args.output / "props.vmf"
        ids = leaf(client, child)
        original_child = child.read_bytes()
        retained, control = None, None
        for placement in LITERALS:
            sources, reference = [child], child
            if placement == "noncommuting":
                reference = args.output / "middle.vmf"
                middle(client, reference, child)
                sources.append(reference)
            for mode in (FULL, DUAL):
                label = f"{placement}-{mode[0]}"
                data, receipt, current = control_pair(client, args, game, label, placement, reference, sources, MODELS, mode)
                report["positive"][label] = receipt
                if placement == "translation" and mode == FULL:
                    retained, control = data, current
        for omitted in ("region", "hidden"):
            source = args.output / f"{omitted}-props.vmf"
            leaf(client, source, omitted=omitted)
            if omitted == "region":
                data, receipt, _ = control_pair(client, args, game, omitted, "translation", source, [source], MODELS, FULL, region=True)
            else:
                current = parent(client, args.output / "hidden-parent.vmf", "translation", source)
                setup(client)
                stage_property(client)
                data, receipt = pair(client, args, game, omitted, current, [source])
                counts(receipt, [source], 1)
                receipt["comparison"] = compare(data, retained, "translation", MODELS)
                receipt["unused_model_status"] = unread_model(client, args, game, current["source"])
            report["omitted"][omitted] = receipt
        author = private_client(args.binary, args.output / "author", game, args.output / "tmp")
        clients.append(author)
        refusal_cases(client, author, args, game, child, ids, control, retained, report["negative"])
        report["source_drift"] = source_drift(client, args, game, control, child, retained)
        if args.baked_fixture_game:
            baked_child = args.output / "baked-props.vmf"
            models = (args.baked_model, args.baked_model)
            leaf(author, baked_child, models)
            _, report["baked"], _ = control_pair(client, args, game, "baked", "translation", baked_child, [baked_child], models, DUAL, bake=True)
        inputs = {str(path): digest(path.read_bytes()) for path in args.output.glob("*.vmf")}
        report["clean_eof"] = {str(index): require_clean_eof(active) for index, active in enumerate(clients)}
        require(child.read_bytes() == original_child and identities(game) == mounted
                and all(digest(Path(path).read_bytes()) == identity for path, identity in inputs.items())
                and (not external or small_files(args.baked_fixture_game) == external),
                "Compilation/EOF changed authored source or private/read-only mounted assets")
        report["sources"], report["assets"] = inputs, mounted
        report["baked_fixture"] = external
        report["status"] = "accepted"
    except BaseException as error:
        failure = error
        report["error"] = f"{type(error).__name__}: {error!r}"
        raise
    finally:
        errors = cleanup_clients(clients, report)
        if errors:
            report["status"] = "failed"
        try:
            write_json(args.output / "results.json", report)
        except BaseException as error:
            errors.append(error)
        if errors:
            raise BaseExceptionGroup("Instance prop validation and cleanup failed", ([failure] if failure else []) + errors) from None
    return report


def main():
    parser = argparse.ArgumentParser(description="Validate719 compiler-only static props in instances through either production private-stdio MCP program. Steps: author/save child and nested assemblies plus independent literal-flat controls; compile foreground/background Fullbright and dual LDR/HDR; compare original sprp6 dictionaries/ordinals/poses/leaves and face lightmaps; check pre-region counts, hidden/region invalid-model no-read behavior, strict refusals preserving prior BSP/LIN and owned drafts, and717 source staleness/repair. No graphics, native game or desktop input.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY, help="Integrated719 hammerr-headless or editor; launched with --headless")
    parser.add_argument("--output", type=Path, required=True, help="New artifact directory below this repository's target/validation; complete tiny MDL/VVD/VTX/material fixtures require no installed game")
    parser.add_argument("--baked-fixture-game", type=Path, help="Optional read-only prepared model/material root under target/validation, at most256 files/64MiB; requires local admitted MDL/VVD/dx90.VTX companions. Adds4 real MCP compile calls and exact nested/flat dual-channel VHV controls")
    parser.add_argument("--baked-model", help="Required with --baked-fixture-game: canonical models/...mdl path, body0/skin0/unit-scale, opaque normal-white receiver with bounds fitting both literal placements")
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(strict=True), args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Keep artifacts under repository target/validation")
    require(bool(args.baked_fixture_game) == bool(args.baked_model), "Supply both baked fixture options together")
    if args.baked_fixture_game:
        args.baked_fixture_game = args.baked_fixture_game.resolve(strict=True)
        require(args.baked_fixture_game.is_relative_to(ROOT / "target/validation") and args.baked_model.startswith("models/")
                and args.baked_model.endswith(".mdl") and args.baked_model == args.baked_model.lower()
                and "\\" not in args.baked_model and ".." not in args.baked_model.split("/"), "Use a canonical model in a prepared private fixture")
        require(all((args.baked_fixture_game / (args.baked_model[:-4] + suffix)).is_file() for suffix in (".mdl", ".vvd", ".dx90.vtx")),
                "Optional real MDL/VVD/VTX fixture companions are incomplete")
    args.output.mkdir(parents=True, exist_ok=False)
    report = validate(args)
    print(json.dumps({"status": report["status"], "compile_calls": report["compile_calls"], "output": str(args.output)}))


if __name__ == "__main__":
    main()
