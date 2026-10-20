import argparse
import json
import re
import struct
from pathlib import Path

from validate_authored_planes import records
from validate_brush_model_poses import payloads
from validate_compile_jobs import stopped_on_exit
from validate_face_lighting import DEFAULT_BINARY, ROOT, detail
from validate_material_albedo import (
    compile_pair,
    digest,
    fixture_texture,
    identities,
    open_client,
    payload,
    require,
    settings,
)
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM

MATERIAL = "hammerr688/unlit"
MODES = [("fullbright", 0), ("direct_ldr", 0), ("direct_ldr_hdr", 0)]
NAMES = {"controller": "pose_window_controller", "rendered": "pose_window_rendered", "foreground": "pose_window_foreground"}
PIVOT = "0.5 -0.25 96.25"
BOUNDS = {"rendered": ([-12, -72, 0], [8, 64, 192]), "foreground": ([-16, -80, -8], [16, 80, 200])}
ROLE_LABELS = {"controller": "controller", "rendered": "rendered window", "foreground": "foreground bmodel"}
NEGATIVE = [
    {"name": "controller-yaw", "role": "controller", "fields": [("angles", "0 45 0")], "modes": MODES},
    {"name": "controller-malformed", "role": "controller", "fields": [("angles", "bad")], "modes": [MODES[0]]},
    {"name": "controller-suffix", "role": "controller", "fields": [("angles#1", "0 0 0")], "modes": [MODES[1]]},
    {"name": "controller-padded", "role": "controller", "fields": [("angles ", "0 45 0")], "modes": [MODES[2]]},
    {"name": "controller-duplicate", "role": "controller", "fields": [("angles", "0 0 0"), ("AnGlEs", "0 45 0")], "modes": [MODES[0]]},
    {"name": "rendered-movelinear-yaw", "role": "rendered", "template": "rendered-movelinear", "fields": [("angles", "0 45 0")], "modes": [MODES[0], MODES[2]]},
    {"name": "foreground-movelinear-yaw", "role": "foreground", "template": "foreground-movelinear", "fields": [("angles", "0 45 0")], "modes": [MODES[0], MODES[1]]},
    {"name": "rendered-movelinear-malformed", "role": "rendered", "template": "rendered-movelinear", "fields": [("angles", "bad")], "modes": [MODES[0]]},
    {"name": "foreground-movelinear-alias", "role": "foreground", "template": "foreground-movelinear", "fields": [("angles#1", "0 0 0")], "modes": [MODES[1]]},
]


def assets(game):
    (game / "materials/hammerr688").mkdir(parents=True)
    (game / "materials/tools").mkdir()
    (game / "gameinfo.txt").write_text('GameInfo { FileSystem { SearchPaths { Game "|gameinfo_path|." } } }\n')
    (game / "materials/hammerr688/zero.vtf").write_bytes(fixture_texture(1, 1, (0.0, 0.0, 0.0), (0, 0, 0, 255)))
    for name, text in [
        ("hammerr688/unlit", 'UnlitGeneric { $basetexture "hammerr688/zero" %compilenolight 1 }'),
        ("tools/toolsnodraw", "UnlitGeneric { %compilenodraw 1 }"),
        ("tools/toolsareaportal", "UnlitGeneric { %compileareaportal 1 }"),
    ]:
        (game / f"materials/{name}.vmt").write_text(text + "\n")


def fixture(client, source):
    client.call("new_map", {"discard_changes": True})
    ids = [client.call("create_block", {"mins": low, "maxs": high, "material": MATERIAL})["id"] for low, high in ROOM]
    for low, high in [
        ([-8, -256, 0], [8, -64, 256]),
        ([-8, 64, 0], [8, 256, 256]),
        ([-8, -64, 192], [8, 64, 256]),
    ]:
        ids.append(client.call("create_block", {"mins": low, "maxs": high, "material": "tools/toolsnodraw"})["id"])
    for classname, origin in [("info_player_terrorist", [-128, -128, 8]), ("info_player_counterterrorist", [128, -128, 8])]:
        ids.append(client.call("create_entity", {"classname": classname, "origin": origin})["id"])
    portal = client.call("create_block", {"mins": [-4, -64, 0], "maxs": [4, 64, 192], "material": "tools/toolsareaportal"})["id"]
    roles = {"controller": client.call("tie_to_entity", {"ids": [portal], "classname": "func_areaportalwindow", "properties": {
        "targetname": NAMES["controller"], "target": NAMES["rendered"], "BackgroundBModel": NAMES["foreground"],
        "FadeStartDist": "64", "FadeDist": "128", "TranslucencyLimit": "0.2",
    }})["id"]}
    for role in ["rendered", "foreground"]:
        low, high = BOUNDS[role]
        block = client.call("create_block", {"mins": low, "maxs": high, "material": MATERIAL})["id"]
        roles[role] = client.call("tie_to_entity", {"ids": [block], "classname": "func_brush", "properties": {
            "targetname": NAMES[role], "origin": PIVOT, "Solidity": "1",
        }})["id"]
    for owner in roles.values():
        client.call("set_entity_properties", {"id": owner, "remove": ["angle", "angles"]})
    ids.extend(roles.values())
    client.call("select", {"ids": [roles["controller"]]})
    client.call("save_map", {"path": str(source)})
    for entity in detail(client, list(roles.values())):
        require(not any(key.lower() in {"angle", "angles"} for key, _value in entity["properties"]), "Absent fixture retained an implicit angle field")
    for role in ["rendered", "foreground"]:
        entity = detail(client, [roles[role]])[0]
        require(entity["origin"] == [0.5, -0.25, 96.25], "Fixture pivot differs")
        require(len(entity["brushes"]) == 1 and entity["brushes"][0]["bounds"] == {"mins": BOUNDS[role][0], "maxs": BOUNDS[role][1]}, "Fixture asymmetric source geometry differs")
    return ids, roles


def templates(client, output, source, roles):
    result = {"brush": source.read_text()}
    for role in ["rendered", "foreground"]:
        client.call("set_entity_properties", {"id": roles[role], "set": {"classname": "func_movelinear"}})
        path = output / f"{role}-movelinear-template.vmf"
        client.call("save_map", {"path": str(path)})
        result[f"{role}-movelinear"] = path.read_text()
        client.call("set_entity_properties", {"id": roles[role], "set": {"classname": "func_brush"}})
    return result


def case_source(source, role, fields):
    marker = f'"targetname" "{NAMES[role]}"'
    require(source.count(marker) == 1, "Pose owner marker is not unique")
    inserted = "".join(f'\n\t"{key}" "{value}"' for key, value in fields)
    return source.replace(marker, marker + inserted)


def entity_pairs(data):
    text = payload(data, 0).decode("ascii").rstrip("\0")
    pattern = re.compile(r'\{\n((?:"[^"\n]*" "[^"\n]*"\n)*)\}\n')
    blocks = list(pattern.finditer(text))
    require("".join(block.group() for block in blocks) == text, "Unexpected entity-lump framing")
    return [re.findall(r'^"([^"\n]*)" "([^"\n]*)"$', block.group(1), flags=re.MULTILINE) for block in blocks]


def zero_difference(reference, actual, role, fields):
    require(reference[:8] == actual[:8] and reference[1032:1036] == actual[1032:1036], "Exact-zero pose changed BSP identity/version/revision")
    require(struct.unpack_from("<2i", reference, 16) == struct.unpack_from("<2i", actual, 16), "Exact-zero pose changed entity-lump version/compression")
    require(payloads(reference) == payloads(actual), "Exact-zero pose changed a nonentity BSP payload")
    before = entity_pairs(reference)
    after = entity_pairs(actual)
    require(len(before) == len(after), "Exact-zero pose changed entity count")
    owner = None
    for index, pairs in enumerate(before):
        if ("targetname", NAMES[role]) in pairs:
            require(owner is None, "Ambiguous exact-zero owner")
            owner = index
    require(owner is not None, "Missing exact-zero owner")
    expected = list(before[owner])
    marker = expected.index(("targetname", NAMES[role]))
    expected[marker + 1:marker + 1] = fields
    for index in range(len(before)):
        require(after[index] == (expected if index == owner else before[index]), "Exact-zero entity changes exceed the authored key/value insertion")
    require(reference != actual, "Authored zero key was erased from exported entity text")
    return {"nonentity_payloads_exact": True, "entity_only_delta": {"targetname": NAMES[role], "inserted": fields}}


def inspect_scene(data, receipt, roles):
    for result in [receipt["result"], receipt["background"]["result"]]:
        stats = result["stats"]
        require(stats["areas"] == 2 and stats["areaportals"] == 1 and stats["areaportal_windows"] == 1, "Window fixture did not establish two areas")
        for selected in [stats, stats.get("hdr")]:
            if selected is not None:
                require(selected["lighting_faces"] == 0 and selected["model_lighting_faces"] == 0 and selected["lit_brush_models"] == 0,
                        "Fixture has lit receivers that could mask the window guard")
    portals = records(data, 21, "<4Hi")
    require(len(portals) == 3 and portals[1][0] == portals[2][0] == 1 and portals[1][4] ^ 1 == portals[2][4], "Reciprocal window area geometry differs")
    models = records(data, 14, "<9f3i")
    require(len(models) == 3, "Window fixture must emit exactly two referenced brush models")
    blocks = entity_pairs(data)
    by_name = {value: pairs for pairs in blocks for key, value in pairs if key == "targetname"}
    window = dict(by_name[NAMES["controller"]])
    require(window["classname"] == "func_areaportalwindow" and "model" not in window and window["PortalVersion"] == "1", "Window controller output differs")
    widths = {}
    for role in ["rendered", "foreground"]:
        entity = dict(by_name[NAMES[role]])
        model = models[int(entity["model"][1:])]
        widths[role] = [model[axis + 3] - model[axis] for axis in range(3)]
    require(widths == {"rendered": [20.0, 136.0, 192.0], "foreground": [32.0, 160.0, 208.0]}, "Compiled asymmetric model bounds differ")
    return {"areas": 2, "window_count": 1, "model_widths": widths, "lit_receivers": 0}


def pose_failure(receipt, role, roles):
    for result in [receipt["result"], receipt["background"]["result"]]:
        require(any(row["level"] == "error" and row["object"] == roles[role]
                    and "initial pose" in row["message"].lower() and "window portal" in row["message"].lower()
                    and ROLE_LABELS[role] in row["message"].lower() and roles["controller"] in row["message"]
                    for row in result["diagnostics"]), f"Missing object/role window pose diagnostic for {role}: {result}")


def opened_case(client, output, label, source, mode, ids, game, roles, fields=(), role=None, error=False, retained=None):
    path = output / f"{label}.vmf"
    path.write_text(case_source(source, role, fields) if fields else source)
    client.call("open_map", {"path": str(path), "discard_changes": True})
    if fields:
        loaded = [tuple(pair) for pair in detail(client, [roles[role]])[0]["properties"]]
        require(all(loaded.count(tuple(pair)) >= fields.count(pair) for pair in fields), "Raw imported pose fields were normalized or discarded before compilation")
    client.call("select", {"ids": [roles["controller"]]})
    settings(client, mode)
    with stopped_on_exit(client):
        data, receipt = compile_pair(client, output, label, ids, path, game, "initial pose" if error else None, retained)
    receipt["source_sha256"] = digest(path.read_bytes())
    if fields:
        receipt["authored_pose"] = {"object": roles[role], "role": role, "fields": fields}
    if error:
        pose_failure(receipt, role, roles)
    else:
        receipt["fixture"] = inspect_scene(data, receipt, roles)
    return data, receipt


def validate(args):
    client = None
    game = args.output / "game"
    source = args.output / "fixture.vmf"
    results = {"status": "failed", "positive": {}, "negative": {}, "binary": str(args.binary),
               "binary_sha256": digest(args.binary.read_bytes()), "script_sha256": digest(Path(__file__).read_bytes()),
               "helpers_sha256": digest((ROOT / "scripts/validate_material_albedo.py").read_bytes()), "native_exercised": False}
    try:
        assets(game)
        client = open_client(args.binary, args.output / "client", game)
        definitions = {item["name"]: item for item in client.rpc("tools/list", {})["tools"]}
        results["tool_discovery"] = {}
        for name in ["compile_map", "start_compile"]:
            description = definitions[name].get("description", "").lower()
            require(all(token in description for token in ["window", "exact-zero", "every lighting mode", "including unlit"]),
                    f"{name}: window pose admission is absent from actual MCP discovery")
            results["tool_discovery"][name] = definitions[name]
        ids, roles = fixture(client, source)
        original = source.read_bytes()
        results["source_sha256"] = digest(original)
        versions = templates(client, args.output, source, roles)
        settings(client, MODES[0])
        client.call("save_build_recipe", {"name": "Window pose admission fixture"})
        baselines = {}
        controls = [("brush", mode) for mode in MODES]
        controls += [("rendered-movelinear", mode) for mode in [MODES[0], MODES[2]]]
        controls += [("foreground-movelinear", mode) for mode in [MODES[0], MODES[1]]]
        for template, mode in controls:
            label = f"absent-{template}-{mode[0]}"
            data, receipt = opened_case(client, args.output, label, versions[template], mode, ids, game, roles)
            baselines[(template, mode[0])] = data
            results["positive"][label] = receipt
        zeros = [("brush", "controller", [("angles", "-0 +0.0 0e-300")], mode) for mode in MODES]
        zeros += [
            ("brush", "rendered", [("angle", "-0.0")], MODES[0]),
            ("brush", "foreground", [("angles", "0 0 0")], MODES[2]),
            ("rendered-movelinear", "rendered", [("angles", "0 0 0")], MODES[2]),
            ("foreground-movelinear", "foreground", [("angle", "0")], MODES[1]),
        ]
        for template, role, fields, mode in zeros:
            label = f"zero-{template}-{role}-{mode[0]}"
            data, receipt = opened_case(client, args.output, label, versions[template], mode, ids, game, roles, fields, role)
            receipt["zero_control"] = zero_difference(baselines[(template, mode[0])], data, role, fields)
            results["positive"][label] = receipt
        retained = baselines[("brush", MODES[0][0])]
        for case in NEGATIVE:
            for mode in case["modes"]:
                label = f"{case['name']}-{mode[0]}"
                results["active_case"] = label
                _, receipt = opened_case(client, args.output, label, versions[case.get("template", "brush")], mode, ids, game, roles,
                                         case["fields"], case["role"], True, retained)
                results["negative"][label] = receipt
        ending_assets = identities(game)
        recipes = Path(client.call("get_build_recipes")["path"])
        recipe_hash = digest(recipes.read_bytes())
        results["clean_eof"] = require_clean_eof(client)
        require(source.read_bytes() == original and identities(game) == ending_assets and digest(recipes.read_bytes()) == recipe_hash, "EOF changed fixture/source/assets/recipes")
        results.pop("active_case", None)
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
    parser = argparse.ArgumentParser(description="Validate window-controller and referenced-model initial pose refusal through both MCP compile paths without graphics or a game.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=ROOT / "target/validation/window-pose-admission")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Artifacts must remain below target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    result = validate(args)
    print(json.dumps({"status": result["status"], "positive": len(result["positive"]), "negative": len(result["negative"])}))


if __name__ == "__main__":
    main()
