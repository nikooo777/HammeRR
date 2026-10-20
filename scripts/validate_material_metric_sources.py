import argparse
import json
import math
import struct
from pathlib import Path

from validate_authored_planes import records
from validate_compile_jobs import stopped_on_exit
from validate_material_albedo import (
    BOUNCED,
    DEFAULT_BINARY,
    DIRECT,
    FULL,
    MATERIAL,
    ROOT,
    assets,
    compile_pair,
    digest,
    fixture,
    fixture_texture,
    floor_base,
    identities,
    open_client,
    payload,
    require,
    settings,
)
from validate_recovery_exit import require_clean_eof

GREEN = (0.0, 0.5, 0.0)
BLUE = (0.0, 0.0, 0.5)
TOOL_RED = (0.75, 0.0, 0.0)
AUTHORED = (0.125, 0.25, 0.75)
NO_PREVIEW = "SDK no-preview dimensions (128x128) selected for "
TOOL_NAME_241 = "deep/" + "a" * 120 + "/" + "b" * 115
BASE_FIELDS = "$basetexture hammerr/base"
ACTIVE_FIELDS = BASE_FIELDS + " $bumpmap hammerr/missing-normal $albedo hammerr/albedo"
RUNTIME_FIELDS = "$basetexture _rt_unused $bumpmap hammerr/missing-normal $albedo _rt_unused"
RHO_FIELDS = '$reflectivity "[0.125 0.25 0.75]"'
POSITIVE = [
    {"name": "tool-name-241", "fields": BASE_FIELDS + " %tooltexture " + TOOL_NAME_241, "dimensions": (8, 1), "rho": GREEN, "modes": [FULL]},
    {"name": "explicit-vtf-nan-payload-name", "fields": BASE_FIELDS + " %tooltexture nan(0).vtf", "dimensions": (16, 16), "rho": GREEN, "modes": [FULL]},
    {"name": "no-preview-unused-too-long-tool", "fields": BASE_FIELDS + " %notooltexture 0 %tooltexture " + TOOL_NAME_241 + "b" * 10, "dimensions": (128, 128), "rho": GREEN, "no_preview": True, "modes": [FULL]},
    {"name": "no-key-base", "fields": BASE_FIELDS, "dimensions": (4, 2), "rho": GREEN, "modes": [FULL]},
    {"name": "no-key-albedo", "fields": ACTIVE_FIELDS, "dimensions": (2, 4), "rho": BLUE, "active": True, "modes": [BOUNCED]},
    {"name": "tool-base", "fields": BASE_FIELDS + " %tooltexture hammerr/tool", "dimensions": (8, 1), "rho": GREEN, "contrast_channel": 1, "modes": [DIRECT, BOUNCED]},
    {"name": "tool-albedo", "fields": ACTIVE_FIELDS + " %tooltexture hammerr/tool", "dimensions": (8, 1), "rho": BLUE, "active": True, "contrast_channel": 2, "modes": [BOUNCED]},
    {"name": "tool-authored-unused-runtime", "fields": RUNTIME_FIELDS + " %tooltexture hammerr/tool " + RHO_FIELDS, "dimensions": (8, 1), "rho": AUTHORED, "active": True, "modes": [BOUNCED]},
    {"name": "tool-optional-base-default-rho", "fields": "%tooltexture hammerr/tool", "dimensions": (8, 1), "rho": (0.5, 0.5, 0.5), "modes": [FULL]},
    {"name": "tool-unused-nonfinite-rho", "fields": BASE_FIELDS + " %tooltexture hammerr/tool-nonfinite", "dimensions": (8, 1), "rho": GREEN, "modes": [FULL]},
    {"name": "explicit-vtf-numeric-name", "fields": BASE_FIELDS + " %tooltexture 0x10.vtf", "dimensions": (16, 16), "rho": GREEN, "modes": [FULL]},
    {"name": "no-preview-zero-both-keys", "fields": BASE_FIELDS + " %notooltexture 0 %tooltexture hammerr/missing-tool", "dimensions": (128, 128), "rho": GREEN, "no_preview": True, "modes": [BOUNCED]},
    {"name": "no-preview-one-unused-invalid-tool", "fields": BASE_FIELDS + " %notooltexture 1 %tooltexture ../unused-invalid", "dimensions": (128, 128), "rho": GREEN, "no_preview": True, "modes": [FULL]},
    {"name": "no-preview-authored-unused-runtime", "fields": RUNTIME_FIELDS + " %notooltexture 0 %tooltexture hammerr/missing-tool " + RHO_FIELDS, "dimensions": (128, 128), "rho": AUTHORED, "active": True, "no_preview": True, "modes": [FULL]},
]
NEGATIVE = [
    {"name": "tool-name-242-present-file", "fields": BASE_FIELDS + " %tooltexture " + TOOL_NAME_241 + "b", "error": "%tooltexture", "unsupported": True, "tool_failure": True},
    {"name": "tool-name-251-present-file", "fields": BASE_FIELDS + " %tooltexture " + TOOL_NAME_241 + "b" * 10, "error": "%tooltexture", "unsupported": True, "tool_failure": True},
    {"name": "numeric-nan-payload-present-file", "fields": BASE_FIELDS + " %tooltexture nan(0)", "error": "%tooltexture", "unsupported": True, "tool_failure": True},
    {"name": "required-tool-missing", "fields": BASE_FIELDS + " %tooltexture hammerr/missing-tool", "error": "hammerr/missing-tool", "tool_failure": True},
    {"name": "authored-cannot-rescue-tool", "fields": BASE_FIELDS + " %tooltexture hammerr/missing-tool " + RHO_FIELDS, "error": "hammerr/missing-tool", "tool_failure": True},
    {"name": "authored-cannot-rescue-corrupt-tool", "fields": BASE_FIELDS + " %tooltexture hammerr/corrupt " + RHO_FIELDS, "error": "hammerr/corrupt", "tool_failure": True},
    {"name": "blank-tool", "fields": BASE_FIELDS + ' %tooltexture ""', "error": "%tooltexture", "tool_failure": True},
    {"name": "numeric-tool-present-file", "fields": BASE_FIELDS + " %tooltexture 0", "error": "%tooltexture", "unsupported": True, "tool_failure": True},
    {"name": "numeric-hex-tool-present-file", "fields": BASE_FIELDS + " %tooltexture 0x10", "error": "%tooltexture", "unsupported": True, "tool_failure": True},
    {"name": "required-runtime-tool", "fields": BASE_FIELDS + " %tooltexture _rt_provider " + RHO_FIELDS, "error": "_rt_provider", "unsupported": True, "tool_failure": True},
    {"name": "tool-cannot-rescue-missing-rho", "fields": "$basetexture hammerr/missing-rho %tooltexture hammerr/tool", "error": "hammerr/missing-rho", "mode": BOUNCED},
    {"name": "tool-cannot-rescue-missing-albedo", "fields": BASE_FIELDS + " $bumpmap hammerr/missing-normal $albedo hammerr/missing-albedo %tooltexture hammerr/tool", "error": "hammerr/missing-albedo"},
    {"name": "tool-cannot-rescue-nonfinite-rho", "fields": "$basetexture hammerr/nonfinite %tooltexture hammerr/tool", "error": "reflectivity must be finite"},
    {"name": "required-runtime-rho", "fields": "$basetexture _rt_provider %tooltexture hammerr/tool", "error": "_rt_provider", "unsupported": True},
    {"name": "empty-no-preview", "fields": BASE_FIELDS + ' %notooltexture ""', "error": "%notooltexture", "unsupported": True},
    {"name": "noncanonical-no-preview", "fields": BASE_FIELDS + " %notooltexture 2", "error": "%notooltexture", "unsupported": True},
    {"name": "no-preview-still-requires-rho", "fields": "$basetexture hammerr/missing-rho %notooltexture 0 %tooltexture hammerr/missing-tool", "error": "hammerr/missing-rho"},
]


def material(game, fields):
    (game / f"materials/{MATERIAL}.vmt").write_text(f"LightmappedGeneric {{ {fields} }}\n")


def measured_metrics(data, dimensions, rho):
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
    require(len(found) == 1, "Missing or ambiguous provider TexData")
    width, height = dimensions
    require(found[0][:3] == rho and found[0][4:] == (width, height, width, height),
            f"Literal provider dimensions/reflectivity differ: {found[0]} versus {dimensions}/{rho}")
    return {"dimensions": list(dimensions), "reflectivity": list(found[0][:3])}


def errors(receipt):
    return [[row["message"] for row in result["diagnostics"] if row["level"] == "error"]
            for result in [receipt["result"], receipt["background"]["result"]]]


def require_preview_diagnostic(receipt, expected):
    for result in [receipt["result"], receipt["background"]["result"]]:
        present = any(row["level"] == "info" and row["message"].startswith(NO_PREVIEW) for row in result["diagnostics"])
        require(present == expected, "Named no-preview diagnostic does not match provider selection")


def contrast(actual, wrong, case, mode):
    result = {}
    for kind in (["ldr", "hdr"] if mode == BOUNCED else ["ldr"]):
        selected = floor_base(actual, kind)
        counterfactual = floor_base(wrong, kind)
        require(len(selected) == len(counterfactual), "Provider contrast receiver counts differ")
        if mode == DIRECT:
            require(selected == counterfactual, "Tool rho changed direct geometric illumination")
            result[kind] = {"direct_samples_exact": True}
        else:
            channel = case["contrast_channel"]
            diffuse = max(left[channel] - right[channel] for left, right in zip(selected, counterfactual, strict=True))
            red = max(right[0] - left[0] for left, right in zip(selected, counterfactual, strict=True))
            require(diffuse > 0.01 and red > 0.01, "Tool rho is not distinguished in actual one-bounce samples")
            result[kind] = {"max_diffuse_channel_increase": diffuse, "max_wrong_red_decrease": red}
    return result


def prepare_assets(game):
    assets(game)
    for name, width, height, rho in [
        ("hammerr/base", 4, 2, GREEN),
        ("hammerr/albedo", 2, 4, BLUE),
        ("hammerr/tool", 8, 1, TOOL_RED),
        ("_rt_provider", 16, 16, TOOL_RED),
        ("0", 16, 16, TOOL_RED),
        ("0x10", 16, 16, TOOL_RED),
        ("nan(0)", 16, 16, TOOL_RED),
    ]:
        (game / f"materials/{name}.vtf").write_bytes(fixture_texture(width, height, rho, (192, 32, 16, 255)))
    for name, width, height in [("tool-nonfinite", 8, 1), ("nonfinite", 4, 2)]:
        data = fixture_texture(width, height, TOOL_RED, (192, 32, 16, 255))
        struct.pack_into("<f", data, 32, math.nan)
        (game / f"materials/hammerr/{name}.vtf").write_bytes(data)
    for name in [TOOL_NAME_241, TOOL_NAME_241 + "b", TOOL_NAME_241 + "b" * 10]:
        target = game / f"materials/{name}.vtf"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(fixture_texture(8, 1, TOOL_RED, (192, 32, 16, 255)))
    (game / "materials/_rt_unused.vtf").write_bytes(b"Unused runtime poison; no metric provider may read this")


def validate(args):
    clients = {}
    games = {}
    source = args.output / "source.vmf"
    results = {"status": "failed", "positive": {}, "negative": {}, "contrast": {},
               "binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()),
               "script_sha256": digest(Path(__file__).read_bytes()),
               "helpers_sha256": digest((ROOT / "scripts/validate_material_albedo.py").read_bytes())}
    try:
        for role in ["actual", "reference", "wrong-tool-rho"]:
            game = args.output / f"{role}-game"
            prepare_assets(game)
            material(game, BASE_FIELDS)
            games[role] = game
            clients[role] = open_client(args.binary, args.output / f"{role}-client", game)
        ids = fixture(clients["actual"], source)
        saved_source = source.read_bytes()
        results["source_sha256"] = digest(saved_source)
        for role, client in clients.items():
            if role != "actual":
                client.call("open_map", {"path": str(source)})
                client.call("select", {"ids": [ids[0]]})
            settings(client, BOUNCED)
            client.call("save_build_recipe", {"name": "Independent metric providers fixture"})
        retained = None
        for case in POSITIVE:
            material(games["actual"], case["fields"])
            reference_fields = "$basetexture hammerr/reference" + (" $bumpmap hammerr/missing-normal" if case.get("active") else "")
            for role, rho in [("reference", case["rho"]), ("wrong-tool-rho", TOOL_RED)]:
                width, height = case["dimensions"]
                (games[role] / "materials/hammerr/reference.vtf").write_bytes(fixture_texture(width, height, rho, (192, 32, 16, 255)))
                material(games[role], reference_fields)
            roles = ["actual", "reference", "wrong-tool-rho"] if "contrast_channel" in case else ["actual", "reference"]
            for mode in case["modes"]:
                label = f"{case['name']}-{mode[0]}-{mode[1]}"
                data = {}
                receipts = {}
                for role in roles:
                    client = clients[role]
                    settings(client, mode)
                    with stopped_on_exit(client):
                        data[role], receipts[role] = compile_pair(client, args.output, f"{label}-{role}", ids, source, games[role])
                    expected_rho = TOOL_RED if role == "wrong-tool-rho" else case["rho"]
                    receipts[role]["metrics"] = measured_metrics(data[role], case["dimensions"], expected_rho)
                    require_preview_diagnostic(receipts[role], role == "actual" and case.get("no_preview", False))
                require(data["actual"] == data["reference"], f"{label}: equivalent-header reference BSP differs")
                results["positive"][label] = receipts
                if "contrast_channel" in case:
                    results["contrast"][label] = contrast(data["actual"], data["wrong-tool-rho"], case, mode)
                if retained is None:
                    retained = data["actual"]
        require(retained is not None, "No genuine successful prior BSP")
        for case in NEGATIVE:
            material(games["actual"], case["fields"])
            settings(clients["actual"], case.get("mode", FULL))
            with stopped_on_exit(clients["actual"]):
                _, receipt = compile_pair(clients["actual"], args.output, case["name"], ids, source, games["actual"], case["error"], retained)
            if case.get("unsupported"):
                require(all(any(case["error"] in message and "unsupported" in message.lower() for message in messages) for messages in errors(receipt)),
                        f"{case['name']}: unsupported provider was reported as a failed ordinary file")
            if case.get("tool_failure"):
                require_preview_diagnostic(receipt, False)
            results["negative"][case["name"]] = receipt
        ending_assets = {role: identities(game) for role, game in games.items()}
        recipes = {role: Path(client.call("get_build_recipes")["path"]) for role, client in clients.items()}
        recipe_hashes = {role: digest(path.read_bytes()) for role, path in recipes.items()}
        results["clean_eof"] = {role: require_clean_eof(client) for role, client in clients.items()}
        require(source.read_bytes() == saved_source, "Compile/EOF changed source VMF")
        require({role: identities(game) for role, game in games.items()} == ending_assets, "EOF changed metric provider assets")
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
    parser = argparse.ArgumentParser(description="Validate independent material metric providers through blocking/background MCP compilation without graphics or a game.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=ROOT / "target/validation/material-metric-sources")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Artifacts must remain below target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    result = validate(args)
    print(json.dumps({"status": result["status"], "positive": len(result["positive"]), "negative": len(result["negative"])}))


if __name__ == "__main__":
    main()
