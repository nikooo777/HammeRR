import argparse
import json
import os
import struct
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, ROOT, detail, face_ref
from validate_material_albedo import (
    digest,
    identities,
    open_client,
    require,
    settings,
    state,
)
from validate_material_source import packed_material, reject
from validate_recovery_exit import require_clean_eof

NORMAL = '$normalmap "test/normal"'
LIMITS = {"material_bytes": 1048576, "material_total_bytes": 4194304, "material_reads": 9,
          "assets": 64, "edges": 128, "issues": 16, "texture_bytes": 16777216,
          "source_bytes": 67108864, "physical_paths": 8192, "path_bytes": 512,
          "texture_axis": 4096, "material_depth": 8}


def texture(width=2, height=2, cube=False, frames=1, depth=None, image_format=0):
    data = bytearray(80)
    data[:4] = b"VTF\0"
    struct.pack_into("<3I", data, 4, 7, 0 if depth is None else 2, 80)
    struct.pack_into("<2H", data, 16, width, height)
    struct.pack_into("<IHH", data, 20, 0x4000 if cube else 0, frames, 0xffff if cube else 0)
    struct.pack_into("<3f", data, 32, 0.25, 0.5, 0.75)
    struct.pack_into("<i", data, 52, image_format)
    data[56] = 1
    struct.pack_into("<i", data, 57, -1)
    if depth is not None:
        struct.pack_into("<H", data, 63, depth)
    return bytes(data) + bytes((32, 96, 160, 255)) * width * height * (6 if cube else 1) * max(1, frames) * max(1, depth or 1)


def assets(game):
    first, second = game / "first", game / "second"
    files = {}

    def put(path, content, mount=first):
        data = content.encode() if isinstance(content, str) else content
        target = mount / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        files[path] = {"data": data, "origin": {"kind": "directory", "mount": str(mount), "file": str(target), "storage_files": [], "paths_truncated": False}}

    for name, width, height in (("normal", 3, 2), ("base", 4, 2), ("replaced", 4, 3), ("reflect", 2, 4), ("refract", 1, 3), ("preview", 3, 1)):
        put(f"materials/test/{name}.vtf", texture(width, height))
    put("materials/test/cube.vtf", texture(cube=True, depth=1))
    put("materials/test/plain.vmt", "UnlitGeneric { }")
    put("materials/test/bottom.vmt", f"Water_DX90 {{ {NORMAL} }}")
    put("materials/test/overlay.vmt", f"Water_DX9_HDR {{ {NORMAL} $underwateroverlay 0 }}")
    put("materials/test/unknown-child.vmt", "Refract { $basetexture test/must-not-follow }")
    put("materials/test/base.vmt", f'Water {{ {NORMAL} $basetexture test/base $reflecttexture test/reflect $refracttexture test/refract $envmap test/cube $bottommaterial test/bottom $underwateroverlay test/overlay %tooltexture test/preview $fogcolor "[0.1 0.2 0.3]" }}', second)
    put("materials/test/selected.vmt", 'Patch { include materials/test/base.vmt replace { $basetexture test/replaced } insert { $forcecheap 0 } }')
    (second / "materials/test/selected.vmt").write_text("UnlitGeneric { $basetexture wrong/winner }")
    declarations = {
        "explicit": f"Water {{ {NORMAL} }}",
        "defaults": "Water { }",
        "runtime": f"Water {{ {NORMAL} $reflecttexture _RT_WaterReflection $refracttexture _rt_WaterRefraction $envmap ENV_CUBEMAP $underwateroverlay 0 }}",
        "unknown-child-root": f"Water {{ {NORMAL} $underwateroverlay test/unknown-child }}",
        "duplicate": f"Water {{ {NORMAL} $NORMALMAP test/base }}",
        "suffix": f"Water {{ {NORMAL} $normalmap#1 test/base }}",
        "proxy": f"Water {{ {NORMAL} Proxies {{ AnimatedTexture {{ animatedTextureVar $normalmap }} }} }}",
        "conditional": f"Water {{ {NORMAL} [$WIN32] {{ $basetexture test/base }} }}",
        "unknown": f"Water {{ {NORMAL} $flowmap test/base }}",
        "conflicting-insert": 'Patch { include materials/test/explicit.vmt insert { $normalmap test/base } }',
        "missing-replace": 'Patch { include materials/test/explicit.vmt replace { $basetexture test/base } }',
        "unsafe": 'Water { $normalmap ../outside }',
        "missing": 'Water { $normalmap test/absent }',
        "missing-include": 'Patch { include materials/test/absent.vmt }',
        "malformed": "invalid",
        "invalid-utf8": b"Water { $normalmap \xff }",
        "cycle-a": f"Water {{ {NORMAL} $bottommaterial test/cycle-b }}",
        "cycle-b": f"Water {{ {NORMAL} $bottommaterial test/cycle-a }}",
        "swapped-runtime": f"Water {{ {NORMAL} $reflecttexture _rt_WaterRefraction }}",
        "blank-normal": 'Water { $normalmap "" }',
    }
    for name, declaration in declarations.items():
        put(f"materials/test/{name}.vmt", declaration)
    for index in range(10):
        child = f"$bottommaterial test/chain-{index + 1}" if index < 9 else ""
        put(f"materials/test/chain-{index}.vmt", f"Water {{ {NORMAL} {child} }}")
    for name, data in (("raw-zero-frame", texture(frames=0)), ("raw-zero-depth", texture(depth=0)),
                       ("animated", texture(frames=2)), ("volume", texture(depth=2)),
                       ("wrong-role", texture(cube=True, depth=1)), ("codec", texture(image_format=99)),
                       ("truncated", texture()[:-1]), ("axis", texture(4097, 1))):
        put(f"materials/test/{name}.vtf", data)
        put(f"materials/test/{name}.vmt", f"Water {{ $normalmap test/{name} }}")
    for name in ("_rt_waterreflection", "_rt_waterrefraction", "env_cubemap"):
        put(f"materials/{name}.vtf", b"Runtime names must not be read as VTF files")
    archive = game / "water_dir.vpk"
    packed_text = f"Water {{ {NORMAL} }}"
    packed_material(archive, packed_text)
    files["materials/test/packed.vmt"] = {"data": packed_text.encode(), "origin": {"kind": "vpk", "mount": str(archive), "file": None, "storage_files": [str(archive)], "paths_truncated": False}}
    (game / "gameinfo.txt").write_text('GameInfo { FileSystem { SearchPaths { game |gameinfo_path|first game |gameinfo_path|second game |gameinfo_path|water_dir.vpk } } }')
    return files


def snapshot(client, ids, source, game):
    return {**state(client, ids, source, game), "property_draft": client.call("get_property_edit"),
            "face_draft": client.call("get_face_selection"), "transform": client.call("get_transform_handles")}


def inspect(client, name, ids, source, game):
    before = snapshot(client, ids, source, game)
    report = client.call("get_material_source", {"material": name})
    require(snapshot(client, ids, source, game) == before, "Water inspection changed protected source/state/drafts/assets")
    return report


def edge(owner, key, reference, target, role, origin="authored", state="mounted", evidence="sdk"):
    return {"owner": f"materials/test/{owner}.vmt", "key": key, "reference": reference, "target": target,
            "role": role, "origin": origin, "state": state, "evidence_basis": evidence}


def runtime(owner, origin="sdk_default"):
    return [edge(owner, key, reference, None, role, origin, "runtime_resource") for key, reference, role in
            (("$reflecttexture", "_rt_WaterReflection", "color_texture"),
             ("$refracttexture", "_rt_WaterRefraction", "color_texture"), ("$envmap", "env_cubemap", "cubemap_texture"))]


def inventory(report, files, expected_state):
    section = report["water_dependencies"]
    require(section["state"] == expected_state, f"Unexpected Water inventory state: {section}")
    require(section["root_material"] == report["vmt_path"], "Water root identity differs from ordinary source")
    require(section["model_basis"] == "css_sdk_dx90_static_water" and section["limits"] == LIMITS, "SDK basis/limits differ from frozen contract")
    require(len(section["issues"]) <= 16 and len(section["assets"]) <= 64 and len(section["edges"]) <= 128, "Report exceeded its bounds")
    paths = [row["path"] for row in section["assets"]]
    require(len(paths) == len(set(paths)), "Shared asset identity was counted twice")
    for row in section["assets"]:
        require(row["kind"] == ("material" if row["path"].endswith(".vmt") else "texture"), "Material/texture identity was confused")
        if row["state"] == "found":
            require(row["sha256"] is not None and row["origin"] is not None, "Found asset omitted its observed hash/origin")
        if row["sha256"] is not None:
            expected = files[row["path"]]
            require(row["bytes"] == len(expected["data"]) and row["sha256"] == digest(expected["data"])
                    and row["origin"] == expected["origin"], f"Wrong inspected bytes or winning origin: {row}")
        elif row["state"] == "missing":
            require(row["bytes"] == 0 and row["origin"] is None and row["path"] not in files, "Missing asset borrowed a healthy source")
    if expected_state == "supported_static_inventory":
        require(report["error"] is None and not section["issues"] and all(row["state"] == "found" for row in section["assets"]), "Supported inventory has partial files/issues")
        counters = section["counters"]
        require(counters == {"asset_lookups": len(paths), "material_reads": sum(row["kind"] == "material" for row in section["assets"]),
                             "material_bytes": sum(row["bytes"] for row in section["assets"] if row["kind"] == "material"),
                             "texture_reads": sum(row["kind"] == "texture" for row in section["assets"]),
                             "texture_bytes": sum(row["bytes"] for row in section["assets"] if row["kind"] == "texture"),
                             "source_bytes": sum(row["bytes"] for row in section["assets"]), "edges": len(section["edges"])}, "Distinct inventory counters differ")
    return section


def exact_edges(section, expected):
    require(sorted(section["edges"], key=lambda row: (row["owner"], row["key"])) == sorted(expected, key=lambda row: (row["owner"], row["key"])), "Typed edges differ from literal declaration model")


def validate(args):
    client = None
    result = {"status": "failed", "binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()),
              "script_sha256": digest(Path(__file__).read_bytes()), "inspection_only": True, "graphics_or_native_exercised": False,
              "helper_sha256": {name: digest((ROOT / "scripts" / name).read_bytes()) for name in
                                ("validate_face_lighting.py", "validate_material_albedo.py", "validate_material_source.py", "validate_recovery_exit.py")},
              "reports": {}}
    try:
        game = args.output / "game"
        files = assets(game)
        temporary = args.output / "tmp"
        temporary.mkdir()
        with patch.dict(os.environ, {"TMPDIR": str(temporary)}):
            client = open_client(args.binary, args.output / "client", game)
        definition = next(tool for tool in client.rpc("tools/list", {})["tools"] if tool["name"] == "get_material_source")
        description = definition.get("description", "").lower()
        require(definition.get("annotations", {}).get("readOnlyHint") is True and definition["inputSchema"].get("additionalProperties") is False
                and all(token in description for token in ("water_dependencies", "sdk", "inventory", "header")), "Actual Water inspection discovery/schema is missing")
        result["discovery"] = definition
        brush = client.call("create_block", {"mins": [0, 0, 0], "maxs": [32, 48, 16], "material": "test/plain"})["id"]
        ids = [brush]
        client.call("select", {"ids": ids})
        source = args.output / "source.vmf"
        client.call("save_map", {"path": str(source)})
        settings(client, ("direct_ldr", 0))
        client.call("save_build_recipe", {"name": "Water source inspection"})
        token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
        client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": "message", "value": "Pending Water source review"}]})
        shape = detail(client, ids)[0]
        client.call("edit_faces", {"faces": [face_ref(shape, shape["faces"][0])], "shift_u": 4, "preview": True})
        report = inspect(client, "MATERIALS\\TEST\\SELECTED.VMT", ids, source, game)
        section = inventory(report, files, "supported_static_inventory")
        expected = [edge("selected", "include", "materials/test/base.vmt", "materials/test/base.vmt", "patch_include")]
        for key, name, role in (("$normalmap", "normal", "normal_texture"), ("$basetexture", "replaced", "color_texture"),
                                ("$reflecttexture", "reflect", "color_texture"), ("$refracttexture", "refract", "color_texture"),
                                ("$envmap", "cube", "cubemap_texture"), ("%tooltexture", "preview", "editor_preview_texture")):
            expected.append(edge("selected", key, f"test/{name}", f"materials/test/{name}.vtf", role, "patch_effective"))
        expected.extend([edge("selected", "$bottommaterial", "test/bottom", "materials/test/bottom.vmt", "compiler_material", "patch_effective"),
                         edge("selected", "$underwateroverlay", "test/overlay", "materials/test/overlay.vmt", "runtime_material", "patch_effective", evidence="installed_client_and_sdk")])
        for name in ("bottom", "overlay"):
            expected.append(edge(name, "$normalmap", "test/normal", "materials/test/normal.vtf", "normal_texture"))
            expected.extend(runtime(name))
        expected.append(edge("overlay", "$underwateroverlay", "0", None, "runtime_material", state="disabled", evidence="installed_client_and_sdk"))
        exact_edges(section, expected)
        require(report["raw_source"] == files["materials/test/selected.vmt"]["data"].decode()
                and [row["path"] for row in report["files"]] == ["materials/test/selected.vmt", "materials/test/base.vmt"], "Water-only files polluted original source provenance")
        observed = {row["path"]: row for row in section["assets"]}
        require(set(observed) == {f"materials/test/{name}.vmt" for name in ("selected", "base", "bottom", "overlay")}
                | {f"materials/test/{name}.vtf" for name in ("normal", "replaced", "reflect", "refract", "cube", "preview")}, "Shared/unused files differ from the literal graph")
        for name, width, height in (("normal", 3, 2), ("replaced", 4, 3), ("reflect", 2, 4), ("refract", 1, 3), ("preview", 3, 1), ("cube", 2, 2)):
            header = observed[f"materials/test/{name}.vtf"]["texture"]
            require(header == {"layout_scope": "existing_vtf_parser", "version": [7, 2 if name == "cube" else 0],
                               "width": width, "height": height, "depth": 1, "authored_depth": 1 if name == "cube" else None,
                               "authored_frames": 1, "first_frame": 65535 if name == "cube" else 0, "faces": 6 if name == "cube" else 1,
                               "mip_count": 1, "flags": 16384 if name == "cube" else 0, "format": "Rgba8888"}, "Literal static VTF header observation differs")
        result["reports"]["patch-static"] = report
        for name in ("explicit", "runtime", "packed"):
            report = inspect(client, f"test/{name}", ids, source, game)
            section = inventory(report, files, "supported_static_inventory")
            expected = [edge(name, "$normalmap", "test/normal", "materials/test/normal.vtf", "normal_texture")]
            expected.extend(runtime(name))
            if name == "runtime":
                expected[1:4] = [edge(name, key, reference, None, role, state="runtime_resource") for key, reference, role in
                                 (("$reflecttexture", "_RT_WaterReflection", "color_texture"), ("$refracttexture", "_rt_WaterRefraction", "color_texture"), ("$envmap", "ENV_CUBEMAP", "cubemap_texture"))]
                expected.append(edge(name, "$underwateroverlay", "0", None, "runtime_material", state="disabled", evidence="installed_client_and_sdk"))
            exact_edges(section, expected)
            require({row["path"] for row in section["assets"]} == {f"materials/test/{name}.vmt", "materials/test/normal.vtf"}, "Explicit normal/runtime spellings caused extra/default file lookups")
            result["reports"][name] = report
        report = inspect(client, "test/defaults", ids, source, game)
        section = inventory(report, files, "unavailable_source")
        require(any(row["path"] == "materials/dev/water_normal.vtf" and row["state"] == "missing" for row in section["assets"]), "Missing SDK normal default was hidden")
        result["reports"]["missing-default"] = report
        default = game / "first/materials/dev/water_normal.vtf"
        default.parent.mkdir()
        default.write_bytes(texture(1, 2))
        files["materials/dev/water_normal.vtf"] = {"data": default.read_bytes(), "origin": {"kind": "directory", "mount": str(game / "first"), "file": str(default), "storage_files": [], "paths_truncated": False}}
        report = inspect(client, "test/defaults", ids, source, game)
        section = inventory(report, files, "supported_static_inventory")
        exact_edges(section, [edge("defaults", "$normalmap", "dev/water_normal", "materials/dev/water_normal.vtf", "normal_texture", "sdk_default"), *runtime("defaults")])
        result["reports"]["present-default"] = report
        report = inspect(client, "test/unknown-child-root", ids, source, game)
        section = inventory(report, files, "unsupported")
        require(any(row["key"] == "$underwateroverlay" and row["target"] == "materials/test/unknown-child.vmt" and row["role"] == "runtime_material"
                    and row["evidence_basis"] == "installed_client_and_sdk" for row in section["edges"])
                and any(row["path"] == "materials/test/unknown-child.vmt" and row["shader"] == "refract" for row in section["assets"])
                and any(row["code"] == "unsupported_shader" for row in section["issues"])
                and not any("must-not-follow" in row["path"] for row in section["assets"]), "Unknown material child was guessed into a texture closure")
        result["reports"]["unknown-child"] = report
        for expected_state, names in (("unsupported", ["duplicate", "suffix", "proxy", "conditional", "unknown", "conflicting-insert", "missing-replace", "cycle-a", "swapped-runtime",
                                                       "raw-zero-frame", "raw-zero-depth", "animated", "volume", "wrong-role", "codec", "axis"]),
                                      ("unavailable_source", ["unsafe", "missing", "missing-include", "malformed", "invalid-utf8", "truncated", "blank-normal"]),
                                      ("limited", ["chain-0"]), ("not_applicable", ["plain"])):
            for name in names:
                report = inspect(client, f"test/{name}", ids, source, game)
                section = inventory(report, files, expected_state)
                legacy_failure = name in ("missing-include", "malformed", "invalid-utf8")
                require(bool(report["error"]) == legacy_failure and (report["resolved"] is None) == legacy_failure,
                        "Water-only outcome replaced ordinary source error/resolution semantics")
                require(expected_state == "not_applicable" or section["issues"], "Partial report has no contextual issue")
                if name == "blank-normal":
                    require(not any(row["path"] == "materials/dev/water_normal.vtf" for row in section["assets"]), "Explicit blank normal silently used the SDK default")
                if expected_state == "limited":
                    require(section["counters"]["material_reads"] <= 9 and section["counters"]["asset_lookups"] <= 64, "Limited traversal exceeded frozen read caps")
                result["reports"][name] = report
        before = snapshot(client, ids, source, game)
        for arguments in ({}, {"material": "../outside"}, {"material": "test/explicit", "extra": True}):
            reject(client, arguments)
        require(snapshot(client, ids, source, game) == before, "Rejected inspection requests changed protected state")
        result["clean_eof"] = require_clean_eof(client)
        require(digest(source.read_bytes()) == before["source"] and identities(game) == before["assets"]
                and digest(Path(before["recipes"]["path"]).read_bytes()) == before["recipe_bytes"], "EOF changed retained inputs")
        result["fixture_assets"] = identities(game)
        result["source_sha256"] = digest(source.read_bytes())
        result["status"] = "accepted"
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        try:
            if client is not None and client.process is not None:
                require_clean_eof(client)
        except Exception as error:
            result["cleanup_error"] = f"{type(error).__name__}: {error}"
            result["status"] = "failed"
            raise
        finally:
            (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description="Validate bounded SDK-qualified Water dependency inspection through shared MCP without compilation, graphics or game execution.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(), args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Artifacts must stay under target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    result = validate(args)
    print(json.dumps({"status": result["status"], "inspections": len(result["reports"])}))


if __name__ == "__main__":
    main()
