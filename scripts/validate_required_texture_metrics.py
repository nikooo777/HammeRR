import argparse
import json
from pathlib import Path

from validate_compile_jobs import stopped_on_exit
from validate_material_albedo import (
    BASE,
    BOUNCED,
    DEFAULT_BINARY,
    FULL,
    MATERIAL,
    ROOT,
    assets,
    compile_pair,
    digest,
    fixture,
    identities,
    open_client,
    require,
    settings,
    texture_metrics,
)
from validate_recovery_exit import require_clean_eof

POSITIVE = [
    {"name": "valid-header", "base": BASE, "selected": BASE},
    {"name": "absent-base", "base": None, "selected": None},
    {"name": "absent-material", "absent_material": True, "selected": None},
]
NEGATIVE = [
    {"name": "required-ordinary", "base": "hammerr/required-missing", "error": "hammerr/required-missing", "modes": [FULL, BOUNCED]},
    {"name": "runtime-present-file", "base": "_rt_requiredmetrics", "error": "_rt_requiredmetrics", "modes": [FULL]},
    {"name": "missing-reflectivity-with-tool-dimensions", "base": "hammerr/required-missing", "extra": {"%tooltexture": "hammerr/tool"}, "error": "hammerr/required-missing"},
    {"name": "missing-reflectivity-with-notool-dimensions", "base": "hammerr/required-missing", "extra": {"%notooltexture": "0"}, "error": "hammerr/required-missing"},
    {"name": "missing-dimensions-with-authored-reflectivity", "base": "hammerr/required-missing", "extra": {"$reflectivity": "[0.1 0.2 0.3]"}, "error": "hammerr/required-missing"},
]


def material(game, case):
    path = game / f"materials/{MATERIAL}.vmt"
    if case.get("absent_material"):
        path.unlink(missing_ok=True)
        return
    values = {"$basetexture": case.get("base"), **case.get("extra", {})}
    fields = " ".join(f'{key} "{value}"' for key, value in values.items() if value is not None)
    path.write_text(f"LightmappedGeneric {{ {fields} }}\n")


def require_unavailable(receipt, case):
    for result in [receipt["result"], receipt["background"]["result"]]:
        messages = [row["message"] for row in result["diagnostics"] if row["level"] == "error"]
        require(any(case["error"] in message and "$basetexture" in message
                    and ("unavailable" in message or "unsupported" in message)
                    for message in messages), f"{case['name']}: missing unavailable metric reason: {messages}")
        if case["name"].startswith("runtime"):
            require(any("_rt_requiredmetrics" in message and "unsupported" in message for message in messages),
                    "Runtime input was reported as an ordinary missing file")


def validate(args):
    client = None
    game = args.output / "game"
    source = args.output / "source.vmf"
    results = {"status": "failed", "positive": {}, "negative": {},
               "binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()),
               "script_sha256": digest(Path(__file__).read_bytes()),
               "helpers_sha256": digest((ROOT / "scripts/validate_material_albedo.py").read_bytes())}
    try:
        assets(game)
        runtime = game / "materials/_rt_requiredmetrics.vtf"
        runtime.write_bytes((game / f"materials/{BASE}.vtf").read_bytes())
        require(not (game / "materials/hammerr/required-missing.vtf").exists(), "Required ordinary input must be absent")
        material(game, POSITIVE[0])
        client = open_client(args.binary, args.output / "client", game)
        ids = fixture(client, source)
        source_bytes = source.read_bytes()
        results["source_sha256"] = digest(source_bytes)
        settings(client, BOUNCED)
        client.call("save_build_recipe", {"name": "Required texture metrics fixture"})
        retained = None
        absent = {}
        for case in POSITIVE:
            material(game, case)
            for mode in [FULL, BOUNCED]:
                settings(client, mode)
                label = f"{case['name']}-{mode[0]}-{mode[1]}"
                with stopped_on_exit(client):
                    data, receipt = compile_pair(client, args.output, label, ids, source, game)
                receipt["metrics"] = texture_metrics(data, case["selected"])
                results["positive"][label] = receipt
                if case["name"] == "valid-header":
                    retained = data
                elif case["name"] == "absent-base":
                    absent[mode] = data
                else:
                    require(data == absent[mode], "Absent VMT and absent base optional defaults differ")
        require(retained is not None, "No genuine successful BSP was retained")
        for case in NEGATIVE:
            material(game, case)
            for mode in case.get("modes", [BOUNCED]):
                settings(client, mode)
                label = f"{case['name']}-{mode[0]}-{mode[1]}"
                with stopped_on_exit(client):
                    _, receipt = compile_pair(client, args.output, label, ids, source, game, case["error"], retained)
                require_unavailable(receipt, case)
                results["negative"][label] = receipt
        ending_assets = identities(game)
        recipes = Path(client.call("get_build_recipes")["path"])
        recipe_hash = digest(recipes.read_bytes())
        results["clean_eof"] = require_clean_eof(client)
        require(source.read_bytes() == source_bytes, "Compile/EOF rewrote source VMF")
        require(identities(game) == ending_assets, "EOF changed fixture assets")
        require(digest(recipes.read_bytes()) == recipe_hash, "EOF changed saved recipes")
        results["runtime_fixture_sha256"] = digest(runtime.read_bytes())
        results["status"] = "accepted"
    except Exception as error:
        results["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        try:
            if client is not None and client.process is not None:
                results["cleanup_eof"] = require_clean_eof(client)
        except Exception as error:
            results["cleanup_error"] = f"{type(error).__name__}: {error}"
            results["status"] = "failed"
            raise
        finally:
            (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    return results


def main():
    parser = argparse.ArgumentParser(description="Validate required texture metric availability through foreground/background MCP compilation without graphics or a game.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=ROOT / "target/validation/required-texture-metrics")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Artifacts must remain below target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    result = validate(args)
    print(json.dumps({"status": result["status"], "positive": len(result["positive"]), "negative": len(result["negative"])}))


if __name__ == "__main__":
    main()
