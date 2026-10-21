import argparse
import json
import struct
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_instance_props as props
from validate_compile_regions import apply as apply_region
from validate_face_lighting import DEFAULT_BINARY, ROOT, detail
from validate_instance_contents import refresh
from validate_instance_lighting import stage_property
from validate_instance_terrain import private_client, property_state, write_json
from validate_material_albedo import digest, identities, require
from validate_recovery_exit import require_clean_eof
from validate_static_props import static_records

ANCHOR = (12, -20, 48)
TRANSLATED = (28, -28, 80)
NESTED = (72, -60, 124)
DIRECT = ("direct_ldr", 0)
ROOT_POSE = ((88, 64, 32), (0, 0, 0))
SECOND_POSES = (((-88, 48, 40), (0, 0, 0)), ((-24, 24, 56), (0, 0, -90)))


def helper(client, name, point, hidden=False):
    identifier = client.call("create_entity", {"classname": "info_lighting", "origin": point,
                                               "properties": {"targetname": name, "angles": "17 43 11"}})["id"]
    if hidden:
        client.call("set_object_visibility", {"action": "hide", "ids": [identifier]})
    return identifier


def child(client, path, models=props.MODELS, reference="Probe", point=ANCHOR, hidden=False, duplicate=None):
    client.call("new_map", {"discard_changes": True})
    anchor = helper(client, "Probe", point, hidden)
    identifiers = [props.prop(client, model, origin, angles, {"lightingorigin": reference})
                   for model, (origin, angles) in zip(models, props.SOURCE, strict=True)]
    if duplicate:
        helper(client, duplicate, [16, 24, 64])
    client.call("save_map", {"path": str(path)})
    saved = path.read_bytes()
    client.call("open_map", {"path": str(path)})
    require(path.read_bytes() == saved and dict(detail(client, [identifiers[0]])[0]["properties"])["lightingorigin"] == reference,
            "Generic helper/property/save/reopen changed authored lighting-origin bytes")
    return {"props": identifiers, "helper": anchor}


def named_middle(client, path, leaf):
    client.call("new_map", {"discard_changes": True})
    identifier = client.call("create_entity", {"classname": "func_instance", "origin": [8, 16, 64],
        "properties": {"file": leaf.name, "angles": "90 0 0", "targetname": "Inner"}})["id"]
    client.call("save_map", {"path": str(path)})
    return identifier


def fixture(client, path, child_path, placement="translation", models=props.MODELS, flat=False,
            repeated=False, root_binding=False, point=None, region=False, prefix="Outer", omitted=False):
    client.call("new_map", {"discard_changes": True})
    ids = props.room(client)
    owners, poses, points, names = [], [], [], []
    if root_binding:
        name = f"{prefix}-{'Inner-' if placement == 'noncommuting' else ''}Probe"
        ids.append(props.prop(client, models[0], *ROOT_POSE, {"lightingorigin": name}))
        poses.append(ROOT_POSE)
        points.append(point or (NESTED if placement == "noncommuting" else TRANSLATED))
    occurrences = [(prefix, (16, -8, 32), "0 0 0", props.LITERALS["translation"], point or TRANSLATED)]
    if placement == "noncommuting":
        occurrences = [(prefix, (16, -8, 128), "0 0 90", props.LITERALS[placement], point or NESTED)]
    if repeated:
        occurrences = [("Left", (16, -8, 32), "0 0 0", props.LITERALS["translation"], TRANSLATED),
                       ("Right", (-64, 32, 32), "0 0 0", SECOND_POSES, (-52, 12, 80))]
    if omitted:
        occurrences = [(prefix, (512, 0, 32), "0 0 0", (), ())]
        ids.append(props.prop(client, props.MODELS[0], *ROOT_POSE))
        poses, points = [ROOT_POSE], [None]
    for scope, origin, angles, literal_poses, literal_point in occurrences:
        name = f"{scope}-{'Inner-' if placement == 'noncommuting' else ''}Probe"
        names.append(name)
        if flat:
            if not omitted:
                ids.extend(props.prop(client, model, position, rotation, {"lightingorigin": name})
                           for model, (position, rotation) in zip(models, literal_poses, strict=True))
                helper(client, name, literal_point)
        else:
            identifier = client.call("create_entity", {"classname": "func_instance", "origin": origin,
                "properties": {"file": child_path.name, "angles": angles, "targetname": scope, "fixup_style": "0"}})["id"]
            owners.append(identifier)
            ids.append(identifier)
        if not omitted:
            poses.extend(literal_poses)
            points.extend([literal_point] * len(models))
    client.call("select", {"ids": owners or ids[:1]})
    if region:
        apply_region(client, low=[-128, -128, 0], high=[128, 128, 224])
    client.call("save_map", {"path": str(path)})
    return {"source": path, "ids": ids, "owner": owners[0] if owners else None, "owners": owners,
            "poses": poses, "points": points, "names": names}


def native_points(data, control, models):
    records = props.native(data, "translation", models, (0,) * len(models), control["poses"])
    for row, point in zip(records["props"], control["points"], strict=True):
        require(bool(row[10] & 2) == (point is not None)
                and row[14:17] == ((0, 0, 0) if point is None else point),
                "Native lighting-origin flag/point differs from independent literals")
    return records


def stats(receipt, helpers, expanded_props, compiled_props, occurrences, files, depth, sources):
    dependencies = [{"path": path.name, "bytes": path.stat().st_size, "sha256": digest(path.read_bytes())}
                    for path in sorted(sources)]
    for result in (receipt["result"], receipt["background"]["result"]):
        value = result["stats"]["instances"]
        require(value.get("lighting_origins", 0) == helpers and (helpers != 0 or "lighting_origins" not in value)
                and value["static_props"] == expanded_props and result["stats"]["static_props"] == compiled_props
                and (value["instances"], value["files"], value["max_expansion_depth"]) == (occurrences, files, depth)
                and value["dependencies"] == dependencies,
                "Expanded lighting-origin/prop counts or omit-zero policy differ before region filtering")


def preview(client, control, helpers):
    before = props.source_state(client, control["source"]), property_state(client), client.call("get_compile_settings")
    result = refresh(client, background=True)
    require(result["stats"]["lighting_origins"] == helpers and result["geometry"]["vertices"] == 0
            and result["geometry"]["indices"] == result["geometry"]["triangles"] == 0
            and not result["available"] and result["face_provenance_total"] == 0
            and result["limits"]["lighting_origin_name_bytes"] == 127
            and result["lighting_origin_preview"] == {"available": False,
                "reason": "Child lighting-origin helpers have no reference geometry or icons; helper bounds, picking and framing are unavailable."},
            "Helper/model reference counts fabricated helper geometry or binding proof")
    require((props.source_state(client, control["source"]), property_state(client), client.call("get_compile_settings")) == before
            and not client.call("get_map_info")["rendering"]["initialized"],
            "Lighting-origin CPU inspection changed source/history/settings/draft or initialized graphics")
    return result


def helper_only_preview(client, author, args):
    leaf = args.output / "helper-only-child.vmf"
    author.call("new_map", {"discard_changes": True})
    helper(author, "Probe", ANCHOR)
    author.call("save_map", {"path": str(leaf)})
    control = props.parent(client, args.output / "helper-only-parent.vmf", "translation", leaf)
    client.call("set_entity_properties", {"id": control["owner"], "set": {"targetname": "Only"}})
    client.call("save_map")
    result = preview(client, control, 1)
    require("static_props" not in result["stats"] and all(owner["bounds"] is None for owner in result["owners"]),
            "Helper-only contents fabricated models or helper bounds")
    return result


def control_pair(client, author, args, game, label, placement="translation", mode=props.FULL,
                 repeated=False, root_binding=False, point=None, region=False, models=props.MODELS, bake=False,
                 prefix="Outer", literal_point=None):
    leaf = args.output / f"{label}-child.vmf"
    child_ids = child(author, leaf, models, point=point or ANCHOR)
    reference, sources = leaf, [leaf]
    if placement == "noncommuting":
        reference = args.output / f"{label}-middle.vmf"
        named_middle(author, reference, leaf)
        sources.append(reference)
    if point is not None:
        require(placement == "translation" and literal_point is not None,
                "Altered helper control requires its own independently authored literal point")
    common = {"placement": placement, "models": models, "repeated": repeated, "root_binding": root_binding,
              "point": literal_point, "region": region, "prefix": prefix}
    control = fixture(client, args.output / f"{label}-parent.vmf", reference, **common)
    flat = fixture(client, args.output / f"{label}-flat.vmf", reference, flat=True, **common)
    client.call("open_map", {"path": str(control["source"]), "discard_changes": True})
    props.setup(client, mode, bake)
    token = stage_property(client)
    data, receipt = props.pair(client, args, game, label, control, sources)
    expected_models = ((models[0],) if root_binding else ()) + models * (2 if repeated else 1)
    stats(receipt, 2 if repeated else 1, 4 if repeated else 2, len(expected_models),
          2 if repeated or placement == "noncommuting" else 1, len(sources), 2 if placement == "noncommuting" else 1, sources)
    receipt["native"] = native_points(data, control, expected_models)
    receipt["preview"] = preview(client, control, 2 if repeated else 1)
    client.call("cancel_property_edit", {"token": token})
    client.call("open_map", {"path": str(flat["source"]), "discard_changes": True})
    props.setup(client, mode, bake)
    manual, receipt["flat"] = props.pair(client, args, game, f"{label}-flat", flat, sources)
    require("instances" not in receipt["flat"]["result"]["stats"], "Literal helpers acquired instance metadata")
    receipt["comparison"] = props.compare(data, manual, placement, expected_models, bake,
                                         (0,) * len(expected_models), control["poses"])
    native_points(manual, flat, expected_models)
    if bake:
        receipt["bake"], entries = props.baked(data, receipt["result"]["stats"], args, models[0])
        receipt["flat"]["bake"], literal_entries = props.baked(manual, receipt["flat"]["result"]["stats"], args, models[0])
        require({name: raw for name, raw in entries.items() if name != props.MANIFEST}
                == {name: raw for name, raw in literal_entries.items() if name != props.MANIFEST},
                "Scoped/literal complete dual VHV bytes differ")
    return data, receipt, control, sources, child_ids


def reference_form(client, control):
    client.call("open_map", {"path": str(control["source"]), "discard_changes": True})
    before = dict(detail(client, [control["owner"]])[0]["properties"])
    source_bytes = control["source"].read_bytes()
    form = client.call("set_instance_reference", {"action": "begin", "id": control["owner"]})
    token = form["token"]
    client.call("set_instance_reference", {"action": "update", "token": token, "file": before["file"],
                                          "origin": before["origin"], "angles": before["angles"]})
    checked = client.call("set_instance_reference", {"action": "check", "token": token})
    require(checked["valid"] and checked["contents_validation"] == "not_checked", "Reference form claimed binding validation")
    client.call("set_instance_reference", {"action": "apply", "token": token})
    require(dict(detail(client, [control["owner"]])[0]["properties"]) == before,
            "Reference authoring form changed unrelated explicit prefix fields")
    require(control["source"].read_bytes() == source_bytes, "No-op reference form rewrote the saved source")
    return checked


def negative_cases(client, author, args, game, prior, control, sources, child_ids, report):
    leaf = sources[0]
    original = leaf.read_bytes()
    parent_bytes = control["source"].read_bytes()
    cases = (
        ("missing-local-root-substitute", "local", {"lightingorigin": "Missing"}, "visible local info_lighting"),
        ("hidden-local", "hidden", None, "visible local info_lighting"),
        ("case-sensitive-local", "local", {"lightingorigin": "probe"}, "visible local info_lighting"),
        ("duplicate-local", "duplicate", "Probe", "duplicate or case-ambiguous name"),
        ("case-ambiguous-local", "duplicate", "probe", "duplicate or case-ambiguous name"),
        ("postfix", "parent", {"fixup_style": "1"}, "Prefix"),
        ("none-style", "parent", {"fixup_style": "2"}, "Prefix"),
        ("implicit-scope", "remove", "targetname", "Fix"),
        ("global-escape", "local", {"lightingorigin": "@Probe"}, "ASCII"),
        ("fallback-name", "parent", {"name": "Fallback"}, "name"),
        ("composed-name-cap", "parent", {"targetname": "A" * 123}, "127"),
        ("root-case-collision", "collision", "outer-probe", "collides"),
        ("reused-prefix", "repeat", None, "collides"),
    )
    for label, kind, changes, reason in cases:
        author.call("open_map", {"path": str(leaf), "discard_changes": True})
        if kind == "local":
            for identifier in child_ids["props"]:
                author.call("set_entity_properties", {"id": identifier, "set": changes})
        elif kind == "hidden":
            author.call("set_object_visibility", {"action": "hide", "ids": [child_ids["helper"]]})
        elif kind == "duplicate":
            helper(author, changes, [16, 24, 64])
        author.call("save_map")
        client.call("open_map", {"path": str(control["source"]), "discard_changes": True})
        if kind == "parent":
            client.call("set_entity_properties", {"id": control["owner"], "set": changes})
        elif kind == "remove":
            client.call("set_entity_properties", {"id": control["owner"], "remove": [changes]})
        elif kind == "collision" or label == "missing-local-root-substitute":
            helper(client, changes if kind == "collision" else "Outer-Missing", [8, 8, 96])
        current = control
        if kind == "repeat":
            identifier = client.call("create_entity", {"classname": "func_instance", "origin": [-64, 32, 32],
                "properties": {"file": leaf.name, "angles": "0 0 0", "targetname": "Outer", "fixup_style": "0"}})["id"]
            current = {**control, "ids": [*control["ids"], identifier], "owner": identifier}
        client.call("save_map")
        props.setup(client)
        stage_property(client)
        try:
            _, report[label] = props.pair(client, args, game, label, current, sources, reason, prior, leaf.name)
            if kind == "hidden" or (kind == "local" and label != "global-escape"):
                for result in (report[label]["result"], report[label]["background"]["result"]):
                    require(any(row["object"] == control["owner"] and reason in row["message"]
                                and any(f"child object {identifier.split(':')[1]}" in row["message"] for identifier in child_ids["props"])
                                for row in result["diagnostics"]), "Deferred local refusal lost its original child prop identity")
        finally:
            leaf.write_bytes(original)
            control["source"].write_bytes(parent_bytes)


def omitted(client, author, args, game):
    leaf = args.output / "omitted-child.vmf"
    child(author, leaf, models=(props.INVALID, props.INVALID), reference="Missing", hidden=True)
    control = fixture(client, args.output / "omitted-parent.vmf", leaf, omitted=True, region=True)
    flat = fixture(client, args.output / "omitted-flat.vmf", leaf, omitted=True, region=True, flat=True)
    client.call("open_map", {"path": str(control["source"]), "discard_changes": True})
    props.setup(client)
    stage_property(client)
    data, receipt = props.pair(client, args, game, "omitted", control, [leaf])
    stats(receipt, 0, 2, 1, 1, 1, 1, [leaf])
    native_points(data, control, (props.MODELS[0],))
    receipt["unread"] = props.unread_model(client, args, game, control["source"])
    client.call("open_map", {"path": str(flat["source"]), "discard_changes": True})
    props.setup(client)
    literal, receipt["flat"] = props.pair(client, args, game, "omitted-flat", flat, [leaf])
    receipt["comparison"] = props.compare(data, literal, "translation", (props.MODELS[0],), False, (0,), control["poses"])
    return receipt


def identity(client, args, game, prior, control, sources):
    client.call("open_map", {"path": str(control["source"]), "discard_changes": True})
    props.setup(client)
    stage_property(client)
    data, receipt = props.pair(client, args, game, "identity-before", control, sources)
    require(data == prior, "Named child control changed before identity check")
    before = props.source_state(client, control["source"]), property_state(client), client.call("get_compile_settings")
    outputs = identities(args.output / "builds")
    leaf = sources[0]
    backup = args.output / "original-child-identity.vmf"
    original = leaf.read_bytes()
    leaf.rename(backup)
    try:
        leaf.write_bytes(original)
        stale = client.call("get_compile_status")
        require(stale["job"]["stale"] and not stale["job"]["instance_dependencies"]["current"]
                and not stale["launch"]["ready"], "Same-byte named child replacement lost original source identity")
        require((props.source_state(client, control["source"]), property_state(client), client.call("get_compile_settings")) == before
                and identities(args.output / "builds") == outputs, "Identity inspection changed protected state or BSP/LIN")
    finally:
        leaf.unlink(missing_ok=True)
        backup.rename(leaf)
    repaired, rebuilt = props.pair(client, args, game, "identity-repaired", control, sources)
    require(repaired == prior, "Restoring named child identity changed native BSP bytes")
    return {"before": receipt, "stale": stale, "repaired": rebuilt}


def unnamed_compatibility(client, author, args, game):
    leaf = args.output / "unnamed-child.vmf"
    props.leaf(author, leaf)
    retained, receipts = None, {}
    for style in (0, 1, 2):
        label = f"unnamed-style-{style}"
        control = props.parent(client, args.output / f"{label}-parent.vmf", "translation", leaf)
        client.call("set_entity_properties", {"id": control["owner"], "set": {"fixup_style": str(style)}})
        client.call("save_map")
        props.setup(client)
        stage_property(client)
        data, receipt = props.pair(client, args, game, label, control, [leaf])
        stats(receipt, 0, 2, 2, 1, 1, 1, [leaf])
        records = props.native(data, "translation", props.MODELS)
        require(all(row[10] & 2 == 0 for row in records["props"]), "Unnamed prop acquired a lighting-origin flag")
        require(retained is None or data == retained, "Existing unnamed0/1/2 helper styles changed whole BSP bytes")
        retained, receipts[str(style)] = data, receipt
    return receipts


def optional_bake(client, author, args, game):
    models = (args.baked_model, args.baked_model)
    data, receipt, control, sources, ids = control_pair(client, author, args, game, "baked", mode=props.DUAL, models=models, bake=True)
    leaf = sources[0]
    original = leaf.read_bytes()
    author.call("open_map", {"path": str(leaf), "discard_changes": True})
    author.call("set_entity_properties", {"id": ids["helper"], "set": {"origin": "12 -20 -8"}})
    author.call("save_map")
    client.call("open_map", {"path": str(control["source"]), "discard_changes": True})
    props.setup(client, props.DUAL, True)
    stage_property(client)
    try:
        moved, moved_receipt = props.pair(client, args, game, "baked-helper-moved", control, sources)
        expected = {**control, "points": [(28, -28, 24)] * 2}
        before, after = static_records(data), native_points(moved, expected, models)
        rows = [tuple((*row[:14], 28, -28, 24, *row[17:])) for row in before["props"]]
        require(after == {**before, "props": rows}, "Moving a helper changed an ordinal, leaf, placement or unrelated native field")
        left_records, right_records = probe.game_lump_records(data)[1], probe.game_lump_records(moved)[1]
        for left, right in zip(left_records, right_records, strict=True):
            require(left[:4] == right[:4], "Helper movement changed original game-lump directory identity")
            if left[0] == props.SPRP:
                expected_payload = bytearray(left[4])
                prefix = 12 + 128 * len(before["models"]) + 2 * len(before["leaves"])
                for ordinal in range(2):
                    struct.pack_into("<3f", expected_payload, prefix + ordinal * 64 + 44, 28, -28, 24)
                require(right[4] == bytes(expected_payload) and right[4] != left[4],
                        "Helper movement changed original sprp6 bytes beyond the two expected native lighting points")
            else:
                require(left == right, "Helper movement changed a non-prop game lump")
        require(all(left == right for index, (left, right) in enumerate(zip(probe.bsp_lumps(data), probe.bsp_lumps(moved), strict=True))
                    if index not in (35, 40)), "Moving a helper changed a non-prop/non-PAK BSP lump")
        moved_receipt["bake"], changed_entries = props.baked(moved, moved_receipt["result"]["stats"], args, args.baked_model)
        original_entries = props.pak_and_flags(data)[0]
        require({name: raw for name, raw in original_entries.items() if name != props.MANIFEST}
                == {name: raw for name, raw in changed_entries.items() if name != props.MANIFEST},
                "Helper movement changed complete all-LOD dual VHV payloads")
        first = receipt["bake"]["manifest"]
        second = moved_receipt["bake"]["manifest"]
        require(first["static_props_sha256"] != second["static_props_sha256"]
                and {key: value for key, value in first.items() if key != "static_props_sha256"}
                == {key: value for key, value in second.items() if key != "static_props_sha256"},
                "Moved helper manifest changed beyond its original native-table digest")
        receipt["helper_moved"] = moved_receipt
    finally:
        leaf.write_bytes(original)
    author.call("open_map", {"path": str(leaf), "discard_changes": True})
    author.call("set_entity_properties", {"id": ids["props"][0], "set": {"origin": "56 16 8"}})
    author.call("save_map")
    try:
        receiver, moved_receipt = props.pair(client, args, game, "baked-receiver-moved", control, sources)
        expected = {**control, "poses": [((72, 8, 40), (0, 0, 0)), props.LITERALS["translation"][1]]}
        native_points(receiver, expected, models)
        moved_receipt["bake"], changed_entries = props.baked(receiver, moved_receipt["result"]["stats"], args, args.baked_model)
        layout = receipt["bake"]["manifest"]["receivers"][0]
        require(layout == moved_receipt["bake"]["manifest"]["receivers"][0],
                "Receiver movement changed original model/checksum/ordinal/hardware layout")
        witnesses = {}
        for channel, name in (("ldr", "sp_0.vhv"), ("hdr", "sp_hdr_0.vhv")):
            old, colors_before = props.read_vhv(original_entries[name], layout["checksum"], layout["meshes"])
            new, colors_after = props.read_vhv(changed_entries[name], layout["checksum"], layout["meshes"])
            changed = sum(a != b for a, b in zip(colors_before, colors_after, strict=True))
            require(changed > 0, f"Prepared fixture has no discriminating receiver-movement {channel} color witness")
            witnesses[channel] = {"changed_vertices": changed, "before_sha256": old["sha256"], "after_sha256": new["sha256"]}
        moved_receipt["color_witness"] = witnesses
        receipt["receiver_moved"] = moved_receipt
    finally:
        leaf.write_bytes(original)
    repaired, receipt["repaired"] = props.pair(client, args, game, "baked-repaired", control, sources)
    require(repaired == data, "Restoring a helper failed exact dual-baked BSP recovery")
    return receipt


def validate(args):
    clients, failure = [], None
    args.compile_calls = 0
    report = {"status": "failed", "binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()),
              "script_sha256": digest(Path(__file__).read_bytes()), "compile_calls": 0,
              "shared_helper_sha256": digest(Path(props.__file__).read_bytes()),
              "expected_compile_calls": 60 + (10 if args.baked_fixture_game else 0), "positive": {}, "negative": {},
              "graphics_initialized": False, "native_game_exercised": False, "desktop_input": False,
              "scope": "Compiler equivalence and original-source ownership for explicit Prefix-scoped child lighting origins. Counts and absent helper/model reference geometry do not prove deferred binding. No native appearance or VHV consumer claim."}
    try:
        (args.output / "tmp").mkdir()
        (args.output / "builds").mkdir()
        external = props.small_files(args.baked_fixture_game) if args.baked_fixture_game else None
        game = props.game_assets(args)
        mounted = identities(game)
        for name in ("client", "author"):
            clients.append(private_client(args.binary, args.output / name, game, args.output / "tmp"))
        client, author = clients
        retained, baseline, sources, ids = None, None, None, None
        for label, placement, mode, repeated, root_binding in (
            ("translated", "translation", props.FULL, False, True),
            ("translated-ldr", "translation", DIRECT, False, True),
            ("nested-dual", "noncommuting", props.DUAL, False, True),
            ("repeated", "translation", props.FULL, True, False),
        ):
            data, receipt, control, dependencies, child_ids = control_pair(
                client, author, args, game, label, placement, mode, repeated, root_binding,
                prefix="A" * 121 if label == "translated-ldr" else "Outer")
            report["positive"][label] = receipt
            if label == "translated":
                retained, baseline, sources, ids = data, control, dependencies, child_ids
        report["reference_form"] = reference_form(client, baseline)
        negative_cases(client, author, args, game, retained, baseline, sources, ids, report["negative"])
        _, report["positive"]["outside-region-helper"], _, _, _ = control_pair(
            client, author, args, game, "outside-region-helper", point=(512, -20, 48), literal_point=(528, -28, 80), region=True)
        report["omitted"] = omitted(client, author, args, game)
        report["identity"] = identity(client, args, game, retained, baseline, sources)
        report["helper_only_preview"] = helper_only_preview(client, author, args)
        report["unnamed_compatibility"] = unnamed_compatibility(client, author, args, game)
        if args.baked_fixture_game:
            report["baked"] = optional_bake(client, author, args, game)
        inputs = {str(path): digest(path.read_bytes()) for path in args.output.glob("*.vmf")}
        report["clean_eof"] = {str(index): require_clean_eof(active) for index, active in enumerate(clients)}
        require(identities(game) == mounted and all(digest(Path(path).read_bytes()) == value for path, value in inputs.items())
                and (not external or props.small_files(args.baked_fixture_game) == external), "Compilation/EOF changed source or mounted assets")
        require(args.compile_calls == report["expected_compile_calls"], "Actual compile request count differs from the focused matrix")
        report.update(status="accepted", sources=inputs, assets=mounted, baked_fixture=external)
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
            raise BaseExceptionGroup("Lighting-origin validation and cleanup failed", ([failure] if failure else []) + errors) from None
    return report


def main():
    parser = argparse.ArgumentParser(description="Validate scoped child info_lighting/prop lightingorigin through both real production MCP compile paths. Requires Python 3.11+ and integrated 722. Steps: generic author/save/reopen and reference form; independent translated, noncommuting and repeated Prefix literal controls in Fullbright/LDR/dual; native flag2/point/ordinal/dictionary/leaf equality; the 127-byte composed-name boundary and strict local scope/style/name failures; outside-region helpers, omitted missing refs and unread invalid assets; same-byte dependency identity and unchanged unnamed 0/1/2 branches. 60 compile requests, 70 with optional real all-LOD dual VHV helper/receiver-movement controls. CPU/headless only; no native appearance, graphics or desktop input.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY, help="Integrated 722 hammerr-headless or editor; private --headless stdio Session")
    parser.add_argument("--output", type=Path, required=True, help="New directory under repository target/validation; complete tiny model fixtures need no installed game")
    parser.add_argument("--baked-fixture-game", type=Path, help="Optional bounded read-only model/material fixture under target/validation, at most256 files/64MiB; same prerequisites as validate_instance_props")
    parser.add_argument("--baked-model", help="Required with --baked-fixture-game: canonical models/...mdl with admitted MDL/VVD/dx90.VTX companions, unit/body0/skin0, opaque normal-white, fitting translated receivers including ordinal0 moved to72 8 40. Both decoded channel color streams must discriminate that movement")
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(strict=True), args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Keep artifacts under repository target/validation")
    require(bool(args.baked_fixture_game) == bool(args.baked_model), "Supply both baked fixture options")
    if args.baked_fixture_game:
        args.baked_fixture_game = args.baked_fixture_game.resolve(strict=True)
        require(args.baked_fixture_game.is_relative_to(ROOT / "target/validation") and args.baked_model.startswith("models/")
                and args.baked_model.endswith(".mdl") and args.baked_model == args.baked_model.lower()
                and "\\" not in args.baked_model and ".." not in args.baked_model.split("/"), "Use a canonical private baked fixture")
        require(all((args.baked_fixture_game / (args.baked_model[:-4] + suffix)).is_file() for suffix in (".mdl", ".vvd", ".dx90.vtx")),
                "Optional real model companions are incomplete")
    args.output.mkdir(parents=True, exist_ok=False)
    report = validate(args)
    print(json.dumps({"status": report["status"], "compile_calls": report["compile_calls"], "output": str(args.output)}))


if __name__ == "__main__":
    main()
