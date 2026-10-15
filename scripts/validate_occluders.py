import argparse
import json
import struct
from pathlib import Path

from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM


def native_occlusion(data):
    offset, length, version, compression = struct.unpack_from("<4i", data, 8 + 16 * 9)
    assert version == 2 and compression == 0
    end = offset + length
    arrays = []
    for layout in ["<3i6fi", "<3i", "<i"]:
        count = struct.unpack_from("<i", data, offset)[0]
        offset += 4
        stride = struct.calcsize(layout)
        assert 0 <= count and offset + count * stride <= end
        arrays.append(list(struct.iter_unpack(layout, data[offset : offset + count * stride])))
        offset += count * stride
    assert offset == end
    occluders, polygons, indices = arrays
    vertices = records(data, 3, "<3f")
    planes = records(data, 1, "<4fi")
    areas = records(data, 20, "<2i")
    for flags, first, count, *bounds_area in occluders:
        assert flags in [0, 1] and count > 0 and 0 <= first < first + count <= len(polygons)
        mins, maxs, area = bounds_area[:3], bounds_area[3:6], bounds_area[6]
        assert 0 < area < len(areas)
        for start, size, plane_index in polygons[first : first + count]:
            assert size >= 3 and 0 <= start < start + size <= len(indices)
            plane = planes[plane_index]
            for (index,) in indices[start : start + size]:
                point = vertices[index]
                assert all(low <= value <= high for low, value, high in zip(mins, point, maxs, strict=True))
                assert abs(sum(a * b for a, b in zip(point, plane[:3], strict=True)) - plane[3]) < 0.02
    return occluders, polygons, indices


def room(client):
    client.call("new_map", {"discard_changes": True})
    for mins, maxs in ROOM:
        client.call("create_block", {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"})
    client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-64, 0, 8]})
    client.call("create_entity", {"classname": "info_player_counterterrorist", "origin": [64, 0, 8]})
    client.call("set_compile_settings", {"lighting": "fullbright", "physics": "none"})


def occluder(client, mins, maxs, active="1"):
    brush = client.call("create_block", {"mins": mins, "maxs": maxs, "material": "tools/toolsoccluder"})["id"]
    return client.call("tie_to_entity", {
        "ids": [brush], "classname": "func_occluder",
        "properties": {"targetname": "visibility_screen", "StartActive": active},
    })["id"]


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    room(client)
    baseline_path = output / "baseline.bsp"
    baseline = client.call("compile_map", {"path": str(baseline_path)})
    screen = occluder(client, [-8, -64, 16], [8, 64, 192], "0")
    client.call("set_entity_properties", {"id": screen, "set": {"occludernumber": "777", "model": "*77"}})
    vmf = output / "occluders.vmf"
    client.call("save_map", {"path": str(vmf)})
    client.call("open_map", {"path": str(vmf)})
    source = vmf.read_bytes()
    before = detail(client, [screen])
    undo = client.call("get_map_info")["undo"]
    path = output / "occluders.bsp"
    built = client.call("compile_map", {"path": str(path)})
    data = path.read_bytes()
    native, polygons, indices = native_occlusion(data)
    check(built["success"] and built["stats"]["occluders"] == len(native) == 1, "Synchronous MCP compile emits one native occluder")
    check(built["stats"]["occluder_polygons"] == len(polygons) == 6 and built["stats"]["occluder_vertex_refs"] == len(indices) == 24, "Shared build statistics match native polygon and vertex-index arrays")
    check(native[0][0] == 1 and native[0][3:9] == (-8, -64, 16, 8, 64, 192), "Start-inactive state and world-space bounds are emitted")
    check(all(built["stats"][key] == baseline["stats"][key] for key in ["brushes", "faces", "models", "clusters"]), "Occluder helper contributes no collision brush, render face, brush model or visibility cluster")
    offset, length, _, _ = struct.unpack_from("<4i", data, 8)
    entities = data[offset : offset + length].decode()
    check('"occludernumber" "0"' in entities and '"occludernumber" "777"' not in entities and '"model" "*77"' not in entities, "Runtime entity retains a fresh zero-based occluder index and consumes stale model binding")
    check(detail(client, [screen]) == before and client.call("get_map_info")["undo"] == undo and source == vmf.read_bytes(), "Compilation preserves authored helpers, saved VMF and undo history")
    background = output / "background.bsp"
    client.call("start_compile", {"path": str(background)})
    job = wait_job(client)
    check(job["status"] == "completed" and job["result"]["stats"]["occluders"] == 1 and background.read_bytes() == data, "Background Session/MCP build produces identical native records")
    check(any("Activate/Deactivate/Toggle" in item["message"] for item in built["diagnostics"]), "Build details explain supported runtime inputs")
    client.call("set_object_visibility", {"ids": [screen], "action": "hide"})
    hidden = output / "hidden.bsp"
    hidden_result = client.call("compile_map", {"path": str(hidden)})
    check(hidden_result["stats"]["occluders"] == 0 and native_occlusion(hidden.read_bytes())[0] == [], "Hidden helpers produce no native occluder or runtime index")
    client.call("undo")
    invalid = output / "invalid-active.vmf"
    invalid.write_text(source.decode().replace('"StartActive" "0"', '"StartActive" "2"'))
    client.call("open_map", {"path": str(invalid), "discard_changes": True})
    previous = output / "preserved.bsp"
    previous.write_bytes(b"previous successful build")
    failed = client.call("compile_map", {"path": str(previous)}, error=True)
    failure = json.loads(failed["content"][0]["text"])
    check(any(item["object"] == screen and item["level"] == "error" and "Start Active" in item["message"] for item in failure["diagnostics"]), "Invalid imported activation state returns an object-linked diagnostic")
    check(previous.read_bytes() == b"previous successful build", "Failed occluder build preserves previous BSP bytes")

    room(client)
    for mins, maxs in [([-8, -256, 0], [8, -64, 256]), ([-8, 64, 0], [8, 256, 256]), ([-8, -64, 192], [8, 64, 256])]:
        client.call("create_block", {"mins": mins, "maxs": maxs, "material": "tools/toolsnodraw"})
    portal = client.call("create_block", {"mins": [-4, -64, 0], "maxs": [4, 64, 192], "material": "tools/toolsareaportal"})["id"]
    client.call("tie_to_entity", {"ids": [portal], "classname": "func_areaportal", "properties": {"StartOpen": "1"}})
    occluder(client, [-160, -32, 16], [-144, 32, 160])
    occluder(client, [144, -32, 16], [160, 32, 160])
    split_path = output / "two-areas.bsp"
    result = client.call("compile_map", {"path": str(split_path)})
    native = native_occlusion(split_path.read_bytes())[0]
    check(result["stats"]["areas"] == 2 and len(native) == 2 and native[0][-1] != native[1][-1], "Occluders in separate rooms reference distinct native visibility areas")
    spanning = occluder(client, [-32, -16, 16], [32, 16, 160])
    failed = client.call("compile_map", {"path": str(previous)}, error=True)
    failure = json.loads(failed["content"][0]["text"])
    check(any(item["object"] == spanning and "spans 2 visibility areas" in item["message"] for item in failure["diagnostics"]), "Occluder crossing an areaportal is rejected with an actionable area diagnostic")
    check(previous.read_bytes() == b"previous successful build", "Area validation failure preserves previous output")
    room(client)
    brushes = [client.call("create_block", {"mins": low, "maxs": high, "material": "tools/toolsoccluder"})["id"] for low, high in [([-2, -96, 0], [2, -24, 192]), ([-2, 24, 0], [2, 96, 192])]]
    islands = client.call("tie_to_entity", {"ids": brushes, "classname": "func_occluder", "properties": {"targetname": "islands", "StartActive": "1"}})["id"]
    before = detail(client, [islands])
    undo = client.call("get_map_info")["undo"]
    from validate_terrain_lod import build
    path, stats = build(client, output, "occluder-islands")
    native, polygons, indices = native_occlusion(path.read_bytes())
    check(stats["occluders"] == len(native) == 1 and len(polygons) == 12 and len(indices) == 48, "Separated helper brushes share one index with their actual polygon islands")
    check(native[0][3:9] == (-2, -96, 0, 2, 96, 192) and stats["brushes"] == 6, "Combined bounds do not become collision or render geometry")
    check(detail(client, [islands]) == before and client.call("get_map_info")["undo"] == undo, "Multi-brush compilation preserves authored geometry and history")
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-occluders"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output)
        (args.output / "results.json").write_text(json.dumps({"checks": len(checks), "passed": checks}, indent=2) + "\n")
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
