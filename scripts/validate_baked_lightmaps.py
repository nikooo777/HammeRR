import argparse
import base64
import hashlib
import json
import math
import struct
from pathlib import Path

from validate_compile_jobs import wait_job
from validate_direct_lighting import FACE_FORMAT
from validate_face_lighting import DEFAULT_BINARY, Client
from validate_material_replacement import preserved_map
from validate_visibility import ROOM


def inspection(client, **arguments):
    result = client.rpc(
        "tools/call", {"name": "inspect_baked_lightmaps", "arguments": arguments}
    )
    assert not result.get("isError"), result
    report = json.loads(next(item["text"] for item in result["content"] if item["type"] == "text"))
    images = [item for item in result["content"] if item["type"] == "image"]
    image = base64.b64decode(images[0]["data"]) if images else None
    if image:
        assert image[:8] == b"\x89PNG\r\n\x1a\n"
        width, height = struct.unpack_from(">II", image, 16)
        assert 0 < width <= 128 and 0 < height <= 128
        assert [width, height] == [report["selected"]["preview"]["width"], report["selected"]["preview"]["height"]]
    return report, image


def lump(data, index):
    offset, size, _, compression = struct.unpack_from("<4i", data, 8 + index * 16)
    assert compression == 0 and 0 <= offset <= offset + size <= len(data)
    return data[offset:offset + size]


def check_source_samples(path, report):
    data = path.read_bytes()
    selected = report["selected"]
    face_lump, samples_lump = (7, 8) if report["channel"] == "ldr" else (58, 53)
    faces = list(struct.iter_unpack(FACE_FORMAT, lump(data, face_lump)))
    face = faces[selected["face"]]
    width, height = face[16] + 1, face[17] + 1
    infos = list(struct.iter_unpack("<16f2i", lump(data, 6)))
    maps = 4 if infos[face[5]][16] & 0x800 else 1
    styles = [style for style in face[8:12] if style != 255]
    assert selected["dimensions"] == [width, height]
    assert selected["styles"] == styles and selected["maps"] == maps
    assert selected["style"] == styles[selected["style_index"]]
    count = width * height
    offset = face[12] + (selected["style_index"] * maps + selected["map_index"]) * count * 4
    payload = lump(data, samples_lump)
    samples = [[math.ldexp(channel, exponent) for channel in rgb]
               for *rgb, exponent in struct.iter_unpack("<3Bb", payload[offset:offset + count * 4])]
    assert len(samples) == selected["sample_count"] == count
    assert selected["selected_sample_bytes"] == count * 4
    for key, expected in [
        ("minimum", [min(sample[axis] for sample in samples) for axis in range(3)]),
        ("maximum", [max(sample[axis] for sample in samples) for axis in range(3)]),
        ("mean", [sum(sample[axis] for sample in samples) / count for axis in range(3)]),
    ]:
        assert all(math.isclose(a, b, rel_tol=1e-10, abs_tol=1e-10)
                   for a, b in zip(selected["linear_rgb"][key], expected)), (key, selected, expected)


def compile_map(client, output, name, lighting):
    client.call("set_compile_settings", {"lighting": lighting, "physics": "none", "bounces": 0})
    path = output / name
    client.call("start_compile", {"path": str(path)})
    job = wait_job(client)
    assert job["status"] == "completed", job
    return path


def validate(client, output):
    checks = []
    with preserved_map(client):
        report, image = inspection(client)
        assert report["build"]["status"] == "unavailable" and image is None
        checks.append("No owned successful build reports unavailable")
        for low, high in ROOM:
            client.call("create_block", {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"})
        client.call("create_entity", {"classname": "info_player_terrorist", "origin": [0, 0, 8]})
        client.call("create_entity", {
            "classname": "light", "origin": [-96, 0, 160],
            "properties": {"_light": "255 0 0 80", "_lightHDR": "0 0 255 1600"},
        })
        client.call("create_entity", {
            "classname": "light", "origin": [96, 0, 160],
            "properties": {"targetname": "inspection_switch", "spawnflags": "1", "_light": "0 0 0 0", "_lightHDR": "0 255 0 160"},
        })
        source = output / "lightmaps.vmf"
        client.call("save_map", {"path": str(source)})
        original_source = source.read_bytes()
        source_hash = hashlib.sha256(original_source).hexdigest()
        before = client.call("get_map_info")
        compile_map(client, output, "fullbright.bsp", "fullbright")
        report, image = inspection(client)
        assert image is None and not any(channel["available"] for channel in report["channels"])
        assert report["diagnostics"]
        checks.append("Fullbright exposes missing lighting diagnostics and no image")
        path = compile_map(client, output, "hdr.bsp", "direct_ldr_hdr")
        reports = {}
        for channel in ["ldr", "hdr"]:
            report, image = inspection(client, channel=channel)
            assert report["build"]["current"] and report["build"]["status"] == "current"
            assert all(item["available"] for item in report["channels"])
            assert image and report["selected"]["sample_count"] > 0
            check_source_samples(path, report)
            (output / f"{channel}.png").write_bytes(image)
            reports[channel] = report
        assert reports["ldr"]["selected"]["linear_rgb"]["maximum"][0] > 0
        assert reports["ldr"]["selected"]["linear_rgb"]["maximum"][2] == 0
        assert reports["hdr"]["selected"]["linear_rgb"]["maximum"][2] > 0
        checks.append("Independent LDR/HDR CPU PNGs and linear intensities match exact BSP bytes")
        hdr = reports["hdr"]
        styled = next(face for face in hdr["faces"] if 32 in face.get("styles", []))
        styled_report, image = inspection(client, channel="hdr", face=styled["face"], style_index=styled["styles"].index(32))
        assert image and styled_report["selected"]["style"] == 32
        check_source_samples(path, styled_report)
        checks.append("Exact nonzero light style selection matches its BSP range")
        paged, _ = inspection(client, offset=1, limit=2)
        assert [face["face"] for face in paged["faces"]] == [1, 2]
        checks.append("Face pagination preserves compiled indices")
        for arguments in [{"channel": "unknown"}, {"limit": 33}, {"map_index": 4}]:
            client.call("inspect_baked_lightmaps", arguments, error=True)
        client.call("set_compile_settings", {"lighting": "direct_ldr"})
        stale, image = inspection(client)
        assert stale["build"]["status"] == "stale" and not stale["build"]["current"] and image
        client.call("set_compile_settings", {"lighting": "direct_ldr_hdr"})
        assert inspection(client)[0]["build"]["current"]
        checks.append("Changed settings mark the successful build stale until restored")
        source.write_bytes(original_source + b"\n// external source change\n")
        stale, image = inspection(client)
        assert stale["build"]["status"] == "stale" and not stale["build"]["current"] and image
        source.write_bytes(original_source)
        checks.append("External source change remains inspectable with explicit stale provenance")
        bsp_bytes = path.read_bytes()
        path.write_bytes(bsp_bytes[:-1] + bytes([bsp_bytes[-1] ^ 1]))
        changed, image = inspection(client)
        assert changed["build"]["status"] == "unavailable" and image is None
        path.unlink()
        missing, image = inspection(client)
        assert missing["build"]["status"] == "unavailable" and image is None
        checks.append("Changed and missing BSPs cannot be mistaken for the owned build")
        after = client.call("get_map_info")
        for key in ["undo", "redo", "camera", "selection", "face_selection", "unsaved_changes"]:
            assert after[key] == before[key], key
        assert hashlib.sha256(source.read_bytes()).hexdigest() == source_hash
        if not client.url:
            assert not after["rendering"]["initialized"]
        checks.append("Inspection preserves document state and leaves headless graphics uninitialized")
        (output / "reports.json").write_text(json.dumps({**reports, "styled": styled_report, "stale_source": stale, "changed": changed, "missing": missing}, indent=2) + "\n")
    result = {"checks": len(checks), "passed": checks, "transport": "http" if client.url else "stdio"}
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"checks": len(checks), "transport": result["transport"]}))


def main():
    parser = argparse.ArgumentParser(description="Compile and inspect bounded baked BSP lightmaps through MCP without graphics or game launch.")
    parser.add_argument("--url")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--gameinfo", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    client = Client(args.url, args.binary.resolve(), output, args.gameinfo)
    try:
        validate(client, output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
