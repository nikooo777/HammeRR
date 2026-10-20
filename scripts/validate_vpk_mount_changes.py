import argparse
import json
import os
import struct
import zlib
from pathlib import Path

from validate_compile_jobs import stopped_on_exit, wait_job
from validate_face_lighting import DEFAULT_BINARY, ROOT
from validate_material_albedo import require
from validate_material_read_limits import (
    after_compile_refresh,
    configure,
    drafts,
    face_preview,
    fingerprint,
    fingerprints,
    fixture,
    read_bsp,
    snapshot,
)
from validate_recipe_coordination import open_process
from validate_recovery_exit import require_clean_eof

HELPERS = (
    "validate_compile_jobs.py", "validate_face_lighting.py", "validate_material_albedo.py",
    "validate_material_read_limits.py", "validate_recipe_coordination.py", "validate_recovery_exit.py",
)
MATERIAL = b"LightmappedGeneric { %compilenodraw 1 }\n"
TOKENS = ("VPK", "index", "changed", "reload mounted content")


def assets(game):
    game.mkdir()
    archive = game / "fixture_dir.vpk"
    tree = b"vmt\0materials/hammerr693\0probe\0"
    tree += struct.pack("<IHHIIH", zlib.crc32(MATERIAL), 0, 0x7FFF, 0, len(MATERIAL), 0xFFFF)
    tree += b"\0\0\0"
    archive.write_bytes(struct.pack("<III", 0x55AA1234, 1, len(tree)) + tree + MATERIAL)
    (game / "gameinfo.txt").write_text(
        'GameInfo { FileSystem { SearchPaths { Game "|gameinfo_path|fixture_dir.vpk" } } }\n')
    return archive


def identity(path):
    value = path.stat()
    return {"device": value.st_dev, "inode": value.st_ino, "bytes": value.st_size}


def failure_diagnostic(result):
    return any(row["level"] == "error" and all(token.lower() in row["message"].lower() for token in TOKENS)
               for row in result["diagnostics"])


def compile_pair(client, output, name, ids, source, game, report, prior=None):
    paths = [output / f"{name}{suffix}.bsp" for suffix in ("", "-background")]
    report.update({"status": "failed", "paths": [str(path) for path in paths], "stale_mount": prior is not None})
    lin = b"11 13 17\n19 23 29\n"
    for path in paths:
        path.with_suffix(".lin").write_bytes(lin)
        if prior is not None:
            path.write_bytes(prior)
    with stopped_on_exit(client):
        face_preview(client, ids)
        before = snapshot(client, ids, source, game)
        expected = after_compile_refresh(before)
        report["protected_before"] = before
        response = client.rpc("tools/call", {"name": "compile_map", "arguments": {"path": str(paths[0])}})
        report["foreground_response"] = response
        content = next(row["text"] for row in response["content"] if row["type"] == "text")
        result = json.loads(content)
        if prior is None:
            require(not response.get("isError") and result["success"] and result["leak"] is None,
                    f"Valid mounted VPK foreground compile failed: {result}")
        else:
            require(response.get("isError") and not result["success"] and result["bsp"] is None
                    and failure_diagnostic(result),
                    f"Stale VPK foreground compile did not explicitly refuse the changed mounted index: {result}")
            require(read_bsp(paths[0]) == prior, "Stale foreground compile replaced the prior BSP")
        report["foreground_after"] = snapshot(client, ids, source, game)
        require(report["foreground_after"] == expected, "Foreground compile changed protected state/assets")
        face_preview(client, ids)
        require(after_compile_refresh(snapshot(client, ids, source, game)) == expected,
                "Restaging the background preview changed protected state")
        started = client.call("start_compile", {"path": str(paths[1])})["job"]
        report["background_started"] = started
        job = wait_job(client)
        report["background"] = job
        require(job["token"] == started["token"], "Background compile token changed")
        if prior is None:
            require(job["status"] == "completed" and job["result"]["success"]
                    and job["result"]["published"] and not job["stale"],
                    f"Fresh mounted VPK background compile failed: {job}")
        else:
            require(job["status"] == "failed" and not job["result"]["published"]
                    and failure_diagnostic(job["result"]),
                    f"Stale VPK background compile did not explicitly refuse the changed mounted index: {job}")
            require(read_bsp(paths[1]) == prior, "Stale background compile replaced the prior BSP")
        report["background_after"] = snapshot(client, ids, source, game)
        require(report["background_after"] == expected, "Background compile changed protected state/assets")
        data = read_bsp(paths[0])
        require(data[:4] == b"VBSP" and read_bsp(paths[1]) == data
                and all(path.with_suffix(".lin").read_bytes() == lin for path in paths),
                "Paired compile changed BSP agreement or retained pointfiles")
        report["fingerprints"] = [fingerprint(path) for path in paths]
        report["status"] = "accepted"
        return data


def validate(args):
    clients = []
    result = {"status": "failed", "binary": str(args.binary), "binary_fingerprint": fingerprint(args.binary),
              "script_fingerprint": fingerprint(Path(__file__)),
              "helper_fingerprints": {name: fingerprint(Path(__file__).parent / name) for name in HELPERS},
              "graphics": False, "native": False, "game": False,
              "before": {}, "stale": {}, "fresh": {}, "clean_eof": [], "cleanup_errors": []}
    try:
        game = args.output / "game"
        archive = assets(game)
        asset_baseline = fingerprints(game)
        client = open_process(args.binary, args.output / "mounted", args.output / "mounted-recipes.json", game, clients)
        source = args.output / "original.vmf"
        ids = fixture(client, source)
        selected = client.call("get_material_source", {"material": "hammerr693/probe"})
        result["initial_material_source"] = selected
        require(selected["error"] is None and selected["raw_source"] == MATERIAL.decode("ascii")
                and selected["files"][0]["origin"]["kind"] == "vpk"
                and selected["files"][0]["origin"]["mount"] == str(archive)
                and {str(path.relative_to(game)) for path in game.rglob("*") if path.is_file()}
                == {"gameinfo.txt", "fixture_dir.vpk"},
                "Initial selected nodraw material must come solely from the private VPK")
        original_source = fingerprint(source)
        baseline = compile_pair(client, args.output, "before", ids, source, game, result["before"])
        original_identity = identity(archive)
        replacement = archive.with_suffix(".replacement")
        replacement.write_bytes(archive.read_bytes())
        require(identity(replacement)["inode"] != original_identity["inode"], "Replacement reused the mounted index inode")
        os.replace(replacement, archive)
        result["replacement"] = {"before": original_identity, "after": identity(archive), "fingerprint": fingerprint(archive)}
        require(identity(archive) != original_identity and fingerprints(game) == asset_baseline,
                "Index replacement must change identity while retaining exactly the same asset bytes")
        compile_pair(client, args.output, "stale", ids, source, game, result["stale"], baseline)
        result["clean_eof"].append(require_clean_eof(client))
        fresh = open_process(args.binary, args.output / "remounted", args.output / "remounted-recipes.json", game, clients)
        fresh.call("open_map", {"path": str(source), "discard_changes": True})
        fresh_ids = [row["id"] for row in fresh.call("find_objects", {"limit": 100})["objects"]]
        configure(fresh)
        fresh.call("save_build_recipe", {"name": "Material source limits"})
        drafts(fresh, fresh_ids)
        data = compile_pair(fresh, args.output, "fresh", fresh_ids, source, game, result["fresh"])
        require(data == baseline, "Fresh mount of the identical index/source changed whole BSP bytes")
        result["clean_eof"].append(require_clean_eof(fresh))
        require(result["clean_eof"] == [0, 0] and fingerprints(game) == asset_baseline
                and fingerprint(source) == original_source, "EOF changed source/assets or did not drain cleanly")
        result["counts"] = {"valid_bsps": 4, "preserved_failure_bsps": 2, "processes": 2, "clean_eof": 2}
        if args.reference_output is not None:
            reference = json.loads((args.reference_output / "results.json").read_text())
            require(reference["status"] == "accepted" and reference["counts"] == result["counts"]
                    and reference["script_fingerprint"] == result["script_fingerprint"]
                    and reference["helper_fingerprints"] == result["helper_fingerprints"]
                    and reference["binary_fingerprint"]["sha256"] != result["binary_fingerprint"]["sha256"],
                    "Reference is not the same accepted frozen oracle from another program")
            for name in ("before", "stale", "fresh"):
                require(reference[name]["status"] == "accepted", "Reference lacks an accepted compile pair")
                for prior, current, expected in zip(reference[name]["paths"], result[name]["paths"], reference[name]["fingerprints"], strict=True):
                    require(fingerprint(Path(prior)) == expected and read_bsp(Path(prior)) == read_bsp(Path(current)),
                            "Programs disagree on valid or preserved failure BSP bytes")
            result["cross_program_six_bsp_bytes_equal"] = True
        result["status"] = "accepted"
    except Exception as error:
        result["status"], result["error"] = "failed", f"{type(error).__name__}: {error}"
    finally:
        for client in clients:
            if client.process is not None:
                try:
                    result["clean_eof"].append(require_clean_eof(client))
                except Exception as error:
                    result["cleanup_errors"].append(f"{type(error).__name__}: {error}")
            elif client.log is not None and not client.log.closed:
                client.log.close()
        for name in ("mounted", "remounted"):
            log = args.output / name / "editor.log"
            if log.exists():
                if log.stat().st_size > 1024 * 1024 or "panic" in log.read_text(errors="replace").lower():
                    result["cleanup_errors"].append(f"Unexpected stderr size or panic: {log}")
        if result["cleanup_errors"]:
            result["status"] = "failed"
        (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    require(result["status"] == "accepted", result.get("error", result["cleanup_errors"]))
    return result


def main():
    parser = argparse.ArgumentParser(description="Validate stale VPK mounted-index rejection through private foreground/background MCP compilation.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--reference-output", type=Path)
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(), args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Keep private artifacts under target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    result = validate(args)
    print(json.dumps({"status": result["status"], **result["counts"], "output": str(args.output)}))


if __name__ == "__main__":
    main()
