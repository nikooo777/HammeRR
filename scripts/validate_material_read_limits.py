import argparse
import hashlib
import json
import os
from copy import deepcopy
from pathlib import Path
from unittest.mock import patch

from validate_authored_planes import records
from validate_build_recipes import protected_state
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_face_lighting import DEFAULT_BINARY, ROOT, detail, face_ref
from validate_material_albedo import (
    digest,
    fixture_texture,
    open_client,
    payload,
    require,
    settings,
)
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM

MATERIAL = "hammerr693/probe"
VMT = "materials/hammerr693/probe.vmt"
BASE = "hammerr693/base"
TOOL = "hammerr693/tool"
LARGE = "hammerr693/large"
NORMAL = f"LightmappedGeneric {{ $basetexture {BASE} }}\n"
RHO = (0.125, 0.25, 0.5)
OLD_BINARY_SHA = "08e6860dc4216817e13a455890828be6ad55ceb1fc27ba68e18d0ef94c3abf86"
HASH_CHUNK_BYTES = 65536
MAX_FIXTURE_BSP_BYTES = 16 * 1024 * 1024


def fingerprint(path):
    before = path.stat()
    hasher = hashlib.sha256()
    count = 0
    with path.open("rb") as source:
        while block := source.read(HASH_CHUNK_BYTES):
            hasher.update(block)
            count += len(block)
    after = path.stat()
    require(before.st_size == count == after.st_size and before.st_mtime_ns == after.st_mtime_ns,
            f"Input changed during streamed fingerprint: {path}")
    return {"bytes": count, "sha256": hasher.hexdigest()}


def fingerprints(directory):
    return {str(path.relative_to(directory)): fingerprint(path) for path in sorted(directory.rglob("*")) if path.is_file()}


def commented(size):
    prefix = NORMAL.encode() + b"//"
    return prefix + b"x" * (size - len(prefix) - 1) + b"\n"


def assets(game):
    folder = game / "materials/hammerr693"
    folder.mkdir(parents=True)
    (game / "gameinfo.txt").write_text('GameInfo { FileSystem { SearchPaths { Game "|gameinfo_path|." } } }\n')
    (folder / "base.vtf").write_bytes(fixture_texture(2, 3, RHO, (32, 96, 160, 255)))
    (folder / "tool.vtf").write_bytes(fixture_texture(4, 1, (0.875, 0.875, 0.875), (255, 32, 16, 255)))
    (game / VMT).write_text(NORMAL)


def configure(client, tracked=False):
    settings(client, ("direct_ldr" if tracked else "fullbright", 0))
    if tracked:
        client.call("set_compile_settings", {"bake_static_prop_vertices": True})


def drafts(client, ids):
    token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
    client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": "message", "value": "Pending material source review"}]})
    face_preview(client, ids)


def face_preview(client, ids):
    brush = detail(client, ids[:1])[0]
    client.call("edit_faces", {"faces": [face_ref(brush, brush["faces"][0])], "shift_u": 4, "preview": True})


def fixture(client, source):
    ids = []
    for low, high in ROOM:
        identifier = client.call("create_block", {"mins": low, "maxs": high, "material": MATERIAL})["id"]
        brush = detail(client, [identifier])[0]
        client.call("edit_faces", {"faces": [face_ref(brush, face) for face in brush["faces"]], "lightmap_scale": 64})
        ids.append(identifier)
    for classname, origin, properties in (("info_player_terrorist", [-64, 0, 8], {}),
                                           ("light", [0, 0, 160], {"_light": "255 192 128 64", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"})):
        ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
    client.call("select", {"ids": ids[:1]})
    configure(client)
    client.call("save_build_recipe", {"name": "Material source limits"})
    client.call("save_map", {"path": str(source)})
    drafts(client, ids)
    return ids


def snapshot(client, ids, source, game):
    require(not client.call("get_map_info")["rendering"]["initialized"], "Validation initialized graphics")
    recipes = client.call("get_build_recipes")
    return {"map": protected_state(client), "history": client.call("get_history"), "objects": detail(client, ids),
            "property_draft": client.call("get_property_edit"), "face_draft": client.call("get_face_selection"),
            "settings": client.call("get_compile_settings"), "recipes": recipes,
            "recipe_file": fingerprint(Path(recipes["path"])), "source": fingerprint(source), "assets": fingerprints(game)}


def after_compile_refresh(before):
    expected = deepcopy(before)
    require(before["map"]["face_selection"] == before["face_draft"], "Face preview inspection disagrees with map state")
    for state in (expected["map"]["face_selection"], expected["face_draft"]):
        require(state["preview"] and state["preview_token"] is not None
                and len(state["preview_faces"]) == 1 and state["preview_faces"][0]["shift_u"] == 4
                and state["atlas"]["preview"] is None, "Compile fixture lacks its isolated face preview")
        state["preview"] = False
        state["preview_token"] = None
        state["preview_faces"] = []
        for face in state["faces"]:
            face["preview"] = None
    require(expected["history"]["navigation_blocked"] == "Apply or cancel the active preview before navigating history.",
            "Fixture history is not blocked solely by its face preview")
    expected["history"]["navigation_blocked"] = None
    return expected


def read_bsp(path):
    require(path.stat().st_size <= MAX_FIXTURE_BSP_BYTES, "Fixture BSP exceeded its bounded read")
    return path.read_bytes()


def metrics(data, dimensions, rho):
    strings, offsets = payload(data, 43), records(data, 44, "<i")
    selected = []
    for row in records(data, 2, "<3f5i"):
        start = offsets[row[3]][0]
        end = strings.find(b"\0", start)
        require(0 <= start <= end < len(strings), "Invalid native material name")
        if strings[start:end].decode("ascii").lower() == MATERIAL:
            selected.append(row)
    require(len(selected) == 1 and selected[0][:3] == rho
            and selected[0][4:] == (*dimensions, *dimensions), f"Literal provider dimensions/reflectivity differ: {selected}")
    return {"dimensions": list(dimensions), "reflectivity": list(rho)}


def compile_pair(client, output, name, ids, source, game, prior=None, errors=()):
    with stopped_on_exit(client):
        face_preview(client, ids)
        before = snapshot(client, ids, source, game)
        expected = after_compile_refresh(before)
        paths = [output / f"{name}{suffix}.bsp" for suffix in ("", "-background")]
        lin = b"11 13 17\n19 23 29\n"
        for path in paths:
            path.with_suffix(".lin").write_bytes(lin)
            if prior is not None:
                path.write_bytes(prior)
        response = client.call("compile_map", {"path": str(paths[0])}, error=bool(errors))
        result = json.loads(response["content"][0]["text"]) if errors else response
        if errors:
            require(not result["success"] and result["bsp"] is None and read_bsp(paths[0]) == prior, "Foreground refusal replaced the genuine prior BSP")
            require(any(row["level"] == "error" and all(token.lower() in row["message"].lower() for token in errors) for row in result["diagnostics"]), f"Foreground source limit lacks context: {result}")
        else:
            require(result["success"] and result["leak"] is None, f"Fixture compile failed: {result}")
        require(snapshot(client, ids, source, game) == expected, "Foreground compile changed protected state/assets beyond the expected face-preview cancellation")
        face_preview(client, ids)
        background_before = snapshot(client, ids, source, game)
        require(after_compile_refresh(background_before) == expected, "Recreating the background face preview changed protected state/assets")
        started = client.call("start_compile", {"path": str(paths[1])})["job"]
        job = wait_job(client)
        require(job["token"] == started["token"], "Background compile token changed")
        if errors:
            require(job["status"] == "failed" and not job["result"]["published"]
                    and any(row["level"] == "error" and all(token.lower() in row["message"].lower() for token in errors) for row in job["result"]["diagnostics"]), f"Background source limit lacks context: {job}")
        else:
            require(job["status"] == "completed" and job["result"]["published"] and job["result"]["success"] and not job["stale"], f"Background compile failed: {job}")
        data = read_bsp(paths[0])
        require(read_bsp(paths[1]) == data and all(path.with_suffix(".lin").read_bytes() == lin for path in paths), "Paired compile changed prior BSP/LIN agreement")
        require(snapshot(client, ids, source, game) == expected, "Background compile changed protected state/assets beyond the expected face-preview cancellation")
        return data, {"result": result, "background": job, "bsp": str(paths[0]), "background_bsp": str(paths[1]),
                      "bsp_sha256": digest(data), "source": str(source), "source_fingerprint": before["source"],
                      "asset_fingerprints": before["assets"], "settings": before["settings"], "recipe_file": before["recipe_file"],
                      "preservation_sha256": digest(json.dumps(expected, sort_keys=True).encode()), "preserved": True,
                      "face_preview_cancellation": {"foreground_token": before["face_draft"]["preview_token"],
                                                    "background_token": background_before["face_draft"]["preview_token"],
                                                    "history_navigation_unblocked": True}}


def old_controls(client, output, game):
    source = output / "source.vmf"
    ids = fixture(client, source)
    baseline, normal = compile_pair(client, output, "normal", ids, source, game)
    metrics(baseline, (2, 3), RHO)
    oversized = commented(1048577)
    require(len(oversized) == 1048577, "Old witness size differs")
    (game / VMT).write_bytes(oversized)
    (output / "oversized-comment.vmt").write_bytes(oversized)
    data, witness = compile_pair(client, output, "old-oversized-comment", ids, source, game)
    require(data == baseline, "Old accepted comment changed whole BSP")
    return {"normal": normal, "oversized_comment": witness}


def validate(args):
    client = None
    result = {"status": "failed", "binary": str(args.binary), "binary_fingerprint": fingerprint(args.binary),
              "script_fingerprint": fingerprint(Path(__file__)), "native_or_graphics_exercised": False,
              "positive": {}, "negative": {}}
    try:
        game = args.output / "game"
        assets(game)
        temporary = args.output / "tmp"
        temporary.mkdir()
        with patch.dict(os.environ, {"TMPDIR": str(temporary)}):
            client = open_client(args.binary, args.output / "client", game)
        tools = {tool["name"]: tool for tool in client.rpc("tools/list", {})["tools"]}
        for name in ("compile_map", "start_compile"):
            description = "".join(tools[name].get("description", "").lower().split())
            require(all(token in description for token in ("permaterialscollection", "vmt", "1mib", "16mib", "4096", "metricvtf", "256mib", "512mib", "stricter")), f"{name}: bounded source discovery is missing")
        result["discovery"] = {name: tools[name] for name in ("compile_map", "start_compile")}
        old = json.loads(args.old_controls.read_text())
        require(old["status"] == "accepted_old_behavior" and old["binary_fingerprint"]["sha256"] == OLD_BINARY_SHA, "Wrong actual pre693 witness")
        require(old["oracle_fingerprint"] == result["script_fingerprint"], "Old witness used different fixture source")
        original = old["controls"]["normal"]
        require(original["asset_fingerprints"] == fingerprints(game)
                and old["controls"]["oversized_comment"]["bsp_sha256"] == original["bsp_sha256"], "Retained old assets/comment witness differ from the literal control")
        source = Path(original["source"])
        require(fingerprint(source) == original["source_fingerprint"] and fingerprint(Path(original["bsp"]))["sha256"] == original["bsp_sha256"], "Retained old source/BSP changed")
        client.call("open_map", {"path": str(source), "discard_changes": True})
        ids = [row["id"] for row in client.call("find_objects", {"limit": 100})["objects"]]
        configure(client)
        client.call("save_build_recipe", {"name": "Material source limits"})
        drafts(client, ids)
        baseline, receipt = compile_pair(client, args.output, "normal", ids, source, game)
        require(digest(baseline) == original["bsp_sha256"], "New ordinary baseline changed old whole BSP")
        receipt["metrics"] = metrics(baseline, (2, 3), RHO)
        result["positive"]["normal"] = receipt
        for name, text, dimensions, rho in (("tool", f"LightmappedGeneric {{ $basetexture {BASE} %tooltexture {TOOL} }}", (4, 1), RHO),
                                           ("absent-base", "LightmappedGeneric { }", (64, 64), (0.5, 0.5, 0.5)),
                                           ("absent-vmt", None, (64, 64), (0.5, 0.5, 0.5))):
            if text is None:
                (game / VMT).unlink()
            else:
                (game / VMT).write_text(text)
            data, receipt = compile_pair(client, args.output, name, ids, source, game)
            receipt["metrics"] = metrics(data, dimensions, rho)
            result["positive"][name] = receipt
        (game / VMT).write_bytes(commented(262145))
        settings(client, ("direct_ldr", 0))
        data, receipt = compile_pair(client, args.output, "medium-ordinary", ids, source, game)
        receipt["metrics"] = metrics(data, (2, 3), RHO)
        result["positive"]["medium-ordinary"] = receipt
        configure(client, tracked=True)
        _, result["negative"]["medium-tracked"] = compile_pair(client, args.output, "medium-tracked", ids, source, game, baseline,
                                                                 ("materials/" + MATERIAL + ".vmt", "262144", "limit"))
        configure(client)
        (game / VMT).write_bytes(commented(1048577))
        _, result["negative"]["oversized-vmt"] = compile_pair(client, args.output, "oversized-vmt", ids, source, game, baseline,
                                                               ("materials/" + MATERIAL + ".vmt", "1048576", "limit"))
        included = game / "materials/hammerr693/oversized-include.vmt"
        included.write_bytes(commented(1048577))
        (game / VMT).write_text("Patch { include materials/hammerr693/oversized-include.vmt }")
        _, result["negative"]["oversized-include"] = compile_pair(client, args.output, "oversized-include", ids, source, game, baseline,
                                                                   ("oversized-include.vmt", "1048576", "limit"))
        large = game / f"materials/{LARGE}.vtf"
        with large.open("wb") as destination:
            destination.write(fixture_texture(2, 3, RHO, (32, 96, 160, 255)))
            destination.truncate(268435457)
        for name, text, provider in (("oversized-base", f"LightmappedGeneric {{ $basetexture {LARGE} }}", "$basetexture"),
                                     ("oversized-tool", f"LightmappedGeneric {{ $basetexture {BASE} %tooltexture {LARGE} }}", "%tooltexture")):
            (game / VMT).write_text(text)
            _, result["negative"][name] = compile_pair(client, args.output, name, ids, source, game, baseline,
                                                       (f"materials/{LARGE}.vtf", provider, "268435456", "limit"))
        (game / VMT).write_text(f'LightmappedGeneric {{ $basetexture {LARGE} %tooltexture {LARGE} %notooltexture 0 $reflectivity "[0.125 0.25 0.5]" }}')
        data, receipt = compile_pair(client, args.output, "unused-no-preview", ids, source, game)
        receipt["metrics"] = metrics(data, (128, 128), RHO)
        result["positive"]["unused-no-preview"] = receipt
        before = snapshot(client, ids, source, game)
        result["clean_eof"] = require_clean_eof(client)
        require(fingerprint(source) == before["source"] and fingerprints(game) == before["assets"]
                and fingerprint(Path(before["recipes"]["path"])) == before["recipe_file"], "EOF changed retained inputs")
        result["old_receipt_fingerprint"] = fingerprint(args.old_controls)
        result["sparse_fixture"] = {"path": str(large), **fingerprint(large)}
        result["status"] = "accepted"
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        try:
            if client is not None and client.process is not None:
                with stopped_on_exit(client):
                    pass
                require_clean_eof(client)
        except Exception as error:
            result["cleanup_error"] = f"{type(error).__name__}: {error}"
            result["status"] = "failed"
            raise
        finally:
            (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description="Validate bounded compiler VMT/VTF reads through both MCP compile paths with streamed sparse-file fingerprints and no native/graphics execution.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--old-controls", type=Path, required=True)
    args = parser.parse_args()
    args.binary, args.output, args.old_controls = args.binary.resolve(), args.output.resolve(), args.old_controls.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Artifacts must stay under target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    result = validate(args)
    print(json.dumps({"status": result["status"], "positive": len(result["positive"]), "negative": len(result["negative"])}))


if __name__ == "__main__":
    main()
