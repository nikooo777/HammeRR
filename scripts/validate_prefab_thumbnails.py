import argparse
import base64
import json
import os
import struct
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, detail
from validate_prefabs import launch, report


def render(client, name, refresh=False):
    result = client.rpc(
        "tools/call",
        {"name": "render_prefab_thumbnail", "arguments": {"name": name, "refresh": refresh}},
    )
    assert not result.get("isError"), result
    image = next(block for block in result["content"] if block["type"] == "image")
    png = base64.b64decode(image["data"])
    assert png.startswith(b"\x89PNG\r\n\x1a\n")
    assert struct.unpack(">II", png[16:24]) == (320, 240)
    metadata = json.loads(next(block["text"] for block in result["content"] if block["type"] == "text"))
    return png, metadata


def gallery(client, query="", offset=0, limit=8, style="flat"):
    result = client.rpc(
        "tools/call",
        {
            "name": "render_prefab_gallery",
            "arguments": {
                "snapshot": report(client)["snapshot"],
                "query": query,
                "offset": offset,
                "limit": limit,
                "style": style,
            },
        },
    )
    assert not result.get("isError"), result
    metadata = json.loads(result["content"][0]["text"])
    images = {}
    for entry in metadata["entries"]:
        if "image_content_index" not in entry:
            assert isinstance(entry.get("error"), str), entry
            continue
        block = result["content"][entry["image_content_index"]]
        assert block["type"] == "image" and block["mimeType"] == "image/png", block
        png = base64.b64decode(block["data"])
        assert png.startswith(b"\x89PNG\r\n\x1a\n")
        assert struct.unpack(">II", png[16:24]) == (320, 240)
        images[entry["name"]] = png
    assert len(result["content"]) == len(images) + 1
    return metadata, images


def validate(args):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    library = args.output / "library"
    client = launch(args.binary, args.output / "session", library)
    try:
        schemas = {tool["name"]: tool["inputSchema"] for tool in client.rpc("tools/list", {})["tools"]}
        names = {"inspect_prefab_thumbnail", "render_prefab_thumbnail", "render_prefab_gallery"}
        check(names <= schemas.keys(), "Prefab thumbnail and gallery tools are discoverable")
        check(all(schemas[name].get("additionalProperties") is False for name in names), "Thumbnail tools reject unknown settings")
        brush = client.call("create_block", {"mins": [0, 0, 0], "maxs": [96, 32, 12], "material": "dev/missing_a"})["id"]
        upright = client.call("create_block", {"mins": [0, 0, 12], "maxs": [16, 32, 96], "material": "dev/missing_a"})["id"]
        point = client.call("create_entity", {"classname": "info_target", "origin": [64, 16, 32]})["id"]
        ids = [brush, upright, point]
        client.call("save_prefab", {"name": "Visual assembly", "ids": ids, "snapshot": report(client)["snapshot"]})
        client.call("save_prefab", {"name": "Smaller assembly", "ids": [brush], "snapshot": report(client)["snapshot"]})
        path = library / "Visual assembly.vmf"
        original = path.read_bytes()
        original_text = original.decode()
        before = client.call("inspect_prefab_thumbnail", {"name": "Visual assembly"})
        check(before["brushes"] == 2 and before["point_helpers"] == 1, "CPU inspection uses actual brush geometry and point helpers")
        check(before["style"] == "flat_geometry" and before["textures_loaded"] is False, "Preview style explicitly omits external textures")
        check(before["graphics"]["initialized"] is False, "CPU inspection leaves graphics uninitialized")
        client.call("copy_objects", {"ids": [brush], "snapshot": client.call("get_clipboard")["snapshot"]})
        client.call("preview_paste", {"offset": [160, 0, 0], "snapshot": client.call("get_clipboard")["snapshot"]})
        clipboard = client.call("get_clipboard")
        history = client.call("get_map_info")["undo"]
        camera = client.call("get_camera")
        geometry = detail(client, ids)

        rejected = client.call(
            "render_prefab_gallery", {"snapshot": report(client)["snapshot"], "limit": 9}, error=True
        )
        check(all(block["type"] != "image" for block in rejected["content"]), "Oversized gallery pages reject without images")
        unsupported, unsupported_images = gallery(client, style="textured")
        check(not unsupported_images and all(entry.get("error") and entry["style"] == "textured" for entry in unsupported["entries"]), "Unsupported Textured gallery entries report errors without Flat substitution")
        check(client.call("inspect_prefab_thumbnail", {"name": "Visual assembly"})["graphics"]["initialized"] is False, "Rejected and unsupported gallery requests keep graphics lazy")

        if not args.cpu_only:
            png, metadata = render(client, "Visual assembly")
            (args.output / "prefab-thumbnail.png").write_bytes(png)
            check(metadata["cached"] is False, "First PNG is rendered from the detached assembly")
            cached_png, cached = render(client, "Visual assembly")
            check(cached["cached"] is True and cached_png == png, "Repeated retrieval returns identical cached PNG pixels")
            refreshed_png, refreshed = render(client, "Visual assembly", refresh=True)
            check(refreshed["cached"] is False and refreshed_png == png, "Explicit refresh regenerates the same geometry image")
            page, images = gallery(client)
            check([entry["name"] for entry in page["entries"]] == ["Smaller assembly", "Visual assembly"], "Gallery preserves the reviewed ordered library page")
            check(set(images) == {"Smaller assembly", "Visual assembly"} and images["Visual assembly"] == png, "Named gallery content indices match actual selected-thumbnail PNGs")
            filtered, filtered_images = gallery(client, query="Visual", limit=1)
            check(len(filtered["entries"]) == 1 and set(filtered_images) == {"Visual assembly"}, "Gallery query and limit render only the requested page")
            broken = library / "A broken.vmf"
            broken.write_text("invalid source")
            mixed, mixed_images = gallery(client)
            check(mixed["entries"][0].get("error") and "image_content_index" not in mixed["entries"][0] and len(mixed_images) == 2, "A failed card has no image while valid sibling cards retain named PNGs")
            broken.unlink()
            stale = report(client)["snapshot"]
            broken.write_text("invalid source")
            failed = client.call("render_prefab_gallery", {"snapshot": stale}, error=True)
            check(all(block["type"] != "image" for block in failed["content"]), "A stale gallery review discards the whole image page")
            broken.unlink()
            extra = []
            for index in range(7):
                path_extra = library / f"Extra {index}.vmf"
                path_extra.write_bytes((library / "Smaller assembly.vmf").read_bytes())
                extra.append(path_extra)
            bounded, bounded_images = gallery(client)
            check(len(bounded["entries"]) == len(bounded_images) == 8 and bounded["next_offset"] == 8, "Gallery rendering retains the eight-card page bound")
            for path_extra in extra:
                path_extra.unlink()
            primed_png, _ = render(client, "Visual assembly")
            check(primed_png == png and client.call("inspect_prefab_thumbnail", {"name": "Visual assembly"})["cached"] is True, "Visual source is cached before exact-byte invalidation coverage")
            previous = path.stat()
            changed = original.replace(b"DEV/MISSING_A", b"DEV/MISSING_B")
            check(changed != original and len(changed) == len(original), "Same-size source edit fixture changes real VMF bytes")
            path.write_bytes(changed)
            os.utime(path, ns=(previous.st_atime_ns, previous.st_mtime_ns))
            check(path.stat().st_size == previous.st_size and path.stat().st_mtime_ns == previous.st_mtime_ns, "Source edit preserves exact file size and mtime")
            inspected = client.call("inspect_prefab_thumbnail", {"name": "Visual assembly"})
            check(inspected["cached"] is False, "Exact-byte revalidation catches changed content with preserved size and mtime")
            changed_png, changed_report = render(client, "Visual assembly")
            check(changed_report["cached"] is False and changed_png == png, "Flat image is regenerated after a material-only source edit")
            path.write_bytes((library / "Smaller assembly.vmf").read_bytes())
            smaller_png, smaller = render(client, "Visual assembly")
            check(smaller_png != png and smaller["brushes"] == 1, "Changed source shape produces changed actual PNG content")

        for text, expected in [
            ("invalid source", "source"),
            (" " * (2 * 1024 * 1024 + 1), "2 MiB"),
            (original_text + '\nentity { "id" "1000" "classname" "func_instance" "origin" "0 0 0" "file" "missing.vmf" }', "func_instance"),
            (original_text + '\nentity { "id" "1000" "classname" "prop_dynamic" "origin" "0 0 0" "model" "models/definitely_missing.mdl" }', "model"),
        ]:
            path.write_text(text)
            inspected = client.call("inspect_prefab_thumbnail", {"name": "Visual assembly"})
            check(expected in inspected["error"], f"CPU inspection exposes {expected} failures")
            failed = client.call("render_prefab_thumbnail", {"name": "Visual assembly"}, error=True)
            check(all(block["type"] != "image" for block in failed["content"]), f"Failed {expected} rendering returns no substitute image")
            if args.cpu_only:
                check(inspected["graphics"]["initialized"] is False, "Source failure does not initialize graphics")

        path.write_bytes(original)
        renamed = library / "Renamed assembly.vmf"
        path.rename(renamed)
        check("no longer exists" in client.call("inspect_prefab_thumbnail", {"name": "Visual assembly"})["error"], "Renamed source cannot retain old cached pixels")
        check("error" not in client.call("inspect_prefab_thumbnail", {"name": "Renamed assembly"}), "Renamed source remains available through its current name")
        if not args.cpu_only:
            repaired_png, repaired = render(client, "Renamed assembly")
            check(repaired_png == png and repaired["cached"] is False, "Repaired source renders successfully under its new name")
        renamed.unlink()
        check("no longer exists" in client.call("inspect_prefab_thumbnail", {"name": "Renamed assembly"})["error"], "Deleted source clears cached visual success")
        check(client.call("get_clipboard") == clipboard, "Visual browsing retains the owned clipboard and placement draft")
        check(client.call("get_map_info")["undo"] == history and detail(client, ids) == geometry, "Visual browsing preserves map geometry and Undo")
        check(client.call("get_camera") == camera, "Isolated thumbnails preserve the active map camera")
        result = {"passed": len(checks), "checks": checks, "graphics": not args.cpu_only, "desktop_input": False}
        (args.output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"passed": len(checks), "output": str(args.output)}))
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cpu-only", action="store_true")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    validate(args)


if __name__ == "__main__":
    main()
