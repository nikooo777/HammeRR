import argparse
import hashlib
import itertools
import json
import math
import os
import struct
from pathlib import Path
from unittest.mock import patch

import validate_native_runtime as runtime
from validate_baked_lightmaps import check_source_samples, inspection
from validate_bump_lighting import texture
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_direct_lighting import FACE_FORMAT
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail, face_ref
from validate_prop_shadow_transforms import snapshot
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM
from validate_water_currents import game_lump_relative_digest

PIVOT = [16.125, -32.25, 24.375]
MINS = [32.125, -44.25, 16.375]
MAXS = [96.125, -20.25, 32.375]
TARGET = "initial_rotated_receiver"
MARKER = f'"targetname" "{TARGET}"'
MAX_BSP = 16 * 1024**2
ROUNDING_INTERVAL = 1e-9
INTENSITIES = {"ldr": 12.01, "hdr": 24.02}
CHANNELS = {"ldr": (7, 8), "hdr": (58, 53)}
IDENTITY = [[1, 0, 0], [0, 1, 0], [0, 0, 1]]
YAW90 = [[0, -1, 0], [1, 0, 0], [0, 0, 1]]
H = math.sqrt(0.5)
MIXED = [[0.5, -0.5 - H / 2, 0.5 - H / 2], [0.5, 0.5 - H / 2, -0.5 - H / 2], [H, 0.5, 0.5]]
PROFILES = [("absent", None, IDENTITY), ("signed-zero", "-0 +0.0 0e-300", IDENTITY),
            ("yaw90", "0 90 0", YAW90), ("mixed", "-45 45 45", MIXED)]
LAMPS = [("red", [15.875, 15.625, 64.375], 0), ("blue", [15.875, 95.75, 24], 2),
         ("green", [79.875, 111.75, 56], 1)]
X, Y, Z = math.sqrt(1 / 6), math.sqrt(1 / 2), math.sqrt(1 / 3)
SAMPLES = {
    "top": {"source": [64, -32, 32.375], "local": [47.875, 0.25, 8], "plane": [0, 0, 1, 8],
            "normals": [[0, 0, 1]], "lm_axes": [[0.125, 0, 0, 2.015625], [0, -0.125, 0, 4.03125]]},
    "front": {"source": [96.125, -32, 24], "local": [80, 0.25, -0.375], "plane": [1, 0, 0, 80],
              "normals": [[1, 0, 0], [Z, math.sqrt(2 / 3), 0], [Z, -X, -Y], [Z, -X, Y]],
              "lm_axes": [[0, 0.125, 0, -4.03125], [0, 0, -0.125, -3.046875]]},
}


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def file_digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def read_bsp(path):
    require(path.is_file() and 1036 <= path.stat().st_size <= MAX_BSP, "BSP exceeds the bounded fixture")
    data = path.read_bytes()
    require(1036 <= len(data) <= MAX_BSP and data[:4] == b"VBSP", "Invalid bounded BSP")
    return data


def lump(data, index):
    offset, size, version, compression = struct.unpack_from("<4i", data, 8 + index * 16)
    require(size >= 0 and compression == 0 and (size == 0 or 1036 <= offset <= len(data) - size), "Invalid native lump range/compression")
    return offset, version, data[offset:offset + size]


def records(data, index, layout, version=0, limit=128):
    _, actual_version, raw = lump(data, index)
    stride = struct.calcsize(layout)
    require(actual_version == version and len(raw) % stride == 0 and len(raw) // stride <= limit, "Unexpected native record framing/count")
    return list(struct.iter_unpack(layout, raw))


def model_layout(data, channel):
    models = records(data, 14, "<9f3i", limit=2)
    require(len(models) == 2 and models[1][:9] == (16, -12, -8, 80, 12, 8, 0, 0, 0) and models[1][11] == 6,
            "Model local geometry/bounds/origin changed")
    face_slot, light_slot = CHANNELS[channel]
    faces = records(data, face_slot, FACE_FORMAT, 0 if channel == "ldr" else 1, 64)
    infos = records(data, 6, "<16f2i", limit=64)
    planes = records(data, 1, "<4fi", limit=128)
    light_offset, light_version, lighting = lump(data, light_slot)
    require(light_version == 1 and 0 < len(lighting) <= MAX_BSP, "Invalid native lighting payload")
    rows = []
    first, count = models[1][10:12]
    require(0 <= first and first + count == len(faces), "Model face ranges do not cover the expected last model")
    for index in range(first, first + count):
        face = faces[index]
        require(0 <= face[0] < len(planes) and 0 <= face[5] < len(infos) and face[6] == -1, "Invalid model face identity")
        info = infos[face[5]]
        width, height = face[16] + 1, face[17] + 1
        maps = 4 if info[16] & 0x800 else 1
        require(face[8:12] == (0, 255, 255, 255) and 1 <= width <= 32 and 1 <= height <= 32, "Unexpected model style/grid")
        start, end = face[12] - 4, face[12] + width * height * maps * 4
        require(0 <= start < end <= len(lighting), "Model style-average/maps exceed lighting")
        rows.append({"face": index, "plane": list(planes[face[0]][:4]), "info": list(info), "face_record": list(face),
                     "dimensions": [width, height], "maps": maps, "relative_range": [start, end],
                     "absolute_range": [light_offset + start, light_offset + end]})
    require(sum(row["maps"] == 4 for row in rows) == 1, "Expected exactly one four-map model face")
    return rows


def raw_sample(data, channel, name):
    selected = SAMPLES[name]
    rows = [row for row in model_layout(data, channel) if row["plane"] == selected["plane"]]
    require(len(rows) == 1, "Missing unique literal sample plane")
    row = rows[0]
    info, face = row["info"], row["face_record"]
    require([info[8:12], info[12:16]] == selected["lm_axes"] and row["maps"] == len(selected["normals"]), "Authored native lightmap frame/bump role changed")
    uv = [sum(a * b for a, b in zip(axis[:3], selected["local"], strict=True)) + axis[3] - face[14 + index]
          for index, axis in enumerate(selected["lm_axes"])]
    width, height = row["dimensions"]
    require(all(value == int(value) for value in uv) and 0 < uv[0] < width - 1 and 0 < uv[1] < height - 1,
            "Literal point is not a strict interior native luxel")
    _, _, lighting = lump(data, CHANNELS[channel][1])
    values = []
    for direction in range(row["maps"]):
        offset = face[12] + (direction * width * height + int(uv[1]) * width + int(uv[0])) * 4
        values.append(lighting[offset:offset + 4].hex())
    return {"face": row["face"], "uv": uv, "raw": values, "dimensions": row["dimensions"]}


def rotate(matrix, vector):
    return [sum(a * b for a, b in zip(row, vector, strict=True)) for row in matrix]


def encode_candidates(linear):
    if max(linear) == 0:
        return ["00000000"]
    lower = [max(0, value - ROUNDING_INTERVAL) for value in linear]
    upper = [value + ROUNDING_INTERVAL for value in linear]
    exponents = {math.frexp(max(bound))[1] - 8 for bound in (lower, upper)}
    candidates = set()
    for exponent in exponents:
        components = [sorted({min(255, int(math.ldexp(value, -exponent))) for value in pair})
                      for pair in zip(lower, upper, strict=True)]
        for rgb in itertools.product(*components):
            candidates.add(struct.pack("<3Bb", *rgb, exponent).hex())
    return sorted(candidates)


def predict(matrix, channel, sample, point_matrix=None, normal_matrix=None, auxiliary_matrix=None):
    selected = SAMPLES[sample]
    point_pose = matrix if point_matrix is None else point_matrix
    normal_pose = matrix if normal_matrix is None else normal_matrix
    point = [a + b for a, b in zip(PIVOT, rotate(point_pose, selected["local"]), strict=True)]
    normals = []
    for index, normal in enumerate(selected["normals"]):
        pose = auxiliary_matrix if index and auxiliary_matrix is not None else normal_pose
        normals.append(rotate(pose, normal))
    linear = [[0.0, 0.0, 0.0] for _ in normals]
    for _, origin, axis in LAMPS:
        delta = [a - b for a, b in zip(origin, point, strict=True)]
        length = math.sqrt(sum(value * value for value in delta))
        factors = [max(0, min(1, sum(a * b for a, b in zip(normal, delta, strict=True)) / length)) for normal in normals]
        if factors[0] > 0:
            for direction, factor in enumerate(factors):
                linear[direction][axis] = INTENSITIES[channel] * factor
    return {"world_point": point, "world_normals": normals, "linear": linear,
            "rgbexp32_candidates": [encode_candidates(value) for value in linear]}


def declarations():
    profiles = {name: {channel: {sample: predict(matrix, channel, sample) for sample in SAMPLES} for channel in CHANNELS}
                for name, _, matrix in PROFILES}
    wrong = {}
    for name, point, normal in [("ignored-pose", IDENTITY, IDENTITY), ("position-only", YAW90, IDENTITY), ("normal-only", IDENTITY, YAW90)]:
        wrong[name] = {channel: {sample: predict(YAW90, channel, sample, point, normal) for sample in SAMPLES} for channel in CHANNELS}
    wrong["unrotated-auxiliary"] = {channel: {sample: predict(YAW90, channel, sample, auxiliary_matrix=IDENTITY) for sample in SAMPLES} for channel in CHANNELS}
    for name, rows in wrong.items():
        separated = []
        for channel in CHANNELS:
            for sample in SAMPLES:
                for index, (correct, bad) in enumerate(zip(profiles["yaw90"][channel][sample]["rgbexp32_candidates"], rows[channel][sample]["rgbexp32_candidates"], strict=True)):
                    if set(correct).isdisjoint(bad):
                        separated.append([channel, sample, index])
        require(separated, f"Frozen raw samples do not distinguish {name}")
        wrong[name]["encoded_separations"] = separated
    return {"version": 1, "pivot": PIVOT, "source_bounds": [MINS, MAXS], "profiles": profiles, "counterfactuals": wrong,
            "matrices": {name: matrix for name, _, matrix in PROFILES}, "lamps": LAMPS, "samples": SAMPLES,
            "formula": "Independent literal Source RzRyRx matrices; world point=P+R*(Q-P), all normals=R*n; constant point intensity*max(0,n dot normalized(L-point)); auxiliary maps only for base-positive lamp",
            "roundoff_interval_linear": ROUNDING_INTERVAL, "quantization": "Independent RGBExp32 truncation; fixed +/-1e-9 linear roundoff, never fitted to observed bytes",
            "scope": "Two strict interior selected luxels and four directional maps through both production MCP paths/modes; broader hull/smoothing/bounce/culling coverage is compiler acceptance"}


def assets(args):
    game = runtime.private_game(args.game_dir, args.output / "source-game", engine_physics=True)
    folder = game / "materials/hammerr680"
    folder.mkdir(parents=True)
    (folder / "white.vtf").write_bytes(texture(bytes([255] * 32 * 32 * 4)))
    (folder / "normal.vtf").write_bytes(texture(bytes([32, 128, 212, 255] * 32 * 32), normal=True))
    for name, extra in [("plain", ""), ("bumped", '$bumpmap "hammerr680/normal"')]:
        (folder / f"{name}.vmt").write_text(f'LightmappedGeneric {{ $basetexture "hammerr680/white" $surfaceprop concrete {extra} }}\n')
    return game


def fixture(client):
    client.call("new_map", {"discard_changes": True})
    ids = [client.call("create_block", {"mins": low, "maxs": high, "material": "hammerr680/plain"})["id"] for low, high in ROOM]
    for identifier in ids:
        shape = detail(client, [identifier])[0]
        client.call("edit_faces", {"faces": [face_ref(shape, face) for face in shape["faces"]], "lightmap_scale": 32})
    block = client.call("create_block", {"mins": MINS, "maxs": MAXS, "material": "hammerr680/plain"})["id"]
    shape = detail(client, [block])[0]
    client.call("edit_faces", {"faces": [face_ref(shape, face) for face in shape["faces"]], "lightmap_scale": 8, "smoothing_groups": []})
    for normal, u, v, name in [([0, 0, 1], [1, 0, 0], [0, -1, 0], "plain"), ([1, 0, 0], [0, 1, 0], [0, 0, -1], "bumped")]:
        face = next(face for face in shape["faces"] if face["normal"] == normal)
        client.call("edit_faces", {"faces": [face_ref(shape, face)], "material": f"hammerr680/{name}", "u_axis": u, "v_axis": v, "shift_u": 0, "shift_v": 0})
    owner = client.call("tie_to_entity", {"ids": [block], "classname": "func_brush", "properties": {"origin": "16.125 -32.25 24.375", "targetname": TARGET}})["id"]
    ids.append(owner)
    client.call("set_entity_properties", {"id": owner, "remove": ["angle", "angles"]})
    for _, origin, axis in LAMPS:
        rgb = [255 if component == axis else 0 for component in range(3)]
        ids.append(client.call("create_entity", {"classname": "light", "origin": origin, "properties": {
            "_light": " ".join(map(str, [*rgb, INTENSITIES["ldr"]])), "_lightHDR": " ".join(map(str, [*rgb, INTENSITIES["hdr"]])),
            "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}})["id"])
    for classname, origin, properties in [
        ("info_player_terrorist", [-192, -176, 8], {}), ("info_player_counterterrorist", [192, 176, 8], {}),
        ("logic_script", [-192, -176, 64], {"targetname": "hammerr_observer", "vscripts": "hammerr_observer.nut", "thinkfunction": "HammeRRThink"}),
        ("prop_physics", [160, -128, 128], {"targetname": "physics_crate", "model": "models/props_junk/wood_crate001a.mdl"}),
    ]:
        ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
    client.call("select", {"ids": [owner]})
    client.call("set_camera", {"projection": "perspective", "position": [16.125, 159.75, 64], "look_at": [16.125, 15.75, 24.375]})
    return ids, owner


def compare_geometry(reference, data, include_lighting=False):
    for index in range(1, 64):
        if not include_lighting and index in (8, 53):
            continue
        _, version_a, a = lump(reference, index)
        _, version_b, b = lump(data, index)
        if index == 35:
            a, b = game_lump_relative_digest(reference), game_lump_relative_digest(data)
        require(version_a == version_b and a == b, f"Pose changed unrelated native lump {index}")
    if not include_lighting:
        for channel, (_, slot) in CHANNELS.items():
            _, _, a = lump(reference, slot)
            _, _, b = lump(data, slot)
            if not a and not b:
                continue
            left, right = model_layout(reference, channel), model_layout(data, channel)
            require([{key: value for key, value in row.items() if key != "absolute_range"} for row in left]
                    == [{key: value for key, value in row.items() if key != "absolute_range"} for row in right], "Pose changed native model-lighting framing")
            require(len(a) == len(b), "Pose changed native lighting payload size")
            cursor = 0
            for start, end in sorted(row["relative_range"] for row in left):
                require(cursor <= start and a[cursor:start] == b[cursor:start], "Pose changed world/unrelated lighting samples")
                cursor = end
            require(a[cursor:] == b[cursor:], "Pose changed unrelated trailing lighting samples")


def compile_pair(client, args, game, ids, source, label, mode, expected):
    before = snapshot(client, args.output, source, ids, game)
    path = args.output / f"{mode}-{label}.bsp"
    result = client.call("compile_map", {"path": str(path)})
    require(result["success"] and result["leak"] is None and result["stats"]["lit_brush_models"] == 1
            and result["stats"]["model_lighting_faces"] == 6 and result["stats"]["bump_faces"] == 1, "Missing admitted model/bump receivers")
    data = read_bsp(path)
    measured = {}
    channels = CHANNELS if mode == "direct_ldr_hdr" else ("ldr",)
    for channel in channels:
        measured[channel] = {}
        for sample in SAMPLES:
            actual = raw_sample(data, channel, sample)
            required = expected[channel][sample]["rgbexp32_candidates"]
            require(all(raw in candidates for raw, candidates in zip(actual["raw"], required, strict=True)),
                    f"{label}/{channel}/{sample}: native raw samples differ from frozen independent pose: {actual} versus {required}")
            measured[channel][sample] = actual
    background = path.with_name(path.stem + "-background.bsp")
    started = client.call("start_compile", {"path": str(background)})["job"]
    job = wait_job(client)
    require(job["token"] == started["token"] and job["status"] == "completed" and job["result"]["published"] and not job["stale"]
            and read_bsp(background) == data, "Foreground/background publication or BSP differs")
    for channel, samples in measured.items():
        for sample in samples.values():
            for index in range(len(sample["raw"])):
                report, _ = inspection(client, channel=channel, face=sample["face"], map_index=index)
                check_source_samples(background, report)
    require(snapshot(client, args.output, source, ids, game) == before, "Compile/inspection changed source/history/geometry/assets/camera/selection/preferences")
    return data, {"bsp": str(path), "bsp_sha256": digest(data), "source": str(source), "source_sha256": file_digest(source),
                  "background_bsp": str(background), "measured": measured, "stats": result["stats"], "protected_state_unchanged": True}


def refusals(client, args, game, ids, owner, seed, previous, mode):
    target = args.output / f"{mode}-preserved.bsp"
    target.write_bytes(previous)
    rows = {}
    for name, fields, classname in [("parented", [('angles', '0 90 0'), ('parentname', 'missing_parent')], "func_brush"),
                                     ("unsupported-class", [('angles', '0 90 0')], "func_door"),
                                     ("ambiguous-origin", [('angles', '0 90 0'), ('origin#1', '0 0 0')], "func_brush"),
                                     ("range", [('angles', '0 360.0001 0')], "func_brush")]:
        source = args.output / f"{mode}-invalid-{name}.vmf"
        text = seed.replace(MARKER, MARKER + "".join(f'\n"{key}" "{value}"' for key, value in fields))
        if classname != "func_brush":
            require(text.count('"classname" "func_brush"') == 1, "Ambiguous class refusal fixture")
            text = text.replace('"classname" "func_brush"', f'"classname" "{classname}"')
        source.write_text(text)
        client.call("open_map", {"path": str(source), "discard_changes": True})
        before = snapshot(client, args.output, source, ids, game)
        result = json.loads(client.call("compile_map", {"path": str(target)}, error=True)["content"][0]["text"])
        require(not result["success"] and any(row["object"] == owner and "initial brush-model lighting pose" in row["message"] for row in result["diagnostics"])
                and target.read_bytes() == previous, f"{name}: missing atomic caster refusal")
        started = client.call("start_compile", {"path": str(target)})["job"]
        job = wait_job(client)
        require(job["token"] == started["token"] and job["status"] == "failed" and not job["result"]["published"]
                and target.read_bytes() == previous and snapshot(client, args.output, source, ids, game) == before,
                f"{name}: failed background compile changed prior BSP/protected state")
        rows[name] = {"foreground": result, "background": job, "prior_bsp_sha256": digest(previous)}
    return rows


def validate(args):
    args.output.mkdir(parents=True, exist_ok=False)
    frozen = declarations()
    write_json(args.output / "reference.json", frozen)
    game = assets(args)
    environment = {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
                   "HAMMERR_PREFABS_DIR": str(args.output / "prefabs"), "XDG_CONFIG_HOME": str(args.output / "config"), "XDG_STATE_HOME": str(args.output / "state")}
    with patch.dict(os.environ, environment):
        client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    results = {"status": "failed", "binary": str(args.binary), "binary_sha256": file_digest(args.binary),
               "script_sha256": file_digest(Path(__file__).resolve()), "assets": str(game / "materials/hammerr680"),
               "asset_identities": {path.name: file_digest(path) for path in sorted((game / "materials/hammerr680").iterdir())}, "cases": {}, "refusals": {}}
    try:
        with stopped_on_exit(client):
            ids, owner = fixture(client)
            seed_path = args.output / "seed.vmf"
            client.call("save_map", {"path": str(seed_path)})
            seed = seed_path.read_text()
            require(seed.count(MARKER) == 1, "Ambiguous fixture model")
            for mode in ("direct_ldr", "direct_ldr_hdr"):
                reference = None
                for label, angles, _ in PROFILES:
                    source = args.output / f"{mode}-{label}.vmf"
                    source.write_text(seed.replace(MARKER, MARKER if angles is None else MARKER + f'\n"angles" "{angles}"'))
                    client.call("open_map", {"path": str(source), "discard_changes": True})
                    client.call("set_compile_settings", {"lighting": mode, "physics": "engine", "bounces": 0, "smoothing_angle": 0,
                                "sky_quality": "fast", "bake_static_prop_vertices": False, "generate_reflections": False, "pack_sky_assets": False})
                    data, report = compile_pair(client, args, game, ids, source, label, mode, frozen["profiles"][label])
                    if reference is None:
                        reference = data
                    compare_geometry(reference, data, label == "signed-zero")
                    results["cases"][f"{mode}/{label}"] = report
                    write_json(args.output / "results.json", results)
                results["refusals"][mode] = refusals(client, args, game, ids, owner, seed, reference, mode)
            require(seed_path.read_text() == seed and not client.call("get_map_info")["rendering"]["initialized"], "Validation changed seed or initialized graphics")
        results["clean_eof_exit_code"] = require_clean_eof(client)
        results["status"] = "accepted"
    finally:
        client.close()
        write_json(args.output / "results.json", results)
    print(json.dumps({"status": results["status"], "cases": len(results["cases"]), "output": str(args.output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate bounded initial func_brush lighting rotations with independent literal matrices and public MCP/native-format oracles; requires installed CS:S engine physics.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary, args.game_dir, args.output = args.binary.resolve(strict=True), args.game_dir.resolve(strict=True), args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target"), "Keep retained artifacts on disk under repository target/")
    validate(args)


if __name__ == "__main__":
    main()
