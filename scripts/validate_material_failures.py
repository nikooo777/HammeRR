import argparse
import json
import struct
from pathlib import Path

from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM


def fixture(output):
    game = output / "game"
    materials = game / "materials/audit"
    materials.mkdir(parents=True)
    texture = bytearray(84)
    texture[:4] = b"VTF\0"
    for offset, value in [(4, 7), (8, 2), (12, 80)]:
        struct.pack_into("<I", texture, offset, value)
    for offset in [16, 18, 24, 63]:
        struct.pack_into("<H", texture, offset, 1)
    texture[56] = 1
    struct.pack_into("<i", texture, 57, -1)
    texture[80:] = bytes([200, 200, 200, 255])
    (materials / "good.vtf").write_bytes(texture)
    (materials / "broken.vtf").write_bytes(b"not a texture")
    (materials / "good.vmt").write_text("LightmappedGeneric { $basetexture audit/good }")
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text('GameInfo { FileSystem { SearchPaths { Game "|gameinfo_path|." } } }\n')
    return gameinfo, materials


def validate(client, output, materials):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    brushes = [client.call("create_block", {"mins": low, "maxs": high, "material": "audit/good"})["id"] for low, high in ROOM]
    client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-64, 0, 8]})
    client.call("create_entity", {"classname": "info_player_counterterrorist", "origin": [64, 0, 8]})
    client.call("set_compile_settings", {"lighting": "fullbright", "physics": "none", "visibility": "detailed"})
    baseline = output / "baseline.bsp"
    result = client.call("compile_map", {"path": str(baseline)})
    check(result["success"], "Valid assets produce the baseline BSP retained after failed builds")
    previous = baseline.read_bytes()
    for name, text, compile_error, reason in [
        ("syntax", "LightmappedGeneric { $basetexture }", True, "missing a value"),
        ("patch", "Patch { include materials/audit/absent.vmt }", True, "absent.vmt"),
        ("texture", "LightmappedGeneric { $basetexture audit/broken }", True, "broken.vtf"),
        ("editor_texture", "LightmappedGeneric { %tooltexture audit/broken $basetexture audit/good }", False, "broken.vtf"),
        ("blend", "WorldVertexTransition { $basetexture audit/good $basetexture2 audit/absent }", False, "absent.vtf"),
    ]:
        file = materials / f"{name}.vmt"
        file.write_text(text)
        client.call("set_material", {"ids": [brushes[0]], "material": f"audit/{name}"})
        source = output / f"{name}.vmf"
        client.call("save_map", {"path": str(source)})
        saved = source.read_bytes()
        before = detail(client, brushes)
        history = client.call("get_map_info")["undo"]
        error = client.call("preview_material", {"material": f"audit/{name}"}, error=True)
        check(reason in error["content"][0]["text"], f"{name}: material preview reports the actual asset failure")
        client.call("run_map_checks")
        issues = client.call("get_map_checks", {"query": "invalid_material", "limit": 100})["issues"]
        check(len(issues) == 6 and all(issue["object"] == brushes[0] and issue["face"] for issue in issues), f"{name}: shared map checks identify every affected face")
        target = output / f"{name}.bsp"
        target.write_bytes(previous)
        response = client.call("compile_map", {"path": str(target)}, error=compile_error)
        result = json.loads(response["content"][0]["text"]) if compile_error else response
        check(result["success"] != compile_error, f"{name}: compiler validates assets used by game geometry")
        if compile_error:
            check(target.read_bytes() == previous and any(item["level"] == "error" for item in result["diagnostics"]), f"{name}: failed synchronous build preserves the previous BSP")
            client.call("start_compile", {"path": str(target)})
            job = wait_job(client)
            check(job["status"] == "failed" and target.read_bytes() == previous, f"{name}: background build shares errors and output preservation")
        check(source.read_bytes() == saved and detail(client, brushes) == before and client.call("get_map_info")["undo"] == history, f"{name}: previews, checks and builds preserve source/history")
        file.write_text("LightmappedGeneric { $basetexture audit/good }")
        check(not client.call("preview_material", {"material": f"audit/{name}"})["missing"], f"{name}: repaired uncached asset previews correctly")
        client.call("run_map_checks")
        check(not client.call("get_map_checks", {"query": "invalid_material"})["issues"], f"{name}: repaired asset clears the check errors")
        check(client.call("compile_map", {"path": str(target)})["success"], f"{name}: repaired asset compiles without substituting a default")
    report = {"passed": len(checks), "checks": checks, "desktop_input": False}
    (output / "validation.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    gameinfo, materials = fixture(args.output)
    client = Client(None, args.binary.resolve(), args.output, gameinfo=gameinfo)
    try:
        validate(client, args.output, materials)
    finally:
        client.close()


if __name__ == "__main__":
    main()
