import argparse
import json
import re
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_instance_lighting_origins as origins
import validate_instance_prop_skins as skins
import validate_instance_props as props
from validate_face_lighting import DEFAULT_BINARY, ROOT, detail
from validate_instance_lighting import stage_property
from validate_instance_terrain import private_client, property_state, write_json
from validate_material_albedo import digest, identities, require
from validate_recovery_exit import require_clean_eof

DEFAULTS = {"fademindist": "-1", "fademaxdist": "0", "fadescale": "1",
            "screenspacefade": "0", "mindxlevel": "0", "maxdxlevel": "0"}
DISTANCE = (
    {"fademindist": "-7", "fademaxdist": "768", "fadescale": "0.5",
     "screenspacefade": "0", "mindxlevel": "80", "maxdxlevel": "0"},
    {"fademindist": "96", "fademaxdist": "384", "fadescale": "0",
     "screenspacefade": "0", "mindxlevel": "0", "maxdxlevel": "95"},
)
SCREEN = (
    {"fademindist": "192", "fademaxdist": "16", "fadescale": "1.75",
     "screenspacefade": "1", "mindxlevel": "0", "maxdxlevel": "0"},
    {"fademindist": "123", "fademaxdist": "0", "fadescale": "2",
     "screenspacefade": "1", "mindxlevel": "0", "maxdxlevel": "0"},
)
DEFAULT_NATIVE = ((0, 0, 1, 0, 0, 0),) * 2
DISTANCE_NATIVE = ((768, 768, 0.5, 1, 80, 0), (96, 384, 0, 1, 0, 95))
SCREEN_NATIVE = ((192, 16, 1.75, 35, 0, 0), (0, 0, 2, 34, 0, 0))
REFUSALS = (
    ("f32-overflow", {"fademindist": "3.5e38"}, "fademindist must be a finite number"),
    ("negative-end", {"fademaxdist": "-1"}, "nonnegative"),
    ("negative-scale", {"fadescale": "-1"}, "nonnegative"),
    ("screen-flag", {"screenspacefade": "2"}, "screenspacefade must be 0 or 1"),
    ("dx-range", {"maxdxlevel": "65536"}, "unsigned native short"),
    ("dx-order", {"mindxlevel": "91", "maxdxlevel": "90"}, "minimum DX level"),
)
REFERENCE_REFUSALS = (
    ("nan-start", {"fademindist": "NaN"}, "fademindist must be a finite number"),
    ("infinite-end", {"fademaxdist": "inf"}, "fademaxdist must be a finite number"),
    ("overflow-scale", {"fadescale": "1e100"}, "fadescale must be a finite number"),
    ("negative-dx", {"mindxlevel": "-1"}, "unsigned native short"),
    ("fractional-dx", {"maxdxlevel": "90.5"}, "maxdxlevel must be an integer"),
    ("fractional-screen", {"screenspacefade": "0.5"}, "screenspacefade must be an integer"),
    ("retained-body", {"body": "1"}, "body"),
    ("retained-scale", {"modelscale": "2"}, "modelscale"),
    ("retained-tint", {"rendercolor": "254 255 255"}, "rendercolor"),
)


def set_values(client, identifiers, values, selected=(0, 0)):
    for identifier, fields, skin in zip(identifiers, values, selected, strict=True):
        client.call("set_entity_properties", {"id": identifier, "set": {**fields, "skin": str(skin)}})


def saved_values(client, path, identifiers, values, selected):
    saved = path.read_bytes()
    client.call("open_map", {"path": str(path), "discard_changes": True})
    rows = detail(client, identifiers)
    require(path.read_bytes() == saved
            and all(all(dict(row["properties"]).get(key) == value for key, value in fields.items())
                    and dict(row["properties"])["skin"] == str(skin)
                    for row, fields, skin in zip(rows, values, selected, strict=True)),
            "Generic property/save/reopen changed raw visibility fields or selected skins")


def child(author, path, values, selected=(0, 0), scoped=False):
    identifiers = (origins.child(author, path, skins.MODELS)["props"] if scoped
                   else props.leaf(author, path, skins.MODELS))
    set_values(author, identifiers, values, selected)
    author.call("save_map")
    saved_values(author, path, identifiers, values, selected)
    return identifiers


def native(data, placement, expected, selected=(0, 0), scoped=False):
    records = props.native(data, placement, skins.MODELS, skins=selected)
    for row, literal in zip(records["props"], expected, strict=True):
        require((row[12], row[13], row[17], row[10], row[18], row[19]) == literal,
                "Original sprp6 fade start/end/scale, exact flag bits or DX levels differ from literals")
        require(row[14:17] == (origins.NESTED if scoped else (0, 0, 0)),
                "Visibility admission changed the existing native lighting-origin point")
    return records


def protected(client, control):
    return (props.source_state(client, control["source"]), property_state(client),
            client.call("get_compile_settings"), client.call("get_camera"))


def reference(client, args, game, control, reason=None, scoped=False):
    before, outputs = protected(client, control), identities(args.output / "builds")
    paths = [(game / model).with_suffix(suffix) for model in props.MODELS for suffix in (".mdl", ".phy")]
    originals = {path: path.read_bytes() for path in paths}
    try:
        for path in paths:
            path.unlink()
        result = client.call("set_instance_contents", {"action": "refresh"})
        if reason:
            require(result["state"] == "failed" and not result["available"]
                    and reason in str(result["error"]) and result["geometry"] is None,
                    "Reference refresh did not refuse malformed header-independent options without a partial preview")
        else:
            require(result["state"] == "ready" and result["stats"]["static_props"] == 2
                    and result["stats"].get("lighting_origins", 0) == int(scoped)
                    and not result["available"] and not result["static_prop_preview"]["available"]
                    and result["geometry"]["vertices"] == result["geometry"]["indices"] == 0
                    and result["geometry"]["triangles"] == result["face_provenance_total"] == 0
                    and all(row["bounds"] is None for row in result["owners"]),
                    "Reference refresh read unavailable assets or fabricated model/family/visibility geometry")
        require(protected(client, control) == before and identities(args.output / "builds") == outputs
                and not client.call("get_map_info")["rendering"]["initialized"],
                "CPU reference refresh changed source/history/draft/camera/settings/BSP/LIN or initialized graphics")
        result["missing_asset_paths"] = [str(path.relative_to(game)) for path in paths]
        return result
    finally:
        for path, original in originals.items():
            path.write_bytes(original)


def bake(data, receipt, game):
    entries, flags = props.pak_and_flags(data)
    require(flags & 3 == 3 and props.MANIFEST in entries, "Scoped screen-fade control omitted dual-channel VHV")
    manifest = json.loads(entries[props.MANIFEST])
    sprp = next(row[4] for row in probe.game_lump_records(data)[1] if row[0] == props.SPRP)
    require(manifest["version"] == 1 and manifest["channels"] == ["ldr", "hdr"]
            and manifest["static_props_version"] == 6 and manifest["static_props_sha256"] == digest(sprp)
            and [row["ordinal"] for row in manifest["receivers"]] == [0, 1],
            "Scoped screen-fade VHV manifest does not bind original native props")
    measured, illuminated = {}, False
    for receiver in manifest["receivers"]:
        require(receiver["model"] == skins.MODEL and receiver["checksum"] == 719
                and receiver["meshes"] == [{"lod": 0, "vertex_count": 6}], "Scoped screen-fade hardware identity differs")
        for channel in ("ldr", "hdr"):
            name = f"sp_{'hdr_' if channel == 'hdr' else ''}{receiver['ordinal']}.vhv"
            asset = next(row for row in manifest["assets"] if row["path"] == name)
            measured[name], colors = props.read_vhv(entries[name], 719, receiver["meshes"])
            require(asset["bytes"] == measured[name]["bytes"] and asset["sha256"] == measured[name]["sha256"],
                    "Scoped screen-fade VHV receipt differs from original bytes")
            illuminated |= any(any(color[:3]) for color in colors)
    require(set(measured) == {name for name in entries if props.VHV_NAME.fullmatch(name)}
            and len(manifest["assets"]) == 4 and illuminated
            and all(result["stats"]["prop_vertex_lighting"]["props"] == 2
                    for result in (receipt["result"], receipt["background"]["result"]))
            and skins.ALTERNATE in {row["path"] for row in manifest["sources"]},
            "Scoped screen-fade VHV lost selected-family sources, ordinals or illumination")
    for source in manifest["sources"]:
        path = Path(source["path"])
        require(not path.is_absolute() and ".." not in path.parts and "\\" not in source["path"], "Unsafe VHV source path")
        raw = (game / path).read_bytes()
        require(len(raw) == source["bytes"] and digest(raw) == source["sha256"], "VHV source receipt differs from private assets")
    receipt["bake"] = {"manifest": manifest, "files": measured}
    return {name: raw for name, raw in entries.items() if name != props.MANIFEST}


def controls(client, author, args, game, label, placement, values, expected, selected=(0, 0), scoped=False):
    path = args.output / f"{label}-child.vmf"
    child(author, path, values, selected, scoped)
    reference_path, sources = path, [path]
    if placement == "noncommuting":
        reference_path = args.output / f"{label}-middle.vmf"
        origins.named_middle(author, reference_path, path)
        sources.append(reference_path)
    if scoped:
        control = origins.fixture(client, args.output / f"{label}-parent.vmf", reference_path, placement, skins.MODELS)
        flat = origins.fixture(client, args.output / f"{label}-flat.vmf", reference_path, placement, skins.MODELS, flat=True)
    else:
        control = props.parent(client, args.output / f"{label}-parent.vmf", placement, reference_path, skins.MODELS)
        flat = props.parent(client, args.output / f"{label}-flat.vmf", placement, models=skins.MODELS)
    set_values(client, flat["ids"][-2:], values, selected)
    client.call("save_map")
    saved_values(client, flat["source"], flat["ids"][-2:], values, selected)
    client.call("open_map", {"path": str(control["source"]), "discard_changes": True})
    client.call("save_map")
    versions = skins.saved_versions(control["source"])
    require(versions == skins.saved_versions(flat["source"]) == {"versioninfo": 2, "world": 2},
            "Literal and instance controls must share the same two-save mapversion lifecycle")
    mode = props.DUAL if scoped else props.FULL
    props.setup(client, mode, scoped)
    token = stage_property(client)
    preview = reference(client, args, game, control, scoped=scoped)
    data, receipt = props.pair(client, args, game, label, control, sources)
    depth = 2 if placement == "noncommuting" else 1
    origins.stats(receipt, int(scoped), 2, 2, depth, len(sources), depth, sources)
    receipt.update(reference=preview, fixture_mapversions=versions, native=native(data, placement, expected, selected, scoped))
    client.call("cancel_property_edit", {"token": token})
    client.call("open_map", {"path": str(flat["source"]), "discard_changes": True})
    props.setup(client, mode, scoped)
    literal, receipt["flat"] = props.pair(client, args, game, f"{label}-flat", flat, sources)
    native(literal, placement, expected, selected, scoped)
    require("instances" not in receipt["flat"]["result"]["stats"], "Literal visibility control acquired instance metadata")
    receipt["comparison"] = props.compare(data, literal, placement, skins.MODELS, scoped, skins=selected)
    if scoped:
        require(bake(data, receipt, game) == bake(literal, receipt["flat"], game),
                "Scoped screen-fade and literal controls differ in original dual-channel VHV/PAK payloads")
    return receipt


def defaults(client, author, args, game):
    path = args.output / "defaults-child.vmf"
    identifiers = child(author, path, (DEFAULTS, DEFAULTS))
    original = path.read_bytes()
    control = props.parent(client, args.output / "defaults-parent.vmf", "translation", path, skins.MODELS)
    client.call("save_map")
    props.setup(client)
    token = stage_property(client)
    preview = reference(client, args, game, control)
    data, receipt = props.pair(client, args, game, "defaults-explicit", control, [path])
    props.counts(receipt, [path], 1)
    receipt.update(native=native(data, "translation", DEFAULT_NATIVE), reference=preview,
                   fixture_mapversions=skins.saved_versions(control["source"]))
    try:
        for identifier in identifiers:
            author.call("set_entity_properties", {"id": identifier, "remove": list(DEFAULTS)})
        author.call("save_map")
        saved_values(author, path, identifiers, ({}, {}), (0, 0))
        require(all(not (set(dict(row["properties"])) & set(DEFAULTS)) for row in detail(author, identifiers)),
                "Absent-default authoring retained a visibility field")
        absent, receipt["absent"] = props.pair(client, args, game, "defaults-absent", control, [path])
        props.counts(receipt["absent"], [path], 1)
        native(absent, "translation", DEFAULT_NATIVE)
        require(absent == data, "Absent versus explicit visibility defaults changed complete original BSP bytes")
        receipt["absent"]["comparison"] = props.compare(absent, data, "translation", skins.MODELS)
    finally:
        path.write_bytes(original)
    client.call("cancel_property_edit", {"token": token})
    return data, receipt, control, path


def changed_fields(original, changes):
    text = original.decode()
    for key, value in changes.items():
        text, count = re.subn(r'("' + re.escape(key) + r'"\s+")[^"]*(")',
                              lambda match: match[1] + value + match[2], text, count=1)
        require(count == 1, "Imported refusal must change exactly one occurrence of each requested field")
    return text


def refusals(client, args, game, prior, control, path):
    original = path.read_bytes()
    report = {"compile": {}, "reference_only": {}}
    for group, cases in (("compile", REFUSALS), ("reference_only", REFERENCE_REFUSALS)):
        for label, changes, reason in cases:
            client.call("open_map", {"path": str(control["source"]), "discard_changes": True})
            props.setup(client)
            stage_property(client)
            try:
                path.write_text(changed_fields(original, changes))
                preview = reference(client, args, game, control, reason)
                if group == "compile":
                    _, report[group][label] = props.pair(client, args, game, label, control, [path], reason, prior, path.name)
                    report[group][label]["reference"] = preview
                else:
                    report[group][label] = preview
            finally:
                path.write_bytes(original)
    client.call("open_map", {"path": str(control["source"]), "discard_changes": True})
    props.setup(client)
    stage_property(client)
    try:
        path.write_text(changed_fields(original, {"model": "models/hammerr728/missing.mdl", "skin": "999", "solid": "6"}))
        require(not (game / "models/hammerr728/missing.mdl").exists(), "Deferred-asset fixture unexpectedly has a model")
        report["deferred_assets"] = reference(client, args, game, control)
        report["deferred_assets"]["deferred"] = {"missing_model": "models/hammerr728/missing.mdl", "skin": 999, "solid": 6}
    finally:
        path.write_bytes(original)
    return report


def validate(args):
    clients, failure = [], None
    args.compile_calls, args.baked_fixture_game = 0, None
    helpers = (props, skins, origins, probe)
    report = {"status": "failed", "binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()),
              "script_sha256": digest(Path(__file__).read_bytes()),
              "shared_helper_sha256": {Path(module.__file__).name: digest(Path(module.__file__).read_bytes()) for module in helpers},
              "compile_calls": 0, "expected_compile_calls": 24, "graphics_initialized": False,
              "native_game_exercised": False, "desktop_input": False, "positive": {},
              "scope": "Compiler-only native fade/DX/screen-bit and original BSP equivalence for translated distance and nested noncommuting screen forms, retained scoped origins/selected skins/dual VHV, absent/default byte identity, and header-independent reference refusals without MDL/PHY files. No child-model preview, model-family certification by reference refresh or native runtime fade/DX appearance claim."}
    try:
        (args.output / "tmp").mkdir()
        (args.output / "builds").mkdir()
        game = skins.game_assets(args)
        mounted = identities(game)
        for name in ("client", "author"):
            clients.append(private_client(args.binary, args.output / name, game, args.output / "tmp"))
        client, author = clients
        prior, report["positive"]["defaults"], control, path = defaults(client, author, args, game)
        report["positive"]["distance"] = controls(client, author, args, game, "distance", "translation", DISTANCE, DISTANCE_NATIVE)
        report["positive"]["screen-scoped-dual"] = controls(client, author, args, game, "screen-scoped-dual", "noncommuting",
                                                          SCREEN, SCREEN_NATIVE, (1, 0), True)
        report["negative"] = refusals(client, args, game, prior, control, path)
        inputs = {str(path): digest(path.read_bytes()) for path in args.output.glob("*.vmf")}
        report["clean_eof"] = {str(index): require_clean_eof(active) for index, active in enumerate(clients)}
        require(identities(game) == mounted and all(digest(Path(path).read_bytes()) == value for path, value in inputs.items()),
                "Compilation/EOF changed saved source bytes or original private assets")
        require(args.compile_calls == report["expected_compile_calls"], "Actual visibility request count differs from the compact matrix")
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
            raise BaseExceptionGroup("Visibility validation and cleanup failed", ([failure] if failure else []) + errors) from None
    return report


def main():
    parser = argparse.ArgumentParser(description="Validate child prop_static fade distances/scales, screen-pixel flags and unsigned-short DX bounds through actual foreground/background production MCP builds. Requires integrated 728 with retained 723/724, Python 3.11+ and existing props/skin/origin helpers. Creates complete tiny two-family MDL/VVD/VTX/material fixtures; compares translated distance and noncommuting scoped screen literals, original sprp6 flags/fade/DX bytes, BSP lumps and dual VHV; checks absent/default whole BSP equality and preserving finite/range/order refusals. Reference refresh runs with MDL/PHY files absent before each real compile, validates only header-independent options and retains protected source/draft/camera/settings/output state. Exactly 24 compile requests per binary. Run separately for hammerr-headless and editor. No installed assets, graphics, desktop input or native-game fade/DX appearance validation.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY, help="Integrated 728 hammerr-headless or editor; runs an isolated --headless stdio MCP Session")
    parser.add_argument("--output", type=Path, required=True, help="New disk artifact directory under this repository's target/validation; retains complete tiny private assets, VMFs, original BSP/LIN files, process logs and results.json; no installed game required")
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(strict=True), args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Keep validation artifacts on disk under repository target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    report = validate(args)
    print(json.dumps({"status": report["status"], "compile_calls": report["compile_calls"], "output": str(args.output)}))


if __name__ == "__main__":
    main()
