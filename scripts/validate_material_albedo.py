import argparse
import hashlib
import json
import math
import os
import struct
from pathlib import Path
from unittest.mock import patch

from validate_authored_planes import records
from validate_build_recipes import protected_state
from validate_bump_lighting import texture
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_direct_lighting import FACE_FORMAT
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail, face_ref
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM

MATERIAL = "hammerr/receiver"
BASE = "hammerr/base"
ALBEDO = "hammerr/albedo"
MISSING_NORMAL = "hammerr/missing-normal"
METRICS = {
    BASE: (4, 2, (0.0625, 0.5, 0.125), (32, 160, 64, 255)),
    ALBEDO: (2, 4, (0.75, 0.125, 0.0625), (192, 32, 16, 255)),
    None: (64, 64, (0.5, 0.5, 0.5), None),
}
FULL = ("fullbright", 0)
DIRECT = ("direct_ldr", 0)
BOUNCED = ("direct_ldr_hdr", 1)
POSITIVE = [
    {"name": "lmg-active", "shader": "LightmappedGeneric", "selected": ALBEDO, "modes": [FULL, DIRECT, BOUNCED]},
    {"name": "wvt-active", "shader": "WorldVertexTransition", "selected": ALBEDO},
    {"name": "two-texture-active", "shader": "WorldTwoTextureBlend", "selected": ALBEDO},
    {"name": "trimmed-zero", "suppression": " 0 ", "selected": ALBEDO},
    {"name": "trimmed-one", "shader": "WorldVertexTransition", "suppression": " 1 ", "selected": BASE},
    {"name": "missing-albedo-legacy-tool", "albedo": None, "extra": {"%tooltexture": "hammerr/tool"}, "selected": BASE},
    {"name": "missing-bump", "bump": None, "selected": BASE},
    {"name": "missing-base", "base": None, "selected": None, "modes": [FULL]},
    {"name": "unknown-shader", "shader": "UnknownShader", "suppression": "2", "selected": BASE},
    {"name": "suppressed-unused-blanks", "bump": "", "albedo": "", "suppression": "1", "selected": BASE, "modes": [FULL]},
    {"name": "unused-corrupt-base", "base": "hammerr/corrupt", "selected": ALBEDO},
]
NEGATIVE = [
    {"name": "missing-lmg", "shader": "LightmappedGeneric", "albedo": "hammerr/missing-albedo", "error": "hammerr/missing-albedo", "mode": FULL},
    {"name": "missing-wvt", "shader": "WorldVertexTransition", "albedo": "hammerr/missing-albedo", "error": "hammerr/missing-albedo"},
    {"name": "missing-two-texture", "shader": "WorldTwoTextureBlend", "albedo": "hammerr/missing-albedo", "error": "hammerr/missing-albedo"},
    {"name": "blank-albedo", "albedo": " ", "error": "$albedo", "mode": FULL},
    {"name": "corrupt-albedo", "albedo": "hammerr/corrupt", "error": "hammerr/corrupt", "mode": DIRECT},
    {"name": "nonfinite-albedo", "albedo": "hammerr/nonfinite", "error": "hammerr/nonfinite"},
    {"name": "unsupported-suppression", "suppression": "2", "error": "$nodiffusebumplighting", "mode": FULL},
    {"name": "tooltexture-presence", "shader": "WorldVertexTransition", "extra": {"%tooltexture": ""}, "error": "%tooltexture"},
    {"name": "notooltexture-presence", "shader": "WorldTwoTextureBlend", "extra": {"%notooltexture": "0"}, "error": "%notooltexture", "mode": FULL},
    {"name": "blank-required-bump", "bump": "", "error": "$bumpmap", "mode": FULL},
    {"name": "blank-required-base", "base": "", "error": "$basetexture", "mode": FULL},
    {"name": "reflectivity-presence", "extra": {"$reflectivity": "[0.1 0.2 0.3]"}, "error": "$reflectivity", "mode": FULL},
]


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def identities(directory):
    return {str(path.relative_to(directory)): {"bytes": path.stat().st_size, "sha256": digest(path.read_bytes())}
            for path in sorted(directory.rglob("*")) if path.is_file()}


def payload(data, index):
    require(data[:4] == b"VBSP" and len(data) >= 1036, "Invalid BSP framing")
    offset, size, _version, compression = struct.unpack_from("<4i", data, 8 + index * 16)
    require(compression == 0 and 0 <= offset <= offset + size <= len(data), "Invalid BSP lump range")
    return data[offset:offset + size]


def texture_metrics(data, selected):
    offsets = records(data, 44, "<i")
    strings = payload(data, 43)
    found = []
    for row in records(data, 2, "<3f5i"):
        require(0 <= row[3] < len(offsets), "Invalid native material name index")
        start = offsets[row[3]][0]
        require(0 <= start < len(strings), "Invalid native material string offset")
        end = strings.find(b"\0", start)
        require(end >= start, "Unterminated native material name")
        if strings[start:end].decode("ascii").lower() == MATERIAL:
            found.append(row)
    require(len(found) == 1, "Fixture material has missing or ambiguous TexData")
    width, height, reflectivity, _pixels = METRICS[selected]
    require(found[0][:3] == reflectivity and found[0][4:] == (width, height, width, height),
            f"Selected texture dimensions/reflectivity differ: {found[0]} versus {METRICS[selected]}")
    return {"dimensions": [width, height], "reflectivity": list(found[0][:3])}


def floor_base(data, kind):
    face_lump, sample_lump = (7, 8) if kind == "ldr" else (58, 53)
    planes = records(data, 1, "<4fi")
    samples = payload(data, sample_lump)
    result = []
    for face in records(data, face_lump, FACE_FORMAT, expected_version=0 if kind == "ldr" else 1):
        plane = planes[face[0]]
        if plane[2] < 0.99 or abs(plane[3]) > 0.001:
            continue
        require(face[8:12] == (0, 255, 255, 255), "Unexpected fixture light style")
        count = (face[16] + 1) * (face[17] + 1)
        start, end = face[12], face[12] + count * 4
        require(count > 0 and 0 <= start < end <= len(samples), "Missing ordinary base samples")
        result.extend(tuple(math.ldexp(component, exponent) for component in rgb)
                      for *rgb, exponent in struct.iter_unpack("<3Bb", samples[start:end]))
    require(result and any(sum(row) > 0 for row in result), "Contrast fixture floor is absent or black")
    return result


def fixture_texture(width, height, rho, pixel):
    data = bytearray(texture(bytes(pixel) * width * height, size=width))
    struct.pack_into("<H", data, 18, height)
    struct.pack_into("<3f", data, 32, *rho)
    require(len(data) == 80 + width * height * 4, "Fixture VTF payload differs")
    require(struct.unpack_from("<2H", data, 16) == (width, height), "Fixture VTF dimensions differ")
    require(struct.unpack_from("<3f", data, 32) == rho and tuple(data[80:84]) == pixel, "Fixture header/pixels differ")
    return data


def assets(game):
    folder = game / "materials/hammerr"
    folder.mkdir(parents=True)
    (game / "gameinfo.txt").write_text('GameInfo { FileSystem { SearchPaths { Game "|gameinfo_path|." } } }\n')
    for path in [BASE, ALBEDO]:
        width, height, rho, pixel = METRICS[path]
        (game / f"materials/{path}.vtf").write_bytes(fixture_texture(width, height, rho, pixel))
    (folder / "tool.vtf").write_bytes(fixture_texture(8, 1, (0.875, 0.875, 0.875), (1, 2, 3, 255)))
    (folder / "corrupt.vtf").write_bytes(b"VTF\0")
    nonfinite = fixture_texture(2, 4, METRICS[ALBEDO][2], METRICS[ALBEDO][3])
    struct.pack_into("<f", nonfinite, 32, math.nan)
    (folder / "nonfinite.vtf").write_bytes(nonfinite)
    require(not (game / f"materials/{MISSING_NORMAL}.vtf").exists(), "Normal asset must stay absent")


def material(game, case, reference=False, raw=False):
    base = case.get("base", BASE)
    if reference:
        base = case["selected"]
    if raw:
        base = BASE
    values = {"$basetexture": base, "$bumpmap": case.get("bump", MISSING_NORMAL),
              "$albedo": None if reference or raw else case.get("albedo", ALBEDO),
              "$nodiffusebumplighting": case.get("suppression"), **case.get("extra", {})}
    fields = " ".join(f'{key} "{value}"' for key, value in values.items() if value is not None)
    (game / "materials/hammerr/receiver.vmt").write_text(f'{case.get("shader", "LightmappedGeneric")} {{ {fields} }}\n')


def open_client(binary, output, game):
    output.mkdir()
    environment = {"HAMMERR_RECENT_MAPS_DIR": str(output / "recent"), "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"),
                   "HAMMERR_PREFABS_DIR": str(output / "prefabs"), "XDG_CONFIG_HOME": str(output / "config"), "XDG_STATE_HOME": str(output / "state")}
    with patch.dict(os.environ, environment):
        return Client(None, binary, output, game / "gameinfo.txt")


def fixture(client, source):
    ids = []
    for low, high in ROOM:
        block = client.call("create_block", {"mins": low, "maxs": high, "material": MATERIAL})["id"]
        ids.append(block)
        shape = detail(client, [block])[0]
        client.call("edit_faces", {"faces": [face_ref(shape, face) for face in shape["faces"]], "lightmap_scale": 64})
    for classname, origin, properties in [
        ("info_player_terrorist", [-128, -128, 8], {}),
        ("info_player_counterterrorist", [128, -128, 8], {}),
        ("light", [0, 0, 200], {"_light": "255 255 255 64", "_lightHDR": "128 255 64 96", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}),
    ]:
        ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
    client.call("select", {"ids": [ids[0]]})
    client.call("save_map", {"path": str(source)})
    return ids


def state(client, ids, source, game):
    require(not client.call("get_map_info")["rendering"]["initialized"], "Validation initialized graphics")
    recipes = client.call("get_build_recipes")
    recipe_path = Path(recipes["path"])
    return {"map": protected_state(client), "history": client.call("get_history"), "objects": detail(client, ids),
            "settings": client.call("get_compile_settings"), "recipes": recipes, "recipe_bytes": digest(recipe_path.read_bytes()),
            "source": digest(source.read_bytes()), "assets": identities(game)}


def settings(client, mode):
    lighting, bounces = mode
    client.call("set_compile_settings", {"lighting": lighting, "physics": "none", "bounces": bounces, "sky_quality": "fast",
                                         "visibility": "fast", "bake_static_prop_vertices": False, "generate_reflections": False, "pack_sky_assets": False})


def compile_pair(client, output, label, ids, source, game, error_token=None, retained=None):
    before = state(client, ids, source, game)
    target = output / f"{label}.bsp"
    if retained is not None:
        target.write_bytes(retained)
    response = client.call("compile_map", {"path": str(target)}, error=error_token is not None)
    result = json.loads(response["content"][0]["text"]) if error_token is not None else response
    if error_token is not None:
        require(not result["success"] and result["bsp"] is None, f"{label}: invalid material succeeded")
        require(any(row["level"] == "error" and error_token in row["message"] for row in result["diagnostics"]), f"{label}: missing material error: {result}")
        require(target.read_bytes() == retained, f"{label}: refusal replaced prior BSP")
    else:
        require(result["success"] and result["leak"] is None, f"{label}: compile failed: {result}")
    require(state(client, ids, source, game) == before, f"{label}: foreground changed protected state")
    foreground = target.read_bytes()
    background = target.with_name(target.stem + "-background.bsp")
    if retained is not None:
        background.write_bytes(retained)
    started = client.call("start_compile", {"path": str(background)})["job"]
    job = wait_job(client)
    require(job["token"] == started["token"], f"{label}: background token changed")
    if error_token is not None:
        require(job["status"] == "failed" and not job["result"]["published"]
                and any(row["level"] == "error" and error_token in row["message"] for row in job["result"]["diagnostics"]), f"{label}: background refusal missing")
    else:
        require(job["status"] == "completed" and job["result"]["success"] and job["result"]["published"] and not job["stale"], f"{label}: background failed")
    require(background.read_bytes() == foreground, f"{label}: foreground/background BSP differs")
    require(state(client, ids, source, game) == before, f"{label}: background changed protected state")
    return foreground, {"bsp_sha256": digest(foreground), "result": result, "background": job, "preserved": True,
                        "inputs": before["assets"], "settings": before["settings"], "recipe_bytes": before["recipe_bytes"]}


def validate(args):
    clients = {}
    games = {}
    results = {"status": "failed", "positive": {}, "negative": {}, "contrast": {},
               "binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()),
               "script_sha256": digest(Path(__file__).read_bytes())}
    source = args.output / "source.vmf"
    try:
        for role in ["actual", "reference", "raw-base"]:
            game = args.output / f"{role}-game"
            assets(game)
            material(game, POSITIVE[0], reference=role == "reference", raw=role == "raw-base")
            games[role] = game
            clients[role] = open_client(args.binary, args.output / f"{role}-client", game)
        ids = fixture(clients["actual"], source)
        saved_source = source.read_bytes()
        for role, client in clients.items():
            if role != "actual":
                client.call("open_map", {"path": str(source)})
                client.call("select", {"ids": [ids[0]]})
            settings(client, BOUNCED)
            client.call("save_build_recipe", {"name": "Albedo fixture"})
        retained = None
        for case in POSITIVE:
            material(games["actual"], case)
            material(games["reference"], case, reference=True)
            for mode in case.get("modes", [BOUNCED]):
                label = f"{case['name']}-{mode[0]}-{mode[1]}"
                measured = {}
                data = {}
                for role in ["actual", "reference"]:
                    settings(clients[role], mode)
                    with stopped_on_exit(clients[role]):
                        data[role], measured[role] = compile_pair(clients[role], args.output, f"{label}-{role}", ids, source, games[role])
                    measured[role]["metrics"] = texture_metrics(data[role], case["selected"])
                require(data["actual"] == data["reference"], f"{label}: effective-base reference BSP differs")
                results["positive"][label] = measured
                if case["name"] == "lmg-active" and mode == BOUNCED:
                    retained = data["actual"]
                if case["name"] == "lmg-active" and mode in [DIRECT, BOUNCED]:
                    settings(clients["raw-base"], mode)
                    with stopped_on_exit(clients["raw-base"]):
                        raw, receipt = compile_pair(clients["raw-base"], args.output, f"{label}-raw-base", ids, source, games["raw-base"])
                    texture_metrics(raw, BASE)
                    differences = {}
                    for kind in (["ldr", "hdr"] if mode == BOUNCED else ["ldr"]):
                        selected = floor_base(data["actual"], kind)
                        original = floor_base(raw, kind)
                        require(len(selected) == len(original), "Contrast receiver counts differ")
                        if mode == DIRECT:
                            require(selected == original, "Header selection changed direct geometric illumination")
                            differences[kind] = {"direct_samples_exact": True}
                        else:
                            red = [left[0] - right[0] for left, right in zip(selected, original, strict=True)]
                            green = [right[1] - left[1] for left, right in zip(selected, original, strict=True)]
                            require(max(red) >= 1.0, "Albedo red reflectivity does not discriminate one bounce")
                            require(max(green) >= 0.5, "Original green reflectivity does not discriminate one bounce")
                            differences[kind] = {"max_red_increase": max(red), "max_green_decrease": max(green)}
                    results["contrast"][label] = {"receipt": receipt, "differences": differences}
        require(retained is not None, "Missing accepted prior BSP")
        for case in NEGATIVE:
            material(games["actual"], case)
            settings(clients["actual"], case.get("mode", BOUNCED))
            with stopped_on_exit(clients["actual"]):
                _, receipt = compile_pair(clients["actual"], args.output, case["name"], ids, source, games["actual"], case["error"], retained)
            results["negative"][case["name"]] = receipt
        for game in games.values():
            require(not (game / f"materials/{MISSING_NORMAL}.vtf").exists(), "Compile created a normal dependency asset")
        ending_assets = {role: identities(game) for role, game in games.items()}
        ending_recipes = {role: Path(client.call("get_build_recipes")["path"]) for role, client in clients.items()}
        recipe_hashes = {role: digest(path.read_bytes()) for role, path in ending_recipes.items()}
        results["clean_eof"] = {role: require_clean_eof(client) for role, client in clients.items()}
        require(source.read_bytes() == saved_source, "Validation or EOF changed saved source")
        require({role: identities(game) for role, game in games.items()} == ending_assets, "EOF changed material assets")
        require({role: digest(path.read_bytes()) for role, path in ending_recipes.items()} == recipe_hashes, "EOF changed saved recipes")
        results["source_sha256"] = digest(source.read_bytes())
        results["status"] = "accepted"
    finally:
        for client in clients.values():
            if client.process is not None:
                client.close()
        (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    return results


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=ROOT / "target/validation/material-albedo")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Retained acceptance artifacts must stay under target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    results = validate(args)
    print(json.dumps({"material_albedo": True, "positive": len(results["positive"]), "negative": len(results["negative"]), "output": str(args.output)}))


if __name__ == "__main__":
    main()
