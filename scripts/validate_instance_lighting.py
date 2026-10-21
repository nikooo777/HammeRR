import argparse
import json
import math
import re
from pathlib import Path

import validate_cubemap_capture_probe as probe
from validate_authored_planes import records
from validate_compile_jobs import stopped_on_exit
from validate_compile_regions import apply as apply_region
from validate_face_lighting import DEFAULT_BINARY, ROOT, detail
from validate_instance_contents import refresh
from validate_instance_terrain import (
    active_ids,
    private_client,
    property_state,
    write_json,
)
from validate_material_albedo import (
    compile_pair,
    digest,
    fixture_texture,
    floor_base,
    identities,
    require,
    settings,
)
from validate_nested_brush_instances import assets, room, source_state
from validate_recovery_exit import require_clean_eof

MODES = (("fullbright", 0), ("direct_ldr", 0), ("direct_ldr_hdr", 0))
POINTFILE = b"13 17 19\n23 29 31\n"
POINT = {"_light": "255 0 0 80", "_lightHDR": "0 0 255 120"}
SPOT = {"_light": "0 255 0 96", "_lightHDR": "255 0 0 160", "pitch": "-45", "_inner_cone": "20", "_cone": "60", "_exponent": "2"}
FGD = '''
@BaseClass = StaticLight [
 angles(angle) : "Angles" : "0 0 0"
 _light(color255) : "Light" : "255 255 255 64"
 _lightHDR(color255) : "HDR light" : "255 255 255 96"
 _lightscaleHDR(float) : "HDR scale" : "1"
 _constant_attn(float) : "Constant attenuation" : "1"
 _linear_attn(float) : "Linear attenuation" : "0"
 _quadratic_attn(float) : "Quadratic attenuation" : "0"
 style(integer) : "Style" : "0"
 defaultstyle(integer) : "Default style" : "0"
 targetname(string) : "Name" : ""
 target(string) : "Target" : ""
 spawnflags(flags) = [ 1 : "Initially dark" : 0 ]
]
@PointClass base(StaticLight) = light [ ]
@PointClass base(StaticLight) = light_spot [
 pitch(float) : "Pitch" : "-90"
 _inner_cone(float) : "Inner cone" : "20"
 _cone(float) : "Outer cone" : "60"
 _exponent(float) : "Exponent" : "2"
]
'''


def setup(client, mode):
    settings(client, mode)
    if not any(row["name"] == "Static instance lights" for row in client.call("get_build_recipes")["recipes"]):
        client.call("save_build_recipe", {"name": "Static instance lights"})


def stage_property(client):
    token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
    client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": "message", "value": "Retain this static instance light review"}]})
    return token


def emitters(client, placement="source", second=False):
    positions = {
        "source": ((-48, 16, 128), (64, -16, 160)),
        "translation": ((-24, 24, 140), (88, -8, 172)),
        "quarter": ((-16, -48, 140), (16, 64, 172)),
    }[placement]
    ids = []
    for index, (classname, properties) in enumerate((("light", POINT), ("light_spot", SPOT))):
        position = list(positions[index])
        if second:
            position[0] += 512
        properties = dict(properties)
        if placement == "quarter":
            properties["angles"] = "0 90 0" if index == 0 else "45 90 0"
        entity = client.call("create_entity", {"classname": classname, "origin": position, "properties": properties})["id"]
        ids.append(entity)
    return ids


def fixture(client, output, fgd):
    client.call("load_entity_definitions", {"path": str(fgd)})
    ids = emitters(client)
    client.call("set_entity_properties", {"id": ids[1], "set": {"pitch": "-45"}})
    leaf = output / "lights.vmf"
    client.call("save_map", {"path": str(leaf)})
    saved = leaf.read_bytes()
    client.call("open_map", {"path": str(leaf)})
    require(leaf.read_bytes() == saved, "Reopening the authored child changed its saved bytes")
    rows = detail(client, ids)
    for row in rows:
        properties = dict(row["properties"])
        require(properties["angles"] == "0 0 0" and properties["style"] == properties["defaultstyle"] == properties["spawnflags"] == "0"
                and properties["targetname"] == properties["target"] == "", "FGD-created child lost its neutral static defaults")
    require(dict(rows[1]["properties"])["pitch"] == "-45", "Explicit-pitch property edit did not survive reopen")
    middle = output / "middle.vmf"
    client.call("new_map", {"discard_changes": True})
    client.call("create_entity", {"classname": "func_instance", "origin": [8, 16, 4], "properties": {"file": leaf.name, "angles": "0 0 0"}})
    client.call("save_map", {"path": str(middle)})
    return leaf, middle, ids


def parent(client, path, placement, middle=None, doubled=False):
    client.call("new_map", {"discard_changes": True})
    ids = room(client)
    client.call("delete_objects", {"ids": ids[-1:]})
    ids = ids[:-1]
    owners = []
    if middle is None:
        ids.extend(emitters(client, placement))
        if doubled:
            ids.extend(emitters(client, placement, True))
    else:
        for extra in ((0, 512) if doubled else (0,)):
            owner = client.call("create_entity", {"classname": "func_instance", "origin": [16 + extra, -8, 8],
                                                   "properties": {"file": middle.name, "angles": "0 90 0" if placement == "quarter" else "0 0 0"}})["id"]
            owners.append(owner)
            ids.append(owner)
    client.call("select", {"ids": owners or ids[:1]})
    if doubled:
        apply_region(client, low=[-128, -128, 0], high=[88, 128, 224])
    client.call("save_map", {"path": str(path)})
    return ids, owners


def pair(client, output, label, source, ids, game, sources, error=None, prior=None, owner=None):
    inputs = {str(path): digest(path.read_bytes()) for path in sources}
    draft = property_state(client)
    for suffix in ("", "-background"):
        (output / f"{label}{suffix}.lin").write_bytes(POINTFILE)
    with stopped_on_exit(client):
        data, receipt = compile_pair(client, output, label, ids, source, game, error, prior)
    require(len(data) <= 16 * 1024 * 1024, "Private light fixture exceeded its BSP read budget")
    require(all((output / f"{label}{suffix}.lin").read_bytes() == POINTFILE for suffix in ("", "-background")),
            "Static child-light compilation changed prior LIN bytes")
    require(property_state(client) == draft and all(digest(Path(path).read_bytes()) == value for path, value in inputs.items()),
            "Static child-light compilation changed its draft or a saved dependency")
    require(receipt["result"]["game"] is None, "Headless validation launched a game")
    if error:
        for result in (receipt["result"], receipt["background"]["result"]):
            require(any(row["level"] == "error" and row["object"] == owner and error in row["message"] for row in result["diagnostics"]),
                    "Child-light refusal lost the selectable root helper or contextual reason")
    else:
        require(receipt["background"]["result"]["published"] and receipt["background"]["native_runtime"] is None,
                "Successful compile did not publish through the ordinary guarded job")
    receipt["instance_sources"] = inputs
    receipt["property_draft_present"] = draft is not None
    receipt["property_draft_preserved"] = True
    return data, receipt


def counts(receipt, sources, doubled=False):
    copies = 2 if doubled else 1
    dependencies = [{"path": path.name, "bytes": len(path.read_bytes()), "sha256": digest(path.read_bytes())} for path in sorted(sources)]
    for result in (receipt["result"], receipt["background"]["result"]):
        stats = result["stats"]["instances"]
        require([stats[key] for key in ("instances", "files", "max_expansion_depth", "world_brushes", "detail_brushes", "point_lights", "spot_lights")]
                == [2 * copies, 2, 2, 0, 0, copies, copies], "Expanded canonical child-light counts changed or were region-filtered")
        require(stats["dependencies"] == dependencies, "Child-light source receipts differ from the actual saved files")


def preview(client):
    source = Path(client.call("get_map_info")["path"])
    before = (source_state(client, source), client.call("get_compile_settings"), property_state(client))
    report = refresh(client, background=True)
    require(report["state"] == "ready" and report["stats"]["point_lights"] == 1 and report["stats"]["spot_lights"] == 1
            and report["geometry"]["vertices"] == report["geometry"]["indices"] == report["geometry"]["triangles"] == 0
            and not report["available"] and report["owners"][0]["bounds"] is None and report["face_provenance_total"] == 0
            and report["static_light_preview"]["available"] is False
            and report["static_light_preview"]["reason"] == "Child lights, illumination and icons are not shown in the reference or authored-lighting preview.",
            "Light-only reference inspection omitted counts or fabricated display geometry")
    require((source_state(client, source), client.call("get_compile_settings"), property_state(client)) == before
            and not client.call("get_map_info")["rendering"]["initialized"],
            "Light-only CPU refresh changed the source, document, history, settings or draft, or initialized graphics")
    return report


def entity_rows(data):
    raw = probe.bsp_lumps(data)[0][2].rstrip(b"\0").decode()
    result = []
    for block in re.findall(r"\{([^{}]*)\}", raw):
        pairs = re.findall(r'"([^"\\]*)"\s+"([^"\\]*)"', block)
        require(len(dict(pairs)) == len(pairs), "Native entity contains duplicate property names")
        result.append(dict(pairs))
    return result


def emitters_check(data, mode, placement, boundary=False):
    expected_positions = {"translation": {1: (-24, 24, 140), 2: (88, -8, 172)},
                          "quarter": {1: (-16, -48, 140), 2: (16, 64, 172)}}[placement]
    entities = [row for row in entity_rows(data) if row.get("classname") in ("light", "light_spot")]
    require(len(entities) == 2 and not any(row.get("classname") == "func_instance" for row in entity_rows(data)),
            "Native entity text lost an admitted emitter or retained a helper")
    for entity in entities:
        kind = 1 if entity["classname"] == "light" else 2
        require(max(abs(actual - expected) for actual, expected in zip(
            (float(value) for value in entity["origin"].split()), expected_positions[kind], strict=True)) <= 1e-8,
            "Emitted light origin differs from the literal placement")
        colors = POINT if kind == 1 else SPOT
        require(all(entity[key] == colors[key] for key in ("_light", "_lightHDR")), "Native emitter altered original channel-specific light values")
        if kind == 1:
            angles = tuple(float(value) for value in entity["angles"].split())
            expected = (0, 90, 0) if placement == "quarter" else (0, 0, 0)
            require(max(abs(a - b) for a, b in zip(angles, expected, strict=True)) <= 1e-8,
                    "Point-light angles differ from the literal instance pose")
        else:
            require(abs(float(entity["pitch"]) + 45) <= 1e-8, "Emitted spotlight lost its canonical explicit pitch")
            pitch = math.radians(float(entity["pitch"]))
            yaw = math.radians(float(entity["angles"].split()[1]))
            beam = (math.cos(pitch) * math.cos(yaw), math.cos(pitch) * math.sin(yaw), math.sin(pitch))
            expected = (math.sqrt(0.5), 0, -math.sqrt(0.5)) if placement == "translation" else (0, math.sqrt(0.5), -math.sqrt(0.5))
            require(max(abs(a - b) for a, b in zip(beam, expected, strict=True)) <= 1e-9, "Emitted explicit-pitch spotlight beam differs from the literal direction")
    channels = ("ldr", "hdr") if mode == "direct_ldr_hdr" else ("ldr",)
    result = {}
    for channel in channels:
        rows = records(data, 15 if channel == "ldr" else 54, "<9f3i7f3i")
        if mode == "fullbright":
            require(not rows, "Fullbright unexpectedly baked native world lights")
            continue
        baked = {1} if boundary else {1, 2}
        require(len(rows) == len(baked) and {row[10] for row in rows} == baked,
                "Native world-light classes differ from the explicitly retained bake set")
        for row in rows:
            kind = row[10]
            intensity = ({1: (80, 0, 0), 2: (0, 96, 0)} if channel == "ldr" else {1: (0, 0, 120), 2: (160, 0, 0)})[kind]
            require(row[:3] == expected_positions[kind]
                    and max(abs(actual - expected / 255) for actual, expected in zip(row[3:6], intensity, strict=True)) <= 1e-7
                    and row[11] == 0 and row[21] == -1,
                    "Native light origin, intensity, style or static ownership differs from independent literals")
            if kind == 2:
                beam = (math.sqrt(0.5), 0, -math.sqrt(0.5)) if placement == "translation" else (0, math.sqrt(0.5), -math.sqrt(0.5))
                require(max(abs(a - b) for a, b in zip(row[6:9], beam, strict=True)) <= 1e-6
                        and abs(row[12] - math.cos(math.radians(20))) <= 1e-6 and abs(row[13] - 0.5) <= 1e-6 and row[14] == 2,
                        "Native spotlight normal or cone differs from the independent explicit-pitch emitter")
        samples = floor_base(data, channel)
        require(any(sum(sample) > 0 for sample in samples), "Admitted child lights did not illuminate the retained floor")
        result[channel] = {"world_lights": rows, "floor_samples": len(samples), "floor_sha256": digest(json.dumps(samples).encode())}
    if mode == "direct_ldr_hdr":
        require(probe.bsp_lumps(data)[8][2] != probe.bsp_lumps(data)[53][2], "HDR reused the LDR lightmaps despite different authored channels")
    return result


def compare(actual, manual, mode, placement, boundary=False):
    left, right = probe.bsp_lumps(actual), probe.bsp_lumps(manual)
    changed = [index for index, (a, b) in enumerate(zip(left, right, strict=True)) if a != b]
    require(set(changed) <= {0, 15, 35, 54}, f"Independent flattened geometry or lightmaps differ in BSP lumps: {changed}")
    game_lumps = probe.verify_game_lump_preservation(actual, manual)
    actual_entities, manual_entities = entity_rows(actual), entity_rows(manual)
    classes = {"light", "light_spot"}
    require([row for row in actual_entities if row.get("classname") not in classes]
            == [row for row in manual_entities if row.get("classname") not in classes],
            "Independent flattened world or non-light entity properties differ")
    for classname in classes:
        first = next(row for row in actual_entities if row.get("classname") == classname)
        second = next(row for row in manual_entities if row.get("classname") == classname)
        pose = {"origin", "angles", "pitch", "hammerid"}
        require({key: value for key, value in first.items() if key not in pose}
                == {key: value for key, value in second.items() if key not in pose},
                "Independent flattened emitter properties differ outside checked pose/source identity fields")
        for row in (first, second):
            if "hammerid" in row:
                require(row["hammerid"].isdigit(), "Emitter source identity is not an integer")
    for index in (15, 54):
        a, b = records(actual, index, "<9f3i7f3i"), records(manual, index, "<9f3i7f3i")
        require(len(a) == len(b), "Flattened emitter count differs")
        for first, second in zip(a, b, strict=True):
            require(first[:6] == second[:6] and first[9:] == second[9:]
                    and max(abs(x - y) for x, y in zip(first[6:9], second[6:9], strict=True)) <= 1e-6,
                    "Flattened native emitter differs beyond the predeclared floating normal bound")
    return {"different_lumps": changed, "native": emitters_check(actual, mode, placement, boundary),
            "manual_native": emitters_check(manual, mode, placement, boundary), "non_emitter_lumps_exact_except_game_lump_offsets": True,
            "game_lump_preservation": game_lumps,
            "nested_entities": actual_entities, "manual_entities": manual_entities,
            "entity_scope": "All non-light entities and non-pose emitter properties match exactly. Origins/channels/beams are checked against literals; explicit pitch takes precedence over redundant angles.x. Original source IDs and BSP bytes are retained."}


def positive(client, args, game, leaf, middle, report):
    controls, retained = {}, None
    for placement, modes in (("translation", MODES), ("quarter", MODES[2:])):
        nested, flat = args.output / f"{placement}-parent.vmf", args.output / f"{placement}-flat.vmf"
        ids, owners = parent(client, nested, placement, middle)
        flat_ids, _ = parent(client, flat, placement)
        for mode in modes:
            client.call("open_map", {"path": str(nested)})
            client.call("select", {"ids": owners})
            setup(client, mode)
            token = stage_property(client)
            ready = preview(client)
            data, receipt = pair(client, args.output, f"{placement}-{mode[0]}", nested, ids, game, [leaf, middle])
            counts(receipt, [leaf, middle])
            receipt["reference_preview"] = ready
            client.call("cancel_property_edit", {"token": token})
            client.call("open_map", {"path": str(flat)})
            setup(client, mode)
            literal, manual = pair(client, args.output, f"{placement}-{mode[0]}-flat", flat, flat_ids, game, [leaf, middle])
            receipt["comparison"] = compare(data, literal, mode[0], placement)
            receipt["flat"] = manual
            report["positive"][f"{placement}-{mode[0]}"] = receipt
            if placement == "translation" and mode[0] == "fullbright":
                retained = data
                controls = {"source": nested, "ids": ids, "owner": owners[0]}
    nested, flat = args.output / "region-parent.vmf", args.output / "region-flat.vmf"
    ids, owners = parent(client, nested, "translation", middle, True)
    flat_ids, _ = parent(client, flat, "translation", doubled=True)
    client.call("open_map", {"path": str(nested)})
    setup(client, MODES[2])
    data, receipt = pair(client, args.output, "region", nested, ids, game, [leaf, middle])
    counts(receipt, [leaf, middle], True)
    client.call("open_map", {"path": str(flat)})
    setup(client, MODES[2])
    literal, manual = pair(client, args.output, "region-flat", flat, flat_ids, game, [leaf, middle])
    receipt["comparison"] = compare(data, literal, "direct_ldr_hdr", "translation", boundary=True)
    require(any(row["level"] == "warning" and row["object"] == owners[0]
                and row["position"] == [88, -8, 172] and "does not illuminate the world" in row["message"]
                and "instance lights.vmf, child object" in row["message"]
                for row in receipt["result"]["diagnostics"]),
            "The inclusive boundary spotlight was not retained with an explicit root-owned unavailable-bake diagnostic")
    receipt["boundary_spotlight"] = "Retained in entity text at the inclusive region boundary; sealing places it in solid space, so its baked world light is unavailable with an explicit diagnostic."
    receipt["flat"] = manual
    require(receipt["result"]["stats"]["compile_region"]["maxs"] == [88, 128, 224], "Region did not preserve its literal inclusive boundary")
    report["positive"]["region"] = receipt
    return retained, controls


def refusals(client, author, args, game, leaf, middle, light_ids, retained, controls, report):
    source, ids, owner = controls["source"], controls["ids"], controls["owner"]
    client.call("open_map", {"path": str(source)})
    client.call("select", {"ids": [owner]})
    setup(client, MODES[0])
    token = stage_property(client)
    original = leaf.read_bytes()
    for label, target, values, remove, reason in (
        ("named-child", light_ids[0], {"targetname": "unsupported_child_name"}, [], "empty targetname"),
        ("styled-child", light_ids[0], {"style": "1"}, [], "requires style 0"),
        ("missing-pitch", light_ids[1], {}, ["pitch"], "pitch must be finite"),
    ):
        author.call("open_map", {"path": str(leaf), "discard_changes": True})
        author.call("set_entity_properties", {"id": target, "set": values, "remove": remove})
        author.call("save_map")
        require(leaf.read_bytes() != original, "Refusal fixture did not change its authored child")
        try:
            _, report["negative"][label] = pair(client, args.output, label, source, ids, game, [leaf, middle], reason, retained, owner)
        finally:
            leaf.write_bytes(original)
    repaired, report["repaired"] = pair(client, args.output, "repaired", source, ids, game, [leaf, middle])
    require(repaired == retained, "Restoring original child bytes did not restore the original fullbright BSP")
    ready = preview(client)
    before = source_state(client, source), client.call("get_property_edit")
    backup = args.output / "retained-lights.vmf"
    leaf.rename(backup)
    leaf.write_bytes(original)
    status = client.call("get_compile_status")
    contents = client.call("inspect_instance_contents")
    require(status["job"]["stale"] and not status["job"]["instance_dependencies"]["current"]
            and contents["state"] == "stale" and not contents["available"], "Same-byte child replacement did not stale original dependency identities")
    require((source_state(client, source), client.call("get_property_edit")) == before,
            "Dependency staleness changed the saved parent, history or owned property draft")
    preview(client)
    data, receipt = pair(client, args.output, "fresh-identity", source, ids, game, [leaf, middle])
    require(data == retained, "Fresh child identity changed the original accepted BSP bytes")
    report["identity"] = {"before": ready, "stale_job": status, "stale_preview": contents, "rebuilt": receipt}
    client.call("cancel_property_edit", {"token": token})


def validate(args):
    clients = []
    game, leaf = args.output / "game", None
    report = {"status": "failed", "binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()),
              "script_sha256": digest(Path(__file__).read_bytes()), "native_game_exercised": False,
              "graphics_initialized": False, "desktop_input": False, "compile_calls": 32, "positive": {}, "negative": {}}
    try:
        temporary = args.output / "tmp"
        temporary.mkdir()
        assets(game)
        tools = game / "materials/tools"
        tools.mkdir()
        (tools / "toolsblack.vtf").write_bytes(fixture_texture(2, 2, (0, 0, 0), (0, 0, 0, 255)))
        (tools / "toolsblack.vmt").write_text('LightmappedGeneric { $basetexture "tools/toolsblack" }\n')
        fgd = args.output / "static-lights.fgd"
        fgd.write_text(FGD)
        client = private_client(args.binary, args.output / "client", game, temporary)
        clients.append(client)
        leaf, middle, light_ids = fixture(client, args.output, fgd)
        original = leaf.read_bytes()
        retained, controls = positive(client, args, game, leaf, middle, report)
        author = private_client(args.binary, args.output / "author", game, temporary)
        clients.append(author)
        author.call("load_entity_definitions", {"path": str(fgd)})
        refusals(client, author, args, game, leaf, middle, light_ids, retained, controls, report)
        report["author_clean_eof"] = require_clean_eof(author)
        report["clean_eof"] = require_clean_eof(client)
        restarted = private_client(args.binary, args.output / "restart", game, temporary)
        clients.append(restarted)
        restarted.call("load_entity_definitions", {"path": str(fgd)})
        restarted.call("open_map", {"path": str(controls["source"])})
        setup(restarted, MODES[0])
        preview(restarted)
        data, report["restart"] = pair(restarted, args.output, "restart", controls["source"], active_ids(restarted), game, [leaf, middle])
        require(data == retained, "Restart did not recapture the original static child-light sources")
        inputs, mounted = {str(path): digest(path.read_bytes()) for path in args.output.glob("*.vmf")}, identities(game)
        report["restart_clean_eof"] = require_clean_eof(restarted)
        require(leaf.read_bytes() == original and identities(game) == mounted
                and all(digest(Path(path).read_bytes()) == identity for path, identity in inputs.items()), "EOF changed authored sources or materials")
        report["sources"] = inputs
        report["assets"] = mounted
        report["status"] = "accepted"
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        try:
            for client in clients:
                if client.process is not None:
                    with stopped_on_exit(client):
                        pass
                    require_clean_eof(client)
        finally:
            write_json(args.output / "results.json", report)
    return report


def main():
    parser = argparse.ArgumentParser(description="Validate static child point/explicit-pitch spot lights using private production MCP and independent literal flattened emitter/lightmap controls; no graphics, native game or desktop input.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY, help="hammerr-headless or editor, launched with --headless")
    parser.add_argument("--output", type=Path, required=True, help="New private artifact directory below target/validation; no installed game or external FGD required")
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(), args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Artifacts must stay under target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    report = validate(args)
    print(json.dumps({"status": report["status"], "compile_calls": report["compile_calls"], "output": str(args.output)}))


if __name__ == "__main__":
    main()
