import argparse
import json
import math
import struct
from pathlib import Path

from validate_authored_planes import records
from validate_compile_jobs import stopped_on_exit
from validate_material_albedo import (
    ALBEDO,
    BASE,
    BOUNCED,
    DEFAULT_BINARY,
    DIRECT,
    FULL,
    MATERIAL,
    METRICS,
    ROOT,
    assets,
    compile_pair,
    digest,
    fixture,
    fixture_texture,
    floor_base,
    identities,
    material,
    open_client,
    payload,
    require,
    settings,
)
from validate_recovery_exit import require_clean_eof

AUTHORED = (0.5, 0.0, 0.0)
HEADER = {BASE: (0.0, 0.5, 0.0), ALBEDO: (0.0, 0.0, 0.5)}
POSITIVE = [
    {"name": "raw-base", "base": BASE, "albedo": None, "bump": None, "selected": BASE, "modes": [FULL, BOUNCED]},
    {"name": "active-albedo", "base": BASE, "albedo": ALBEDO, "selected": ALBEDO, "modes": [DIRECT, BOUNCED]},
    {"name": "unused-nonfinite-header", "base": "hammerr/nonfinite", "albedo": None, "bump": None, "selected": BASE, "modes": [FULL], "unused_header": True},
]
NEGATIVE = [
    {"name": "runtime-albedo-present-file", "albedo": "_rt_albedo", "bump": "hammerr/missing-normal", "error": "_rt_albedo"},
    {"name": "scalar", "rho": "0.5", "error": "$reflectivity"},
    {"name": "brace", "rho": "{0.5 0 0}", "error": "$reflectivity"},
    {"name": "malformed", "rho": "[red 0 0]", "error": "$reflectivity"},
    {"name": "arity", "rho": "[0.5 0]", "error": "$reflectivity"},
    {"name": "nonfinite", "rho": "[NaN 0 0]", "error": "$reflectivity"},
    {"name": "range", "rho": "[1.1 0 0]", "error": "$reflectivity"},
    {"name": "nonfinite-header-without-override", "base": "hammerr/nonfinite", "rho": None, "error": "hammerr/nonfinite"},
    {"name": "absent-dimensions", "base": None, "error": "$reflectivity"},
    {"name": "missing-dimensions", "base": "hammerr/missing-dimensions", "error": "hammerr/missing-dimensions"},
    {"name": "missing-albedo-dimensions", "albedo": "hammerr/missing-albedo", "bump": "hammerr/missing-normal", "error": "hammerr/missing-albedo"},
    {"name": "missing-required-tool-dimensions", "extra": {"%tooltexture": "hammerr/missing-tool"}, "error": "hammerr/missing-tool"},
    {"name": "corrupt-required-tool-dimensions", "albedo": ALBEDO, "bump": "hammerr/missing-normal", "extra": {"%tooltexture": "hammerr/corrupt"}, "error": "hammerr/corrupt"},
]


def measured_metrics(data, selected, reflectivity):
    offsets = records(data, 44, "<i")
    strings = payload(data, 43)
    found = []
    for row in records(data, 2, "<3f5i"):
        require(0 <= row[3] < len(offsets), "Invalid material name index")
        start = offsets[row[3]][0]
        require(0 <= start < len(strings), "Invalid material name offset")
        end = strings.find(b"\0", start)
        require(end >= start, "Unterminated material name")
        if strings[start:end].decode("ascii").lower() == MATERIAL:
            found.append(row)
    require(len(found) == 1, "Missing or ambiguous reflectivity TexData")
    width, height = {BASE: (4, 2), ALBEDO: (2, 4)}[selected]
    require(found[0][:3] == reflectivity and found[0][4:] == (width, height, width, height),
            f"Literal TexData dimensions/reflectivity differ: {found[0]}")
    return {"dimensions": [width, height], "reflectivity": list(found[0][:3])}


def contrast(actual, header, case, mode):
    differences = {}
    for kind in (["ldr", "hdr"] if mode == BOUNCED else ["ldr"]):
        selected = floor_base(actual, kind)
        original = floor_base(header, kind)
        require(len(selected) == len(original), "Contrast receiver counts differ")
        if mode == DIRECT:
            require(selected == original, "Reflectivity override changed direct illumination")
            differences[kind] = {"direct_samples_exact": True}
        else:
            old_channel = 1 if case["selected"] == BASE else 2
            red = max(left[0] - right[0] for left, right in zip(selected, original, strict=True))
            old = max(right[old_channel] - left[old_channel] for left, right in zip(selected, original, strict=True))
            require(red > 0.01 and old > 0.01, "Authored reflectivity does not discriminate actual one bounce")
            differences[kind] = {"max_red_increase": red, "max_header_channel_decrease": old}
    return differences


def validate(args):
    clients = {}
    games = {}
    source = args.output / "source.vmf"
    results = {"status": "failed", "positive": {}, "negative": {}, "contrast": {},
               "binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()),
               "script_sha256": digest(Path(__file__).read_bytes()),
               "helpers_sha256": digest((ROOT / "scripts/validate_material_albedo.py").read_bytes())}
    try:
        for role in ["actual", "reference", "header"]:
            game = args.output / f"{role}-game"
            assets(game)
            for selected in [BASE, ALBEDO]:
                width, height, _rho, pixel = METRICS[selected]
                rho = AUTHORED if role == "reference" else HEADER[selected]
                (game / f"materials/{selected}.vtf").write_bytes(fixture_texture(width, height, rho, pixel))
            (game / "materials/_rt_albedo.vtf").write_bytes((game / f"materials/{BASE}.vtf").read_bytes())
            nonfinite = fixture_texture(4, 2, HEADER[BASE], METRICS[BASE][3])
            struct.pack_into("<f", nonfinite, 32, math.nan)
            (game / "materials/hammerr/nonfinite.vtf").write_bytes(nonfinite)
            material(game, {**POSITIVE[0], "extra": {"$reflectivity": "[0.5 0 0]"} if role == "actual" else {}})
            games[role] = game
            clients[role] = open_client(args.binary, args.output / f"{role}-client", game)
        definitions = clients["actual"].rpc("tools/list", {})["tools"]
        by_name = {definition["name"]: definition for definition in definitions}
        for name in ["get_compile_settings", "set_compile_settings", "compile_map"]:
            declaration = by_name[name]["description"]
            require("authored material reflectivity" in declaration, f"{name}: authored reflectivity missing from discovery")
            require("diffuse or tool VTF headers" in declaration, f"{name}: independent dimension headers missing from discovery")
            require("SDK no-preview 128x128 convention" in declaration and "%notooltexture 0 or 1" in declaration,
                    f"{name}: admitted no-preview convention missing from discovery")
            require("measured TextureHeader" not in declaration, f"{name}: stale header-only discovery")
            require("selected VTF dimensions" not in declaration and "tool-preview combinations reject" not in declaration,
                    f"{name}: stale coupled-provider discovery")
        flag_schema = by_name["set_compile_settings"]["inputSchema"]["properties"]["bake_static_prop_vertices"]
        require("diffuse or tool VTF headers" in flag_schema["description"]
                and "SDK no-preview 128x128 convention" in flag_schema["description"]
                and "%notooltexture 0 or 1" in flag_schema["description"],
                "Derived static-prop boolean schema omits independent dimension providers")
        results["tool_discovery"] = {name: by_name[name] for name in ["get_compile_settings", "set_compile_settings", "compile_map"]}
        results["discovery"] = {}
        for role, client in clients.items():
            limits = client.call("get_compile_settings")["prop_vertex_lighting"]["limits"]
            require(limits["indirect_reflectivity"] == "texture_header_or_authored_material", "Authored reflectivity absent from shared discovery")
            require(limits["indirect_dimensions"] == "header_or_native_no_preview", "Independent dimensions absent from shared discovery")
            require(limits["indirect_dimension_sources"] == ["diffuse_vtf_header", "tool_vtf_header", "native_no_preview_convention"],
                    "Independent dimension source kinds differ from shared discovery")
            results["discovery"][role] = {key: limits[key] for key in ["indirect_reflectivity", "indirect_dimensions", "indirect_dimension_sources"]}
        ids = fixture(clients["actual"], source)
        saved_source = source.read_bytes()
        results["source_sha256"] = digest(saved_source)
        for role, client in clients.items():
            if role != "actual":
                client.call("open_map", {"path": str(source)})
                client.call("select", {"ids": [ids[0]]})
            settings(client, BOUNCED)
            client.call("save_build_recipe", {"name": "Authored reflectivity fixture"})
        retained = None
        for case in POSITIVE:
            material(games["actual"], {**case, "extra": {"$reflectivity": "[0.5 0 0]"}})
            material(games["reference"], case, reference=True)
            material(games["header"], case)
            for mode in case["modes"]:
                label = f"{case['name']}-{mode[0]}-{mode[1]}"
                data = {}
                receipts = {}
                roles = ["actual", "reference"] if case.get("unused_header") else clients
                for role in roles:
                    client = clients[role]
                    settings(client, mode)
                    with stopped_on_exit(client):
                        data[role], receipts[role] = compile_pair(client, args.output, f"{label}-{role}", ids, source, games[role])
                    expected = HEADER[case["selected"]] if role == "header" else AUTHORED
                    receipts[role]["metrics"] = measured_metrics(data[role], case["selected"], expected)
                require(data["actual"] == data["reference"], f"{label}: authored/header-only reference BSP differs")
                results["positive"][label] = receipts
                if mode != FULL:
                    results["contrast"][label] = contrast(data["actual"], data["header"], case, mode)
                retained = data["actual"]
        require(retained is not None, "No genuine successful prior BSP")
        for case in NEGATIVE:
            values = {"base": BASE, "albedo": None, "bump": None, **case,
                      "extra": {"$reflectivity": case.get("rho", "[0.5 0 0]"), **case.get("extra", {})}}
            material(games["actual"], values)
            mode = BOUNCED if case["name"] in ["missing-dimensions", "missing-albedo-dimensions"] else FULL
            settings(clients["actual"], mode)
            with stopped_on_exit(clients["actual"]):
                _, receipt = compile_pair(clients["actual"], args.output, case["name"], ids, source, games["actual"], case["error"], retained)
            results["negative"][case["name"]] = receipt
        ending_assets = {role: identities(game) for role, game in games.items()}
        recipes = {role: Path(client.call("get_build_recipes")["path"]) for role, client in clients.items()}
        recipe_hashes = {role: digest(path.read_bytes()) for role, path in recipes.items()}
        results["clean_eof"] = {role: require_clean_eof(client) for role, client in clients.items()}
        require(source.read_bytes() == saved_source, "Compile/EOF changed source VMF")
        require({role: identities(game) for role, game in games.items()} == ending_assets, "EOF changed fixture assets")
        require({role: digest(path.read_bytes()) for role, path in recipes.items()} == recipe_hashes, "EOF changed saved recipes")
        results["status"] = "accepted"
    except Exception as error:
        results["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        cleanup_errors = {}
        for role, client in clients.items():
            if client.process is not None:
                try:
                    results.setdefault("cleanup_eof", {})[role] = require_clean_eof(client)
                except Exception as error:
                    cleanup_errors[role] = f"{type(error).__name__}: {error}"
        if cleanup_errors:
            results["cleanup_error"] = cleanup_errors
            results["status"] = "failed"
        (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        if cleanup_errors:
            raise RuntimeError(f"MCP EOF cleanup failed: {cleanup_errors}")
    return results


def main():
    parser = argparse.ArgumentParser(description="Validate authored reflectivity metrics and one-bounce contrast through both MCP compile paths without graphics or a game.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=ROOT / "target/validation/material-reflectivity")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Artifacts must remain below target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    result = validate(args)
    print(json.dumps({"status": result["status"], "positive": len(result["positive"]), "negative": len(result["negative"])}))


if __name__ == "__main__":
    main()
