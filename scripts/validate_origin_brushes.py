import argparse
import json
from pathlib import Path

from validate_authored_planes import dot, records
from validate_compile_jobs import wait_job
from validate_face_lighting import ROOT, Client, detail


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def room():
        client.call("new_map", {"discard_changes": True})
        for mins, maxs in [
            ([-272, -272, -16], [272, 272, 0]),
            ([-272, -272, 256], [272, 272, 272]),
            ([-272, -272, 0], [-256, 272, 256]),
            ([256, -272, 0], [272, 272, 256]),
            ([-256, -272, 0], [256, -256, 256]),
            ([-256, 256, 0], [256, 272, 256]),
        ]:
            client.call("create_block", {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"})
        client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-64, 0, 8]})

    def block(material, mins, maxs):
        return client.call("create_block", {"material": material, "mins": mins, "maxs": maxs})["id"]

    check(not client.call("preview_material", {"material": "tools/toolsorigin"})["missing"], "Installed CS:S origin material resolves")
    for classname in ["func_brush", "func_door_rotating", "trigger_multiple"]:
        for volume in [False, True]:
            label = f"{classname} volume={volume}"
            room()
            body = block("dev/dev_measuregeneric01", [0.125, 0.25, 64.375], [64.125, 64.25, 128.375])
            solid = int(body.split(":")[1])
            client.call("transform_objects", {"ids": [body], "rotation": [17, -23, 31]})
            ids = [body]
            if volume:
                ids.append(block("tools/toolsorigin", [28.125, 12.25, 60.375], [36.125, 20.25, 68.375]))
            owner = client.call("tie_to_entity", {"ids": ids, "classname": classname, "properties": {"origin": "80.125 48.25 96.375", "targetname": "pivot_test"}})["id"]
            vmf = output / f"{classname}-{int(volume)}.vmf"
            client.call("save_map", {"path": str(vmf)})
            client.call("open_map", {"path": str(vmf)})
            before = detail(client, [owner])
            history = client.call("get_map_info")["undo"]
            saved = vmf.read_bytes()
            topology = client.call("get_brush_components", {"solids": [solid]})["brushes"][0]
            world_points = [vertex["position"] for vertex in topology["vertices"]]
            by_side = {face["side"]: [world_points[index] for index in face["vertices"]] for face in topology["faces"]}
            bsp = output / f"{classname}-{int(volume)}.bsp"
            if volume:
                client.call("start_compile", {"path": str(bsp)})
                job = wait_job(client)
                check(job["status"] == "completed", f"{label}: shared background compilation succeeds")
                compiled = job["result"]
            else:
                compiled = client.call("compile_map", {"path": str(bsp)})
            check(compiled["success"] and compiled["leak"] is None, f"{label}: sealed native map compiles")
            check(detail(client, [owner]) == before and history == client.call("get_map_info")["undo"] and vmf.read_bytes() == saved, f"{label}: compilation preserves source and history")
            data = bsp.read_bytes()
            models = records(data, 14, "<9f3i")
            brushes = records(data, 18, "<3i")
            planes = records(data, 1, "<4fi")
            sides = records(data, 19, "<HhhBB")
            infos = records(data, 6, "<16f2i")
            pivot = [32.125, 16.25, 64.375] if volume else [80.125, 48.25, 96.375]
            local_points = [[value - offset for value, offset in zip(point, pivot, strict=True)] for point in world_points]
            check(len(models) == 2 and len(brushes) == 7, f"{label}: pivot volume emits no extra brush or model")
            check(models[1][6:9] == (0, 0, 0), f"{label}: BSP model origin stays zero")
            check(all(abs(models[1][axis] - min(point[axis] for point in local_points)) < 1e-4 and abs(models[1][axis + 3] - max(point[axis] for point in local_points)) < 1e-4 for axis in range(3)), f"{label}: native model bounds use local geometry")
            first, count, _ = brushes[-1]
            authored = [side for side in sides[first:first + count] if side[3] == 0]
            faces = next(brush for brush in before[0]["brushes"] if brush["solid"] == solid)["faces"]
            for native, face in zip(authored, faces, strict=True):
                plane = planes[native[0]]
                check(max(abs(dot(plane[:3], [value - offset for value, offset in zip(point, pivot, strict=True)]) - plane[3]) for point in by_side[face["side"]]) <= 0.01, f"{label}: authored local face {face['side']} is preserved")
                info = infos[native[1]]
                for index, key in enumerate(["uaxis", "vaxis"]):
                    axis = [float(value) for value in face[key].replace("[", "").replace("]", "").split()]
                    coefficients = [value / axis[4] for value in axis[:3]]
                    row = info[index * 4:index * 4 + 4]
                    check(all(abs(dot(row[:3], [value - offset for value, offset in zip(point, pivot, strict=True)]) + row[3] - dot(coefficients, point) - axis[3]) < 1e-4 for point in by_side[face["side"]]), f"{label}: face {face['side']} {key} retains world texels")
            offset = int.from_bytes(data[8:12], "little")
            length = int.from_bytes(data[12:16], "little")
            entities = data[offset:offset + length].decode()
            check(f'"origin" "{" ".join(str(value) for value in pivot)}"' in entities and '"model" "*1"' in entities, f"{label}: fractional runtime pivot and model link survive")

    for mode in ["world", "detail", "duplicate", "only"]:
        room()
        body = block("dev/dev_measuregeneric01", [0, 0, 64], [64, 64, 128])
        origin = block("tools/toolsorigin", [28, 12, 60], [36, 20, 68])
        if mode != "world":
            ids = [origin] if mode == "only" else [body, origin]
            if mode == "duplicate":
                ids.append(block("tools/toolsorigin", [0, 0, 60], [8, 8, 68]))
            client.call("tie_to_entity", {"ids": ids, "classname": "func_detail" if mode == "detail" else "func_brush"})
        destination = output / f"invalid-{mode}.bsp"
        destination.write_bytes(b"previous build")
        result = client.call("compile_map", {"path": str(destination)}, error=True)
        diagnostics = json.loads(result["content"][0]["text"])["diagnostics"]
        check(any(diagnostic["level"] == "error" and diagnostic["object"] and "origin" in diagnostic["message"].lower() for diagnostic in diagnostics), f"{mode}: shared diagnostics explain invalid origin")
        check(destination.read_bytes() == b"previous build", f"{mode}: rejection preserves previous output")
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-origin-brushes"))
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
