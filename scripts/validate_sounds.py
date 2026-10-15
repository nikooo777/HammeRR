import argparse
import base64
import io
import json
import math
import os
import re
import struct
import time
import wave
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client
from validate_terrain_lod import build
from validate_visibility import ROOM


def audio(client, output, value, variant=0, volume=0.5):
    result = client.rpc(
        "tools/call",
        {
            "name": "render_sound_preview",
            "arguments": {"value": value, "variant": variant, "volume": volume},
        },
    )
    assert not result.get("isError"), result
    metadata = json.loads(
        next(item["text"] for item in result["content"] if item["type"] == "text")
    )
    content = next(item for item in result["content"] if item["type"] == "audio")
    assert content["mimeType"] == "audio/wav"
    data = base64.b64decode(content["data"])
    output.write_bytes(data)
    with wave.open(io.BytesIO(data)) as reader:
        assert reader.getnchannels() == 2 and reader.getsampwidth() == 2
        assert reader.getframerate() == 44100 and reader.getnframes() <= 441000
        samples = memoryview(reader.readframes(reader.getnframes())).cast("h")
        rms = math.sqrt(
            sum(sample * sample for sample in samples) / max(1, len(samples))
        )
    assert len(data) <= 2 * 1024 * 1024
    assert math.isclose(metadata["preview"]["volume"], volume, abs_tol=1e-6)
    return metadata, rms


def validate(args):
    os.environ["SDL_AUDIODRIVER"] = "dummy"
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call("create_block", {"mins": low, "maxs": high})
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [64, 64, 8]},
        )
        entity = client.call(
            "create_entity",
            {
                "classname": "ambient_generic",
                "origin": [0, 0, 32],
                "properties": {
                    "message": "",
                    "targetname": "sound_demo",
                    "spawnflags": "49",
                },
            },
        )["id"]
        inspector = client.call("get_entity_inspector", {"id": entity})
        field = next(row for row in inspector["properties"] if row["key"] == "message")
        assert field["control"] == "sound" and not field["advanced"], field
        catalog = client.call(
            "browse_sounds", {"query": "DoSpark", "group": "scripts", "limit": 10}
        )
        assert catalog["total"] == 1, catalog
        assert any(
            row["id"] == "script:scripts/game_sounds_ambient_generic.txt"
            for row in catalog["groups"]
        )
        cue = client.call("inspect_sound", {"value": "DoSpark", "variant": 1})
        assert len(cue["waves"]) == 5 and cue["path"] == "ambient/energy/zap2.wav"
        assert cue["pitch"] == "100,120" and cue["codec"].endswith("WAV")
        music = client.call(
            "browse_sounds",
            {"query": "music/ravenholm_1.mp3", "group": "files", "limit": 1},
        )
        assert music["total"] == 1
        assert (
            client.call("inspect_sound", {"value": "music/ravenholm_1.mp3"})["codec"]
            == "MP3"
        )
        for tool, invalid in [
            ("browse_sounds", {"limit": 101}),
            ("browse_sounds", {"typo": True}),
            ("inspect_sound", {"value": "../outside.wav"}),
            ("inspect_sound", {"value": "DoSpark", "variant": 999}),
            ("set_sound_preview_volume", {"volume": 1.1}),
            ("begin_sound_choice", {"id": entity, "key": "targetname"}),
        ]:
            client.call(tool, invalid, error=True)
        before = client.call("get_objects", {"ids": [entity]})
        history = client.call("get_map_info")["undo"]
        camera = client.call("get_camera")
        token = client.call("begin_sound_choice", {"id": entity, "key": "message"})[
            "token"
        ]
        assert (
            client.call(
                "begin_sound_choice", {"id": entity, "key": "message", "resume": True}
            )["token"]
            == token
        )
        client.call("choose_sound", {"token": token, "value": "DoSpark", "variant": 1})
        client.call(
            "choose_sound", {"token": token, "value": "missing.wav"}, error=True
        )
        assert client.call("get_sound_choice")["sound"]["variant"] == 1
        audible, high = audio(client, args.output / "spark.wav", "DoSpark", 1)
        silent, low = audio(client, args.output / "muted.wav", "DoSpark", 1, 0)
        assert high > 0 and low == 0
        music_audio, _ = audio(
            client, args.output / "music.wav", "music/ravenholm_1.mp3", volume=0.25
        )
        assert 9.99 <= music_audio["preview"]["duration"] <= 10.0
        client.call("set_sound_preview_volume", {"volume": 0.25})
        first = client.call("play_sound_preview", {"value": "music/ravenholm_1.mp3"})
        assert first["sound"]["preview"]["volume"] == 0.25
        time.sleep(0.2)
        assert client.call("get_sound_preview")["state"] == "playing"
        client.call("play_sound_preview", {"value": "missing.wav"}, error=True)
        assert client.call("get_sound_preview")["token"] == first["token"]
        second = client.call("play_sound_preview", {"value": "DoSpark", "variant": 1})
        client.call("stop_sound_preview", {"token": first["token"]}, error=True)
        assert client.call("get_sound_preview")["token"] == second["token"]
        client.call("stop_sound_preview", {"token": second["token"]})
        assert client.call("get_sound_preview")["state"] == "idle"
        assert not client.call("get_map_info")["rendering"]["initialized"]
        assert client.call("get_objects", {"ids": [entity]}) == before
        assert (
            client.call("get_map_info")["undo"] == history
            and client.call("get_camera") == camera
        )
        client.call("apply_sound_choice", {"token": token})
        assert (
            dict(
                client.call("get_objects", {"ids": [entity]})["objects"][0][
                    "properties"
                ]
            )["message"]
            == "DoSpark"
        )
        assert client.call("browse_sounds", {"group": "used"})["total"] == 1
        client.call("undo")
        assert client.call("get_objects", {"ids": [entity]}) == before
        client.call("redo")
        old = client.call("begin_sound_choice", {"id": entity, "key": "message"})[
            "token"
        ]
        current = client.call("begin_sound_choice", {"id": entity, "key": "message"})[
            "token"
        ]
        client.call("cancel_sound_choice", {"token": old}, error=True)
        client.call("choose_sound", {"token": current, "value": "DoSpark"})
        client.call(
            "transform_objects",
            {"ids": [entity], "translation": [8, 0, 0], "preview": True},
        )
        client.call("apply_sound_choice", {"token": current}, error=True)
        assert not client.call("get_sound_choice")["valid"]
        client.call("cancel_sound_choice", {"token": current})
        client.call("cancel_transform_preview")
        token = client.call("begin_sound_choice", {"id": entity, "key": "message"})[
            "token"
        ]
        client.call("choose_sound", {"token": token, "value": "DoSpark"})
        client.call("reload_sound_assets")
        assert not client.call("get_sound_choice")["valid"]
        client.call("cancel_sound_choice", {"token": token})
        client.call("set_compile_settings", {"lighting": "direct_ldr"})
        bsp, stats = build(client, args.output, "sounds")
        data = bsp.read_bytes()
        offset, length = struct.unpack_from("<ii", data, 8)
        entities = data[offset : offset + length].decode().rstrip("\0")
        block = next(
            block
            for block in re.findall(r"\{[^{}]*\}", entities)
            if '"sound_demo"' in block
        )
        assert re.search(r'"message"\s+"DoSpark"', block)
        vmf = args.output / "sounds.vmf"
        client.call("save_map", {"path": str(vmf)})
        client.call("open_map", {"path": str(vmf), "discard_changes": True})
        assert (
            dict(
                client.call("get_objects", {"ids": [entity]})["objects"][0][
                    "properties"
                ]
            )["message"]
            == "DoSpark"
        )
        (args.output / "validation.json").write_text(
            json.dumps(
                {
                    "catalog": catalog,
                    "cue": cue,
                    "audio": [audible, silent, music_audio],
                    "stats": stats,
                    "audio_driver": "dummy",
                },
                indent=2,
            )
            + "\n"
        )
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-sounds"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"sounds": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
