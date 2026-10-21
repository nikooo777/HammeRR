import argparse
import json
import math
import re
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_instance_props as props
from validate_compile_regions import apply as apply_region
from validate_face_lighting import DEFAULT_BINARY, ROOT, detail
from validate_instance_contents import refresh
from validate_instance_lighting import entity_rows, stage_property
from validate_instance_prop_skins import saved_versions
from validate_instance_terrain import private_client, property_state, write_json
from validate_material_albedo import digest, fixture_texture, identities, require
from validate_nested_brush_instances import assets, fixture_blocks, room, source_state
from validate_recovery_exit import require_clean_eof

TEAMS = ("info_player_terrorist", "info_player_counterterrorist")
SOURCE = (((-24, 16, 8), None), ((40, -8, 24), (0, 0, -90)))
LITERALS = {
    "translation": (((-8, 8, 40), (0, 0, 0)), ((56, -16, 56), (0, 0, -90))),
    "noncommuting": (((32, -96, 160), (0, 90, 90)), ((48, -32, 136), (0, 90, 0)),
                     ((-48, -56, 160), (0, 90, 90)), ((-32, 8, 136), (0, 90, 0))),
}
BASES = {
    "translation": (((1, 0, 0), (0, 1, 0), (0, 0, 1)),
                    ((1, 0, 0), (0, 0, -1), (0, 1, 0))),
    "noncommuting": (((0, 1, 0), (0, 0, 1), (1, 0, 0)),
                     ((0, 1, 0), (-1, 0, 0), (0, 0, 1))) * 2,
}
PREVIEW_REASON = ("Child player-spawn models and icons are not shown in the reference preview; "
                  "spawn bounds, picking and framing are unavailable.")
ERROR = 0.00001


def spawn(client, classname, origin, angles, legacy=False, extra=None):
    properties = {"spawnflags": "0", "targetname": "", "target": ""}
    if angles is not None:
        properties["angles"] = " ".join(map(str, angles))
    if legacy:
        properties["angle"] = "0"
    properties.update(extra or {})
    identifier = client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"]
    if angles is None:
        client.call("set_entity_properties", {"id": identifier, "remove": ["angles", "angle"]})
    return identifier


def leaf(client, path, omitted=False):
    client.call("new_map", {"discard_changes": True})
    ids = [spawn(client, classname, origin, angles, legacy=index == 1)
           for index, (classname, (origin, angles)) in enumerate(zip(TEAMS, SOURCE, strict=True))]
    if omitted:
        hidden = spawn(client, TEAMS[0], [0, 0, 16], [0, 0, 0], extra={"enabled": "1"})
        client.call("set_object_visibility", {"action": "hide", "ids": [hidden]})
        spawn(client, TEAMS[1], [512, 0, 32], [0, 0, 0])
    client.call("save_map", {"path": str(path)})
    saved = path.read_bytes()
    client.call("open_map", {"path": str(path)})
    fields = [dict(row["properties"]) for row in detail(client, ids)]
    require(path.read_bytes() == saved and "angles" not in fields[0] and "angle" not in fields[0]
            and fields[1]["angles"] == "0 0 -90" and fields[1]["angle"] == "0",
            "Saved/reopened child lost its absent-angle default or explicit neutral legacy angle")
    return ids


def parent(client, path, placement, child=None, region=False, bounds=False):
    client.call("new_map", {"discard_changes": True})
    ids = room(client)
    owners = []
    if child is None:
        for index, (origin, angles) in enumerate(LITERALS[placement]):
            ids.append(spawn(client, TEAMS[index % 2], origin, angles, legacy=placement == "translation" and index == 1))
    else:
        poses = (((16, -8, 32), "0 0 0"),) if placement == "translation" else (
            ((16, -8, 128), "0 0 90"), ((-64, 32, 128), "0 0 90"))
        if bounds:
            poses = (((-16376, 0, 0), "0 0 0"),)
        for origin, angles in poses:
            owner = client.call("create_entity", {"classname": "func_instance", "origin": origin,
                "properties": {"file": child.name, "angles": angles}})["id"]
            owners.append(owner)
            ids.append(owner)
    client.call("select", {"ids": owners or ids[:1]})
    if region:
        apply_region(client, low=[-128, -128, 0], high=[128, 128, 224])
    client.call("save_map", {"path": str(path)})
    saved = path.read_bytes()
    client.call("open_map", {"path": str(path)})
    require(path.read_bytes() == saved, "Reopening the completed parent rewrote source bytes")
    client.call("select", {"ids": owners or ids[:1]})
    return {"source": path, "ids": ids, "owner": owners[0] if owners else None, "owners": owners}


def counts(receipt, sources, instances, depth, spawns):
    expected = {"instances": instances, "files": len(sources), "max_expansion_depth": depth,
                "world_brushes": 0, "detail_brushes": 0, "player_spawns": spawns}
    dependencies = [{"path": path.name, "bytes": path.stat().st_size, "sha256": digest(path.read_bytes())}
                    for path in sorted(sources)]
    for result in (receipt["result"], receipt["background"]["result"]):
        stats = result["stats"]["instances"]
        require({key: stats[key] for key in expected} == expected and stats["dependencies"] == dependencies,
                "Expanded player-spawn counts or saved dependency receipts differ before region filtering")
        require(all(key not in stats for key in ("point_lights", "spot_lights", "static_props", "lighting_origins", "terrain_patches")),
                "Spawn-only assembly invented another omit-zero feature count")


def basis(raw):
    angles = tuple(float(value) for value in raw.split())
    require(len(angles) == 3 and all(math.isfinite(value) for value in angles), "Runtime spawn has invalid angles")
    pitch, yaw, roll = map(math.radians, angles)
    cp, sp, cy, sy, cr, sr = math.cos(pitch), math.sin(pitch), math.cos(yaw), math.sin(yaw), math.cos(roll), math.sin(roll)
    return ((cy * cp, sy * cp, -sp),
            (cy * sp * sr - sy * cr, sy * sp * sr + cy * cr, cp * sr),
            (cy * sp * cr + sy * sr, sy * sp * cr - cy * sr, cp * cr))


def emitted(data, placement, region):
    rows = entity_rows(data)
    require(not any(row.get("classname") == "func_instance" for row in rows), "Runtime entity text retained an instance helper")
    children, ordinary = [], []
    expected = list(zip(LITERALS[placement], BASES[placement], strict=True))
    for row in rows:
        if row.get("classname") not in TEAMS or row.get("origin") == "-192 -192 8":
            ordinary.append(row)
            continue
        position = tuple(float(value) for value in row["origin"].split())
        require(len(position) == 3 and all(math.isfinite(value) for value in position), "Runtime spawn has invalid origin")
        matches = [index for index, ((origin, _angles), _basis) in enumerate(expected)
                   if max(abs(a - b) for a, b in zip(position, origin, strict=True)) <= ERROR]
        require(len(matches) == 1, "Runtime spawn differs from every independent literal origin")
        index = matches[0]
        legacy = "0" if placement == "translation" and index == 1 else None
        require(row["classname"] == TEAMS[index % 2] and row.get("angle") == legacy and row["hammerid"].isdigit(),
                "Runtime spawn lost its team, generated hammerid, or placement-specific legacy-angle semantics")
        actual_basis = basis(row.get("angles", "0 0 0"))
        require(all(abs(a - b) <= ERROR for actual, wanted in zip(actual_basis, expected[index][1], strict=True)
                    for a, b in zip(actual, wanted, strict=True)),
                "Runtime spawn orientation differs from the independent literal basis vectors")
        children.append((index, row))
    require([index for index, _row in children] == list(range(len(expected)))
            and len({row["hammerid"] for _index, row in children}) == len(expected),
            "Runtime spawn count/order, repeated occurrence or generated identity differs")
    root_spawns = [row for row in ordinary if row.get("classname") in TEAMS]
    require(len(root_spawns) == (0 if region else 1), "Ordinary parent spawn region selection changed")
    return children, ordinary


def compare(actual, literal, placement, region=False):
    left, ordinary_left = emitted(actual, placement, region)
    right, ordinary_right = emitted(literal, placement, region)
    differences = []
    for (index, a), (_index, b) in zip(left, right, strict=True):
        require({key: value for key, value in a.items() if key not in ("origin", "angles", "hammerid")}
                == {key: value for key, value in b.items() if key not in ("origin", "angles", "hammerid")},
                "Literal/runtime spawn key-value semantics differ outside independently checked poses and generated hammerid")
        differences.append({"ordinal": index, "generated_hammerid": a["hammerid"], "literal_hammerid": b["hammerid"],
                            "generated_origin": a["origin"], "literal_origin": LITERALS[placement][index][0],
                            "literal_basis": BASES[placement][index],
                            "generated_angles": a.get("angles"), "literal_angles": b.get("angles")})
    require(ordinary_left == ordinary_right, "Unrelated runtime entities differ from the literal control")
    a, b = probe.bsp_lumps(actual), probe.bsp_lumps(literal)
    game_lumps = probe.verify_game_lump_preservation(actual, literal)
    exact_lumps = [index for index in range(1, 64) if index != probe.GAME_LUMP]
    require(a[0][:2] == b[0][:2] and all(a[index] == b[index] for index in exact_lumps)
            and actual[4:8] == literal[4:8]
            and actual[1032:1036] == literal[1032:1036],
            "Nonentity BSP payload, format or authored map revision differs from the literal control")
    return {"entity_pose_and_identity_differences": differences, "exact_nonentity_lumps": exact_lumps,
            "game_lump_preservation": game_lumps,
            "literal_bsp_sha256": digest(literal), "actual_bsp_sha256": digest(actual)}


def preview(client, control, count):
    before = source_state(client, control["source"]), property_state(client), client.call("get_compile_settings")
    report = refresh(client, background=True)
    require(report["stats"]["player_spawns"] == count and not report["available"]
            and report["geometry"]["vertices"] == report["geometry"]["indices"] == report["geometry"]["triangles"] == 0
            and report["face_provenance_total"] == 0 and all(row["bounds"] is None for row in report["owners"])
            and {row["owner"] for row in report["owners"]} == set(control["owners"])
            and report["player_spawn_preview"] == {"available": False, "reason": PREVIEW_REASON},
            "Spawn-only reference report lost counts/owners or fabricated model geometry, icons or bounds")
    client.call("set_instance_contents", {"action": "frame", "token": str(report["token"]), "ids": control["owners"]}, error=True)
    require((source_state(client, control["source"]), property_state(client), client.call("get_compile_settings")) == before
            and not client.call("get_map_info")["rendering"]["initialized"],
            "Spawn contents refresh or unavailable framing changed source/history/draft/settings/view or initialized graphics")
    return report


def reference_form(client, control):
    before = source_state(client, control["source"]), property_state(client)
    fields = dict(detail(client, [control["owner"]])[0]["properties"])
    form = client.call("set_instance_reference", {"action": "begin", "id": control["owner"]})
    token = form["token"]
    client.call("set_instance_reference", {"action": "update", "token": token, "file": fields["file"],
                                          "origin": fields["origin"], "angles": fields["angles"]})
    checked = client.call("set_instance_reference", {"action": "check", "token": token})
    require(checked["valid"] and checked["contents_validation"] == "not_checked", "Reference metadata check claimed spawn-content validation")
    client.call("set_instance_reference", {"action": "cancel", "token": token})
    require((source_state(client, control["source"]), property_state(client)) == before, "Reference form changed the completed source or history")
    return checked


def control_pair(client, args, game, label, placement, child, sources, mode, region=False):
    control = parent(client, args.output / f"{label}-parent.vmf", placement, child, region)
    props.setup(client, mode)
    reference = reference_form(client, control)
    stage_property(client)
    data, receipt = props.pair(client, args, game, label, control, sources)
    nested = placement == "noncommuting"
    counts(receipt, sources, 4 if nested else 1, 2 if nested else 1, 4 if nested else (3 if region else 2))
    receipt["preview"] = preview(client, control, 4 if nested else (3 if region else 2))
    receipt["reference_form"] = reference
    if label == "translation":
        receipt["source_drift"] = source_drift(client, args, game, control, child, data)
    flat = parent(client, args.output / f"{label}-flat.vmf", placement, region=region)
    require(saved_versions(control["source"]) == saved_versions(flat["source"]),
            "Completed parent and literal controls have different authored save versions")
    props.setup(client, mode)
    require(property_state(client) is None, "Fresh literal fixture unexpectedly retained another document's property draft")
    literal, receipt["literal"] = props.pair(client, args, game, f"{label}-flat", flat, [])
    for result in (receipt["literal"]["result"], receipt["literal"]["background"]["result"]):
        require(not result["stats"].get("instances"), "Ordinary literal control invented expanded player-spawn statistics")
    receipt["comparison"] = compare(data, literal, placement, region)
    receipt["saved_versions"] = saved_versions(control["source"])
    return data, receipt, control


def source_drift(client, args, game, control, child, prior):
    original = child.read_bytes()
    fresh = client.call("get_compile_status")
    require(not fresh["job"]["stale"] and fresh["job"]["instance_dependencies"]["current"],
            "Newly published spawn job is already stale")
    stable = source_state(client, control["source"]), property_state(client), client.call("get_compile_settings")
    outputs = identities(args.output / "builds")
    try:
        child.write_bytes(original + b"\n")
        stale = client.call("get_compile_status")
        require(stale["job"]["stale"] and not stale["job"]["instance_dependencies"]["current"]
                and not stale["launch"]["ready"], "Changed child source did not stale retained spawn publication")
        require((source_state(client, control["source"]), property_state(client), client.call("get_compile_settings")) == stable
                and identities(args.output / "builds") == outputs, "Source staleness inspection changed the parent/draft/settings or BSP/LIN")
    finally:
        child.write_bytes(original)
    repaired, receipt = props.pair(client, args, game, "source-repaired", control, [child])
    current = client.call("get_compile_status")
    require(repaired == prior and not current["job"]["stale"] and current["job"]["instance_dependencies"]["current"],
            "Recompilation after source repair did not restore fresh publication with exact prior BSP bytes")
    return {"fresh": fresh, "stale": stale, "repaired": receipt, "current": current,
            "restored_source_sha256": digest(child.read_bytes())}


def refusal_cases(client, author, args, game, child, child_ids, control, prior, report):
    original = child.read_bytes()
    cases = (
        ("unsupported-enabled", {"enabled": "1"}, "property enabled is unsupported"),
        ("named-spawn", {"targetname": "DynamicSpawn"}, "requires empty targetname"),
        ("legacy-yaw", {"angle": "90"}, "legacy angle must be absent or zero"),
        ("nonfinite-origin", None, "Instance origin requires three finite coordinates"),
        ("connections", None, "cannot have brushes, connections or opaque blocks"),
    )
    client.call("open_map", {"path": str(control["source"]), "discard_changes": True})
    props.setup(client)
    stage_property(client)
    for label, fields, reason in cases:
        try:
            if fields is not None:
                author.call("open_map", {"path": str(child), "discard_changes": True})
                author.call("set_entity_properties", {"id": child_ids[0], "set": fields})
                author.call("save_map")
            else:
                text = original.decode()
                blocks = fixture_blocks(text, "entity")
                require(len(blocks) == 2, "Imported unsupported fixture must contain exactly two child spawns")
                if label == "nonfinite-origin":
                    changed, total = re.subn(r'"origin"\s+"[^"]*"', '"origin" "NaN 16 8"', blocks[0], count=1)
                    require(total == 1, "Nonfinite imported fixture did not change exactly one origin")
                else:
                    changed = blocks[0][:-1] + '\nconnections\n{\n"OnUser1" "target,Use,,0,-1"\n}\n}'
                require(text.count(blocks[0]) == 1, "Unsupported imported child block is ambiguous")
                child.write_text(text.replace(blocks[0], changed))
            _, report[label] = props.pair(client, args, game, label, control, [child], reason, prior, child.name)
            child_object = child_ids[0].split(":")[-1]
            require(all(any(row["level"] == "error" and row["object"] == control["owner"]
                            and f"Child player spawn {child_object}:" in row["message"]
                            for row in result["diagnostics"])
                        for result in (report[label]["result"], report[label]["background"]["result"])),
                    "Refusal lost the true original child object identity")
            report[label]["expected_child_object"] = child_object
        finally:
            child.write_bytes(original)
    middle = args.output / "bounds-middle.vmf"
    props.middle(author, middle, child, origin=(16376, 0, 0), angles="0 0 0")
    bounds = parent(client, args.output / "bounds-parent.vmf", "translation", middle, bounds=True)
    props.setup(client)
    stage_property(client)
    _, report["intermediate-bounds"] = props.pair(client, args, game, "intermediate-bounds", bounds, [child, middle],
        "origin exceeds finite Source compile bounds", prior, child.name)
    child_object = child_ids[1].split(":")[-1]
    require(all(any(row["level"] == "error" and row["object"] == bounds["owner"]
                    and f"Child player spawn {child_object}:" in row["message"]
                    for row in result["diagnostics"])
                for result in (report["intermediate-bounds"]["result"], report["intermediate-bounds"]["background"]["result"])),
            "Intermediate-bounds refusal lost the true leaf spawn object")
    report["intermediate-bounds"]["expected_child_object"] = child_object


def validate(args):
    clients, failure = [], None
    args.compile_calls = 0
    report = {"status": "failed", "binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()),
              "script_sha256": digest(Path(__file__).read_bytes()), "compile_calls": 0, "expected_compile_calls": 26,
              "graphics_initialized": False, "native_game_exercised": False, "desktop_input": False,
              "positive": {}, "negative": {},
              "scope": "Compiler equivalence of unnamed child terrorist/counterterrorist spawns. Independent literal origins/bases and exact nonentity BSP payloads; CPU preview reports counts and explicit model/icon unavailability. No native spawn selection, team balancing, runtime clearance or rendering claim."}
    try:
        (args.output / "tmp").mkdir()
        (args.output / "builds").mkdir()
        game = args.output / "game"
        assets(game)
        (game.parent / "cstrike_linux64").unlink()
        tools = game / "materials/tools"
        tools.mkdir()
        (tools / "toolsblack.vtf").write_bytes(fixture_texture(2, 2, (0, 0, 0), (0, 0, 0, 255)))
        (tools / "toolsblack.vmt").write_text('LightmappedGeneric { $basetexture "tools/toolsblack" }\n')
        mounted = identities(game)
        client = private_client(args.binary, args.output / "client", game, args.output / "tmp")
        clients.append(client)
        author = private_client(args.binary, args.output / "author", game, args.output / "tmp")
        clients.append(author)
        child = args.output / "spawns.vmf"
        ids = leaf(author, child)
        original = child.read_bytes()
        controls = {}
        for placement, mode in (("translation", props.FULL), ("noncommuting", props.DUAL)):
            reference, sources = child, [child]
            if placement == "noncommuting":
                reference = args.output / "middle.vmf"
                props.middle(author, reference, child)
                sources.append(reference)
            data, receipt, control = control_pair(client, args, game, placement, placement, reference, sources, mode)
            report["positive"][placement] = receipt
            controls[placement] = data, control
        omitted = args.output / "omitted-spawns.vmf"
        leaf(author, omitted, omitted=True)
        _, report["positive"]["hidden-and-region"], _ = control_pair(
            client, args, game, "hidden-and-region", "translation", omitted, [omitted], props.FULL, region=True)
        prior, control = controls["translation"]
        refusal_cases(client, author, args, game, child, ids, control, prior, report["negative"])
        inputs = {str(path): digest(path.read_bytes()) for path in args.output.glob("*.vmf")}
        require(child.read_bytes() == original and identities(game) == mounted
                and all(not active.call("get_map_info")["rendering"]["initialized"] for active in clients),
                "Acceptance changed authored child bytes/private assets or initialized graphics")
        report["clean_eof"] = {str(index): require_clean_eof(active) for index, active in enumerate(clients)}
        require(all(digest(Path(path).read_bytes()) == identity for path, identity in inputs.items()) and identities(game) == mounted,
                "Clean EOF changed completed source files or private assets")
        require(args.compile_calls == report["expected_compile_calls"], "Actual compile request count differs from the focused matrix")
        report["sources"], report["assets"], report["status"] = inputs, mounted, "accepted"
    except BaseException as error:
        failure = error
        report["error"] = f"{type(error).__name__}: {error!r}"
        raise
    finally:
        report["compile_calls"] = args.compile_calls
        errors = props.cleanup_clients(clients, report)
        if errors:
            report["status"] = "failed"
        try:
            write_json(args.output / "results.json", report)
        except BaseException as error:
            errors.append(error)
        if errors:
            raise BaseExceptionGroup("Instance spawn validation and cleanup failed", ([failure] if failure else []) + errors) from None
    return report


def main():
    parser = argparse.ArgumentParser(description="Validate 726 child player spawns through either production private-stdio MCP program. Author/save/reopen both team classes; compare translated Fullbright and repeated nested noncommuting dual builds with independent literal origins/orientation bases and exact nonentity BSP payloads; inspect explicit unavailable CPU preview; check hidden/region omission, source staleness, six atomic refusals and clean EOF. 26 actual foreground/background compile requests. No installed game, model assets, graphics, native runtime or desktop input; compiler equivalence does not establish native spawn selection, balancing or clearance.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY, help="Integrated 726 hammerr-headless or editor, launched privately with --headless")
    parser.add_argument("--output", type=Path, required=True, help="New directory under repository target/validation; stores tiny private wall materials, authored VMFs, BSP/LIN controls and results.json")
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(strict=True), args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Keep retained artifacts under repository target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    result = validate(args)
    print(json.dumps({"status": result["status"], "compile_calls": result["compile_calls"], "output": str(args.output)}))


if __name__ == "__main__":
    main()
