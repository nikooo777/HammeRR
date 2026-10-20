import argparse
import hashlib
import json
import os
import shutil
import struct
from pathlib import Path
from unittest.mock import patch

import prop_bounce_reference as reference
import validate_cubemap_capture_probe as probe
import validate_prop_shadows as legacy
from validate_build_recipes import protected_state
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail
from validate_prop_shadow_transmission import (
    identities,
    native_center,
    require,
    write_json,
)
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM

REQUEST = 0x00040000
CLAMP_S = 0x00000004
CLAMP_T = 0x00000008
POINTS = [(-24, -24, 0), (24, -24, 0), (0, 24, 0)]
UVS = [(-0.375, 0.125), (0.625, 0.125), (0.125, 1.125)]
ALPHAS = [[0, 64, 128, 192], [255, 32, 96, 160], [16, 80, 144, 208], [48, 112, 176, 240]]
COMPOSED = "center 0.25 0.75 scale 2 3 rotate 90 translate 0.5625 -0.1875"
ENCODED = {
    0: {"ldr": "00000000", "hdr": "00000000"},
    63: {"ldr": "fc0000fe", "hdr": "0000fcff"},
    79: {"ldr": "9e0000ff", "hdr": "00009e00"},
    95: {"ldr": "be0000ff", "hdr": "0000be00"},
    111: {"ldr": "de0000ff", "hdr": "0000de00"},
    127: {"ldr": "fe0000ff", "hdr": "0000fe00"},
    143: {"ldr": "8f000000", "hdr": "00008f01"},
    191: {"ldr": "bf000000", "hdr": "0000bf01"},
    207: {"ldr": "cf000000", "hdr": "0000cf01"},
    239: {"ldr": "ef000000", "hdr": "0000ef01"},
    255: {"ldr": "ff000000", "hdr": "0000ff01"},
}
CASES = [
    {"name": "identity-fractional", "transform": "rotate 0", "uv": [0.125, 0.625], "texel": [0, 2], "alpha8": 16, "remaining": 239},
    {"name": "positive90-fractional", "transform": "rotate 90", "uv": [0.375, 0.125], "texel": [1, 0], "alpha8": 64, "remaining": 191},
    {"name": "negative90-fractional", "transform": "rotate -90", "uv": [0.625, 0.875], "texel": [2, 3], "alpha8": 176, "remaining": 79},
    {"name": "halfturn-fractional", "transform": "rotate 180", "uv": [0.875, 0.375], "texel": [3, 1], "alpha8": 160, "remaining": 95},
    {"name": "noncardinal30-fractional", "transform": "rotate 30", "uv_interval": [[0.1127, 0.1128], [0.4207, 0.4208]], "texel": [0, 1], "alpha8": 255, "remaining": 0},
    {"name": "composed-repeat", "transform": COMPOSED, "uv": [1.1875, 0.3125], "texel": [0, 1], "alpha8": 255, "remaining": 0},
    {"name": "composed-clamp-s", "transform": COMPOSED, "flags": CLAMP_S, "uv": [1.1875, 0.3125], "texel": [3, 1], "alpha8": 160, "remaining": 95},
    {"name": "negative-scale", "transform": "center 0.25 0.75 scale -2 3 rotate 90 translate 0.5625 -0.1875", "uv": [1.1875, 0.8125], "texel": [0, 3], "alpha8": 48, "remaining": 207},
    {"name": "negative-t-repeat", "transform": "rotate -90 translate 0 -1", "uv": [0.625, -0.125], "texel": [2, 3], "alpha8": 176, "remaining": 79},
    {"name": "negative-t-clamp", "transform": "rotate -90 translate 0 -1", "flags": CLAMP_T, "uv": [0.625, -0.125], "texel": [2, 0], "alpha8": 128, "remaining": 127},
    {"name": "positive90-cutout", "mode": "cutout", "transform": "rotate 90", "uv": [0.375, 0.125], "texel": [1, 0], "alpha8": 64, "remaining": 255},
    {"name": "negative90-cutout", "mode": "cutout", "transform": "rotate -90", "uv": [0.625, 0.875], "texel": [2, 3], "alpha8": 176, "remaining": 0},
]
COUNTERFACTUALS = [
    {"case": "positive90-fractional", "error": "ignored rotation", "uv": [0.125, 0.625], "texel": [0, 2], "alpha8": 16, "remaining": 239},
    {"case": "positive90-fractional", "error": "reversed rotation", "uv": [0.625, 0.875], "texel": [2, 3], "alpha8": 176, "remaining": 79},
    {"case": "halfturn-fractional", "error": "ignored rotation", "uv": [0.125, 0.625], "texel": [0, 2], "alpha8": 16, "remaining": 239},
    {"case": "noncardinal30-fractional", "error": "reversed noncardinal rotation", "uv_interval": [[0.2377, 0.2378], [0.7957, 0.7958]], "texel": [0, 3], "alpha8": 48, "remaining": 207},
    {"case": "composed-repeat", "error": "reversed rotation", "uv": [0.4375, 0.8125], "texel": [1, 3], "alpha8": 112, "remaining": 143},
    {"case": "composed-repeat", "error": "rotation before nonuniform scale", "uv": [1.0625, 0.1875], "texel": [0, 0], "alpha8": 0, "remaining": 255},
    {"case": "composed-repeat", "error": "ignored rotation", "uv": [0.5625, 0.1875], "texel": [2, 0], "alpha8": 128, "remaining": 127},
    {"case": "composed-repeat", "error": "translation before rotation", "uv": [0.8125, 1.0625], "texel": [3, 0], "alpha8": 192, "remaining": 63},
    {"case": "composed-repeat", "error": "ignored authored center", "uv": [0.6875, -0.4375], "texel": [2, 2], "alpha8": 144, "remaining": 111},
    {"case": "composed-clamp-s", "error": "repeat instead of clamp S", "uv": [1.1875, 0.3125], "texel": [0, 1], "alpha8": 255, "remaining": 0},
    {"case": "negative-scale", "error": "lost scale sign", "uv": [1.1875, 0.3125], "texel": [0, 1], "alpha8": 255, "remaining": 0},
    {"case": "negative-t-clamp", "error": "repeat instead of clamp T", "uv": [0.625, -0.125], "texel": [2, 3], "alpha8": 176, "remaining": 79},
    {"case": "positive90-cutout", "error": "reversed cutout rotation", "uv": [0.625, 0.875], "texel": [2, 3], "alpha8": 176, "remaining": 0},
    {"case": "negative90-cutout", "error": "ignored cutout rotation", "uv": [0.125, 0.625], "texel": [0, 2], "alpha8": 16, "remaining": 255},
]
REFUSALS = [
    ("malformed-angle", "rotate invalid", "invalid $basetexturetransform"),
    ("incomplete-angle", "rotate", "incomplete $basetexturetransform"),
    ("duplicate-angle", "rotate 90 rotate -90", "duplicate $basetexturetransform"),
    ("nonfinite-angle", "rotate NaN", "finite and within"),
    ("out-of-bounds-angle", "rotate 1024.00001", "finite and within"),
]
PREFERENCE_TOOLS = ("get_preview_defaults", "get_grid_defaults", "get_navigation_settings", "get_keyboard_shortcuts")
PREFERENCE_FILES = ("preview-defaults.json", "grid-defaults.json", "navigation-defaults.json", "shortcuts.json", "material-favorites.json")


def alpha_texture(flags=0):
    raw = legacy.alpha_texture()
    struct.pack_into("<I", raw, 20, 0x2000 | flags)
    for y, row in enumerate(ALPHAS):
        for x, alpha in enumerate(row):
            raw[80 + (y * 4 + x) * 4 + 3] = alpha
    return raw


def fixtures(game):
    legacy.fixture(game)
    path = game / "models/props/test.mdl"
    mdl = bytearray(path.read_bytes())
    require(len(mdl) == 1024 and struct.unpack_from("<3i", mdl, 4)[:2] == (48, 7)
            and struct.unpack_from("<2H", mdl, 388) == (0, 1), "Caster model header or skin family differs")
    struct.pack_into("<I", mdl, 152, struct.unpack_from("<I", mdl, 152)[0] | REQUEST)
    path.write_bytes(mdl)
    vvd_path = game / "models/props/test.vvd"
    vvd = bytearray(vvd_path.read_bytes())
    require(len(vvd) == 256 and struct.unpack_from("<4i", vvd, 4) == (4, 7, 1, 4), "Caster vertex roots differ")
    for index, (point, uv) in enumerate(zip(POINTS, UVS, strict=True)):
        require(struct.unpack_from("<3f", vvd, 64 + index * 48 + 16) == point, "Literal caster triangle position differs")
        struct.pack_into("<2f", vvd, 64 + index * 48 + 40, *uv)
    vvd_path.write_bytes(vvd)
    vtx = (game / "models/props/test.dx90.vtx").read_bytes()
    require(len(vtx) == 173 and struct.unpack_from("<i", vtx, 16)[0] == 7
            and struct.unpack_from("<i", vtx, 146)[0] == 3 and struct.unpack_from("<3H", vtx, 134) == (0, 1, 2)
            and [struct.unpack_from("<H", vtx, 102 + index * 9)[0] for index in range(4)] == [0, 1, 2, 3],
            "Caster hardware roots or actual one-triangle draw strip differ")
    weights = (0.25, 0.25, 0.5)
    require([sum(weight * point[axis] for weight, point in zip(weights, POINTS, strict=True)) for axis in range(3)] == [0, 0, 0]
            and [sum(weight * uv[axis] for weight, uv in zip(weights, UVS, strict=True)) for axis in range(2)] == [0.125, 0.625],
            "Known vertical center ray no longer has its literal interior barycentrics and UV")


def material(transform, mode="fractional"):
    field = "$alphatest 1" if mode == "cutout" else "$translucent 1"
    return f'VertexLitGeneric {{ {field} $basetexture test $basetexturetransform "{transform}" }}\n'


def declaration(row):
    x, y = row["texel"]
    require(ALPHAS[y][x] == row["alpha8"], "Declared texel identity and literal alpha byte disagree")
    return {**row, "rgbexp32": ENCODED[row["remaining"]]}


def reference_report():
    cases = {case["name"]: declaration(case) for case in CASES}
    counterfactuals = []
    for row in COUNTERFACTUALS:
        wrong = declaration(row)
        require(all(wrong["rgbexp32"][channel] != cases[row["case"]]["rgbexp32"][channel] for channel in ("ldr", "hdr")),
                f"Literal encoded profile cannot distinguish {row['error']}")
        counterfactuals.append(wrong)
    return {"version": 1, "cases": cases, "counterfactuals": counterfactuals, "alpha8_rows": ALPHAS,
            "native_contract": "T(C+T) Rz(A) S T(-C); positive UV rotation has rows cos,-sin;sin,cos",
            "native_evidence": {"materialsystem_so_sha256": "2ac4db05ea8ccc0ce63627a37c6a0551b2f9d21faf0cfd6c7128bf3f39622928", "elf_parser": "0x350bb-0x351c0", "record": "aidocs/673_secondary_normal_compiler_contract.md"},
            "ray": {"floor_center": [0, 0, 0], "light": [0, 0, 100], "caster_origin": [0, 0, 50], "local_points": POINTS,
                    "authored_uvs": UVS, "interior_barycentrics": [0.25, 0.25, 0.5], "interpolated_uv": [0.125, 0.625]},
            "quantization": "Literal RGBExp32 bytes for red255.125 LDR and blue510.25 HDR times remaining/255, independently truncated; no tolerance or compiler encoder",
            "boundary_policy": "All repeated coordinates lie strictly inside texels; noncardinal coordinate intervals are predeclared. Clamp inputs are strictly beyond the texture edge. No native float boundary parity claim.",
            "scope": "One actual alpha triangle and one center luxel per LDR/HDR channel through public foreground/background compilation; consumer propagation and old zero-rotation BSP preservation use separate gates."}


def private_preferences(client, output):
    stores = {name: client.call(name) for name in PREFERENCE_TOOLS}
    files = {}
    for directory in ("config", "state", "recent", "prefabs"):
        files[directory] = identities(output / directory)
    for path in [output / "recipes.json", *(output / "recovery" / name for name in PREFERENCE_FILES)]:
        files[str(path.relative_to(output))] = reference.digest(path.read_bytes()) if path.exists() else None
    return {"stores": stores, "files": files}


def snapshot(client, output, source, ids, game):
    return {"state": protected_state(client), "history": client.call("get_history"), "objects": detail(client, ids),
            "source_sha256": reference.digest(source.read_bytes()), "assets": identities(game),
            "settings": client.call("get_compile_settings"), "recipes": client.call("get_build_recipes"),
            "preferences": private_preferences(client, output)}


def inspect(data, case, prediction):
    measured = {}
    for channel in ("ldr", "hdr"):
        floor = native_center(data, channel)
        require(set(floor["styles"]) == {"0"} and len(floor["styles"]["0"]) == 1, "Unexpected style or directional maps")
        actual = floor["styles"]["0"][0]
        require(actual["raw"] == prediction["rgbexp32"][channel],
                f"{case['name']} {channel} differs from the predeclared texel/alpha/encoded sample: {actual} versus {prediction}")
        discriminators = []
        for wrong in COUNTERFACTUALS:
            if wrong["case"] != case["name"]:
                continue
            wrong_raw = ENCODED[wrong["remaining"]][channel]
            require(actual["raw"] != wrong_raw, f"Native bytes failed to distinguish {wrong['error']}")
            discriminators.append({"error": wrong["error"], "wrong_raw": wrong_raw, "actual_raw": actual["raw"]})
        measured[channel] = {"floor": floor, "encoded_counterfactuals": discriminators}
    return measured


def compile_pair(client, args, source, ids, game, case, prediction):
    before = snapshot(client, args.output, source, ids, game)
    shutil.copytree(game, args.output / "inputs" / case["name"])
    path = args.output / f"{case['name']}.bsp"
    result = client.call("compile_map", {"path": str(path)})
    require(result["success"] and result["leak"] is None, result)
    stats = result["stats"]
    require(stats["prop_shadow_props"] == stats["prop_shadow_models"] == stats["prop_shadow_triangles"] == 1
            and stats["prop_shadow_alpha_triangles"] == stats["prop_shadow_alpha_materials"] == stats["prop_shadow_alpha_textures"] == 1
            and stats["prop_shadow_alpha_bytes"] == 16 and stats["prop_shadow_disabled"] == 0
            and stats["prop_shadow_work"] > 0 and stats["lighting_total_work"] > 0,
            "Requested real one-triangle alpha preparation or charged traversal scope differs")
    data = probe.read_bounded(path, reference.MAX_BSP_BYTES)
    measured = inspect(data, case, prediction)
    background = args.output / f"{case['name']}-background.bsp"
    started = client.call("start_compile", {"path": str(background)})["job"]
    job = wait_job(client)
    require(job["token"] == started["token"] and job["status"] == "completed" and job["result"]["published"]
            and not job["stale"] and probe.read_bounded(background, reference.MAX_BSP_BYTES) == data,
            "Foreground/background native BSP bytes or publication differ")
    require(snapshot(client, args.output, source, ids, game) == before, "Compile changed VMF/history/objects/assets/selection/camera/preferences/settings/recipes")
    return path, {"sha256": reference.digest(data), "stats": stats, "channels": measured, "inputs": before["assets"],
                  "protected_state_unchanged": True, "foreground_background_identical": True}


def rejected_pairs(client, args, source, ids, game, output, caster):
    path = game / "materials/models/test/two.vmt"
    original, previous = path.read_bytes(), probe.read_bounded(output, reference.MAX_BSP_BYTES)
    failures = {}
    try:
        for name, transform, fragment in REFUSALS:
            path.write_text(material(transform))
            before = snapshot(client, args.output, source, ids, game)
            shutil.copytree(game, args.output / "inputs" / name)
            failed = json.loads(client.call("compile_map", {"path": str(output)}, error=True)["content"][0]["text"])
            require(not failed["success"] and output.read_bytes() == previous
                    and any(row["object"] == caster and fragment in row["message"] for row in failed["diagnostics"]),
                    f"{name}: malformed rotation did not fail atomically on the actual caster")
            started = client.call("start_compile", {"path": str(output)})["job"]
            job = wait_job(client)
            require(job["token"] == started["token"] and job["status"] == "failed" and not job["result"]["published"]
                    and output.read_bytes() == previous
                    and any(row["object"] == caster and fragment in row["message"] for row in job["result"]["diagnostics"]),
                    f"{name}: failed background compilation replaced the prior BSP or lost the caster diagnostic")
            require(snapshot(client, args.output, source, ids, game) == before, f"{name}: rejected compile changed protected state")
            failures[name] = {"foreground": failed, "background": job, "prior_bsp_sha256": reference.digest(previous), "protected_state_unchanged": True}
    finally:
        path.write_bytes(original)
    return failures


def validate(args):
    args.output.mkdir(parents=True, exist_ok=False)
    game = args.output / "game"
    fixtures(game)
    declared = reference_report()
    write_json(args.output / "reference.json", declared)
    with args.binary.open("rb") as binary_file:
        binary_sha256 = hashlib.file_digest(binary_file, "sha256").hexdigest()
    environment = {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
                   "HAMMERR_PREFABS_DIR": str(args.output / "prefabs"), "XDG_CONFIG_HOME": str(args.output / "config"), "XDG_STATE_HOME": str(args.output / "state")}
    with patch.dict(os.environ, environment):
        client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    results = {"binary": str(args.binary), "binary_sha256": binary_sha256, "cases": {}}
    try:
        with stopped_on_exit(client):
            client.call("new_map", {"discard_changes": True})
            ids = [client.call("create_block", {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"})["id"] for low, high in ROOM]
            ids.append(client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-64, 0, 8]})["id"])
            ids.append(client.call("create_entity", {"classname": "light", "origin": [0, 0, 100], "properties": {
                "_light": "255 0 0 255.125", "_lightHDR": "0 0 255 510.25", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}})["id"])
            caster = client.call("create_entity", {"classname": "prop_static", "origin": [0, 0, 50], "properties": {
                "model": "models/props/test.mdl", "skin": "1", "solid": "0", "disablevertexlighting": "1"}})["id"]
            ids.append(caster)
            client.call("set_compile_settings", {"lighting": "direct_ldr_hdr", "physics": "none", "bounces": 0, "sky_quality": "fast", "bake_static_prop_vertices": False})
            client.call("select", {"ids": [caster]})
            client.call("set_camera", {"projection": "perspective", "position": [160, -160, 96], "look_at": [0, 0, 50]})
            for case in CASES:
                (game / "materials/models/test/two.vmt").write_text(material(case["transform"], case.get("mode", "fractional")))
                (game / "materials/test.vtf").write_bytes(alpha_texture(case.get("flags", 0)))
                source = args.output / f"{case['name']}.vmf"
                client.call("save_map", {"path": str(source)})
                path, report = compile_pair(client, args, source, ids, game, case, declared["cases"][case["name"]])
                results["cases"][case["name"]] = report
                if case["name"] == "identity-fractional":
                    results["atomic_refusals"] = rejected_pairs(client, args, source, ids, game, path, caster)
                write_json(args.output / "results.json", results)
            require(not client.call("get_map_info")["rendering"]["initialized"], "Headless transform checks initialized graphics")
        results["clean_eof_exit_code"] = require_clean_eof(client)
        write_json(args.output / "results.json", results)
    finally:
        client.close()
    print(json.dumps({"cases": len(CASES), "atomic_refusals": len(REFUSALS), "dual_channel": True, "literal_texel_and_encoded_oracle": True, "output": str(args.output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate requested static-prop rotated alpha shadows through public MCP compilation and literal native-format predictions.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(), args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target"), "Keep retained artifacts on disk under target/")
    validate(args)


if __name__ == "__main__":
    main()
