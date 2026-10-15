import argparse
import json
import re
from pathlib import Path

from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_material_failures import fixture
from validate_visibility import ROOM


def render_issues(client, path):
    image, metadata = client.render_result(width=640, height=400,
        camera={"position": [210, -220, 170], "look_at": [64, 0, 48]})
    path.write_bytes(image)
    return metadata["preview_issues"]


def validate(client, output, override, render):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    for low, high in ROOM:
        client.call("create_block", {"mins": low, "maxs": high, "material": "audit/good"})
    client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-64, 0, 8]})
    client.call("create_entity", {"classname": "info_player_counterterrorist", "origin": [64, 0, 8]})
    client.call("set_compile_settings", {"lighting": "fullbright", "physics": "none"})
    bsp = output / "retained.bsp"
    check(client.call("compile_map", {"path": str(bsp)})["success"], "Healthy mounted content compiles")
    previous = bsp.read_bytes()
    blocked = override / "materials"
    blocked.symlink_to("materials")
    for tool, arguments in [
        ("search_materials", {"query": "audit"}),
        ("browse_materials", {"query": "audit"}),
        ("preview_material", {"material": "audit/good"}),
    ]:
        error = client.call(tool, arguments, error=True)
        check("materials" in error["content"][0]["text"], f"{tool} exposes the failed first mount instead of borrowing the second")
    client.call("run_map_checks")
    report = client.call("get_map_checks", {"limit": 200})["issues"]
    check(any(issue["code"] == "asset_scan_failed" and issue["severity"] == "error" for issue in report), "Map checks report discovery failures")
    check(not any(issue["code"] in ["missing_material", "missing_model"] for issue in report), "Failed discovery does not invent missing assets")
    client.call("compile_map", {"path": str(bsp)}, error=True)
    check(bsp.read_bytes() == previous, "A failed first mount cannot publish a build using a lower-priority material")
    client.call("start_compile", {"path": str(bsp)})
    check(wait_job(client)["status"] == "failed" and bsp.read_bytes() == previous, "Background builds share read failures and retain the BSP")
    blocked.unlink()
    check(client.call("search_materials", {"query": "audit"})["materials"] == ["audit/good"], "Genuinely absent higher-priority content permits the configured next mount")
    check(client.call("browse_materials", {"query": "audit"})["total"] == 1, "Material catalog retries failed discovery after repair")
    check(not client.call("preview_material", {"material": "audit/good"})["missing"], "Repaired content previews without restarting")
    for folder, tool in [("models", "browse_models"), ("sound", "browse_sounds")]:
        blocked = override / folder
        blocked.symlink_to(folder)
        error = client.call(tool, {"query": "audit"}, error=True)
        check(folder in error["content"][0]["text"], f"{tool} reports the failing content directory")
        blocked.unlink()
        check(client.call(tool, {"query": "audit"})["total"] == 1, f"{tool} retries after repair without caching an empty catalog")
    patch = client.call("create_block", {"mins": [0, -64, 32], "maxs": [128, 64, 48], "material": "audit/good"})["id"]
    shape = detail(client, [patch])[0]
    top = next(face for face in shape["faces"] if face["normal"][2] > 0.99)
    target = face_ref(shape, top)
    client.call("select_faces", {"faces": [target]})
    client.call("create_terrain", {"power": 2})
    draft = client.call("preview_terrain_smoothing", {"angle": 45.5})
    client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})
    healthy = output / "healthy.vmf"
    client.call("save_map", {"path": str(healthy)})
    original = healthy.read_text()
    unpaired, replacements = re.subn(r'\bnormals\s*\{[^{}]*\}', "", original)
    assert replacements == 1
    nonfinite, replacements = re.subn(r'("elevation"\s*)"[^"]*"', r'\g<1>"NaN"', original)
    assert replacements == 1 and original.count('"smooth" "45.5"') == 1
    if render:
        check(not render_issues(client, output / "healthy.png"), "Healthy terrain has no preview diagnostics")
    for name, text, code in [
        ("smoothing", original.replace('"smooth" "45.5"', '"smooth" "broken"'), "displacement_smoothing"),
        ("unpaired", unpaired, "invalid_displacement"),
        ("nonfinite", nonfinite, "invalid_displacement"),
    ]:
        path = output / f"{name}.vmf"
        path.write_text(text)
        client.call("open_map", {"path": str(path)})
        history = client.call("get_map_info")["undo"]
        saved = path.read_bytes()
        client.call("run_map_checks")
        issues = client.call("get_map_checks", {"query": code})["issues"]
        check(any(issue["face"] == target and issue["severity"] == "error" for issue in issues), f"{name}: invalid imported terrain has a face-linked check error")
        if render:
            issues = render_issues(client, output / f"{name}.png")
            check(len(issues) == 1 and issues[0]["face"] == target, f"{name}: rendering reports the omitted face instead of fabricating shading")
            check(render_issues(client, output / f"{name}-cached.png") == issues, f"{name}: cached render retains the same diagnostics")
        client.call("compile_map", {"path": str(bsp)}, error=True)
        check(bsp.read_bytes() == previous, f"{name}: failed compilation retains the previous BSP")
        check(path.read_bytes() == saved and client.call("get_map_info")["undo"] == history, f"{name}: checking, rendering and compilation preserve the imported source/history")
        client.call("open_map", {"path": str(healthy)})
        if render:
            check(not render_issues(client, output / f"{name}-restored.png"), f"{name}: restored source clears the cached render error")
    result = {"passed": len(checks), "checks": checks, "renders": render, "desktop_input": False}
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"passed": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--render", action="store_true")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True)
    gameinfo, _ = fixture(output)
    game = gameinfo.parent
    override = output / "override"
    override.mkdir()
    gameinfo.write_text(f'GameInfo {{ FileSystem {{ SearchPaths {{ Game "{override}" Game "|gameinfo_path|." }} }} }}\n')
    for folder, name in [("models", "audit.mdl"), ("sound", "audit.wav")]:
        (game / folder).mkdir()
        (game / folder / name).write_bytes(b"catalog entry only")
    client = Client(None, args.binary.resolve(), output, gameinfo=gameinfo)
    try:
        validate(client, output, override, args.render)
    finally:
        client.close()


if __name__ == "__main__":
    main()
