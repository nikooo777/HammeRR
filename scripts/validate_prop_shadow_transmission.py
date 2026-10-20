import argparse
import itertools
import json
import math
import os
import shutil
import struct
from pathlib import Path
from unittest.mock import patch

import prop_bounce_reference as reference
import prop_indirect_fixtures as portable
import validate_cubemap_capture_probe as probe
import validate_prop_shadows as legacy
from validate_build_recipes import protected_state
from validate_bump_lighting import texture
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail, face_ref
from validate_prop_vertex_lighting import (
    MANIFEST,
    inspect_readonly,
    pak_and_flags,
    read_vhv,
)
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM

REQUEST = 0x00040000
INTENSITIES = {"ldr": 255.125, "hdr": 510.25}
LAYOUT = [{"lod": 0, "vertex_count": 6}]
CASES = [
    {"name": "clear", "alphas": [], "remaining": 255},
    {"name": "alpha-zero", "alphas": [0], "remaining": 255},
    {"name": "alpha-quarter", "alphas": [64], "remaining": 191},
    {"name": "alpha-half", "alphas": [128], "remaining": 127},
    {"name": "alpha-three-quarters", "alphas": [192], "remaining": 63},
    {"name": "alpha-opaque", "alphas": [255], "remaining": 0},
    {"name": "layers-quarter-quarter", "alphas": [64, 64], "remaining": 127},
    {"name": "layers-quarter-half", "alphas": [64, 128], "remaining": 63},
    {"name": "layers-half-quarter", "alphas": [128, 64], "remaining": 63},
    {"name": "layers-saturated", "alphas": [128, 128], "remaining": 0},
    {"name": "coincident-other-owners", "alphas": [64, 128], "remaining": 63, "heights": [50, 50]},
    {"name": "behind-light", "alphas": [255], "remaining": 255, "heights": [150, 60]},
    {"name": "native-no-shadow", "alphas": [255], "remaining": 255, "disabled": True},
    {"name": "unflagged-opaque", "alphas": [128], "remaining": 0, "requested": False},
    {"name": "legacy-cutout-hole", "alphas": [64], "remaining": 255, "mode": "cutout"},
    {"name": "legacy-cutout-block", "alphas": [128], "remaining": 0, "mode": "cutout"},
    {"name": "legacy-plain-opaque", "alphas": [0], "remaining": 0, "mode": "opaque"},
    {"name": "unlit-fractional", "alphas": [64], "remaining": 191, "shader": "UnlitGeneric"},
    {"name": "own-skip-retains-other", "alphas": [128], "remaining": 127, "skip_own": True},
    {"name": "spot-half", "alphas": [128], "remaining": 127, "spot": True},
    {"name": "styled-directional-half", "alphas": [128], "remaining": 127, "styled": True},
]


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def identities(directory):
    return {str(path.relative_to(directory)): {"bytes": path.stat().st_size, "sha256": reference.digest(path.read_bytes())}
            for path in sorted(directory.rglob("*")) if path.is_file()}


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def alpha_texture(alpha):
    require(isinstance(alpha, int) and 0 <= alpha <= 255, "Alpha must be one literal mip0 byte")
    raw = legacy.alpha_texture(alpha)
    struct.pack_into("<I", raw, 20, 0x2000)
    for index in range(16):
        raw[80 + index * 4 + 3] = alpha
    return raw


def fixtures(game):
    legacy.fixture(game)
    original = (game / "models/props/test.mdl").read_bytes()
    templates = {}
    for suffix, material in (("a", b"taa"), ("b", b"tbb")):
        mdl = bytearray(original)
        mdl[736:739] = material
        templates[suffix] = bytes(mdl)
        for extension in ("mdl", "vvd", "dx90.vtx"):
            data = mdl if extension == "mdl" else (game / f"models/props/test.{extension}").read_bytes()
            (game / f"models/props/layer-{suffix}.{extension}").write_bytes(data)
    portable.model(game, "receiver", [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)])
    reference.write(game / "materials/models/oracle/receiver.vmt", 'VertexLitGeneric { $model 1 $basetexture "transmission/white" }\n')
    reference.write(game / "materials/transmission/white.vtf", portable.texture((0, 0, 0)))
    reference.write(game / "materials/transmission/world.vtf", portable.texture((0.5, 0.5, 0.5)))
    reference.write(game / "materials/transmission/normal.vtf", texture(bytes([128, 128, 255, 255] * 32 * 32), normal=True))
    reference.write(game / "materials/transmission/bump.vmt", 'LightmappedGeneric { $basetexture "transmission/world" $bumpmap "transmission/normal" }\n')
    for suffix in templates:
        reference.write(game / f"materials/transmission/{suffix}.vtf", alpha_texture(0))
    caster_vtx = (game / "models/props/layer-a.dx90.vtx").read_bytes()
    require(struct.unpack_from("<i", caster_vtx, 146)[0] == 3 and struct.unpack_from("<3H", caster_vtx, 134) == (0, 1, 2),
            "Caster is not the independently known one-triangle draw strip")
    receiver_vtx = (game / "models/oracle/receiver.dx90.vtx").read_bytes()
    require([struct.unpack_from("<H", receiver_vtx, 260 + index * 9)[0] for index in range(6)] == reference.HARDWARE,
            "Actual receiver hardware-root indices differ")
    receiver_vvd = (game / "models/oracle/receiver.vvd").read_bytes()
    for index, (x, y, z) in enumerate(reference.POINTS):
        require(struct.unpack_from("<6f", receiver_vvd, 64 + index * 48 + 16) == (x, y, 0, 0, 0, 1),
                "Literal receiver root positions or normals differ")
    return templates


def prepare_case(client, game, case, templates, casters, receivers, light, floor):
    for index, suffix in enumerate(("a", "b")):
        mdl = bytearray(templates[suffix])
        flags = struct.unpack_from("<I", mdl, 152)[0]
        struct.pack_into("<I", mdl, 152, flags | REQUEST if case.get("requested", True) else flags & ~REQUEST)
        (game / f"models/props/layer-{suffix}.mdl").write_bytes(mdl)
        mode = case.get("mode", "fractional")
        field = "$translucent 1" if mode == "fractional" else "$alphatest 1" if mode == "cutout" else ""
        material = f'{case.get("shader", "VertexLitGeneric")} {{ {field} $basetexture "transmission/{suffix}" }}\n'
        (game / f"materials/models/test/{'taa' if suffix == 'a' else 'tbb'}.vmt").write_text(material)
        alpha = case["alphas"][index] if index < len(case["alphas"]) else 0
        (game / f"materials/transmission/{suffix}.vtf").write_bytes(alpha_texture(alpha))
        active = index < len(case["alphas"]) and not case.get("disabled", False)
        height = case.get("heights", [50, 60])[index]
        client.call("set_entity_properties", {"id": casters[index], "set": {"origin": f"0 0 {height}", "disableshadows": "0" if active else "1"}})
    for receiver in receivers:
        client.call("set_entity_properties", {"id": receiver, "set": {"disableselfshadowing": "1" if case.get("skip_own") else "0"}})
    client.call("set_entity_properties", {"id": light, "set": {"classname": "light_spot" if case.get("spot") else "light", "angles": "-90 0 0",
                "_cone": "60", "_inner_cone": "45", "_exponent": "0"}})
    client.call("edit_faces", {"faces": [floor], "material": "transmission/bump" if case.get("styled") else "dev/dev_measuregeneric01"})
    client.call("set_compile_settings", {"bake_static_prop_vertices": not case.get("styled", False)})
    if case.get("styled"):
        return client.call("create_entity", {"classname": "light", "origin": [0, 0, 100], "properties": {"targetname": "transmission_style",
                    "_light": "0 255 0 255.125", "_lightHDR": "0 255 0 510.25", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}})["id"]
    return None


def native_interval(linear):
    radius = reference.ROUNDING_INTERVAL
    lower, upper = [max(0, value - radius) for value in linear], [value + radius for value in linear]
    candidates = set()
    for exponent in {reference.exponent_for(max(lower)), reference.exponent_for(max(upper))}:
        components = [sorted({min(255, int(math.ldexp(value, -exponent))) for value in pair})
                      for pair in zip(lower, upper, strict=True)]
        for rgb in itertools.product(*components):
            candidates.add(struct.pack("<3Bb", *rgb, exponent).hex())
    return {"linear": linear, "roundoff_interval_linear": radius, "rgbexp32_candidates": sorted(candidates),
            "vhv": reference.encoded_interval(linear)}


def expected(case):
    ratio = case["remaining"] / 255
    channels = {}
    for channel, intensity in INTENSITIES.items():
        axis = 0 if channel == "ldr" else 2
        styles = {"0": []}
        if case.get("styled"):
            styles["32"] = []
        for style, rows in styles.items():
            for angular in ([1, math.sqrt(1 / 3), math.sqrt(1 / 3), math.sqrt(1 / 3)] if case.get("styled") else [1]):
                rows.append(native_interval([intensity * ratio * angular if component == (1 if style == "32" else axis) else 0
                                             for component in range(3)]))
        vertices = {}
        for ordinal in (0, 1):
            vertices[str(ordinal)] = []
            for x, y, z in reference.POINTS:
                factor = 0.35 if ordinal else 84 / math.sqrt(84 * 84 + x * x + y * y)
                if case.get("spot"):
                    factor *= 84 / math.sqrt(84 * 84 + x * x + y * y)
                vertices[str(ordinal)].append(native_interval([intensity * ratio * factor if component == axis else 0 for component in range(3)]))
        channels[channel] = {"floor_styles": styles, "receiver_root_vertices": vertices}
    return {"sampled_alpha8": case["alphas"], "remaining_coverage_byte": case["remaining"], "visibility": ratio, "channels": channels}


def native_center(data, channel):
    face_slot, light_slot = (7, 8) if channel == "ldr" else (58, 53)
    faces = reference.records(data, face_slot, reference.FACE_FORMAT, 64, (0,) if channel == "ldr" else (1,))
    planes = reference.records(data, 1, "<4fi", 128)
    infos = reference.records(data, 6, "<16f2i", 64)
    lighting = reference.lump(data, light_slot, (1,))
    floors = []
    for index, face in enumerate(faces):
        require(face[0] < len(planes) and 0 <= face[5] < len(infos), "Invalid native face identity")
        if planes[face[0]][:4] != (0, 0, 1, 0):
            continue
        info = infos[face[5]]
        width, height = face[16] + 1, face[17] + 1
        require(face[6] == -1 and 1 <= width <= 65 and 1 <= height <= 65, "Unknown floor source geometry or grid")
        uv = [info[11] - face[14], info[15] - face[15]]
        require(all(value == int(value) for value in uv) and 0 <= uv[0] < width and 0 <= uv[1] < height, "Floor center is not a native luxel")
        maps = 4 if info[16] & 0x800 else 1
        styles = [style for style in face[8:12] if style != 255]
        require(styles in ([0], [0, 32]), "Unexpected floor styles")
        rows = {}
        for style_index, style in enumerate(styles):
            rows[str(style)] = []
            for direction in range(maps):
                offset = face[12] + ((style_index * maps + direction) * width * height + int(uv[1]) * width + int(uv[0])) * 4
                require(0 <= offset and offset + 4 <= len(lighting), "Floor sample exceeds native lighting bounds")
                raw = bytes(lighting[offset:offset + 4])
                rows[str(style)].append({"raw": raw.hex(), "decoded": reference.rgbexp(raw)})
        floors.append({"face": index, "dimensions": [width, height], "center_uv": uv, "styles": rows})
    require(len(floors) == 1, "Expected one unsplit native floor")
    return floors[0]


def matches(color, interval):
    return all(low <= value <= high for value, (low, high) in zip(color, interval, strict=True))


def inspect(data, case, prediction, game):
    measured = {}
    for channel in INTENSITIES:
        floor = native_center(data, channel)
        expected_styles = prediction["channels"][channel]["floor_styles"]
        require(floor["styles"].keys() == expected_styles.keys(), "Native style membership differs")
        for style, rows in floor["styles"].items():
            for actual, required in zip(rows, expected_styles[style], strict=True):
                require(actual["raw"] in required["rgbexp32_candidates"], f"{case['name']} {channel} style{style} differs from literal raw-alpha coverage: {actual} versus {required}")
        measured[channel] = {"floor": floor}
    if case.get("styled"):
        return measured
    entries, flags = pak_and_flags(data)
    require(flags & 3 == 3 and set(entries) == {MANIFEST, "sp_0.vhv", "sp_hdr_0.vhv", "sp_1.vhv", "sp_hdr_1.vhv"}, "Generated receiver bundle differs")
    manifest = json.loads(entries[MANIFEST])
    require(manifest["channels"] == ["ldr", "hdr"] and [row["ordinal"] for row in manifest["receivers"]] == [0, 1]
            and manifest["missing_sources"] == [], "Generated receiver/source identity differs")
    for source in manifest["sources"]:
        path = source["path"]
        require(not path.startswith("/") and ".." not in path.split("/") and "\\" not in path, "Unsafe source identity")
        raw = (game / path).read_bytes()
        require({"bytes": len(raw), "sha256": reference.digest(raw)} == {key: source[key] for key in ("bytes", "sha256")}, "Generated source closure differs from actual original bytes")
    sampled = len(case["alphas"]) if not case.get("disabled") and case.get("requested", True) and case.get("mode", "fractional") != "opaque" else 0
    required_textures = {f"materials/transmission/{suffix}.vtf" for suffix in ("a", "b")[:sampled]}
    actual_textures = {row["path"] for row in manifest["sources"] if row["path"] in ("materials/transmission/a.vtf", "materials/transmission/b.vtf")}
    require(actual_textures == required_textures, "Fractional/cutout VTF source scope differs from requested active casters")
    for receiver in manifest["receivers"]:
        ordinal = receiver["ordinal"]
        require(receiver["model"] == "models/oracle/receiver.mdl" and receiver["checksum"] == 619 and receiver["meshes"] == LAYOUT,
                "Actual hardware receiver identity differs")
        for channel in INTENSITIES:
            name = f"sp_{'hdr_' if channel == 'hdr' else ''}{ordinal}.vhv"
            report, colors = read_vhv(entries[name], 619, LAYOUT)
            root_rows = prediction["channels"][channel]["receiver_root_vertices"][str(ordinal)]
            require(all(matches(color, root_rows[root]["vhv"]["bgra_interval"]) for color, root in zip(colors, reference.HARDWARE, strict=True)),
                    f"{case['name']} {channel} receiver{ordinal} differs from independent linear transmission: {colors}")
            asset = next(row for row in manifest["assets"] if row["path"] == name)
            require(asset["sha256"] == report["sha256"] and asset["bytes"] == report["bytes"], "Generated VHV hash differs")
            measured[channel][f"receiver_{ordinal}"] = {"hardware_bgra": colors, "hardware_root_ids": reference.HARDWARE, "vhv": report}
    return measured


def snapshot(client, source, ids, game):
    return {"state": protected_state(client), "history": client.call("get_history"), "objects": detail(client, ids),
            "source_sha256": reference.digest(source.read_bytes()), "inputs": identities(game), "recipes": client.call("get_build_recipes")}


def compile_pair(client, args, source, ids, game, case, prediction):
    before = snapshot(client, source, ids, game)
    retained = args.output / "inputs" / case["name"]
    shutil.copytree(game, retained)
    output = args.output / f"{case['name']}.bsp"
    result = client.call("compile_map", {"path": str(output)})
    require(result["success"] and result["leak"] is None, result)
    active = 0 if case.get("disabled") else len(case["alphas"])
    stats = result["stats"]
    require(stats["prop_shadow_props"] == stats["prop_shadow_models"] == stats["prop_shadow_triangles"] == active,
            "Native caster preparation differs from the literal one-triangle-per-owner fixture")
    evaluated = active if case.get("requested", True) and case.get("mode", "fractional") != "opaque" else 0
    require(stats["prop_shadow_alpha_triangles"] == stats["prop_shadow_alpha_materials"] == stats["prop_shadow_alpha_textures"] == evaluated
            and stats["prop_shadow_alpha_bytes"] == evaluated * 16, "Requested mip0 alpha assets have the wrong scope")
    data = probe.read_bounded(output, reference.MAX_BSP_BYTES)
    measured = inspect(data, case, prediction, game)
    background = args.output / f"{case['name']}-background.bsp"
    started = client.call("start_compile", {"path": str(background)})["job"]
    job = wait_job(client)
    require(job["token"] == started["token"] and job["status"] == "completed" and job["result"]["published"]
            and not job["stale"] and background.read_bytes() == data, "Foreground/background bytes or publication differ")
    require(snapshot(client, source, ids, game) == before, "Compilation changed source/history/assets/selection/camera/recipes")
    return output, {"sha256": reference.digest(data), "stats": result["stats"], "channels": measured,
                    "protected_state_unchanged": True, "foreground_background_identical": True, "inputs": before["inputs"]}


def rejected_pair(client, args, source, ids, game, output, caster):
    path = game / "materials/models/test/taa.vmt"
    original, previous = path.read_bytes(), output.read_bytes()
    failures = {}
    try:
        for name, field, fragment in (("mixed-alpha", "$alphatest 1", "$translucent"), ("scalar-modulation", "$alpha 0.5", "$alpha modulation")):
            path.write_text(f'VertexLitGeneric {{ $translucent 1 {field} $basetexture "transmission/a" }}\n')
            before = snapshot(client, source, ids, game)
            shutil.copytree(game, args.output / "inputs" / name)
            failed = json.loads(client.call("compile_map", {"path": str(output)}, error=True)["content"][0]["text"])
            require(not failed["success"] and output.read_bytes() == previous and any(row["object"] == caster and fragment in row["message"] for row in failed["diagnostics"]),
                    "Unsupported requested material did not fail atomically on its caster")
            client.call("start_compile", {"path": str(output)})
            job = wait_job(client)
            require(job["status"] == "failed" and not job["result"]["published"] and output.read_bytes() == previous,
                    "Unsupported background publication replaced the previous BSP")
            require(snapshot(client, source, ids, game) == before, "Failed compile changed source/history/assets or recipes")
            failures[name] = failed
    finally:
        path.write_bytes(original)
    return failures


def fractional_source_inspection(client, args, source, ids, game, case, prediction):
    owned = args.output / "alpha-half-background.bsp"
    discovery = inspect_readonly(client, source, ids, owned, {"channel": "ldr", "vertex_limit": 2})
    arguments = {"build_token": discovery["build"]["token"], "prop": 0, "channel": "ldr", "vertex_limit": 2}
    current = inspect_readonly(client, source, ids, owned, arguments)
    require(current["selected"]["ordinal"] == 0, "Inspection did not retain the selected native receiver")
    require(current["build"]["status"] == "current" and current["bake"]["source_closure"] == "verified",
            "New fractional VTF is not owned by the successful source closure")
    path = game / "materials/transmission/a.vtf"
    original = path.read_bytes()
    try:
        changed = bytearray(original)
        changed[-1] ^= 1
        path.write_bytes(changed)
        stale = inspect_readonly(client, source, ids, owned, arguments)
        require(stale["build"]["status"] == "stale" and stale["historical"] is True and stale["bake"]["source_closure"] == "changed"
                and stale["vertices"]["rows"] == current["vertices"]["rows"], "Changed actual fractional VTF payload did not preserve explicitly historical owned rows")
    finally:
        path.write_bytes(original)
    restored_case = {**case, "name": "alpha-half-source-restored"}
    output, report = compile_pair(client, args, source, ids, game, restored_case, prediction)
    require(output.read_bytes() == (args.output / "alpha-half.bsp").read_bytes(), "Restored fractional VTF did not republish exact original bytes")
    report["post_publication_vtf_mutation_marks_historical"] = True
    return report


def counterfactuals(predictions):
    correct = predictions["layers-quarter-half"]["channels"]["ldr"]["floor_styles"]["0"][0]
    ratios = {"multiplicative_sheets": (191 / 255) * (127 / 255), "nearest_quarter_only": 191 / 255,
              "nearest_half_only": 127 / 255, "binary_half_threshold": 0}
    result = {name: native_interval([INTENSITIES["ldr"] * ratio, 0, 0]) for name, ratio in ratios.items()}
    for name, wrong in result.items():
        require(set(correct["rgbexp32_candidates"]).isdisjoint(wrong["rgbexp32_candidates"]),
                f"Predeclared layer profile cannot distinguish {name} after native encoding")
    doubled = native_interval([INTENSITIES["ldr"] * (127 / 255) ** 2, 0, 0])
    half = predictions["alpha-half"]["channels"]["ldr"]["floor_styles"]["0"][0]
    require(set(half["rgbexp32_candidates"]).isdisjoint(doubled["rgbexp32_candidates"]), "Single-layer profile does not distinguish double attenuation")
    return {"two_layer_wrong_predictions": result, "single_layer_double_attenuation": doubled}


def validate(args):
    args.output.mkdir(parents=True, exist_ok=False)
    game = args.output / "game"
    templates = fixtures(game)
    predictions = {case["name"]: expected(case) for case in CASES}
    write_json(args.output / "reference.json", {"version": 1, "formula": "T=(255-min(255,sum(actual intersected alpha8)))/255; linear light multiplied once before encoding",
               "roundoff_interval_linear": reference.ROUNDING_INTERVAL, "cases": CASES, "predictions": predictions,
               "counterfactuals": counterfactuals(predictions),
               "quantization": "Independent RGBExp32 truncation candidates from +/-1e-9 linear f64 roundoff; independent 628 VHV encoder",
               "geometry": "Each active caster uses exactly the first three-index strip of the existing one-triangle fixture; center and all four receiver-to-light rays are strict interior hits",
               "scope": "Point/spot world base/directional/style and normal/IgnoreNormals prop direct reception; other propagation/ownership/cancellation gates are separate compiler acceptance"})
    environment = {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
                   "HAMMERR_PREFABS_DIR": str(args.output / "prefabs"), "XDG_CONFIG_HOME": str(args.output / "config"), "XDG_STATE_HOME": str(args.output / "state")}
    with patch.dict(os.environ, environment):
        client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    results = {}
    try:
        with stopped_on_exit(client):
            ids = [client.call("create_block", {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"})["id"] for low, high in ROOM]
            floor = detail(client, [ids[0]])[0]
            floor_face = face_ref(floor, next(row for row in floor["faces"] if row["normal"] == [0, 0, 1]))
            ids.append(client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-64, 0, 8]})["id"])
            light = client.call("create_entity", {"classname": "light", "origin": [0, 0, 100], "properties": {"_light": "255 0 0 255.125", "_lightHDR": "0 0 255 510.25",
                                "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}})["id"]
            ids.append(light)
            receivers, casters = [], []
            for ignored in (0, 1):
                receivers.append(client.call("create_entity", {"classname": "prop_static", "origin": [0, 0, 16], "properties": {"model": "models/oracle/receiver.mdl",
                                 "disableshadows": "1", "ignorenormals": str(ignored), "solid": "0"}})["id"])
            for suffix, height in (("a", 50), ("b", 60)):
                casters.append(client.call("create_entity", {"classname": "prop_static", "origin": [0, 0, height], "properties": {"model": f"models/props/layer-{suffix}.mdl",
                               "skin": "1", "solid": "0", "disablevertexlighting": "1", "disableshadows": "1"}})["id"])
            ids.extend([*receivers, *casters])
            client.call("set_compile_settings", {"lighting": "direct_ldr_hdr", "physics": "none", "bounces": 0, "sky_quality": "fast", "bake_static_prop_vertices": True})
            for case in CASES:
                created = prepare_case(client, game, case, templates, casters, receivers, light, floor_face)
                if created is not None:
                    ids.append(created)
                source = args.output / f"{case['name']}.vmf"
                client.call("save_map", {"path": str(source)})
                output, report = compile_pair(client, args, source, ids, game, case, predictions[case["name"]])
                results[case["name"]] = report
                write_json(args.output / "results.json", results)
                if case["name"] == "alpha-half":
                    results["fractional_source_inspection"] = fractional_source_inspection(client, args, source, ids, game, case, predictions[case["name"]])
                    results["atomic_refusals"] = rejected_pair(client, args, source, ids, game, output, casters[0])
                    write_json(args.output / "results.json", results)
            for channel in INTENSITIES:
                left, right = results["layers-quarter-half"]["channels"][channel], results["layers-half-quarter"]["channels"][channel]
                require(left == right, "Reversing alpha64/128 physical layer order changed sampled lighting")
            require(results["alpha-zero"]["stats"]["prop_shadow_work"] > 0
                    and results["alpha-zero"]["stats"]["lighting_total_work"] > 0,
                    "All-transparent real geometry did not retain bounded preparation/traversal work")
            quarter = probe.bsp_lumps((args.output / "alpha-quarter.bsp").read_bytes())
            half = probe.bsp_lumps((args.output / "alpha-half.bsp").read_bytes())
            require(all(left == right for index, (left, right) in enumerate(zip(quarter, half, strict=True))
                        if index not in (0, 8, 40, 51, 52, 53, 55, 56)),
                    "Changing only fractional VTF alpha changed geometry, collision or emitter records")
            write_json(args.output / "results.json", results)
        require_clean_eof(client)
    finally:
        client.close()
    print(json.dumps({"cases": len(CASES), "dual_channel": True, "independent_linear_and_encoded_transmission": True, "output": str(args.output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate requested static-prop fractional texture shadows through shared MCP compilation and independent numeric predictions.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(), args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target"), "Keep retained artifacts on disk under target/")
    validate(args)


if __name__ == "__main__":
    main()
