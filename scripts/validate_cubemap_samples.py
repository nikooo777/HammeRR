import argparse
import json
from pathlib import Path

from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import ROOT, Client, detail


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    client.call("new_map", {"discard_changes": True})
    for mins, maxs in [
        ([-272, -272, -16], [272, 272, 0]),
        ([-272, -272, 256], [272, 272, 272]),
        ([-272, -272, 0], [-256, 272, 256]),
        ([256, -272, 0], [272, 272, 256]),
        ([-256, -272, 0], [256, -256, 256]),
        ([-256, 256, 0], [256, 272, 256]),
    ]:
        client.call("create_block", {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"})
    client.call("create_entity", {"classname": "info_player_terrorist", "origin": [0, 0, 8]})
    ids = []
    origins = []
    for size in range(10):
        origin = [-128.875 + size * 24, -0.875 + size, 64.875]
        origins.append(origin)
        created = client.call("create_entity", {"classname": "env_cubemap", "origin": origin})
        ids.append(created["id"])
        client.call("set_entity_properties", {"id": ids[-1], "set": {"cubemapsize": str(size)}})
    client.call("set_entity_properties", {"id": ids[0], "set": {"sides": "3 4"}})
    vmf = output / "samples.vmf"
    client.call("save_map", {"path": str(vmf)})
    client.call("open_map", {"path": str(vmf)})
    source = vmf.read_bytes()
    before = detail(client, ids)
    history = client.call("get_map_info")["undo"]
    bsp = output / "samples.bsp"
    compiled = client.call("compile_map", {"path": str(bsp)})
    check(compiled["success"] and compiled["stats"]["cubemap_samples"] == 10, "Synchronous MCP compile exposes ten cubemap samples")
    check(detail(client, ids) == before and history == client.call("get_map_info")["undo"] and vmf.read_bytes() == source, "Compilation preserves source helpers, file and history")
    data = bsp.read_bytes()
    samples = records(data, 42, "<3iB3s")
    check(len(samples) == 10, "Native sample lump uses sixteen-byte records")
    for size, (sample, origin) in enumerate(zip(samples, origins, strict=True)):
        check(sample[:3] == tuple(int(value) for value in origin), f"Size {size}: fractional coordinates truncate toward zero")
        check(sample[3] == size and sample[4] == b"\0\0\0", f"Size {size}: byte enum and zero padding are emitted")
    entities = records(data, 0, "<c")
    check(b"env_cubemap" not in b"".join(record[0] for record in entities), "Compile-time helpers are consumed instead of becoming runtime entities")
    check(any(diagnostic["object"] == ids[0] and "sides list is unused" in diagnostic["message"] for diagnostic in compiled["diagnostics"]), "Unsupported manual face binding has an object diagnostic")
    check(any("texture capture" in diagnostic["message"] for diagnostic in compiled["diagnostics"]), "Build diagnostics explain remaining reflection capture work")
    background = output / "background.bsp"
    client.call("start_compile", {"path": str(background)})
    job = wait_job(client)
    check(job["status"] == "completed" and job["result"]["stats"]["cubemap_samples"] == 10, "Background Session/MCP compile exposes the same sample count")
    check(background.read_bytes() == data, "Synchronous and background compiles emit identical records")
    check(client.call("get_compile_status")["launch"]["ready"], "Sample builds retain ordinary launch readiness")
    client.call("set_object_visibility", {"ids": [ids[9]], "action": "hide"})
    hidden = output / "hidden.bsp"
    compiled = client.call("compile_map", {"path": str(hidden)})
    check(compiled["stats"]["cubemap_samples"] == 9 and len(records(hidden.read_bytes(), 42, "<3iB3s")) == 9, "Shared visibility omits hidden samples")
    client.call("undo")
    duplicate = client.call("create_entity", {"classname": "env_cubemap", "origin": origins[0], "properties": {"cubemapsize": "7"}})["id"]
    duplicate_bsp = output / "duplicate.bsp"
    compiled = client.call("compile_map", {"path": str(duplicate_bsp)})
    duplicated = records(duplicate_bsp.read_bytes(), 42, "<3iB3s")
    check(compiled["stats"]["cubemap_samples"] == 11 and duplicated[-1][:3] == duplicated[0][:3] and duplicated[-1][3] == 7, "Source-compatible duplicate positions retain independent samples")
    client.call("delete_objects", {"ids": [duplicate]})
    for size in ["-1", "10", "256", "1.5", "NaN"]:
        invalid = output / f"invalid-{size}.vmf"
        invalid.write_text(source.decode().replace('"cubemapsize" "9"', f'"cubemapsize" "{size}"'))
        client.call("open_map", {"path": str(invalid), "discard_changes": True})
        previous = output / f"invalid-{size}.bsp"
        previous.write_bytes(b"previous successful build")
        failed = client.call("compile_map", {"path": str(previous)}, error=True)
        value = json.loads(failed["content"][0]["text"])
        check(any(diagnostic["level"] == "error" and diagnostic["object"] == ids[9] and "Cubemap Size" in diagnostic["message"] for diagnostic in value["diagnostics"]), f"Invalid size {size}: compiler returns an object diagnostic")
        check(previous.read_bytes() == b"previous successful build", f"Invalid size {size}: rejected output preserves the previous build")
        client.call("set_object_visibility", {"ids": [ids[9]], "action": "hide"})
        recovered = client.call("compile_map", {"path": str(output / f"recovered-{size}.bsp")})
        check(recovered["success"] and recovered["stats"]["cubemap_samples"] == 9, f"Invalid size {size}: hiding the helper restores successful compilation")
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-cubemap-samples"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output)
        (args.output / "results.json").write_text(json.dumps({"checks": len(checks), "passed": checks}, indent=2) + "\n")
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
