import argparse
import hashlib
import json
import struct
from pathlib import Path

from validate_authored_planes import records
from validate_bump_lighting import texture
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_direct_lighting import FACE_FORMAT
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail, face_ref
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM

SHADERS = ["LightmappedGeneric", "WorldVertexTransition", "WorldTwoTextureBlend"]
PROFILES = ["ordinary", "enabled", "zero", "suppressed"]
CHANNELS = {"ldr": (7, 8), "hdr": (58, 53)}
INVALID_VALUES = ["2", "true", "1.0", "", "01", "NaN"]


def payload(data, index):
    offset, size, _version, compression = struct.unpack_from("<4i", data, 8 + index * 16)
    assert compression == 0 and 0 <= offset <= offset + size <= len(data)
    return data[offset:offset + size]


def channels(data):
    infos = records(data, 6, "<16f2i")
    result = {}
    for name, (face_lump, sample_lump) in CHANNELS.items():
        samples = payload(data, sample_lump)
        faces = []
        maps_total = 0
        for face in records(data, face_lump, FACE_FORMAT, expected_version=1 if name == "hdr" else 0):
            if face[12] < 0:
                faces.append(None)
                continue
            styles = tuple(style for style in face[8:12] if style != 255)
            assert styles and face[8:12] == styles + (255,) * (4 - len(styles))
            maps = 4 if infos[face[5]][16] & 0x800 else 1
            count = (face[16] + 1) * (face[17] + 1)
            offset = face[12]
            end = offset + count * maps * len(styles) * 4
            assert count > 0 and offset % 4 == 0
            assert 0 <= offset - len(styles) * 4 <= end <= len(samples)
            base = tuple(samples[offset + style * count * maps * 4:
                                 offset + (style * maps + 1) * count * 4]
                         for style in range(len(styles)))
            assert all(len(value) == count * 4 for value in base)
            faces.append((maps, styles, base, samples[offset - len(styles) * 4:offset]))
            maps_total += maps * len(styles)
        result[name] = (faces, maps_total)
    return result


def assets(output):
    game = output / "source-game"
    folder = game / "materials/hammerr"
    folder.mkdir(parents=True, exist_ok=True)
    (game / "gameinfo.txt").write_text(
        'GameInfo { game "Bump suppression validation" FileSystem { SteamAppId 240 '
        'SearchPaths { Game "|gameinfo_path|." } } }\n'
    )
    (folder / "white.vtf").write_bytes(texture(bytes([255] * 32 * 32 * 4)))
    (folder / "normal.vtf").write_bytes(texture(bytes([128, 128, 255, 255] * 32 * 32), normal=True))
    (folder / "control.vmt").write_text('LightmappedGeneric { $basetexture "hammerr/white" }\n')
    material(folder, SHADERS[0], "ordinary")
    return game


def material(folder, shader, profile):
    bump = '' if profile == "ordinary" else '$bumpmap "hammerr/normal"'
    suppression = {"zero": "$nodiffusebumplighting 0", "suppressed": "$nodiffusebumplighting 1"}.get(profile, "")
    (folder / "receiver.vmt").write_text(
        f'{shader} {{ $basetexture "hammerr/white" {bump} {suppression} }}\n'
    )


def policy_error(result):
    return any("materials/hammerr/receiver.vmt" in diagnostic["message"]
               and "$nodiffusebumplighting" in diagnostic["message"]
               and "literal 0 or 1" in diagnostic["message"]
               for diagnostic in result["diagnostics"])


def invalid_material(folder, shader, value, patch):
    if patch:
        (folder / "base.vmt").write_text(f'{shader} {{ $basetexture "hammerr/white" $bumpmap "hammerr/normal" $nodiffusebumplighting 0 }}\n')
        text = f'Patch {{ include materials/hammerr/base.vmt replace {{ $nodiffusebumplighting "{value}" }} }}\n'
    else:
        text = f'{shader} {{ $basetexture "hammerr/white" $bumpmap "hammerr/normal" $nodiffusebumplighting "{value}" }}\n'
    (folder / "receiver.vmt").write_text(text)


def create_block(client, low, high, material_name="hammerr/receiver"):
    block = client.call("create_block", {"mins": low, "maxs": high, "material": material_name})["id"]
    shape = detail(client, [block])[0]
    client.call("edit_faces", {"faces": [face_ref(shape, face) for face in shape["faces"]], "lightmap_scale": 64})
    return block


def scene(client):
    ids = [create_block(client, low, high) for low, high in ROOM]
    for low, high, classname, properties in [
        ([-192, -128, 8], [-160, -96, 24], "func_detail", {}),
        ([-32, -32, 8], [32, 32, 24], "func_brush", {"origin": "101.5 -67.25 32.125"}),
    ]:
        block = create_block(client, low, high)
        ids.append(client.call("tie_to_entity", {"ids": [block], "classname": classname, "properties": properties})["id"])
    terrain = create_block(client, [64, 64, 0], [192, 192, 16])
    ids.append(terrain)
    shape = detail(client, [terrain])[0]
    face = face_ref(shape, next(face for face in shape["faces"] if face["normal"][2] > 0.99))
    client.call("create_terrain", {"faces": [face], "power": 2})
    sculpt = client.call("preview_terrain_sculpt", {"faces": [face], "mode": "raise", "center": [128, 128, 16], "radius": 90, "amount": 8})
    client.call("commit_face_preview", {"token": sculpt["state"]["preview_token"]})
    ids.append(create_block(client, [128, -128, 0], [160, -96, 16], "hammerr/control"))
    for classname, origin, properties in [
        ("info_player_terrorist", [-224, -224, 8], {}),
        ("info_player_counterterrorist", [224, -224, 8], {}),
        ("light", [-96, 0, 180], {"_light": "255 0 0 100", "_lightHDR": "0 0 255 200", "_constant_attn": "1", "_quadratic_attn": "0"}),
        ("light_spot", [96, 0, 180], {"targetname": "suppression_style", "angles": "-90 0 0", "_cone": "60", "_cone2": "85", "_light": "0 255 0 60", "_lightHDR": "255 0 0 120", "_constant_attn": "1", "_quadratic_attn": "0"}),
    ]:
        ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
    client.call("set_compile_settings", {"lighting": "direct_ldr_hdr", "physics": "none", "sky_quality": "fast", "bounces": 1})
    return ids


def validate(args):
    game = assets(args.output)
    client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    results = {}
    refusals = []
    try:
        with stopped_on_exit(client):
            ids = scene(client)
            source = args.output / "suppression.vmf"
            client.call("save_map", {"path": str(source)})
            source_bytes = source.read_bytes()
            before = detail(client, ids)
            history = client.call("get_map_info")["undo"]
            for shader in SHADERS:
                built = {}
                decoded = {}
                for profile in PROFILES:
                    material(game / "materials/hammerr", shader, profile)
                    stem = f"{shader.lower()}-{profile}"
                    path = args.output / f"{stem}.bsp"
                    result = client.call("compile_map", {"path": str(path)})
                    assert result["success"], result
                    data = path.read_bytes()
                    views = channels(data)
                    stats = result["stats"]
                    assert stats["displacements"] == 1 and stats["lit_brush_models"] == 1
                    for name, (faces, total) in views.items():
                        channel_stats = stats if name == "ldr" else stats["hdr"]
                        assert total == channel_stats["light_style_maps"]
                        active = [face for face in faces if face is not None]
                        assert active and any(len(face[1]) > 1 for face in active)
                        assert channel_stats["styled_faces"] > 0
                        if profile in ["ordinary", "suppressed"]:
                            assert all(face[0] == 1 for face in active)
                            assert channel_stats["bump_faces"] == channel_stats["bump_lightmaps"] == 0
                        else:
                            assert any(face[0] == 4 for face in active)
                            assert channel_stats["bump_faces"] > 0 and channel_stats["bump_lightmaps"] > 0
                    background = args.output / f"{stem}-background.bsp"
                    client.call("start_compile", {"path": str(background)})
                    job = wait_job(client)
                    assert job["status"] == "completed", job
                    assert background.read_bytes() == data
                    assert job["result"]["stats"]["bump_lightmaps"] == stats["bump_lightmaps"]
                    assert detail(client, ids) == before
                    assert client.call("get_map_info")["undo"] == history
                    assert source.read_bytes() == source_bytes
                    built[profile] = data
                    decoded[profile] = views
                    results[stem] = {"sha256": hashlib.sha256(data).hexdigest(), "stats": stats}
                assert built["ordinary"] == built["suppressed"]
                assert built["enabled"] == built["zero"]
                for name in CHANNELS:
                    ordinary = decoded["ordinary"][name][0]
                    enabled = decoded["enabled"][name][0]
                    assert len(ordinary) == len(enabled)
                    for plain, bumped in zip(ordinary, enabled):
                        assert (plain is None) == (bumped is None)
                        if plain is not None:
                            assert plain[1:] == bumped[1:]
                retained = args.output / f"{shader.lower()}-retained.bsp"
                retained.write_bytes(built["ordinary"])
                for value, patch in [(value, False) for value in INVALID_VALUES] + [("2", True)]:
                    invalid_material(game / "materials/hammerr", shader, value, patch)
                    response = client.call("compile_map", {"path": str(retained)}, error=True)
                    failed = json.loads(next(item["text"] for item in response["content"] if item["type"] == "text"))
                    assert not failed["success"] and policy_error(failed), failed
                    assert retained.read_bytes() == built["ordinary"]
                    client.call("start_compile", {"path": str(retained)})
                    job = wait_job(client)
                    assert job["status"] == "failed" and not job["result"]["published"] and policy_error(job["result"]), job
                    assert retained.read_bytes() == built["ordinary"]
                    assert source.read_bytes() == source_bytes
                    assert detail(client, ids) == before
                    assert client.call("get_map_info")["undo"] == history
                    refusals.append({"shader": shader, "value": value, "patch": patch})
    finally:
        require_clean_eof(client)
    (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    (args.output / "refusals.json").write_text(json.dumps(refusals, indent=2) + "\n")
    return results


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=ROOT / "target/validation/bump-suppression")
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.binary = args.binary.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    results = validate(args)
    print(json.dumps({"bump_suppression": True, "profiles": len(results), "refusals": len(SHADERS) * (len(INVALID_VALUES) + 1), "output": str(args.output)}))


if __name__ == "__main__":
    main()
