import argparse
import json
import math
import os
import re
import struct
from pathlib import Path
from unittest.mock import patch

from validate_compile_jobs import stopped_on_exit
from validate_cubemap_capture_probe import bsp_lumps
from validate_detail_props import detail_records
from validate_face_lighting import DEFAULT_BINARY, ROOT, detail, face_ref
from validate_generated_detail_tint import world_tint
from validate_material_albedo import (
    compile_pair,
    digest,
    fixture_texture,
    identities,
    open_client,
    require,
    settings,
)
from validate_material_details import set_world
from validate_prop_shadows import fixture as model_fixture
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM

KEY = "_detail_lighting_tint"
WORLD_TINT = "128 64 7"
ENTRY_TINT = "129 127 64"
MODES = [("fullbright", 0), ("direct_ldr", 0), ("direct_ldr_hdr", 1)]
MODEL = "models/props/test.mdl"
PLAIN = "hammerr691/plain"
DICTIONARY = "scripts/detail-entry-tint.vbsp"
MODEL_FIELDS = f'model "{MODEL}" upright 1 minAngle 0 maxAngle 0'
CARD_FIELDS = 'sprite "0 0 8 8 16" spritesize "0.5 0 4 8" detailOrientation 2 upright 1'


def checked_sha(value):
    require(re.fullmatch(r"[0-9a-f]{64}", value) is not None, "Supply the exact retained old binary SHA256")
    return value


def save_source(client, source):
    client.call("save_map", {"path": str(source)})
    text, count = re.subn(r'(?m)^(\s*"mapversion"\s+)"\d+"', r'\g<1>"1"', source.read_text())
    require(count == 2, "Fixture must have exactly two mapversion fields")
    source.write_text(text)
    client.call("open_map", {"path": str(source), "discard_changes": True})


def assets(game):
    model_fixture(game)
    mdl_path = game / MODEL
    mdl = bytearray(mdl_path.read_bytes())
    for offset in (104, 128):
        struct.pack_into("<3f", mdl, offset, -1, -1, 0)
    for offset in (116, 140):
        struct.pack_into("<3f", mdl, offset, 1, 1, 2)
    mdl_path.write_bytes(mdl)
    vvd_path = game / "models/props/test.vvd"
    vvd = bytearray(vvd_path.read_bytes())
    for index, point in enumerate(((-1, -1, 0), (1, -1, 0), (1, 1, 0), (-1, 1, 0))):
        struct.pack_into("<3f", vvd, 64 + index * 48 + 16, *point)
    vvd_path.write_bytes(vvd)
    for name in ("one", "two"):
        (game / f"materials/models/test/{name}.vmt").write_text("UnlitGeneric { }\n")
    folder = game / "materials/hammerr691"
    folder.mkdir()
    (game / "scripts").mkdir()
    (folder / "base.vtf").write_bytes(fixture_texture(2, 2, (0.5, 0.25, 0.125), (160, 96, 48, 255)))
    (folder / "atlas.vtf").write_bytes(fixture_texture(16, 16, (0.0, 0.0, 0.0), (255, 255, 255, 255)))
    for name in ("plain", "models", "cards"):
        extra = "" if name == "plain" else f"%detailtype {name}"
        (folder / f"{name}.vmt").write_text(f'LightmappedGeneric {{ $basetexture "hammerr691/base" {extra} }}\n')
    (folder / "atlas.vmt").write_text('UnlitGeneric { $basetexture "hammerr691/atlas" $translucent 1 $nocull 1 $vertexcolor 1 $vertexalpha 1 }\n')


def dictionary(fields="", mixed=False, density=8192, amount=1, misplaced=None, unused=False, group_label="group"):
    definitions = []
    for name, asset in (("models", MODEL_FIELDS), ("cards", CARD_FIELDS)):
        entries = f'item {{ {asset} amount {amount} {fields if misplaced is None else ""} }}'
        if mixed:
            entries = f'a {{ {asset} amount 0.5 {KEY} "255 0 0" }} b {{ {asset} amount 0.5 {KEY} "0 255 0" }}'
        low = f'low {{ alpha 0 bad {{ {asset} {KEY} "bad" }} }}' if unused else ""
        group_field = fields if misplaced == "group" else ""
        type_field = fields if misplaced == "type" else ""
        definitions.append(f'{name} {{ density {density} {type_field} {low} {group_label} {{ alpha 1 {group_field} {entries} }} }}')
    root_field = fields if misplaced == "root" else ""
    return "detail { " + root_field + " " + " ".join(definitions) + " }\n"


def fixture(client, source):
    client.call("new_map", {"discard_changes": True})
    ids = []
    for low, high in ROOM:
        identifier = client.call("create_block", {"mins": low, "maxs": high, "material": PLAIN})["id"]
        brush = detail(client, [identifier])[0]
        client.call("edit_faces", {"faces": [face_ref(brush, face) for face in brush["faces"]], "lightmap_scale": 64})
        ids.append(identifier)
    surfaces = []
    for name, low, high in (("models", [-128, -32, 0], [-64, 32, 8]), ("cards", [64, -32, 0], [128, 32, 8])):
        identifier = client.call("create_block", {"mins": low, "maxs": high, "material": PLAIN})["id"]
        brush = detail(client, [identifier])[0]
        top = next(face for face in brush["faces"] if face["normal"] == [0, 0, 1])
        reference = face_ref(brush, top)
        client.call("edit_faces", {"faces": [reference], "material": f"hammerr691/{name}", "lightmap_scale": 64})
        surfaces.append(reference)
        if name == "cards":
            identifier = client.call("tie_to_entity", {"ids": [identifier], "classname": "func_detail"})["id"]
        ids.append(identifier)
    for classname, origin, properties in (
        ("info_player_terrorist", [-192, -160, 8], {}),
        ("prop_detail", [0, 64, 16], {"model": MODEL, KEY: "64 255 128"}),
        ("light", [0, -64, 160], {"style": "0", "_light": "255 192 128 96", "_lightHDR": "96 160 255 128"}),
        ("light", [0, -64, 160], {"style": "1", "_light": "255 0 0 128", "_lightHDR": "0 0 0 0"}),
        ("light", [0, -64, 160], {"style": "2", "_light": "0 0 0 0", "_lightHDR": "0 0 255 160"}),
    ):
        if classname == "light":
            properties = {**properties, "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}
        ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
    set_world(client, "detailvbsp", DICTIONARY)
    set_world(client, "detailmaterial", "hammerr691/atlas")
    client.call("select", {"ids": ids[:1]})
    settings(client, MODES[0])
    client.call("save_build_recipe", {"name": "Detail entry tint fixture"})
    save_source(client, source)
    return ids, surfaces


def rows(data):
    return detail_records(data, allow_sprites=True, allow_baked=True, allow_styles=True)


def generated(prop):
    return prop["origin"][2] == 8


def layout(record):
    return {"models": record["models"], "sprites": record["sprites"],
            "props": [{key: value for key, value in prop.items() if key != "lighting"} for prop in record["props"]],
            "ldr_styles": [value[4] for value in record["style_lighting_ldr"] or []],
            "hdr_styles": [value[4] for value in record["style_lighting_hdr"] or []]}


def coverage(record):
    require(len(record["props"]) == 65 and record["models"] == [MODEL] and len(record["sprites"]) == 1, "Fixture native dictionaries/counts differ")
    require(sum(generated(prop) and "model" in prop for prop in record["props"]) == 32
            and sum(prop.get("kind") == 1 for prop in record["props"]) == 32
            and sum(not generated(prop) for prop in record["props"]) == 1, "World/detail/explicit fixture coverage differs")


def unrelated(reference, actual):
    for index, (before, after) in enumerate(zip(bsp_lumps(reference), bsp_lumps(actual), strict=True)):
        if index != 35:
            require(before == after, f"Entry tint changed unrelated BSP lump {index}")
    ignored = {int.from_bytes(name, "big") for name in (b"dprp", b"dplt", b"dplh")}
    def other_game_lumps(data):
        offset, length = struct.unpack_from("<2i", data, 8 + 35 * 16)
        require(0 <= offset <= offset + length <= len(data), "Invalid game-lump range")
        count = struct.unpack_from("<i", data, offset)[0]
        require(0 <= count <= 64 and 4 + count * 16 <= length, "Invalid game-lump directory")
        entries = []
        for index in range(count):
            identifier, flags, version, start, size = struct.unpack_from("<IHHii", data, offset + 4 + index * 16)
            require(offset <= start <= start + size <= offset + length, "Invalid game-lump entry")
            if identifier not in ignored:
                entries.append((identifier, flags, version, data[start:start + size]))
        return entries
    require(other_game_lumps(reference) == other_game_lumps(actual), "Entry tint changed another native game lump")


def product_check(reference, actual, world=(255, 255, 255), entry=(255, 255, 255)):
    require(layout(reference) == layout(actual), "Tint changed RNG/placement/dictionaries/style membership")
    for before, after in zip(reference["props"], actual["props"], strict=True):
        pairs = [(before["lighting"], after["lighting"])]
        for name in ("style_lighting_ldr", "style_lighting_hdr"):
            require((reference[name] is None) == (actual[name] is None), "Tint changed style channel availability")
            start, count = before["light_styles"], before["light_style_count"]
            if reference[name] is not None:
                pairs.extend(zip(reference[name][start:start + count], actual[name][start:start + count], strict=True))
        if not generated(before):
            require(all(left == right for left, right in pairs), "Dictionary or World tint changed explicit entity lighting")
            continue
        for left, right in pairs:
            for axis in range(3):
                factor = world[axis] * entry[axis] / 65025
                target = math.ldexp(right[axis], right[3])
                expected = math.ldexp(left[axis], left[3]) * factor
                error = math.ldexp(1, right[3]) + math.ldexp(1, left[3]) * factor
                require(target == 0 if factor == 0 else abs(target - expected) <= error,
                        f"Tint is not one linear product before encoding: {left}, {right}, {world}, {entry}")


def pair(client, output, label, ids, source, game, mode, text, retained=None, failure=False):
    (game / DICTIONARY).write_text(text)
    dictionary_copy = output / f"{label}.vbsp"
    dictionary_copy.write_text(text)
    settings(client, mode)
    lin = b"11 13 17\n19 23 29\n"
    for suffix in ("", "-background"):
        (output / f"{label}{suffix}.lin").write_bytes(lin)
    with stopped_on_exit(client):
        data, receipt = compile_pair(client, output, label, ids, source, game, KEY if failure else None, retained)
    require(all((output / f"{label}{suffix}.lin").read_bytes() == lin for suffix in ("", "-background")), "Compile changed the prior LIN")
    receipt.update({"source": str(source), "source_sha256": digest(source.read_bytes()), "dictionary": str(dictionary_copy), "dictionary_sha256": digest(dictionary_copy.read_bytes()),
                    "bsp": str(output / f"{label}.bsp"), "background_bsp": str(output / f"{label}-background.bsp")})
    return data, receipt


def old_controls(client, output, game):
    source = output / "fixture.vmf"
    ids, _surfaces = fixture(client, source)
    text = dictionary()
    require(KEY not in text, "Old controls must not use the new dictionary key")
    result = {}
    for world_name, value in (("omitted", None), ("world", WORLD_TINT)):
        world_tint(client, value)
        for mode in MODES:
            name = f"{mode[0]}-{world_name}"
            source = output / f"{name}.vmf"
            save_source(client, source)
            data, receipt = pair(client, output, name, ids, source, game, mode, text)
            coverage(rows(data))
            result[name] = receipt
    return result


def discovery(client):
    definitions = {tool["name"]: tool for tool in client.rpc("tools/list", {})["tools"]}
    for name in ("compile_map", "start_compile"):
        description = definitions[name].get("description", "").lower()
        require(all(token in description for token in (KEY, "dictionary", "linear")), f"{name}: missing actual entry-tint discovery")
    return {name: definitions[name] for name in ("compile_map", "start_compile")}


def validate(args):
    client = None
    game = args.output / "game"
    result = {"status": "failed", "binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()),
              "script_sha256": digest(Path(__file__).read_bytes()), "old_binary_sha256": args.old_binary_sha256,
              "helper_sha256": digest((ROOT / "scripts/validate_material_albedo.py").read_bytes()), "native_exercised": False,
              "old_controls": {}, "positive": {}, "negative": {}, "generated_hdr_precision": "387_failed_open_unchanged"}
    try:
        assets(game)
        temporary = args.output / "tmp"
        temporary.mkdir()
        with patch.dict(os.environ, {"TMPDIR": str(temporary)}):
            client = open_client(args.binary, args.output / "client", game)
        result["discovery"] = discovery(client)
        old = json.loads(args.controls.read_text())
        require(old["status"] == "frozen_old_controls" and old["binary_sha256"] == args.old_binary_sha256, "Wrong retained pre-entry control binary")
        require(old["oracle_sha256"] == result["script_sha256"] and old["fixture_assets"] == identities(game), "Old controls used different fixture source/assets")
        result["old_controls_receipt_sha256"] = digest(args.controls.read_bytes())
        settings(client, MODES[0])
        client.call("save_build_recipe", {"name": "Detail entry tint fixture"})
        controls = {}
        for name, expected in old["controls"].items():
            source = Path(expected["source"])
            require(digest(source.read_bytes()) == expected["source_sha256"], "Retained old source changed")
            text = Path(expected["dictionary"]).read_text()
            require(KEY not in text and digest(text.encode()) == expected["dictionary_sha256"], "Retained old dictionary changed or includes the new key")
            require(digest(Path(expected["bsp"]).read_bytes()) == expected["bsp_sha256"]
                    and Path(expected["background_bsp"]).read_bytes() == Path(expected["bsp"]).read_bytes(), "Retained old BSP controls changed")
            client.call("open_map", {"path": str(source), "discard_changes": True})
            ids = [row["id"] for row in client.call("find_objects", {"limit": 200})["objects"]]
            mode = next(mode for mode in MODES if name == f"{mode[0]}-omitted" or name == f"{mode[0]}-world")
            data, receipt = pair(client, args.output, "old-" + name, ids, source, game, mode, text)
            require(digest(data) == expected["bsp_sha256"], "Omitted entry changed actual old BSP bytes")
            coverage(rows(data))
            controls[name] = data
            result["old_controls"][name] = receipt
        require(len(controls) == 6, "Retained old controls are incomplete")
        fixture_source = Path(old["controls"]["fullbright-omitted"]["source"]).read_bytes()
        source = args.output / "fixture.vmf"
        source.write_bytes(fixture_source)
        client.call("open_map", {"path": str(source), "discard_changes": True})
        ids = [row["id"] for row in client.call("find_objects", {"limit": 200})["objects"]]
        for mode in MODES:
            for world_name, value in (("omitted", None), ("world", WORLD_TINT)):
                world_tint(client, value)
                name = f"white-{mode[0]}-{world_name}"
                source = args.output / f"{name}.vmf"
                save_source(client, source)
                data, receipt = pair(client, args.output, name, ids, source, game, mode, dictionary(f'{KEY} "255 255 255"'))
                require(data == controls[f"{mode[0]}-{world_name}"], "New white entry changed old omitted whole BSP")
                result["positive"][name] = receipt
            world_tint(client, None)
            name = "entry-world-equivalence-" + mode[0]
            source = args.output / f"{name}.vmf"
            save_source(client, source)
            data, receipt = pair(client, args.output, name, ids, source, game, mode, dictionary(f'{KEY} "{WORLD_TINT}"'))
            require(data == controls[f"{mode[0]}-world"], "Entry-only differs from proven World-tint whole BSP")
            product_check(rows(controls[f"{mode[0]}-omitted"]), rows(data), entry=(128, 64, 7))
            result["positive"][name] = receipt
        for mode in (MODES[0], MODES[2]):
            world_tint(client, WORLD_TINT)
            name = "fractional-" + mode[0]
            source = args.output / f"{name}.vmf"
            save_source(client, source)
            data, receipt = pair(client, args.output, name, ids, source, game, mode, dictionary(f'{KEY} "{ENTRY_TINT}"'))
            reference = controls[f"{mode[0]}-omitted"]
            product_check(rows(reference), rows(data), (128, 64, 7), (129, 127, 64))
            unrelated(reference, data)
            if mode == MODES[0]:
                require(all(prop["lighting"] == [129, 63, 3, -1] for prop in rows(data)["props"] if generated(prop)), "Fractional Fullbright literal differs")
            result["positive"][name] = receipt
        world_tint(client, None)
        source = args.output / "mixed.vmf"
        save_source(client, source)
        data, receipt = pair(client, args.output, "mixed", ids, source, game, MODES[0], dictionary(mixed=True))
        mixed = rows(data)
        require(layout(mixed) == layout(rows(controls["fullbright-omitted"])), "Mixed tint changed RNG or native indices")
        for model in (True, False):
            colors = {tuple(prop["lighting"]) for prop in mixed["props"] if generated(prop) and ("model" in prop) == model}
            require(colors == {(255, 0, 0, 0), (0, 255, 0, 0)}, "Same-asset entry colors collapsed into a dictionary-index tint")
        unrelated(controls["fullbright-omitted"], data)
        result["positive"]["mixed"] = receipt
        source = args.output / "black.vmf"
        save_source(client, source)
        data, receipt = pair(client, args.output, "black", ids, source, game, MODES[2], dictionary(f'{KEY} "0 0 0"'))
        reference = rows(controls["direct_ldr_hdr-omitted"])
        black = rows(data)
        product_check(reference, black, entry=(0, 0, 0))
        for prop in black["props"]:
            if generated(prop):
                require(prop["lighting"] == [0, 0, 0, 0] and prop["light_style_count"] == 2, "Black tint discarded the base/style union")
                start = prop["light_styles"]
                for array in (black["style_lighting_ldr"], black["style_lighting_hdr"]):
                    require(array[start:start + 2] == [[0, 0, 0, 0, 1], [0, 0, 0, 0, 2]], "Black tint changed native style membership or color")
        unrelated(controls["direct_ldr_hdr-omitted"], data)
        result["positive"]["black"] = receipt
        for label, text in (("unused-low-group", dictionary(unused=True)), ("key-like-group-name", dictionary(group_label=KEY))):
            source = args.output / f"{label}.vmf"
            save_source(client, source)
            data, receipt = pair(client, args.output, label, ids, source, game, MODES[0], text)
            require(data == controls["fullbright-omitted"], "Unused group or legal block label changed default bytes")
            result["positive"][label] = receipt
        invalid = [
            ("arity", dictionary(f'{KEY} "1 2"')),
            ("fraction", dictionary(f'{KEY} "1.5 0 0"')),
            ("range", dictionary(f'{KEY} "256 0 0"')),
            ("duplicate", dictionary(f'{KEY} "255 255 255" {KEY.upper()} "255 255 255"')),
            ("nonscalar", dictionary(f'{KEY} {{ bad 1 }}')),
            ("misplaced-root", dictionary(f'{KEY} "128 64 0"', misplaced="root")),
            ("misplaced-type", dictionary(f'{KEY} "128 64 0"', misplaced="type")),
            ("misplaced-group", dictionary(f'{KEY} "128 64 0"', misplaced="group")),
            ("selected-zero-amount", dictionary(f'{KEY} "bad"', amount=0)),
            ("selected-zero-density", dictionary(f'{KEY} "bad"', density=0)),
        ]
        retained = controls["fullbright-omitted"]
        for name, text in invalid:
            source = args.output / f"{name}.vmf"
            save_source(client, source)
            _, result["negative"][name] = pair(client, args.output, name, ids, source, game, MODES[0], text, retained, True)
        source = args.output / "unrequested.vmf"
        patches = [detail(client, [identifier])[0] for identifier in ids if identifier.startswith("solid:")]
        faces = [face_ref(brush, face) for brush in patches for face in brush["faces"] if face["material"] == "hammerr691/models"]
        entity = next(item for item in detail(client, ids) if item.get("classname") == "func_detail")
        faces.extend(face_ref(brush, face) for brush in entity["brushes"] for face in brush["faces"] if face["material"] == "hammerr691/cards")
        require(len(faces) == 2, "Unrequested control lost the two authored scatter surfaces")
        client.call("edit_faces", {"faces": faces, "material": PLAIN})
        set_world(client, "detailvbsp", "scripts/never-created.vbsp")
        save_source(client, source)
        data, receipt = pair(client, args.output, "unrequested", ids, source, game, MODES[0], dictionary(f'{KEY} "bad"'))
        require(len(rows(data)["props"]) == 1 and not generated(rows(data)["props"][0]), "Unrequested dictionary was consumed or changed explicit details")
        result["positive"]["unrequested"] = receipt
        result["channel_samples"] = channel_samples(rows(controls["direct_ldr_hdr-omitted"]))
        require(result["old_controls"]["direct_ldr_hdr-omitted"]["result"]["stats"]["bounces"] == 1
                and result["old_controls"]["direct_ldr_hdr-omitted"]["result"]["stats"]["bounce_samples"] > 0, "One-bounce control did not exercise real reflected samples")
        final_assets = identities(game)
        final_sources = {str(path): digest(path.read_bytes()) for path in args.output.glob("*.vmf")}
        recipes = Path(client.call("get_build_recipes")["path"])
        recipe_hash = digest(recipes.read_bytes())
        result["clean_eof"] = require_clean_eof(client)
        require(identities(game) == final_assets and all(digest(Path(path).read_bytes()) == identity for path, identity in final_sources.items())
                and digest(recipes.read_bytes()) == recipe_hash, "EOF changed assets/sources/recipes")
        result["source_hashes"] = final_sources
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


def channel_samples(record):
    ldr, hdr = record["style_lighting_ldr"], record["style_lighting_hdr"]
    require(ldr is not None and hdr is not None and len(ldr) == len(hdr) > 0, "Dual style sample arrays are absent")
    for left, right in zip(ldr, hdr, strict=True):
        require(left[4] == right[4] and left[4] in (1, 2), "LDR/HDR style union differs")
        if left[4] == 1:
            require(left[0] > 0 and left[1:3] == [0, 0] and right[:3] == [0, 0, 0], "LDR-only red style was not independently baked")
        else:
            require(left[:3] == [0, 0, 0] and right[0:2] == [0, 0] and right[2] > 0, "HDR-only blue style was not independently baked")
    return {"ldr_red": ldr[:4], "hdr_blue": hdr[:4], "records_per_channel": len(ldr)}


def main():
    parser = argparse.ArgumentParser(description="Validate mounted per-entry generated detail lighting tint through both MCP compile paths without rendering or game execution.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--controls", type=Path, required=True)
    parser.add_argument("--old-binary-sha256", required=True)
    args = parser.parse_args()
    args.binary, args.output, args.controls = args.binary.resolve(), args.output.resolve(), args.controls.resolve()
    args.old_binary_sha256 = checked_sha(args.old_binary_sha256)
    require(args.output.is_relative_to(ROOT / "target/validation"), "Artifacts must stay below target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    result = validate(args)
    print(json.dumps({"status": result["status"], "positive": len(result["positive"]), "negative": len(result["negative"])}))


if __name__ == "__main__":
    main()
