import argparse
import hashlib
import io
import itertools
import json
import math
import struct
import subprocess
import zipfile
from pathlib import Path

import prop_bounce_reference as reference
import prop_indirect_fixtures as portable
import validate_cubemap_capture_probe as probe
from validate_prop_vertex_lighting import MANIFEST, pak_and_flags, read_vhv, require
from validate_static_props import static_records


ROOT = Path(__file__).resolve().parent.parent
COUNT = 32
SDK_C_BITS = 0x3EB50481
SDK_C = 11863169 / 33554432
DIRECT_FACTOR = 0.35
RHO = 0.875
INTENSITIES = {"ldr": 64, "hdr": 128}
MATERIAL = "hammerr652/room"
POINTS = reference.POINTS
HARDWARE = reference.HARDWARE
LAYOUT = [{"lod": 0, "vertex_count": 6}]
NORMALS = {"up": (0, 0, 1), "down": (0, 0, -1), "slanted": (0.6, 0, 0.8)}
VARIANTS = {"sphere-up": ("up", True), "sphere-down": ("down", True),
            "sphere-slanted": ("slanted", True), "normal-up": ("up", False), "normal-down": ("down", False)}
BOUNDS = ((-256, 256), (-256, 256), (0, 256))
FACE_SPECS = {"west": (0, -256, (1, 0, 0)), "east": (0, 256, (-1, 0, 0)),
              "south": (1, -256, (0, 1, 0)), "north": (1, 256, (0, -1, 0)),
              "floor": (2, 0, (0, 0, 1)), "ceiling": (2, 256, (0, 0, -1))}
REQUIRED_COUNTERFACTUALS = ("wrong_final_world", "doubled_direct", "unit_sphere", "hemisphere_unit", "hemisphere_sdk")


def file_identity(path):
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            hasher.update(block)
    return {"bytes": path.stat().st_size, "sha256": hasher.hexdigest()}


def fixture_contract(name, normal_name, ignored):
    return {"version": 1, "variant": name, "quality": COUNT, "room_bounds": BOUNDS,
            "material": MATERIAL, "source_rho": [RHO] * 3, "lightmap_scale": 64,
            "light_origin": [0, 0, 128], "channel_intensities": INTENSITIES,
            "attenuation": {"constant": 1, "linear": 0, "quadratic": 0}, "style": 0, "sky": False,
            "receiver": {"origin": [0, 0, 16], "angles": [0, 0, 0], "world_root_points": POINTS,
                         "authored_normal": NORMALS[normal_name], "ignorenormals": ignored,
                         "disableshadows": True, "disableselfshadowing": False,
                         "checksum": 619, "hardware_root_ids": HARDWARE},
            "normalization": {"sdk_f32_bits": SDK_C_BITS, "exact_fraction": "11863169/33554432",
                              "direct_factor": DIRECT_FACTOR},
            "linear_roundoff_interval": reference.ROUNDING_INTERVAL,
            "scope": "Prop transfer from published W0..W4, not independent world-solver validation"}


def fixtures(output):
    output.mkdir(parents=True, exist_ok=False)
    for name, (normal_name, ignored) in VARIANTS.items():
        directory = output / name
        game = directory / "game"
        portable.model(game, "receiver", [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)])
        path = game / "models/oracle/receiver.vvd"
        vvd = bytearray(path.read_bytes())
        for index in range(4):
            struct.pack_into("<3f", vvd, 64 + index * 48 + 28, *NORMALS[normal_name])
        reference.write(path, vvd)
        for material, rho in ((MATERIAL, (RHO,) * 3), ("hammerr652/white", (0, 0, 0))):
            reference.write(game / f"materials/{material}.vtf", portable.texture(rho))
            reference.write(game / f"materials/{material}.vmt", f'LightmappedGeneric {{ $basetexture "{material}" }}\n')
        reference.write(game / "materials/models/oracle/receiver.vmt",
                        'VertexLitGeneric { $model 1 $basetexture "hammerr652/white" }\n')
        reference.write(directory / "gameinfo.txt", 'GameInfo { game "HammeRR spherical prop oracle" FileSystem { SteamAppId 240 SearchPaths { game "|gameinfo_path|game" } } }\n')
        bounds = [((-272, -272, -16), (272, 272, 0)), ((-272, -272, 256), (272, 272, 272)),
                  ((-272, -272, 0), (-256, 272, 256)), ((256, -272, 0), (272, 272, 256)),
                  ((-256, -272, 0), (256, -256, 256)), ((-256, 256, 0), (256, 272, 256))]
        solids = [portable.box(100 + index, low, high, MATERIAL, scale=64) for index, (low, high) in enumerate(bounds)]
        light = portable.entity(9001, "light", origin="0 0 128", _light=f"255 0 0 {INTENSITIES['ldr']}",
                                _lightHDR=f"0 0 255 {INTENSITIES['hdr']}", _constant_attn="1", _linear_attn="0",
                                _quadratic_attn="0", style="0")
        receiver = portable.entity(9002, "prop_static", origin="0 0 16", angles="0 0 0", model="models/oracle/receiver.mdl",
                                   solid="0", skin="0", disableshadows="1", disablevertexlighting="0",
                                   disableselfshadowing="0", ignorenormals="1" if ignored else "0")
        player = portable.entity(9003, "info_player_terrorist", origin="0 -192 8")
        vmf = 'versioninfo { "editorversion" "400" "mapversion" "1" "formatversion" "100" }\nworld { "id" "1" "classname" "worldspawn" "mapversion" "1"\n' + "\n".join(solids) + "\n}\n" + "\n".join((light, receiver, player)) + "\n"
        reference.write(directory / "depth-room.vmf", vmf)
        reference.write(directory / "contract.json", json.dumps(fixture_contract(name, normal_name, ignored), indent=2) + "\n")
    frozen = {str(path.relative_to(output)): file_identity(path) for path in sorted(output.rglob("*")) if path.is_file()}
    reference.write(output / "files.json", json.dumps(frozen, indent=2) + "\n")
    return frozen


def verify_fixtures(output, frozen):
    actual = {str(path.relative_to(output)): file_identity(path) for path in sorted(output.rglob("*"))
              if path.is_file() and path != output / "files.json"}
    require(actual == frozen and json.loads((output / "files.json").read_text()) == frozen, "Fixture input bytes changed")
    for name, (normal_name, ignored) in VARIANTS.items():
        directory = output / name
        require(json.loads((directory / "contract.json").read_text()) == json.loads(json.dumps(fixture_contract(name, normal_name, ignored))),
                f"Literal fixture contract differs: {name}")
        game = directory / "game"
        vvd = (game / "models/oracle/receiver.vvd").read_bytes()
        normal = tuple(reference.f32(value) for value in NORMALS[normal_name])
        for index, point in enumerate(POINTS):
            position = struct.unpack_from("<3f", vvd, 64 + index * 48 + 16)
            require(position == (point[0], point[1], 0) and struct.unpack_from("<3f", vvd, 64 + index * 48 + 28) == normal,
                    "Actual VVD root position or literal normal differs")
        vtx = (game / "models/oracle/receiver.dx90.vtx").read_bytes()
        require([struct.unpack_from("<H", vtx, 260 + index * 9)[0] for index in range(6)] == HARDWARE,
                "Actual VTX hardware-root correspondence differs")
        texture = (game / f"materials/{MATERIAL}.vtf").read_bytes()
        require(len(texture) == 144 and texture[:4] == b"VTF\0" and struct.unpack_from("<3f", texture, 32) == (RHO,) * 3,
                "Actual measured VTF reflectivity differs")


def six_faces(raw, channel):
    face_slot, light_slot = (7, 8) if channel == "ldr" else (58, 53)
    faces = reference.records(raw, face_slot, reference.FACE_FORMAT, 64, (0,) if channel == "ldr" else (1,))
    planes = reference.records(raw, 1, "<4fi", 128)
    infos = reference.records(raw, 6, "<16f2i", 64)
    vertices = reference.records(raw, 3, "<3f", 128)
    edges = reference.records(raw, 12, "<2H", 256)
    surfedges = reference.records(raw, 13, "<i", 256)
    models = reference.records(raw, 14, "<9f3i", 1)
    texdata = reference.records(raw, 2, "<3f5i", 64)
    names = reference.records(raw, 44, "<i", 64)
    strings = bytes(reference.lump(raw, 43))
    lighting = reference.lump(raw, light_slot, (1,))
    require(len(strings) <= 16384 and len(lighting) % 4 == 0, "Source string or lighting layout exceeds fixture scope")
    require(len(models) == 1 and len(faces) == 6 and models[0][10:12] == (0, 6), "World must contain exactly the six known native faces")
    sources = {}
    for index, face in enumerate(faces):
        require(face[0] < len(planes) and face[1] == (face[0] & 1) and 0 <= face[5] < len(infos), "Invalid native face plane/side or texinfo")
        plane = planes[face[0]]
        selected = [name for name, (axis, boundary, normal) in FACE_SPECS.items()
                    if plane[:4] == (*normal, normal[axis] * boundary)]
        require(len(selected) == 1 and selected[0] not in sources, "Source face is outside the literal axis-aligned room")
        name = selected[0]
        axis, boundary, normal = FACE_SPECS[name]
        require(face[4] == 4 and face[3] >= 0 and face[3] + 4 <= len(surfedges), "Source face is not one native quad")
        points = []
        for (signed,) in surfedges[face[3]:face[3] + 4]:
            require(abs(signed) < len(edges), "Invalid source edge")
            vertex = edges[abs(signed)][0 if signed >= 0 else 1]
            require(vertex < len(vertices), "Invalid source vertex")
            points.append(vertices[vertex])
        expected = set(itertools.product(*[(boundary,) if component == axis else limits for component, limits in enumerate(BOUNDS)]))
        require(set(points) == expected, f"Native {name} quad differs from the literal room")
        info = infos[face[5]]
        require(info[16] & (0x2 | 0x4 | 0x80 | 0x400 | 0x800) == 0 and face[6] == -1
                and face[8:12] == (0, 255, 255, 255), "Sky, nodraw, unlit, bumped, displaced or styled source is unsupported")
        require(0 <= info[17] < len(texdata), "Invalid native texdata identity")
        texture = texdata[info[17]]
        require(texture[:3] == (RHO,) * 3 and 0 <= texture[3] < len(names), "Native source reflectivity differs")
        start = names[texture[3]][0]
        end = strings.find(b"\0", start) if 0 <= start < len(strings) else -1
        require(end >= start >= 0 and strings[start:end].decode("ascii").lower() == MATERIAL, "Native source material identity differs")
        dimensions = [face[16] + 1, face[17] + 1]
        require(all(1 <= value <= 33 for value in dimensions), "Source luxel dimensions exceed fixture scope")
        offset, total = face[12], dimensions[0] * dimensions[1]
        require(offset >= 4 and offset % 4 == 0 and offset + total * 4 <= len(lighting), "Invalid native source lighting range")
        samples = [reference.rgbexp(lighting[offset + row * 4:offset + (row + 1) * 4]) for row in range(total)]
        average = reference.rgbexp(lighting[offset - 4:offset])
        active = 0 if channel == "ldr" else 2
        require(all(value[component] == 0 for value in [average, *samples] for component in range(3) if component != active),
                "Source field mixes the literal red LDR/blue HDR channels")
        projection = [list(info[8:12]), list(info[12:16])]
        require(all(math.isfinite(value) for row in projection for value in row), "Nonfinite native projection")
        sources[name] = {"face": index, "plane": face[0], "plane_values": list(plane), "side": face[1],
                         "texinfo": face[5], "texdata": info[17], "material": MATERIAL, "rho": list(texture[:3]),
                         "corners": points, "normal": normal, "dimensions": dimensions, "mins": list(face[14:16]),
                         "projection": projection, "lighting_offset": offset, "average": average,
                         "average_raw": bytes(lighting[offset - 4:offset]).hex(), "samples": samples,
                         "samples_sha256": reference.digest(lighting[offset:offset + total * 4]),
                         "face_record_sha256": reference.digest(struct.pack(reference.FACE_FORMAT, *face))}
    require(set(sources) == set(FACE_SPECS), "A native source face is missing")
    return sources


def read_world(path):
    require(path.stat().st_size <= reference.MAX_BSP_BYTES, "World source exceeds the 16 MiB bound")
    with path.open("rb") as stream:
        raw = stream.read(reference.MAX_BSP_BYTES + 1)
    require(1036 <= len(raw) <= reference.MAX_BSP_BYTES and raw[:4] == b"VBSP"
            and struct.unpack_from("<i", raw, 4)[0] in (19, 20), "Unsupported bounded BSP header")
    flags = reference.lump(raw, 59)
    require(len(flags) in (0, 4) and (not flags or struct.unpack("<I", flags)[0] & 3 == 0), "World source has baked-prop flags")
    pak = reference.lump(raw, 40)
    if pak:
        with zipfile.ZipFile(io.BytesIO(pak)) as archive:
            require(len(archive.infolist()) <= 128 and all(not entry.filename.lower().endswith(".vhv")
                    and entry.filename.lower() != MANIFEST for entry in archive.infolist()), "World source contains generated prop assets")
    return {"file": path.name, "bytes": len(raw), "sha256": reference.digest(raw),
            "channels": {channel: six_faces(raw, channel) for channel in INTENSITIES}}


def sphere_directions():
    result = []
    for index in range(COUNT):
        z = 1 - (2 * index + 1) / COUNT
        angle = index * 2.399963229728653
        radius = math.sqrt(1 - z * z)
        result.append((radius * math.cos(angle), radius * math.sin(angle), z))
    return result


def hemisphere_directions(normal):
    require(normal in ((0, 0, 1), (0, 0, -1)), "Hemisphere counterfactual is limited to the literal +/-Z controls")
    result = []
    sign = normal[2]
    for index in range(COUNT):
        fraction = (2 * index + 1) / (2 * COUNT)
        angle = index * 2.399963229728653
        radius = math.sqrt(fraction)
        result.append((sign * radius * math.cos(angle), radius * math.sin(angle), sign * math.sqrt(1 - fraction)))
    return result


def first_exit(point, direction):
    require(all(low < value < high for value, (low, high) in zip(point, BOUNDS, strict=True)), "Receiver is outside the known room")
    candidates = []
    for name, (axis, boundary, normal) in FACE_SPECS.items():
        if direction[axis] == 0:
            continue
        distance = (boundary - point[axis]) / direction[axis]
        if distance > 0:
            candidates.append((distance, name))
    candidates.sort()
    require(candidates and (len(candidates) == 1 or candidates[1][0] - candidates[0][0] > 1e-8),
            "Ray has a tied or invalid axis-plane exit; no geometric fallback is supported")
    distance, name = candidates[0]
    hit = [value + distance * delta for value, delta in zip(point, direction, strict=True)]
    axis, boundary, normal = FACE_SPECS[name]
    hit[axis] = boundary
    require(all(low <= value <= high for value, (low, high) in zip(hit, BOUNDS, strict=True))
            and sum(a * b for a, b in zip(normal, direction, strict=True)) < 0, "Ray exit is not a front-facing source")
    return name, hit, distance


def source_value(source, point, distance, cone):
    width, height = source["dimensions"]
    uv = []
    for row, minimum, maximum in zip(source["projection"], source["mins"], (width - 1, height - 1), strict=True):
        coordinate = sum(row[axis] * point[axis] for axis in range(3)) + row[3] - minimum
        uv.append(min(max(coordinate, 0), maximum))
    u, v = uv
    x0, y0 = math.floor(u), math.floor(v)
    x1, y1 = min(x0 + 1, width - 1), min(y0 + 1, height - 1)
    weights = [((x0, y0), (1 - u + x0) * (1 - v + y0)), ((x1, y0), (u - x0) * (1 - v + y0)),
               ((x0, y1), (1 - u + x0) * (v - y0)), ((x1, y1), (u - x0) * (v - y0))]
    local = [math.fsum(source["samples"][y * width + x][axis] * weight for (x, y), weight in weights) for axis in range(3)]
    blend = min(1, max(0, (distance * cone - 20) / 20))
    value = [source["rho"][axis] * ((1 - blend) * local[axis] + blend * source["average"][axis]) for axis in range(3)]
    return value, {"uv": uv, "bilinear": [{"sample": y * width + x, "weight": weight,
                                           "rgb": source["samples"][y * width + x]} for (x, y), weight in weights],
                   "local": local, "stored_average": source["average"], "blend": blend, "reflected": value}


def gather(sources, point, directions, cone, normalization):
    colors, hits = [], []
    for index, direction in enumerate(directions):
        name, hit, distance = first_exit(point, direction)
        source = sources[name]
        value, filtered = source_value(source, hit, distance, cone)
        colors.append(value)
        hits.append({"ray": index, "direction": direction, "face_name": name, "native_face": source["face"],
                     "point": hit, "distance": distance, **filtered})
    return [math.fsum(value[axis] for value in colors) / (len(directions) * normalization) for axis in range(3)], hits


def direct(channel, point, normal=None):
    factor = DIRECT_FACTOR
    if normal is not None:
        delta = [-point[0], -point[1], 128 - point[2]]
        length = math.sqrt(sum(value * value for value in delta))
        factor = max(0, sum(a * b for a, b in zip(normal, delta, strict=True)) / length)
    return [INTENSITIES[channel] * factor if axis == (0 if channel == "ldr" else 2) else 0 for axis in range(3)]


def added(*colors):
    return [math.fsum(color[axis] for color in colors) for axis in range(3)]


def encoded(direct_color, reflected, gain=1):
    return reference.encoded_interval(added(direct_color, [gain * value for value in reflected]))


def arithmetic_checks():
    require(struct.unpack("<I", struct.pack("<f", SDK_C))[0] == SDK_C_BITS and SDK_C == float(reference.f32(0.35355)),
            "SDK normalization is not the selected exact f32 literal")
    sphere_cone = math.tan(math.acos(1 - 2 / COUNT))
    hemisphere_cone = math.tan(math.acos(1 - 1 / COUNT))
    sphere_filtered = 8 + 32 * min(1, max(0, (80 * sphere_cone - 20) / 20))
    hemisphere_filtered = 8 + 32 * min(1, max(0, (80 * hemisphere_cone - 20) / 20))
    require(abs(sphere_filtered - 23.511589229482855) < 1e-12
            and abs(hemisphere_filtered - 8.773177530606546) < 1e-12, "Independent filter-cone literal differs")
    recurrence = [32.0]
    for index in range(4):
        value = 32 + 0.75 * recurrence[-1]
        exponent = reference.exponent_for(value)
        recurrence.append(math.ldexp(int(math.ldexp(value, -exponent)), exponent))
    require(recurrence == [32, 56, 74, 87.5, 97.5], "Independent synthetic world recurrence differs")
    recurrence_separation = []
    for depth in range(2, 5):
        correct = 22.4 + recurrence[depth - 1] / SDK_C
        accumulated = 22.4 + math.fsum(recurrence[:depth]) / SDK_C
        require(accumulated > correct + reference.ROUNDING_INTERVAL, "Repeated-gather recurrence is not discriminated")
        recurrence_separation.append({"depth": depth, "correct": correct, "repeated_gathers": accumulated})
    return {"sdk_c": SDK_C, "sdk_c_f32_bits": SDK_C_BITS, "sphere_cone": sphere_cone,
            "hemisphere_cone": hemisphere_cone, "uniform_one": 1 / SDK_C, "uniform_64": 64 / SDK_C,
            "half_64": 32 / SDK_C, "single_valid_64": {str(count): 64 / (count * SDK_C) for count in (32, 128, 512)},
            "filter_witness": {"distance": 80, "local": 8, "stored_average": 40,
                               "sphere": sphere_filtered, "hemisphere": hemisphere_filtered},
            "synthetic_non_compiled_recurrence": {"world_values": recurrence, "separation": recurrence_separation}}


def oracle(world_paths, output, commands):
    require(len(world_paths) == 5 and len(commands) == 5 and len(set(world_paths)) == 5,
            "Supply exactly five distinct world-only outputs with matching retained commands, W0 through W4")
    arithmetic = arithmetic_checks()
    worlds = [read_world(path) for path in world_paths]
    for depth, (world, command) in enumerate(zip(worlds, commands, strict=True)):
        require(command["depth"] == depth and command["bake_static_prop_vertices"] is False and command["returncode"] == 0
                and command["bsp"] == {"bytes": world["bytes"], "sha256": world["sha256"]},
                "World order/bytes differ from the retained successful world-only compiler invocation")
    sphere = sphere_directions()
    hemisphere = {name: hemisphere_directions(NORMALS[name]) for name in ("up", "down")}
    sphere_cone, hemisphere_cone = arithmetic["sphere_cone"], arithmetic["hemisphere_cone"]
    transfers, traces = [], []
    for world in worlds:
        transfer, trace = {}, {}
        for channel in INTENSITIES:
            sources = world["channels"][channel]
            transfer[channel], trace[channel] = [], []
            for point in POINTS:
                reflected, hits = gather(sources, point, sphere, sphere_cone, SDK_C)
                wrong_cone, _ = gather(sources, point, sphere, hemisphere_cone, SDK_C)
                normal_gathers = {name: gather(sources, point, directions, hemisphere_cone, 1)[0]
                                  for name, directions in hemisphere.items()}
                transfer[channel].append({"sphere": reflected, "hemisphere_cone": wrong_cone, **normal_gathers})
                trace[channel].append({"point": point, "sphere": hits})
        transfers.append(transfer)
        traces.append(trace)
    expected, controls, wrong = {}, {name: {} for name in hemisphere}, {}
    for depth in range(5):
        key = f"N{depth}"
        expected[key] = {}
        for name in controls:
            controls[name][key] = {}
        if depth:
            wrong[key] = {}
        for channel in INTENSITIES:
            expected[key][channel] = []
            for name in controls:
                controls[name][key][channel] = []
            if depth:
                wrong[key][channel] = {name: [] for name in (*REQUIRED_COUNTERFACTUALS, "hemisphere_cone", "repeated_previous_states")}
            for root, point in enumerate(POINTS):
                d = direct(channel, point)
                source = transfers[depth - 1][channel][root] if depth else None
                reflected = source["sphere"] if depth else [0, 0, 0]
                expected[key][channel].append(encoded(d, reflected))
                for name in controls:
                    normal_direct = direct(channel, point, NORMALS[name])
                    controls[name][key][channel].append(encoded(normal_direct, source[name] if depth else [0, 0, 0]))
                if not depth:
                    continue
                cases = {"wrong_final_world": encoded(d, transfers[depth][channel][root]["sphere"]),
                         "doubled_direct": encoded(added(d, d), reflected), "unit_sphere": encoded(d, reflected, SDK_C),
                         "hemisphere_unit": encoded(d, source["down"]), "hemisphere_sdk": encoded(d, source["down"], 1 / SDK_C),
                         "hemisphere_cone": encoded(d, source["hemisphere_cone"]),
                         "repeated_previous_states": encoded(d, added(*(transfers[index][channel][root]["sphere"] for index in range(depth))))}
                for name, prediction in cases.items():
                    wrong[key][channel][name].append(prediction)
    separation, linear_separation, admitted = {}, {}, True
    for depth in range(1, 5):
        key = f"N{depth}"
        separation[key] = {}
        linear_separation[key] = {}
        for channel in INTENSITIES:
            axis = 2 if channel == "ldr" else 0
            witnesses = {name: reference.separated(expected[key][channel], wrong[key][channel][name], axis)
                         for name in REQUIRED_COUNTERFACTUALS}
            if depth >= 2:
                witnesses["previous_depth"] = reference.separated(expected[f"N{depth - 1}"][channel], expected[key][channel], axis)
            admitted = admitted and all(witnesses.values())
            separation[key][channel] = witnesses
            linear_separation[key][channel] = {name: [root for root, (left, right) in enumerate(
                zip(expected[key][channel], wrong[key][channel][name], strict=True))
                if abs(left["linear"][0 if channel == "ldr" else 2] - right["linear"][0 if channel == "ldr" else 2])
                > reference.ROUNDING_INTERVAL] for name in ("hemisphere_cone", "repeated_previous_states")}
    report = {"version": 1, "admitted": bool(admitted), "quality": COUNT,
              "scope": "Independent prop transfer from separately published W0..W4; world solver correctness and native appearance are separate",
              "formula": "PN=encode(D+sum(filtered source(W(N-1))*rho)/(Q*c32)); D once; every sphere direction counts",
              "roundoff_interval_linear": reference.ROUNDING_INTERVAL, "counterfactual_hemisphere_normal": NORMALS["down"],
              "hardware_root_ids": HARDWARE, "profile": fixture_contract("sphere-up", "up", True),
              "arithmetic": arithmetic, "world_compiler_commands": commands,
              "expected_flagged_root_vertices": expected, "expected_normal_controls": controls,
              "expected_flagged_hardware_vertices": {depth: {channel: [rows[root] for root in HARDWARE]
                                                            for channel, rows in channels.items()} for depth, channels in expected.items()},
              "counterfactuals": wrong, "same_root_byte_separation": separation,
              "additional_linear_separation": linear_separation,
              "worlds": [{**{key: value for key, value in world.items() if key != "channels"},
                          "sources": {channel: {name: {key: value for key, value in source.items() if key != "samples"}
                                                for name, source in sources.items()} for channel, sources in world["channels"].items()},
                          "sphere_hits": trace} for world, trace in zip(worlds, traces, strict=True)]}
    reference.write(output, json.dumps(report, indent=2) + "\n")
    require(admitted, f"Physical fixture fails required same-root quantized separation; retain {output} and change a physical parameter")
    return report, worlds


def compile_map(binary, fixtures_path, output, depth, bake, commands, command_path):
    command = [str(binary), str(fixtures_path / "depth-room.vmf"), "--game", str(fixtures_path / "gameinfo.txt"),
               "-o", str(output), "--lighting", "direct_ldr_hdr", "--sky-quality", "fast", "--bounces", str(depth)]
    if bake:
        command.append("--bake-static-prop-vertices")
    result = subprocess.run(command, capture_output=True, text=True, timeout=60)
    log = output.with_suffix(".log")
    log.write_text(result.stdout + result.stderr)
    commands.append({"argv": command, "depth": depth, "bake_static_prop_vertices": bake,
                     "returncode": result.returncode, "log": log.name,
                     "bsp": file_identity(output) if output.is_file() else None,
                     "map_input": file_identity(fixtures_path / "depth-room.vmf"),
                     "contract": file_identity(fixtures_path / "contract.json"),
                     "measured_source_vtf": file_identity(fixtures_path / f"game/materials/{MATERIAL}.vtf")})
    reference.write(command_path, json.dumps(commands, indent=2) + "\n")
    require(result.returncode == 0 and output.is_file(), f"Compiler rejected {output.name}; see {log}")
    return probe.read_bounded(output, reference.MAX_BSP_BYTES)


def matches(color, prediction):
    return all(low <= value <= high for value, (low, high) in zip(color, prediction["bgra_interval"], strict=True))


def inspect(data, world, fixture, variant, depth, report):
    lumps, expected_lumps = probe.bsp_lumps(data), probe.bsp_lumps(world)
    require(all(actual == expected for index, (actual, expected) in enumerate(zip(lumps, expected_lumps, strict=True))
                if index not in (40, 59)), "Prop reception changed world/game/geometry native lumps")
    normal_name, ignored = VARIANTS[variant]
    props = static_records(data)
    require(props["models"] == ["models/oracle/receiver.mdl"] and len(props["props"]) == 1, "Native receiver identity differs")
    prop = props["props"][0]
    require(prop[:6] == (0, 0, 16, 0, 0, 0) and bool(prop[10] & 8) == ignored and prop[10] & (64 | 128) == 0,
            "Native actual origin/angles or receiver flags differ")
    entries, flags = pak_and_flags(data)
    require(flags & 3 == 3 and set(entries) == {MANIFEST, "sp_0.vhv", "sp_hdr_0.vhv"}, "Generated prop assets are incomplete or extra")
    manifest = json.loads(entries[MANIFEST])
    require(manifest["version"] == 1 and manifest["channels"] == ["ldr", "hdr"] and len(manifest["receivers"]) == 1,
            "Generated channel/receiver identity differs")
    receiver = manifest["receivers"][0]
    require(receiver["ordinal"] == 0 and receiver["checksum"] == 619 and receiver["meshes"] == LAYOUT
            and receiver["model"] == "models/oracle/receiver.mdl", "Generated hardware receiver identity differs")
    require(manifest["missing_sources"] == [], "Portable source closure has missing assets")
    for source in manifest["sources"]:
        name = source["path"]
        require(not name.startswith("/") and "\\" not in name and ".." not in name.split("/"), "Unsafe manifest source path")
        require(file_identity(fixture / "game" / name) == {"bytes": source["bytes"], "sha256": source["sha256"]},
                f"Retained original source bytes differ: {name}")
    textures = [source["path"] for source in manifest["sources"] if source["path"].endswith(".vtf")]
    require(textures == ([f"materials/{MATERIAL}.vtf"] if depth else []), "Measured source identity has the wrong depth scope")
    expected = report["expected_flagged_root_vertices"] if ignored else report["expected_normal_controls"][normal_name]
    channels = {}
    for channel in INTENSITIES:
        path = "sp_hdr_0.vhv" if channel == "hdr" else "sp_0.vhv"
        identity, colors = read_vhv(entries[path], 619, LAYOUT)
        predicted = expected[f"N{depth}"][channel]
        require(all(matches(color, predicted[root]) for color, root in zip(colors, HARDWARE, strict=True)),
                f"{variant} N{depth} {channel} differs from independent {'sphere' if ignored else 'hemisphere'} transfer: {colors}")
        asset = next(row for row in manifest["assets"] if row["path"] == path)
        require(asset["sha256"] == identity["sha256"] and asset["bytes"] == identity["bytes"], "Generated VHV byte identity differs")
        channels[channel] = {"vhv": identity, "hardware_bgra": colors, "hardware_root_ids": HARDWARE}
    return {"sha256": reference.digest(data), "world_source_depth": depth - 1 if depth else None,
            "world_native_lumps_unchanged": True, "channels": channels}


def validate(args):
    args.output.mkdir(parents=True, exist_ok=False)
    fixture_path = args.output / "fixtures"
    frozen = fixtures(fixture_path)
    verify_fixtures(fixture_path, frozen)
    commands, worlds = [], {}
    command_path = args.output / "commands.json"
    binary_identity = file_identity(args.binary)
    reference.write(args.output / "binary.json", json.dumps({"path": str(args.binary), **binary_identity}, indent=2) + "\n")
    for family, variant in (("sphere", "sphere-up"), ("normal", "normal-up")):
        worlds[family] = []
        for depth in range(5):
            output = args.output / f"{family}-W{depth}.bsp"
            compile_map(args.binary, fixture_path / variant, output, depth, False, commands, command_path)
            worlds[family].append(output)
    for sphere_path, normal_path in zip(worlds["sphere"], worlds["normal"], strict=True):
        a, b = probe.bsp_lumps(sphere_path.read_bytes()), probe.bsp_lumps(normal_path.read_bytes())
        require(all(left == right for index, (left, right) in enumerate(zip(a, b, strict=True)) if index != 35),
                "Changing only receiver angular authoring changed world-only output beyond native static props")
    report, decoded = oracle(worlds["sphere"], args.output / "reference.json", commands[:5])
    for path, sphere in zip(worlds["normal"], decoded, strict=True):
        normal = read_world(path)
        require(normal["channels"] == sphere["channels"], "Ordinary control has a different independently decoded source field")
    results = {}
    for variant, (normal_name, ignored) in VARIANTS.items():
        results[variant] = {}
        for depth in range(5):
            output = args.output / f"{variant}-N{depth}.bsp"
            data = compile_map(args.binary, fixture_path / variant, output, depth, True, commands, command_path)
            world = worlds["sphere" if ignored else "normal"][depth].read_bytes()
            results[variant][f"N{depth}"] = inspect(data, world, fixture_path / variant, variant, depth, report)
            reference.write(args.output / "results.json", json.dumps(results, indent=2) + "\n")
    for depth in range(5):
        key = f"N{depth}"
        for channel in INTENSITIES:
            sphere_colors = [results[variant][key]["channels"][channel]["hardware_bgra"]
                             for variant in VARIANTS if VARIANTS[variant][1]]
            require(all(colors == sphere_colors[0] for colors in sphere_colors), "Flagged output depends on its valid authored normal")
            require(results["normal-up"][key]["channels"][channel]["hardware_bgra"] !=
                    results["normal-down"][key]["channels"][channel]["hardware_bgra"], "Ordinary control lost normal sensitivity")
    verify_fixtures(fixture_path, frozen)
    require(file_identity(args.binary) == binary_identity, "Compiler binary changed during validation")
    print("Passed N0..4 in both channels: independent six-face sphere transfer, same-root counterfactual separation, three valid normals, ordinary controls and exact world parity.")


def main():
    parser = argparse.ArgumentParser(description="Validate IgnoreNormals N0..4 against an independent bounded six-face world-transfer oracle.")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/hammerr-compile")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary = args.binary.expanduser().resolve()
    args.output = args.output.expanduser().resolve()
    validate(args)


if __name__ == "__main__":
    main()
