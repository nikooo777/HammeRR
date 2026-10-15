import argparse
import json
from pathlib import Path
from unittest.mock import patch

from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client
from validate_occluders import room


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    room(client)
    original = output / "room.vmf"
    client.call("save_map", {"path": str(original)})
    baseline = output / "baseline.bsp"
    client.call("compile_map", {"path": str(baseline)})
    full_map = baseline.read_bytes()
    source = original.read_text()
    cases = [
        ("legacy-incomplete", 'cordon { "active" "1" "mins" "(-64 -64 0)" }', True),
        ("modern-empty", 'cordons { "active" "true" }', True),
        ("ambiguous", 'cordon { "active" "0" "active" "1" }', True),
        ("disabled", 'cordons { "active" "false" cordon { "active" "true" } }', False),
    ]
    for name, metadata, rejected in cases:
        vmf = output / f"{name}.vmf"
        vmf.write_text(source + "\n" + metadata + "\n")
        client.call("open_map", {"path": str(vmf), "discard_changes": True})
        before = client.call("get_map_info")
        saved = vmf.read_bytes()
        target = output / f"{name}.bsp"
        target.write_bytes(full_map)
        response = client.call("compile_map", {"path": str(target)}, error=rejected)
        result = json.loads(response["content"][0]["text"]) if rejected else response
        if rejected:
            check(not result["success"] and any("cordon" in item["message"].lower() and item["level"] == "error" for item in result["diagnostics"]), f"{name}: synchronous build explains invalid cordons")
        else:
            check(result["success"], f"{name}: global disable overrides active child records")
        check(target.read_bytes() == full_map, f"{name}: synchronous build preserves full-map output")
        client.call("start_compile", {"path": str(target)})
        job = wait_job(client)
        check(job["status"] == ("failed" if rejected else "completed"), f"{name}: background build agrees with synchronous support")
        if rejected:
            check(any("cordon" in item["message"].lower() for item in job["result"]["diagnostics"]), f"{name}: background details retain the cordon diagnostic")
        check(target.read_bytes() == full_map, f"{name}: background build preserves output bytes")
        after = client.call("get_map_info")
        check(after["undo"] == before["undo"] and after["selection"] == before["selection"] and vmf.read_bytes() == saved, f"{name}: both build paths preserve source, selection and history")
    result = {"passed": len(checks), "checks": checks, "desktop_input": False}
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"passed": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    with patch.dict("os.environ", {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json")}):
        client = Client(None, args.binary.resolve(), args.output)
    try:
        validate(client, args.output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
