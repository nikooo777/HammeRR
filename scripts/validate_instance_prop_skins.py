import argparse
import json
import re
import struct
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_instance_lighting_origins as origins
import validate_instance_props as props
from validate_face_lighting import DEFAULT_BINARY, ROOT, detail
from validate_instance_contents import refresh
from validate_instance_lighting import stage_property
from validate_instance_terrain import private_client, property_state, write_json
from validate_material_albedo import digest, identities, require
from validate_nested_brush_instances import fixture_blocks
from validate_recovery_exit import require_clean_eof

MODEL = props.MODELS[0]
MODELS = (MODEL, MODEL)
ALTERNATE = "materials/models/hammerr719/alternate.vmt"
FLAGS = {"ignorenormals": 8, "disableshadows": 16, "disablevertexlighting": 64, "disableselfshadowing": 128}


def game_assets(args):
    game = props.game_assets(args)
    for name in props.MODELS:
        path = game / name
        data = bytearray(path.read_bytes())
        require(len(data) == 1024 and all(struct.unpack_from("<i", data, offset)[0] == value
                                        for offset, value in ((204, 1), (208, 256), (220, 1), (224, 1), (228, 388))),
                "Two-family fixture requires the existing complete fixed-layout model")
        for offset, value in ((204, 2), (224, 2), (320, 416)):
            struct.pack_into("<i", data, offset, value)
        struct.pack_into("<2H", data, 388, 0, 1)
        data[736:752] = b"alternate\0".ljust(16, b"\0")
        path.write_bytes(data)
    (game / ALTERNATE).write_text('VertexLitGeneric { $model 1 $basetexture "hammerr719/white" }\n')
    return game


def set_skins(client, identifiers, skins):
    for identifier, skin in zip(identifiers, skins, strict=True):
        changes = {"remove": ["skin"]} if skin is None else {"set": {"skin": str(skin)}}
        client.call("set_entity_properties", {"id": identifier, **changes})


def set_flags(client, identifiers, flags):
    for identifier, values in zip(identifiers, flags, strict=True):
        client.call("set_entity_properties", {"id": identifier, "set": values})


def leaf(client, path, skins, scoped=False, omitted=None):
    identifiers = origins.child(client, path, MODELS)["props"] if scoped else props.leaf(client, path, MODELS)
    set_skins(client, identifiers, skins)
    if omitted:
        extra = props.prop(client, props.MODELS[1], [512, 0, 32] if omitted == "region" else [0, 0, 16],
                           [0, 0, 0], {"skin": "2"})
        if omitted == "hidden":
            client.call("set_object_visibility", {"action": "hide", "ids": [extra]})
    client.call("save_map")
    saved = path.read_bytes()
    client.call("open_map", {"path": str(path)})
    require(path.read_bytes() == saved
            and [dict(row["properties"]).get("skin") for row in detail(client, identifiers)]
            == [None if skin is None else str(skin) for skin in skins],
            "Saving/reopening selected-skin properties changed child bytes or raw values")
    return identifiers


def lighting_points(records, scoped):
    for row in records["props"]:
        require(bool(row[10] & 2) == scoped and row[14:17] == (origins.NESTED if scoped else (0, 0, 0)),
                "Skin admission changed the existing scoped lighting-origin contract")


def native(data, placement, skins, scoped=False):
    records = props.native(data, placement, MODELS, skins=tuple(0 if skin is None else int(skin) for skin in skins))
    lighting_points(records, scoped)
    return records


def compare(actual, literal, placement, skins, scoped=False):
    receipt = props.compare(actual, literal, placement, MODELS, skins=tuple(0 if skin is None else int(skin) for skin in skins))
    lighting_points(receipt["nested_native"], scoped)
    lighting_points(receipt["literal_native"], scoped)
    return receipt


def saved_versions(path):
    text, versions = path.read_text(), {}
    for name in ("versioninfo", "world"):
        blocks = fixture_blocks(text, name)
        require(len(blocks) == 1, "Saved fixture must contain one versioninfo and world block")
        values = re.findall(r'"mapversion"\s+"([0-9]+)"', blocks[0])
        require(len(values) == 1, "Saved fixture must contain one authored mapversion per block")
        versions[name] = int(values[0])
    require(versions["versioninfo"] == versions["world"], "Saved fixture mapversion fields disagree")
    return versions


def controls(client, author, args, game, label, placement, skins, mode, scoped=False, region=False, omitted=None, flags=None):
    child_path = args.output / f"{label}-child.vmf"
    identifiers = leaf(author, child_path, skins, scoped, omitted)
    if flags:
        set_flags(author, identifiers, flags)
        author.call("save_map")
    reference, sources = child_path, [child_path]
    if placement == "noncommuting":
        reference = args.output / f"{label}-middle.vmf"
        origins.named_middle(author, reference, child_path)
        sources.append(reference)
    if scoped:
        control = origins.fixture(client, args.output / f"{label}-parent.vmf", reference, placement, MODELS)
        flat = origins.fixture(client, args.output / f"{label}-flat.vmf", reference, placement, MODELS, flat=True)
    else:
        control = props.parent(client, args.output / f"{label}-parent.vmf", placement, reference, MODELS, region)
        flat = props.parent(client, args.output / f"{label}-flat.vmf", placement, models=MODELS, region=region)
    set_skins(client, flat["ids"][-2:], skins)
    if flags:
        set_flags(client, flat["ids"][-2:], flags)
    client.call("save_map")
    client.call("open_map", {"path": str(control["source"]), "discard_changes": True})
    client.call("save_map")
    versions = saved_versions(control["source"])
    require(versions == saved_versions(flat["source"]) == {"versioninfo": 2, "world": 2},
            "Completed parent and flat fixtures must share the same two-save lifecycle")
    props.setup(client, mode)
    token = stage_property(client)
    data, receipt = props.pair(client, args, game, label, control, sources)
    receipt["fixture_mapversions"] = versions
    origins.stats(receipt, 1 if scoped else 0, 3 if region else 2, 2,
                  2 if placement == "noncommuting" else 1, len(sources), 2 if placement == "noncommuting" else 1, sources)
    if omitted:
        receipt["unread_status"] = no_read_status(client, args, game, control)
    client.call("cancel_property_edit", {"token": token})
    client.call("open_map", {"path": str(flat["source"]), "discard_changes": True})
    props.setup(client, mode)
    literal, receipt["flat"] = props.pair(client, args, game, f"{label}-flat", flat, sources)
    require("instances" not in receipt["flat"]["result"]["stats"], "Literal selected skins acquired instance metadata")
    receipt["comparison"] = compare(data, literal, placement, skins, scoped)
    if flags:
        expected = [sum(FLAGS[key] for key, value in values.items() if int(value) == 1) | (2 if scoped else 0) for values in flags]
        require([row[10] for row in receipt["comparison"]["nested_native"]["props"]] == expected,
                "Authored static-lighting fields did not set their exact native flag bits")
        receipt["native_flag_bits"] = expected
    return data, receipt, control, sources, identifiers


def no_read_status(client, args, game, control):
    state = props.source_state(client, control["source"]), property_state(client), client.call("get_compile_settings")
    outputs = identities(args.output / "builds")
    files = [(game / props.MODELS[1]), (game / props.MODELS[1]).with_suffix(".phy")]
    originals = {path: path.read_bytes() for path in files}
    try:
        for path in files:
            path.write_bytes(b"Omitted out-of-range skin assets must remain unread\n")
        status = client.call("get_compile_status")
        require(not status["job"]["stale"] and status["job"]["static_prop_source_issue"] is None
                and status["job"]["instance_dependencies"]["current"], "Omitted skin caused an early model/PHY read or guard")
        require((props.source_state(client, control["source"]), property_state(client), client.call("get_compile_settings")) == state
                and identities(args.output / "builds") == outputs, "Omitted skin inspection changed state or BSP/LIN")
        return status
    finally:
        for path, data in originals.items():
            path.write_bytes(data)


def refusals(client, args, game, prior, control, sources, identifiers):
    original = sources[0].read_text()
    report = {}
    for label, value, reason in (("negative", "-1", "skin"), ("fractional", "1.5", "skin"),
                                 ("out-of-range", "2", "skin must be from 0 to 1")):
        changed, count = re.subn(r'("skin"\s+")0(")', rf'\g<1>{value}\2', original, count=1)
        require(count == 1, "Unsupported imported fixture must change exactly one skin field")
        client.call("open_map", {"path": str(control["source"]), "discard_changes": True})
        props.setup(client)
        stage_property(client)
        try:
            sources[0].write_text(changed)
            if label == "out-of-range":
                before = props.source_state(client, control["source"]), property_state(client), client.call("get_compile_settings")
                ready = refresh(client, background=True)
                require(ready["stats"]["static_props"] == 2 and not ready["available"]
                        and ready["geometry"]["vertices"] == ready["geometry"]["indices"] == 0
                        and ready["static_prop_preview"]["available"] is False,
                        "Reference refresh certified or rendered an unavailable skin family")
                require((props.source_state(client, control["source"]), property_state(client), client.call("get_compile_settings")) == before
                        and not client.call("get_map_info")["rendering"]["initialized"], "Skin-family CPU refresh changed protected state or initialized graphics")
                report["range_preview"] = ready
            _, report[label] = props.pair(client, args, game, f"skin-{label}", control, sources, reason, prior, sources[0].name)
            if label == "out-of-range":
                require(all(any(row["object"] == control["owner"] and reason in row["message"]
                                and f"child object {identifiers[0].split(':')[1]}" in row["message"] for row in result["diagnostics"])
                            for result in (report[label]["result"], report[label]["background"]["result"])),
                        "Retained skin-range refusal lost its original child identity")
        finally:
            sources[0].write_text(original)
    return report


def bake_manifest(data, receipt, game, skins):
    native(data, "translation", skins)
    entries, flags = props.pak_and_flags(data)
    require(flags & 3 == 3 and props.MANIFEST in entries, "Selected-material bake omitted a channel or manifest")
    manifest = json.loads(entries[props.MANIFEST])
    sprp = next(row[4] for row in probe.game_lump_records(data)[1] if row[0] == props.SPRP)
    require(manifest["version"] == 1 and manifest["channels"] == ["ldr", "hdr"]
            and manifest["static_props_version"] == 6 and manifest["static_props_sha256"] == digest(sprp)
            and [row["ordinal"] for row in manifest["receivers"]] == [0, 1], "Selected-material manifest/native ordinal identity differs")
    measured, illuminated = {}, False
    for receiver in manifest["receivers"]:
        require(receiver["model"] == MODEL and receiver["checksum"] == 719
                and receiver["meshes"] == [{"lod": 0, "vertex_count": 6}], "Tiny selected-material hardware layout differs")
        for channel in ("ldr", "hdr"):
            name = f"sp_{'hdr_' if channel == 'hdr' else ''}{receiver['ordinal']}.vhv"
            asset = next(row for row in manifest["assets"] if row["path"] == name)
            measured[name], colors = props.read_vhv(entries[name], 719, receiver["meshes"])
            require(asset["bytes"] == measured[name]["bytes"] and asset["sha256"] == measured[name]["sha256"],
                    "Selected-material VHV is absent or corrupt")
            illuminated |= any(any(color[:3]) for color in colors)
    require(set(measured) == {name for name in entries if props.VHV_NAME.fullmatch(name)} and len(manifest["assets"]) == 4
            and receipt["result"]["stats"]["prop_vertex_lighting"]["props"] == 2 and illuminated,
            "Selected-material bake count/membership/illumination differs")
    for source in manifest["sources"]:
        path = Path(source["path"])
        require(not path.is_absolute() and ".." not in path.parts and "\\" not in source["path"], "Unsafe material source receipt")
        raw = (game / path).read_bytes()
        require(len(raw) == source["bytes"] and digest(raw) == source["sha256"], "Selected-material source receipt differs from original bytes")
    receipt["bake"] = {"manifest": manifest, "files": measured}
    return manifest


def selected_material(client, author, args, game):
    child_path = args.output / "selected-material-child.vmf"
    identifiers = leaf(author, child_path, (0, 0))
    control = props.parent(client, args.output / "selected-material-parent.vmf", "translation", child_path, MODELS)
    props.setup(client, props.DUAL, True)
    stage_property(client)
    path, report = game / ALTERNATE, {}
    original = path.read_bytes()
    try:
        path.write_text('VertexLitGeneric { $model 1 $basetexture "hammerr719/white" $bumpmap unsupported }\n')
        zero, report["skin0"] = props.pair(client, args, game, "material-skin0", control, [child_path])
        manifest = bake_manifest(zero, report["skin0"], game, (0, 0))
        require(ALTERNATE not in {row["path"] for row in manifest["sources"]}, "Unused alternate family was consumed as a bake source")
        author.call("open_map", {"path": str(child_path), "discard_changes": True})
        set_skins(author, identifiers, (None, None))
        author.call("save_map")
        require(not re.search(r'"skin"\s+"', child_path.read_text()), "Absent/default0 fixture retained a stored skin")
        absent, report["absent0"] = props.pair(client, args, game, "material-absent0", control, [child_path])
        require(absent == zero, "Absent/default0 changed the complete original BSP/VHV bytes")
        report["absent0"]["legacy_default_comparison"] = props.compare(absent, zero, "translation", MODELS, True)
        set_skins(author, identifiers, (1, 1))
        author.call("save_map")
        _, report["selected1-refusal"] = props.pair(client, args, game, "material-skin1-refused", control, [child_path],
                                                   "$bumpmap", zero, child_path.name)
        set_flags(author, identifiers, ({"disablevertexlighting": "1"},) * 2)
        author.call("save_map")
        opted_out, report["vertex-opt-out"] = props.pair(client, args, game, "material-vertex-opt-out", control, [child_path])
        records = native(opted_out, "translation", (1, 1))
        entries, bits = props.pak_and_flags(opted_out)
        require([row[10] for row in records["props"]] == [64, 64] and bits & 3 == 0
                and props.MANIFEST not in entries and not any(props.VHV_NAME.fullmatch(name) for name in entries)
                and all(result["stats"].get("prop_vertex_lighting") is None
                        for result in (report["vertex-opt-out"]["result"], report["vertex-opt-out"]["background"]["result"])),
                "Explicit vertex-lighting opt-out consumed unsupported receiver material or generated VHV")
        author.call("set_entity_properties", {"id": identifiers[0], "set": {"model": "models/hammerr723/missing.mdl"}})
        author.call("save_map")
        _, report["opt-out-missing-model"] = props.pair(client, args, game, "opt-out-missing-model", control, [child_path],
                                                        "was not found", opted_out, child_path.name)
        author.call("set_entity_properties", {"id": identifiers[0], "set": {"model": MODEL}})
        set_flags(author, identifiers, ({"disablevertexlighting": "0"},) * 2)
        author.call("save_map")
    finally:
        path.write_bytes(original)
    one, report["selected1-restored"] = props.pair(client, args, game, "material-skin1-restored", control, [child_path])
    manifest = bake_manifest(one, report["selected1-restored"], game, (1, 1))
    require(ALTERNATE in {row["path"] for row in manifest["sources"]}, "Selected skin1 did not consume its actual alternate material")
    return report


def validate(args):
    clients, failure = [], None
    args.compile_calls, args.baked_fixture_game = 0, None
    report = {"status": "failed", "binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()),
              "script_sha256": digest(Path(__file__).read_bytes()), "shared_helper_sha256": digest(Path(props.__file__).read_bytes()),
              "origin_helper_sha256": digest(Path(origins.__file__).read_bytes()),
              "compile_calls": 0, "expected_compile_calls": 32, "graphics_initialized": False,
              "native_game_exercised": False, "desktop_input": False, "positive": {}, "omitted": {},
              "scope": "Compiler equivalence for selected child skin families, authored static-lighting flag bits, vertex-lighting opt-out and actual selected-material admission/source receipts. No alternate native appearance or engine VHV-consumer claim."}
    try:
        (args.output / "tmp").mkdir()
        (args.output / "builds").mkdir()
        game = game_assets(args)
        mounted = identities(game)
        for name in ("client", "author"):
            clients.append(private_client(args.binary, args.output / name, game, args.output / "tmp"))
        client, author = clients
        report["model_families"] = [client.call("inspect_model", {"path": MODEL, "skin": skin, "body": 0}) for skin in (0, 1)]
        require(all(value["static_prop"] and value["vertices"] == 4 and value["triangles"] == 2 and value["skins"] == 2
                    and value["skin"] == skin and value["body"] == 0 and value["material_error"] is None
                    and len(value["materials"]) == 1 and f"models/hammerr719/{material}" in value["materials"][0]
                    for skin, material, value in zip((0, 1), ("receiver", "alternate"), report["model_families"], strict=True)),
                "Complete CPU model inspection did not bind two actual distinct skin-family materials")
        retained, baseline, sources, ids = None, None, None, None
        for label, placement, skins, mode, scoped in (
            ("translated", "translation", (0, "01"), props.FULL, False),
            ("nested-scoped-dual", "noncommuting", (1, 0), props.DUAL, True),
        ):
            flags = ({key: "1" for key in FLAGS}, {key: "0" for key in FLAGS}) if scoped else None
            data, receipt, control, dependencies, identifiers = controls(
                client, author, args, game, label, placement, skins, mode, scoped, flags=flags)
            report["positive"][label] = receipt
            if label == "translated":
                retained, baseline, sources, ids = data, control, dependencies, identifiers
        report["negative"] = refusals(client, args, game, retained, baseline, sources, ids)
        _, report["omitted"]["region"], _, _, _ = controls(
            client, author, args, game, "region", "translation", (0, 1), props.FULL, region=True, omitted="region")
        child_path = args.output / "hidden-child.vmf"
        leaf(author, child_path, (0, 1), omitted="hidden")
        hidden = props.parent(client, args.output / "hidden-parent.vmf", "translation", child_path, MODELS)
        client.call("save_map")
        versions = saved_versions(hidden["source"])
        require(versions == saved_versions(baseline["source"]),
                "Completed hidden and translated fixtures must share the same saved mapversions")
        props.setup(client)
        stage_property(client)
        data, report["omitted"]["hidden"] = props.pair(client, args, game, "hidden", hidden, [child_path])
        report["omitted"]["hidden"]["fixture_mapversions"] = versions
        origins.stats(report["omitted"]["hidden"], 0, 2, 2, 1, 1, 1, [child_path])
        report["omitted"]["hidden"]["comparison"] = compare(data, retained, "translation", (0, 1))
        report["omitted"]["hidden"]["unread_status"] = no_read_status(client, args, game, hidden)
        report["selected_material"] = selected_material(client, author, args, game)
        inputs = {str(path): digest(path.read_bytes()) for path in args.output.glob("*.vmf")}
        report["clean_eof"] = {str(index): require_clean_eof(active) for index, active in enumerate(clients)}
        require(identities(game) == mounted and all(digest(Path(path).read_bytes()) == value for path, value in inputs.items()),
                "Compilation/EOF changed saved sources or original private assets")
        require(args.compile_calls == report["expected_compile_calls"], "Actual selected-skin request count differs from the compact matrix")
        report.update(status="accepted", sources=inputs, assets=mounted)
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
            raise BaseExceptionGroup("Selected-skin validation and cleanup failed", ([failure] if failure else []) + errors) from None
    return report


def main():
    parser = argparse.ArgumentParser(description="Validate selected skins and authored static-lighting flags in referenced props through actual foreground/background production MCP builds. Requires integrated 723/724, Python 3.11+ and the existing props/origin validation helpers. Steps: build complete two-family MDL/VVD/VTX fixtures and inspect both actual material bindings; compare mixed same-model 0/1 and 1/0 independent translated/noncommuting literal controls with native flag bits and retained 722 scoped origins; check absent/default0 whole BSP equality, malformed/negative/retained-range refusal, hidden/region no-read, selected-material dual-bake rejection/source receipts and explicit vertex-lighting opt-out that still requires MDL. 32 requests per binary. No installed assets, graphics, native games or alternate appearance claim.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY, help="Integrated 723/724 hammerr-headless or editor, private --headless stdio Session")
    parser.add_argument("--output", type=Path, required=True, help="New directory under repository target/validation; bounded complete tiny fixtures need no installed game")
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(strict=True), args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Keep artifacts under repository target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    report = validate(args)
    print(json.dumps({"status": report["status"], "compile_calls": report["compile_calls"], "output": str(args.output)}))


if __name__ == "__main__":
    main()
