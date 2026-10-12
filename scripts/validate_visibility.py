import argparse
import json
import struct
from pathlib import Path

from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import ROOT, Client, detail


ROOM = [
    ([-272, -272, -16], [272, 272, 0]),
    ([-272, -272, 256], [272, 272, 272]),
    ([-272, -272, 0], [-256, 272, 256]),
    ([256, -272, 0], [272, 272, 256]),
    ([-256, -272, 0], [256, -256, 256]),
    ([-256, 256, 0], [256, 272, 256]),
]


def visibility(data):
    offset, length, version, compression = struct.unpack_from("<4i", data, 8 + 4 * 16)
    assert version == 0 and compression == 0 and length <= 16 * 1024 * 1024
    payload = data[offset:offset + length]
    count = struct.unpack_from("<i", payload)[0]
    assert 0 < count <= 32768
    width = (count + 7) // 8
    rows = []
    for cluster in range(count):
        pair = []
        for kind in range(2):
            cursor = struct.unpack_from("<i", payload, 4 + cluster * 8 + kind * 4)[0]
            assert 4 + count * 8 <= cursor < len(payload)
            row = []
            while len(row) < width:
                value = payload[cursor]
                cursor += 1
                if value:
                    row.append(value)
                else:
                    run = payload[cursor]
                    cursor += 1
                    assert 0 < run <= width - len(row)
                    row.extend([0] * run)
            assert count % 8 == 0 or row[-1] >> (count % 8) == 0
            pair.append(row)
        rows.append(pair)
    return count, length, rows


def cluster_at(data, point):
    model = records(data, 14, "<9f3i")[0]
    nodes = records(data, 5, "<3i6h2H2h")
    leaves = records(data, 10, "<ihh6h4Hhh", expected_version=1)
    planes = records(data, 1, "<4fi")
    cursor = model[9]
    while cursor >= 0:
        node = nodes[cursor]
        plane = planes[node[0]]
        distance = sum(a * b for a, b in zip(plane[:3], point, strict=True)) - plane[3]
        cursor = node[1 + int(distance < 0)]
    cluster = leaves[-1 - cursor][1]
    assert cluster >= 0
    return cluster


def bit(row, cluster):
    return bool(row[cluster // 8] & (1 << (cluster % 8)))


def blocked(a, b, boxes):
    for mins, maxs in boxes:
        enter, leave = 0.0, 1.0
        intersects = True
        for axis in range(3):
            direction = b[axis] - a[axis]
            if direction == 0:
                if not mins[axis] <= a[axis] <= maxs[axis]:
                    intersects = False
                    break
                continue
            low = (mins[axis] - a[axis]) / direction
            high = (maxs[axis] - a[axis]) / direction
            enter = max(enter, min(low, high))
            leave = min(leave, max(low, high))
            if enter > leave:
                intersects = False
                break
        if intersects:
            return True
    return False


def validate(client, output):
    checks = []
    metrics = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    for label, walls in [
        ("room", []),
        ("disconnected", [([-8, -256, 0], [8, 256, 256])]),
        ("doorway", [([-8, -256, 0], [8, -32, 256]), ([-8, 32, 0], [8, 256, 256])]),
        ("bent", [([-8, -256, 0], [8, 128, 256]), ([-256, -8, 0], [-8, 8, 256])]),
        ("detail", []),
        ("submodel", []),
    ]:
        client.call("new_map", {"discard_changes": True})
        ids = []
        for mins, maxs in ROOM + walls:
            ids.append(client.call("create_block", {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"})["id"])
        for classname, origin in [("info_player_terrorist", [-64, -64, 8]), ("info_player_counterterrorist", [64, -64, 8])]:
            ids.append(client.call("create_entity", {"classname": classname, "origin": origin})["id"])
        if label in ["detail", "submodel"]:
            body = client.call("create_block", {"mins": [-32, -32, 64], "maxs": [32, 32, 128], "material": "dev/dev_measuregeneric01"})["id"]
            ids.append(client.call("tie_to_entity", {"ids": [body], "classname": "func_detail" if label == "detail" else "func_brush"})["id"])
        vmf = output / f"{label}.vmf"
        client.call("save_map", {"path": str(vmf)})
        client.call("open_map", {"path": str(vmf)})
        source = vmf.read_bytes()
        before = detail(client, ids)
        history = client.call("get_map_info")["undo"]
        bsp = output / f"{label}.bsp"
        result = client.call("compile_map", {"path": str(bsp)})
        data = bsp.read_bytes()
        count, size, rows = visibility(data)
        check(result["success"] and result["leak"] is None, f"{label}: sealed native map compiles")
        check(result["stats"]["clusters"] == count and result["stats"]["visibility_bytes"] == size, f"{label}: shared metrics match native visibility records")
        check(any("conservative portal bounds" in diagnostic["message"] for diagnostic in result["diagnostics"]), f"{label}: diagnostics state the current visibility mode")
        check(before == detail(client, ids) and history == client.call("get_map_info")["undo"] and source == vmf.read_bytes(), f"{label}: compilation preserves source and history")
        check(all(bit(pair[0], index) and bit(pair[1], index) for index, pair in enumerate(rows)), f"{label}: all clusters see and hear themselves")
        check(all(bit(rows[a][0], b) == bit(rows[b][0], a) for a in range(count) for b in range(count)), f"{label}: visible sets are reciprocal")
        check(all(not bit(pair[0], index) or bit(pair[1], index) for pair in rows for index in range(count)), f"{label}: audible sets include every visible cluster")
        background = output / f"{label}-background.bsp"
        client.call("start_compile", {"path": str(background)})
        job = wait_job(client)
        check(job["status"] == "completed" and job["result"]["stats"]["clusters"] == count and job["result"]["stats"]["visibility_bytes"] == size, f"{label}: background Session/MCP status exposes the same metrics")
        check(data == background.read_bytes(), f"{label}: both compile APIs emit identical BSP bytes")
        if label in ["room", "detail", "submodel"]:
            check(count == 1, f"{label}: detail and model geometry do not split world visibility")
        a = cluster_at(data, [-64, -64, 64])
        b = cluster_at(data, [64, -64, 64])
        if label == "disconnected":
            check(count == 2 and a != b and not bit(rows[a][0], b) and not bit(rows[a][1], b), "Disconnected rooms do not share visibility or audibility")
        if label == "bent":
            check(count > 1 and not bit(rows[a][0], b), "Bent structural passage culls the occluded room")
        if label == "doorway":
            check(count > 1 and bit(rows[a][0], b), "Doorway retains direct room visibility")
        points = [(x, y, z) for x in [-192, -64, 64, 192] for y in [-192, -64, 64, 192] for z in [16, 128, 240]]
        clear = 0
        for start in points:
            for end in points:
                if not blocked(start, end, ROOM + walls):
                    clear += 1
                    assert bit(rows[cluster_at(data, start)][0], cluster_at(data, end)), f"{label}: clear sightline {start} -> {end} was culled"
        check(clear > len(points) * 10, f"{label}: every independently checked clear sightline stays visible")
        metrics.append({"fixture": label, "clusters": count, "portals": result["stats"]["visibility_portals"], "bytes": size, "clear_sightlines": clear})
        if label == "submodel":
            nodes = records(data, 5, "<3i6h2H2h")
            leaves = records(data, 10, "<ihh6h4Hhh", expected_version=1)
            model = records(data, 14, "<9f3i")[1]
            pending = [model[9]]
            while pending:
                cursor = pending.pop()
                if cursor >= 0:
                    pending.extend(nodes[cursor][1:3])
                else:
                    assert leaves[-1 - cursor][1] == -1
            check(True, "Brush submodel leaves have no world visibility cluster")
    return checks, metrics


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-visibility"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks, metrics = validate(client, args.output)
        (args.output / "results.json").write_text(json.dumps({"checks": len(checks), "passed": checks, "metrics": metrics}, indent=2) + "\n")
        print(json.dumps({"checks": len(checks), "metrics": metrics, "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
