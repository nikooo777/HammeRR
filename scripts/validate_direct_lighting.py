import argparse
import json
import math
import struct
from pathlib import Path

from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM

FACE_FORMAT = "<HBBihhhh4Bif2i2ii2HI"


def lightmaps(data):
    offset, size, version, compression = struct.unpack_from("<4i", data, 8 + 8 * 16)
    assert version == 1 and compression == 0 and size % 4 == 0
    assert 0 <= offset <= offset + size <= len(data) and size <= 16 * 1024**2
    return data[offset : offset + size]


def floor_samples(data):
    faces = records(data, 7, FACE_FORMAT)
    planes = records(data, 1, "<4fi")
    infos = records(data, 6, "<16f2i")
    vertices = records(data, 3, "<3f")
    edges = records(data, 12, "<2H")
    surfedges = records(data, 13, "<i")
    colors = lightmaps(data)
    result = []
    for face in faces:
        normal = planes[face[0]]
        if normal[2] < 0.99 or abs(normal[3]) > 0.01 or face[12] < 0:
            continue
        info = infos[face[5]]
        s, t = info[8:12], info[12:16]
        determinant = s[0] * t[1] - s[1] * t[0]
        width, height = face[16] + 1, face[17] + 1
        assert face[8:12] == (0, 255, 255, 255) and face[12] >= 4
        assert face[12] + width * height * 4 <= len(colors)
        polygon = [
            vertices[edges[abs(edge[0])][0 if edge[0] >= 0 else 1]][:2]
            for edge in surfedges[face[3] : face[3] + face[4]]
        ]
        for v in range(height):
            for u in range(width):
                a, b = face[14] + u - s[3], face[15] + v - t[3]
                x, y = (
                    (a * t[1] - s[1] * b) / determinant,
                    (s[0] * b - a * t[0]) / determinant,
                )
                red, green, blue, exponent = struct.unpack_from(
                    "<3Bb", colors, face[12] + (v * width + u) * 4
                )
                border = False
                candidates = []
                for index, (ax, ay) in enumerate(polygon):
                    bx, by = polygon[(index + 1) % len(polygon)]
                    dx, dy = bx - ax, by - ay
                    border |= dx * (y - ay) - dy * (x - ax) > 1e-6
                    length = dx * dx + dy * dy
                    ratio = max(0, min(1, ((x - ax) * dx + (y - ay) * dy) / length))
                    candidate = (ax + ratio * dx, ay + ratio * dy)
                    candidates.append(candidate)
                if border:
                    x, y = min(
                        candidates,
                        key=lambda candidate: (
                            (candidate[0] - x) ** 2 + (candidate[1] - y) ** 2
                        ),
                    )
                result.append(
                    (
                        [x, y, 0],
                        [channel * 2**exponent for channel in (red, green, blue)],
                        2**exponent,
                        border,
                    )
                )
    return result


def center_sample(data):
    return next(
        color
        for point, color, _, _ in floor_samples(data)
        if abs(point[0]) < 0.001 and abs(point[1]) < 0.001
    )


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
    check(
        {"get_compile_settings", "set_compile_settings"}.issubset(tools),
        "Both shared lighting-setting MCP tools are registered",
    )
    settings = client.call("get_compile_settings")
    check(
        settings["lighting"] == "fullbright",
        "Fullbright remains the initial build mode",
    )
    client.call("set_compile_settings", {"lighting": "unknown"}, error=True)
    check(
        client.call("get_compile_settings") == settings,
        "Invalid modes leave settings unchanged",
    )
    client.call("set_compile_settings", {"lighting": "direct_ldr"})
    for label in [
        "red",
        "far",
        "occluded",
        "hidden",
        "fractional",
        "unsupported",
        "percentage",
        "hard-fade",
        "percentage-spot",
        "invalid-percentage",
    ]:
        client.call("new_map", {"discard_changes": True})
        ids = [
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )["id"]
            for low, high in ROOM
        ]
        ids.append(
            client.call(
                "create_entity",
                {"classname": "info_player_terrorist", "origin": [-64, 0, 8]},
            )["id"]
        )
        height = 200 if label == "far" else 100
        color = [255, 128, 32] if label == "fractional" else [255, 0, 0]
        properties = {
            "_light": " ".join(map(str, color + [200])),
            "_constant_attn": "0",
            "_linear_attn": "0",
            "_quadratic_attn": "1",
            "_fifty_percent_distance": "0",
            "_zero_percent_distance": "0",
            "_hardfalloff": "0",
            "style": "1" if label == "unsupported" else "0",
        }
        if label in [
            "percentage",
            "hard-fade",
            "percentage-spot",
            "invalid-percentage",
        ]:
            properties.update(
                {
                    "_fifty_percent_distance": "100",
                    "_zero_percent_distance": "50"
                    if label == "invalid-percentage"
                    else "200",
                    "_hardfalloff": "1"
                    if label in ["hard-fade", "percentage-spot"]
                    else "0",
                }
            )
        if label == "percentage-spot":
            properties.update(
                {"pitch": "-90", "_inner_cone": "20", "_cone": "45", "_exponent": "2"}
            )
        lamp = client.call(
            "create_entity",
            {
                "classname": "light_spot" if label == "percentage-spot" else "light",
                "origin": [0, 0, height],
                "properties": properties,
            },
        )["id"]
        ids.append(lamp)
        if label == "hidden":
            client.call("set_object_visibility", {"ids": [lamp], "action": "hide"})
        if label == "occluded":
            ids.append(
                client.call(
                    "create_block",
                    {
                        "mins": [-16, -16, 32],
                        "maxs": [16, 16, 48],
                        "material": "dev/dev_measuregeneric01",
                    },
                )["id"]
            )
        if label == "fractional":
            floor = detail(client, [ids[0]])[0]
            top = next(face for face in floor["faces"] if face["normal"] == [0, 0, 1])
            client.call(
                "select_faces",
                {"faces": [{"solid": floor["solid"], "side": top["side"]}]},
            )
            client.call(
                "edit_faces",
                {
                    "shift_u": 12,
                    "shift_v": 4,
                    "scale_u": -0.25,
                    "scale_v": 0.25,
                    "rotation": 17,
                    "lightmap_scale": 8,
                },
            )
        if label == "hard-fade":
            client.call(
                "create_entity",
                {
                    "classname": "logic_script",
                    "origin": [0, 0, 32],
                    "properties": {
                        "targetname": "hammerr_observer",
                        "vscripts": "hammerr_observer.nut",
                        "thinkfunction": "HammeRRThink",
                    },
                },
            )
            client.call(
                "create_entity",
                {
                    "classname": "prop_physics",
                    "origin": [160, -128, 128],
                    "properties": {
                        "targetname": "physics_crate",
                        "model": "models/props_junk/wood_crate001a.mdl",
                    },
                },
            )
        client.call(
            "set_compile_settings",
            {"physics": "engine" if label == "hard-fade" else "none"},
        )
        vmf = output / f"{label}.vmf"
        client.call("save_map", {"path": str(vmf)})
        client.call("open_map", {"path": str(vmf)})
        before = detail(client, ids)
        history = client.call("get_map_info")["undo"]
        saved = vmf.read_bytes()
        bsp = output / f"{label}.bsp"
        if label in ["unsupported", "invalid-percentage"]:
            client.call("set_compile_settings", {"lighting": "fullbright"})
            check(
                client.call("compile_map", {"path": str(bsp)})["success"],
                f"{label}: Fullbright permits unsupported lighting properties",
            )
            previous = bsp.read_bytes()
            client.call("set_compile_settings", {"lighting": "direct_ldr"})
            check(
                client.call("get_compile_status")["job"]["stale"],
                "Changing lighting marks the previous build stale",
            )
            response = client.call("compile_map", {"path": str(bsp)}, error=True)
            failed = json.loads(response["content"][0]["text"])
            check(
                not failed["success"]
                and any(
                    message["object"] == lamp and message["level"] == "error"
                    for message in failed["diagnostics"]
                ),
                f"{label}: rejected light has an object diagnostic",
            )
            check(
                bsp.read_bytes() == previous,
                f"{label}: rejected light preserves the previous BSP",
            )
        else:
            compiled = client.call("compile_map", {"path": str(bsp)})
            check(compiled["success"], f"{label}: Direct LDR compiles")
            data = bsp.read_bytes()
            stats = compiled["stats"]
            check(
                stats["lighting_mode"] == "direct_ldr" and stats["lighting_faces"] > 0,
                f"{label}: shared result reports actual baked faces",
            )
            check(
                stats["lighting_bytes"]
                == (stats["lightmap_samples"] + stats["lighting_faces"]) * 4,
                f"{label}: native bytes include one average per face",
            )
            world_lights = records(data, 15, "<9f3i7f3i")
            check(
                len(world_lights)
                == stats["world_lights"]
                == (0 if label == "hidden" else 1),
                f"{label}: world-light count matches visible emitters",
            )
            if label in ["red", "far", "occluded", "hidden"]:
                expected = (
                    [0, 0, 0]
                    if label in ["occluded", "hidden"]
                    else [50 if label == "far" else 200, 0, 0]
                )
                check(
                    center_sample(data) == expected,
                    f"{label}: native floor sample matches clear/far/shadow/hidden reference",
                )
            if label in ["percentage", "hard-fade", "percentage-spot"]:
                check(
                    center_sample(data) == [100, 0, 0],
                    f"{label}: 50% reach receives half brightness",
                )
                coefficients = [8 / 261, 0.01 * 8 / 261, 0.006325 * 8 / 261]
                light_record = world_lights[0]
                check(
                    abs(light_record[3] - 200 / 255) < 1e-6 and light_record[15] == 0,
                    f"{label}: native intensity omits legacy distance normalization",
                )
                check(
                    all(
                        abs(a - b) < 1e-7
                        for a, b in zip(light_record[16:19], coefficients, strict=True)
                    ),
                    f"{label}: native attenuation matches the SDK curve",
                )
                count = 0
                for point, actual, quantization, _ in floor_samples(data):
                    distance = math.sqrt(point[0] ** 2 + point[1] ** 2 + height**2)
                    reference = (
                        200
                        / (
                            coefficients[0]
                            + coefficients[1] * distance
                            + coefficients[2] * distance**2
                        )
                        * height
                        / distance
                    )
                    if label in ["hard-fade", "percentage-spot"]:
                        t = max(0, min(1, 1 - (distance - 175) / 25))
                        reference *= t**3 * (t * (6 * t - 15) + 10)
                    if label == "percentage-spot":
                        angle = height / distance
                        inner, outer = (
                            math.cos(math.radians(20)),
                            math.cos(math.radians(45)),
                        )
                        reference *= (
                            0
                            if angle <= outer
                            else angle
                            * (
                                1
                                if angle >= inner
                                else ((angle - outer) / (inner - outer)) ** 2
                            )
                        )
                    assert abs(
                        actual[0] - reference
                    ) <= quantization + 0.001 and actual[1:] == [0, 0], (
                        label,
                        point,
                        actual,
                        reference,
                    )
                    count += 1
                check(
                    count > 100,
                    f"{label}: floor falloff agrees with an independent radiance reference",
                )
            if label == "fractional":
                interior, borders = 0, 0
                for point, actual, quantization, border in floor_samples(data):
                    if max(abs(point[0]), abs(point[1])) >= 128:
                        continue
                    distance = math.sqrt(point[0] ** 2 + point[1] ** 2 + height**2)
                    reference = [
                        (channel / 255) ** 2.2
                        * 200
                        * 10000
                        / distance**2
                        * height
                        / distance
                        for channel in color
                    ]
                    assert all(
                        abs(a - b) <= quantization + 0.001
                        for a, b in zip(actual, reference)
                    ), (point, actual, reference)
                    interior += not border
                    borders += border
                check(
                    interior > 20 and borders > 0,
                    "Fractional/mirrored projection matches independent radiance across interior and clamped border samples",
                )
            background = output / f"{label}-background.bsp"
            client.call("start_compile", {"path": str(background)})
            job = wait_job(client)
            check(
                job["status"] == "completed"
                and job["settings"]["lighting"] == "direct_ldr",
                f"{label}: background job freezes shared lighting settings",
            )
            check(
                background.read_bytes() == data,
                f"{label}: synchronous and background MCP output agree",
            )
        check(
            detail(client, ids) == before
            and history == client.call("get_map_info")["undo"]
            and vmf.read_bytes() == saved,
            f"{label}: baking preserves source and undo history",
        )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-direct-lighting")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output)
        (args.output / "results.json").write_text(
            json.dumps({"checks": len(checks), "passed": checks}, indent=2) + "\n"
        )
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
