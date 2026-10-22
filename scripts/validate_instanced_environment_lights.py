import argparse
import hashlib
import json
import math
import re
import sys
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_instanced_visclusters as shared
from validate_authored_planes import records
from validate_collision_topology import document_state, identity
from validate_compile_regions import apply as apply_region
from validate_direct_sky import sample
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail
from validate_instance_contents import refresh
from validate_instance_lighting import entity_rows
from validate_instance_props import cleanup_clients
from validate_material_albedo import floor_base, payload, require, settings
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM

DEFAULT_GAME = Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source"
REASON = ("Child lights, sun/sky illumination and icons are not shown in the reference or authored-lighting preview; "
          "child light bounds, picking and framing are unavailable.")
SOURCE = (16, 128, 64)
MAIN = {"_light": "255 0 0 192", "_ambient": "0 0 255 48", "_lightHDR": "0 255 0 96",
        "_lightscaleHDR": "2", "_ambientHDR": "255 0 0 64", "_AmbientScaleHDR": "0.5", "SunSpreadAngle": "10"}
FALLBACK = {"_light": "255 0 0 192", "_ambient": "0 0 255 48", "SunSpreadAngle": "10"}
GEOMETRY = ("vertices", "indices", "triangles", "line_vertices")
PLACEMENTS = {"translation": {"middle_origin": [8, 16, 4], "middle_angles": "0 0 0", "root_origin": [16, -8, 8],
                             "root_angles": "0 0 0", "origin": (40, 136, 76), "yaw": -90, "beam": (0, -math.sqrt(0.5), -math.sqrt(0.5))},
              "nested": {"middle_origin": [8, 16, 64], "middle_angles": "0 90 0", "root_origin": [-64, -16, 8],
                         "root_angles": "90 0 0", "origin": (64, 16, 128), "yaw": 180, "beam": (-math.sqrt(0.5), 0, -math.sqrt(0.5))}}


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def mount_game(path, root):
    path.mkdir()
    mounts = [path, root / "cstrike/cstrike_pak.vpk", root / "cstrike/cstrike_english.vpk", root / "cstrike",
              root / "hl2/hl2_textures.vpk", root / "hl2/hl2_misc.vpk", root / "hl2", root / "platform/platform_misc.vpk", root / "platform"]
    search = "\n".join(f'game "{mount}"' for mount in mounts)
    (path / "gameinfo.txt").write_text(f'GameInfo {{ FileSystem {{ SearchPaths {{ {search} }} }} }}\n')


def vector(raw):
    values = tuple(map(float, raw.split()))
    require(len(values) == 3 and all(math.isfinite(value) for value in values), "Invalid emitted environment vector")
    return values


def worldlights(data, slot):
    rows = records(data, slot, "<9f3i7f3i")
    require(all(math.isfinite(value) for row in rows for value in row[:9] + row[12:19]),
            "Native worldlight contains nonfinite floating data")
    return rows


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


def sun(client, origin, yaw, values):
    identifier = client.call("create_entity", {"classname": "light_environment", "origin": origin})["id"]
    defaults = client.call("get_entity_class", {"classname": "light_environment"})["defaults"]
    before = dict(detail(client, [identifier])[0]["properties"])
    expected = {"classname": "light_environment", **defaults, "origin": " ".join(map(str, origin))}
    require(before == expected and defaults["pitch"] == "0" and defaults["_AmbientScaleHDR"] == "1"
            and defaults["SunSpreadAngle"] == "0" and defaults["_ambientHDR"] == "-1 -1 -1 1",
            "Normal installed-FGD creation changed defaults/canonical casing or synthesized unsupported fields")
    changes = {"angles": f"0 {yaw} 0", "pitch": "-45", **values}
    client.call("set_entity_properties", {"id": identifier, "set": changes})
    authored = dict(detail(client, [identifier])[0]["properties"])
    require(authored == {**expected, **changes}, "Ordinary environment property edit changed unedited catalog defaults")
    return identifier, {"defaults": defaults, "properties": authored}


def leaf(client, path, custody, values=MAIN, outside=False, mixed=False, source_origin=None):
    shared.new_map(client)
    origin = source_origin if source_origin is not None else ((8192, 128, 64) if outside else SOURCE)
    identifier, authored = sun(client, origin, -90, values)
    if mixed:
        client.call("create_block", {"mins": [-8, -8, 16], "maxs": [8, 8, 32], "material": "dev/dev_measuregeneric01"})
        brush = client.call("create_block", {"mins": [32, -8, 16], "maxs": [48, 8, 32], "material": "dev/dev_measuregeneric01"})["id"]
        client.call("tie_to_entity", {"ids": [brush], "classname": "func_detail"})
    shared.save(client, path, custody)
    require(dict(detail(client, [identifier])[0]["properties"]) == authored["properties"], "Environment save/reopen changed authored scalar strings")
    return identifier, authored


def middle(client, path, child, custody, placement, origin=None):
    shared.new_map(client)
    client.call("save_map", {"path": str(path)})
    pose = PLACEMENTS[placement]
    owner, form = shared.authored_instance(client, child, pose["middle_origin"] if origin is None else origin, pose["middle_angles"])
    shared.save(client, path, custody)
    return {"owner": owner, "form": form}


def parent(client, path, custody, placement, values=MAIN, child=None, count=1, root_sun=False, region=False, closed=False, outside=False, root_origin=None):
    shared.new_map(client)
    for index, bounds in enumerate(ROOM):
        identifier = client.call("create_block", {"mins": bounds[0], "maxs": bounds[1], "material": "dev/dev_measuregeneric01"})["id"]
        if index == 1 and not closed:
            brush = detail(client, [identifier])[0]
            face = next(face for face in brush["faces"] if face["normal"] == [0, 0, -1])
            client.call("edit_faces", {"faces": [{"solid": brush["solid"], "side": face["side"]}], "material": "tools/toolsskybox"})
    client.call("create_block", {"mins": [-96, -96, 32], "maxs": [96, 96, 48], "material": "dev/dev_measuregeneric01"})
    client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-160, -160, 8]})
    client.call("save_map", {"path": str(path)})
    pose, owners = PLACEMENTS[placement], []
    if root_sun:
        sun(client, pose["origin"], pose["yaw"], values)
    if child:
        for index in range(count):
            origin = list(pose["root_origin"] if root_origin is None else root_origin)
            origin[1] += index * 64
            owner, _ = shared.authored_instance(client, child, origin, pose["root_angles"])
            owners.append(owner)
    else:
        origin = (64, 8192, 128) if outside else pose["origin"]
        sun(client, origin, pose["yaw"], values)
    if region:
        apply_region(client, low=[-240, -240, -16], high=[240, 240, 272])
    shared.save(client, path, custody)
    client.call("select", {"ids": owners})
    return {"source": path, "owners": owners}


def preview(client, control, count, mixed=False):
    before, camera = document_state(client), client.call("get_camera")
    ready = refresh(client, background=True)
    require(ready["state"] == "ready" and ready["stats"].get("environment_lights", 0) == count
            and ("environment_lights" in ready["stats"]) == bool(count)
            and ("static_light_preview" in ready) == bool(count), "Environment inspection count/zero omission differs")
    require(ready["stats"].get("point_lights", 0) == ready["stats"].get("spot_lights", 0) == 0, "Sun was counted as point/spot light")
    if count:
        require(ready["static_light_preview"] == {"available": False, "reason": REASON}, "Parent inspection claims child sun/sky illumination")
    if mixed:
        require(ready["available"] and ready["stats"]["world_brushes"] == ready["stats"]["detail_brushes"] == 1
                and ready["geometry"]["triangles"] > 0, "Mixed reference lost ordinary target geometry")
    else:
        require(not ready["available"] and all(ready["geometry"][key] == 0 for key in GEOMETRY)
                and all(owner["bounds"] is None for owner in ready["owners"]), "Light-only reference acquired geometry/bounds")
        client.call("set_instance_contents", {"action": "frame", "token": str(ready["token"]), "ids": control["owners"]}, error=True)
    require(document_state(client) == before and client.call("get_camera") == camera
            and not client.call("get_map_info")["rendering"]["initialized"], "Reference inspection/frame changed source/history/camera or initialized rendering")
    return ready


def configure(client, mode):
    settings(client, (mode, 0))
    client.call("set_compile_settings", {"pack_static_prop_assets": False, "pack_water_assets": False})
    if not any(recipe["name"] == "Global instance sun and sky" for recipe in client.call("get_build_recipes")["recipes"]):
        client.call("save_build_recipe", {"name": "Global instance sun and sky"})


def pair(client, args, game, name, control, sources, mode, error=None, count=1):
    shared.reviewed_open(client, control["source"])
    configure(client, mode)
    case = {}
    data = shared.pair(client, args, game, name, control, sources, case, failure=error)
    if sources:
        for result in (case["foreground"], case["background"]["result"]):
            if not error:
                expanded = result["stats"]["instances"]
                require(expanded["environment_lights"] == count and expanded.get("point_lights", 0) == expanded.get("spot_lights", 0) == 0
                        and expanded["instances"] == 2 * count and expanded["files"] == 2 and expanded["max_expansion_depth"] == 2,
                        "Expanded-before-region environment occurrence counts differ")
            else:
                require(any(row["level"] == "error" and row["object"] in control["owners"]
                            and sources[0].name in row["message"] for row in result["diagnostics"]), "Environment refusal lost child source ownership")
    case.update(bsp=str(args.output / "builds" / f"{name}-foreground.bsp"), sha256=hashlib.sha256(data).hexdigest())
    return data, case


def native(data, mode, placement, fallback=False, outside=False, closed=False, duplicate=False):
    pose = PLACEMENTS[placement]
    emitted = [row for row in entity_rows(data) if row.get("classname") == "light_environment"]
    require(len(emitted) == (2 if duplicate else 1) and not any(row.get("classname") == "func_instance" for row in entity_rows(data)), "Global emitter/helper entity count differs")
    expected_origin = (64, 8192, 128) if outside else pose["origin"]
    if not duplicate:
        require(max(abs(a - b) for a, b in zip(vector(emitted[0]["origin"]), expected_origin, strict=True)) <= 1e-8, "Environment marker differs from independent literal")
        pitch = math.radians(float(emitted[0]["pitch"]))
        yaw = math.radians(vector(emitted[0]["angles"])[1])
        beam = (math.cos(pitch) * math.cos(yaw), math.cos(pitch) * math.sin(yaw), math.sin(pitch))
        require(max(abs(a - b) for a, b in zip(beam, pose["beam"], strict=True)) <= 1e-9, "Explicit-pitch sun direction differs from independent literal")
    channels = {}
    for channel, slot in (("ldr", 15), ("hdr", 54)):
        rows = worldlights(data, slot)
        channels[channel] = rows
        if mode == "fullbright" or (channel == "hdr" and mode != "direct_ldr_hdr"):
            require(not rows, "Unbaked channel acquired global worldlights")
            continue
        require(len(rows) == 2 and [row[10] for row in rows] == [3, 5], "Global source did not emit separate SKY/SKY_AMBIENT records")
        colors = ((192, 0, 0), (0, 0, 48)) if channel == "ldr" or fallback else ((0, 192, 0), (32, 0, 0))
        for index, row in enumerate(rows):
            require(row[:3] == expected_origin and row[9] == row[20] == row[21] == -1 and row[11] == 0,
                    "Native sky origin/cluster/style/ownership differs")
            require(max(abs(a - b / 255) for a, b in zip(row[3:6], colors[index], strict=True)) <= 1e-7, "Native sun/ambient channel, fallback or scale differs")
            expected_normal = pose["beam"] if index == 0 else (0, 0, 0)
            require(max(abs(a - b) for a, b in zip(row[6:9], expected_normal, strict=True)) <= 1e-6, "Native sky direction/ambient normal differs")
        require(payload(data, 8 if channel == "ldr" else 53) and payload(data, 56 if channel == "ldr" else 55), "Baked sky has no lightmap/ambient-probe data")
        if not closed:
            colors = floor_base(data, channel)
            expected_channels = (0, 2) if channel == "ldr" or fallback else (0, 1)
            require(all(any(color[index] > 0 for color in colors) for index in expected_channels)
                    and all(color[index] == 0 for color in colors for index in range(3) if index not in expected_channels),
                    "Sun and sky ambient did not produce their independent channel colors")
    if mode != "fullbright":
        under, exposed = sample(data, [0, 0, 0]), sample(data, [-160, -160, 0])
        require(sum(under) == 0 and (sum(exposed) == 0 if closed else exposed[0] > 0 and exposed[2] > 0),
                "Independent opaque-shadow/closed-sky/sun-and-ambient exposure control failed")
    if mode == "direct_ldr_hdr":
        require((payload(data, 15) == payload(data, 54)) if fallback else (payload(data, 8) != payload(data, 53)), "HDR fallback/channel discrimination differs")
    return {"native_sky_records": 0 if mode == "fullbright" else 2, "expected_origin": expected_origin,
            "independent_beam": pose["beam"], "emitted": emitted, "channels": channels,
            "fallback": fallback, "closed_sky": closed}


def compare(actual, literal, mode, placement, **kwargs):
    a, b = probe.bsp_lumps(actual), probe.bsp_lumps(literal)
    changed = [index for index, pair in enumerate(zip(a, b, strict=True)) if pair[0] != pair[1]]
    require(set(changed) <= {0, 15, 35, 54}, f"Independent literal geometry/lightmap/ambient payloads differ: {changed}")
    relocation = probe.verify_game_lump_preservation(actual, literal)
    first, second = entity_rows(actual), entity_rows(literal)
    require([row for row in first if row.get("classname") != "light_environment"]
            == [row for row in second if row.get("classname") != "light_environment"], "Non-environment world/entity metadata differs; check balanced Saves")
    excluded = {"origin", "angles", "pitch", "hammerid"}
    left = [row for row in first if row.get("classname") == "light_environment"]
    right = [row for row in second if row.get("classname") == "light_environment"]
    require(len(left) == len(right) == 1 and {k: v for k, v in left[0].items() if k not in excluded}
            == {k: v for k, v in right[0].items() if k not in excluded}, "Environment original scalar/case data differs outside independently checked pose/source identity")
    for slot in (15, 54):
        x, y = worldlights(actual, slot), worldlights(literal, slot)
        require(len(x) == len(y), "Native sky record count differs")
        for p, q in zip(x, y, strict=True):
            require(p[:6] == q[:6] and p[9:] == q[9:] and max(abs(u - v) for u, v in zip(p[6:9], q[6:9], strict=True)) <= 1e-6,
                    "Worldlight differs outside established713 native-vector representation bound")
    return {"different_lumps": changed, "game_lump_preservation": relocation,
            "child": native(actual, mode, placement, **kwargs), "literal": native(literal, mode, placement, **kwargs),
            "representation": "Exact complete non-emitter payloads; existing 713 entity pose/source identity, native normal 1e-6 and absolute game-lump relocation seams. Original outputs retained without normalization."}


def validate(args):
    args.custody = {str(args.binary): identity(args.binary), str(args.fgd): identity(args.fgd), str(Path(__file__).resolve()): identity(Path(__file__).resolve())}
    for module in tuple(sys.modules.values()):
        source = getattr(module, "__file__", None)
        if source and Path(source).resolve().is_relative_to(ROOT / "scripts"):
            path = Path(source).resolve()
            args.custody[str(path)] = identity(path)
    for name in ("tmp", "builds"):
        (args.output / name).mkdir()
    game = args.output / "game"
    mount_game(game, args.game_dir)
    report = {"schema": 755, "status": "failed", "binary": str(args.binary), "binary_sha256": digest(args.binary),
              "script_sha256": digest(Path(__file__)), "native_rendering_claim": False, "hdr_hardware_claim": False,
              "physics_mode": "none", "expected_compile_calls": 38,
              "positive": {}, "negative": {}, "preview": {}}
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
            loaded = client.call("load_entity_definitions", {"path": str(args.fgd)})
            for row in loaded["sources"]:
                path = Path(row["path"] if isinstance(row, dict) else row)
                args.custody[str(path)] = identity(path)
        compiler, author = clients
        leaves = {}
        for variant, values, outside in (("main", MAIN, False), ("fallback", FALLBACK, False), ("outside", MAIN, True)):
            path = args.output / f"{variant}-leaf.vmf"
            identifier, authored = leaf(author, path, args.custody, values, outside)
            leaves[variant] = (path, identifier, authored)
        controls = {}
        for placement, variant, modes, region in (("translation", "main", ("fullbright", "direct_ldr", "direct_ldr_hdr"), False),
                ("nested", "main", ("direct_ldr_hdr",), False), ("translation", "fallback", ("direct_ldr_hdr",), False),
                ("nested", "outside", ("direct_ldr",), True)):
            source, _, authored = leaves[variant]
            mid = args.output / f"{placement}-{variant}-middle.vmf"
            middle(author, mid, source, args.custody, placement)
            parent_source = args.output / f"{placement}-{variant}-parent.vmf"
            control = parent(compiler, parent_source, args.custody, placement, child=mid, region=region)
            controls[(placement, variant)] = (control, mid)
            report["preview"][f"{placement}-{variant}"] = preview(compiler, control, 1)
            flat = parent(author, args.output / f"{placement}-{variant}-literal.vmf", args.custody, placement,
                          values=FALLBACK if variant == "fallback" else MAIN, region=region, outside=variant == "outside")
            require(metadata(control["source"]) == metadata(flat["source"]), "Balanced normal Saves produced different authored versioninfo/world metadata")
            for mode in modes:
                label = f"{placement}-{variant}-{mode}"
                data, receipt = pair(compiler, args, game, label, control, [source, mid], mode)
                literal, manual = pair(author, args, game, label + "-literal", flat, [], mode)
                receipt.update(authored=authored, authored_metadata=metadata(flat["source"]), literal=manual, comparison=compare(data, literal, mode, placement,
                               fallback=variant == "fallback", outside=variant == "outside"))
                report["positive"][label] = receipt
        source, _, _ = leaves["main"]
        control, mid = controls[("translation", "main")]
        closed = parent(compiler, args.output / "closed.vmf", args.custody, "translation", child=mid, closed=True)
        data, receipt = pair(compiler, args, game, "closed", closed, [source, mid], "direct_ldr")
        receipt["control"] = native(data, "direct_ldr", "translation", closed=True)
        report["positive"]["closed"] = receipt
        duplicate = "one visible light_environment"
        for label, variant, root_sun, count, region in (("repeated", "main", False, 2, False),
                ("root-child", "main", True, 1, False), ("region-outside-duplicate", "outside", False, 2, True)):
            source, _, _ = leaves[variant]
            placement = "nested" if region else "translation"
            _, mid = controls[(placement, variant)]
            bad = parent(compiler, args.output / f"{label}.vmf", args.custody, placement, child=mid, count=count, root_sun=root_sun, region=region)
            _, receipt = pair(compiler, args, game, label, bad, [source, mid], "direct_ldr", error=duplicate, count=count)
            report["negative"][label] = receipt
            if label == "repeated":
                data, full = pair(compiler, args, game, "duplicate-fullbright", bad, [source, mid], "fullbright", count=2)
                full["control"] = native(data, "fullbright", "translation", duplicate=True)
                report["positive"]["duplicate-fullbright"] = full
        for label, source_origin, mid_origin, root_origin in (("source-bounds", (16385, 128, 64), [0, 0, 0], [0, 0, 0]),
                ("intermediate-bounds", SOURCE, [16380, 0, 0], [-16380, 0, 0])):
            source = args.output / f"{label}-leaf.vmf"
            leaf(author, source, args.custody, source_origin=source_origin)
            mid = args.output / f"{label}-middle.vmf"
            middle(author, mid, source, args.custody, "translation", origin=mid_origin)
            bad = parent(compiler, args.output / f"{label}-parent.vmf", args.custody, "translation", child=mid, root_origin=root_origin)
            _, report["negative"][label] = pair(compiler, args, game, label, bad, [source, mid], "fullbright",
                                               error="light origin exceeds finite Source compile bounds")
        mixed = args.output / "mixed-leaf.vmf"
        helper, _ = leaf(author, mixed, args.custody, mixed=True)
        mixed_mid = args.output / "mixed-middle.vmf"
        middle(author, mixed_mid, mixed, args.custody, "translation")
        owner = parent(compiler, args.output / "mixed-parent.vmf", args.custody, "translation", child=mixed_mid)
        first = preview(compiler, owner, 1, mixed=True)
        shared.reviewed_open(author, mixed)
        author.call("delete_objects", {"ids": [helper]})
        physical = args.output / "physical-leaf.vmf"
        shared.save(author, physical, args.custody)
        physical_mid = args.output / "physical-middle.vmf"
        middle(author, physical_mid, physical, args.custody, "translation")
        owner = parent(compiler, args.output / "physical-parent.vmf", args.custody, "translation", child=physical_mid)
        second = preview(compiler, owner, 0, mixed=True)
        require(all(first["geometry"][key] == second["geometry"][key] for key in GEOMETRY)
                and first["face_provenance_total"] == second["face_provenance_total"]
                and [row["bounds"] for row in first["owners"]] == [row["bounds"] for row in second["owners"]], "Environment helper altered actual mixed reference geometry/provenance/bounds")
        report["preview"]["mixed"] = {"with_environment": first, "physical_only": second}
        for client in clients:
            require_clean_eof(client)
        require(all(identity(Path(path)) == expected for path, expected in args.custody.items()), "Source/binary/FGD/sibling/output custody changed")
        if args.compare_report:
            previous = json.loads(args.compare_report.read_text())
            require(previous["status"] == "passed" and previous["positive"].keys() == report["positive"].keys(), "Cross-binary case scope differs")
            for name, case in report["positive"].items():
                require(Path(case["bsp"]).read_bytes() == Path(previous["positive"][name]["bsp"]).read_bytes(), "Actual MCP binaries produced different environment BSP bytes")
            report["cross_binary_exact_bsp_parity"] = True
        report["status"] = "passed"
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
            raise BaseExceptionGroup("Instanced environment-light public validation failed", failures)


def main():
    parser = argparse.ArgumentParser(description="Validate normal installed-FGD child sun/sky composition, independent literal directions and LDR/HDR worldlight/lightmap/ambient data through an actual production MCP binary. No native game, pixels or hardware qualification.",
        epilog="Requires Python 3.11+, installed SourceSDK cstrike.fgd and mounted CS:S materials, accepted 753/754 prerequisites and a matching binary. Caller owns the single 8-job/24 GiB pipeline. Creates fresh disk artifacts under target/validation; no project build or native launch is performed.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--game-dir", type=Path, default=DEFAULT_GAME)
    parser.add_argument("--fgd", type=Path, default=shared.DEFAULT_FGD)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--compare-report", type=Path)
    args = parser.parse_args()
    args.binary, args.game_dir, args.fgd = (path.resolve(strict=True) for path in (args.binary, args.game_dir, args.fgd))
    args.output = args.output.resolve()
    require(math.isfinite(args.timeout) and args.timeout > 0 and not args.output.exists() and args.output.is_relative_to(ROOT / "target/validation"), "Use a finite positive timeout and fresh output under target/validation")
    args.output.mkdir(parents=True, mode=0o700)
    args.compile_calls = 0
    validate(args)
    print(json.dumps({"status": "passed", "output": str(args.output), "compile_calls": args.compile_calls}))


if __name__ == "__main__":
    main()
