import argparse
import json
from pathlib import Path

from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM, cluster_at


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    check(not client.call("preview_material", {"material": "tools/toolsskybox"})["missing"], "Installed CS:S sky material resolves")
    for label in ["closed", "sky", "doorway", "disconnected", "bent", "u_turn", "hidden", "submodel"]:
        client.call("new_map", {"discard_changes": True})
        boxes = list(ROOM)
        if label in ["doorway", "disconnected", "bent"]:
            boxes.pop(1)
            boxes.extend([([-272, -272, 256], [0, 272, 272]), ([0, -272, 256], [272, 272, 272])])
            boxes.extend([([-8, -256, 0], [8, -32, 256]), ([-8, 32, 0], [8, 256, 256])] if label == "doorway" else [([-8, -256, 0], [8, 128, 256]), ([-256, -8, 0], [-8, 8, 256])] if label == "bent" else [([-8, -256, 0], [8, 256, 256])])
        if label == "u_turn":
            boxes = list(ROOM)
            boxes.pop(1)
            boxes.extend([
                ([-272, -272, 256], [0, -8, 272]),
                ([-272, -8, 256], [0, 272, 272]),
                ([0, -272, 256], [272, 272, 272]),
                ([-8, -256, 0], [8, 128, 256]),
                ([-240, -8, 0], [-8, 8, 256]),
                ([8, -8, 0], [240, 8, 256]),
            ])
        ids = []
        for mins, maxs in boxes:
            material = "tools/toolsskybox" if (label in ["doorway", "disconnected"] and mins == [-272, -272, 256]) or (label == "bent" and mins == [0, -272, 256]) or (label == "u_turn" and mins == [-272, -272, 256]) else "dev/dev_measuregeneric01"
            ids.append(client.call("create_block", {"mins": mins, "maxs": maxs, "material": material})["id"])
        spawns = [("info_player_terrorist", [-64, -64, 8])]
        if label != "u_turn":
            spawns.append(("info_player_counterterrorist", [64, -64, 8]))
        for classname, origin in spawns:
            ids.append(client.call("create_entity", {"classname": classname, "origin": origin})["id"])
        if label == "sky":
            ceiling = detail(client, [ids[1]])[0]
            side = next(face for face in ceiling["faces"] if face["normal"] == [0, 0, -1])
            client.call("select_faces", {"faces": [{"solid": ceiling["solid"], "side": side["side"]}]})
            client.call("edit_faces", {"material": "tools/toolsskybox"})
        if label in ["hidden", "submodel"]:
            body = client.call("create_block", {"mins": [-32, -32, 64], "maxs": [32, 32, 96], "material": "tools/toolsskybox"})["id"]
            if label == "hidden":
                client.call("set_object_visibility", {"ids": [body], "action": "hide"})
                ids.append(body)
            else:
                ids.append(client.call("tie_to_entity", {"ids": [body], "classname": "func_brush"})["id"])
        vmf = output / f"{label}.vmf"
        client.call("save_map", {"path": str(vmf)})
        client.call("open_map", {"path": str(vmf)})
        source = vmf.read_bytes()
        before = detail(client, ids)
        history = client.call("get_map_info")["undo"]
        bsp = output / f"{label}.bsp"
        result = client.call("compile_map", {"path": str(bsp)})
        check(result["success"], f"{label}: native map compiles")
        data = bsp.read_bytes()
        leaves = records(data, 10, "<ihh6h4Hhh", expected_version=1)
        clusters = {leaf[1]: (leaf[2] >> 9) & 127 for leaf in leaves if leaf[1] >= 0}
        check(all(not flags & 2 for flags in clusters.values()), f"{label}: no radial flag without radial culling")
        check(all(((leaf[2] >> 9) & 127) == 0 for leaf in leaves if leaf[1] < 0), f"{label}: solid and submodel leaves retain no sky flags")
        count = sum(bool(flags & 5) for flags in clusters.values())
        check(result["stats"]["sky_clusters"] == count, f"{label}: shared sky count matches native cluster flags")
        if label in ["closed", "hidden", "submodel"]:
            check(count == 0, f"{label}: omitted sky cannot mark world clusters")
        elif label == "disconnected":
            check(clusters[cluster_at(data, [-64, -64, 64])] == 1 and clusters[cluster_at(data, [64, -64, 64])] == 0, "Disconnected room cannot inherit sky through an opaque wall")
        elif label == "u_turn":
            check(clusters[cluster_at(data, [-64, -64, 64])] == 1 and clusters[cluster_at(data, [64, -64, 64])] == 1 and clusters[cluster_at(data, [248, 0, 64])] == 0, "Immutable sky seeds cannot flood through a connected U-turn chain")
            check(count > 0 and cluster_at(data, [248, 0, 64]) >= 0, "One flood seed reaches the complete U-turn passage")
        elif label == "bent":
            check(clusters[cluster_at(data, [-64, -64, 64])] == 0 and clusters[cluster_at(data, [64, -64, 64])] == 1 and clusters[cluster_at(data, [-64, 192, 64])] == 1, "A sealed wing does not inherit the visible wing's sky")
        else:
            check(clusters[cluster_at(data, [-64, -64, 64])] == 1 and clusters[cluster_at(data, [64, -64, 64])] == 1, f"{label}: directly or portal-visible sky marks both sampled spaces")
        check(before == detail(client, ids) and history == client.call("get_map_info")["undo"] and source == vmf.read_bytes(), f"{label}: sky compilation preserves source and history")
        path = output / f"{label}-background.bsp"
        client.call("start_compile", {"path": str(path)})
        job = wait_job(client)
        check(job["status"] == "completed" and job["result"]["stats"]["sky_clusters"] == count, f"{label}: background Session exposes the same sky count")
        check(path.read_bytes() == data, f"{label}: both compile APIs emit identical sky flags")
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-sky-flags"))
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
