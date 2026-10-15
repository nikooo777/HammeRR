import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail


def validate(client, output, render):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def inspect():
        return client.call("get_brush_components")

    def style(draft, refs, texture):
        return client.call("update_component_partition_texture", {
            "token": draft["token"], "partitions": refs, "texture": texture,
        })["preview"]

    def resume(solid, side):
        return client.call("resume_component_partitions", {
            "sources": [{"solid": solid, "side": side}],
            "snapshot": inspect()["snapshot"],
        })["preview"]

    tools = {tool["name"]: tool["inputSchema"] for tool in client.rpc("tools/list", {})["tools"]}
    check("update_component_partition_texture" in tools, "Texture placement is discoverable")
    check("place_component_partition_texture" in tools, "Per-piece Fit and Justify are discoverable")

    def place(draft, refs, **settings):
        return client.call("place_component_partition_texture", {
            "token": draft["token"], "partitions": refs, **settings,
        })["preview"]

    def piece_bounds(draft):
        part = draft["partition"]["partitions"][0]
        extruded = client.call("preview_partition_extrusion", {
            "token": draft["token"], "height": 16.125, "texture_lock": True,
        })["preview"]
        bounds = {}
        for output_piece in extruded["extrusion"]["pieces"]:
            piece = next(piece for piece in part["loops"] if piece["id"] == output_piece["partition"])
            top = next(face for face in output_piece["faces"] if face["surface"] == "top")
            values = []
            for vertex in piece["vertices"]:
                point = part["vertices"][vertex]["position"].copy()
                point[2] += 16.125
                values.append([
                    sum(a * b for a, b in zip(top[key]["axis"], point, strict=True)) / top[key]["scale"] + top[key]["offset"]
                    for key in ["uaxis", "vaxis"]
                ])
            bounds[piece["id"]] = (
                [min(value[axis] for value in values) for axis in range(2)],
                [max(value[axis] for value in values) for axis in range(2)],
            )
        flat = client.call("raise_component_partition", {"token": extruded["token"], "rise": 0})["preview"]
        return flat, bounds

    textures = [
        {"shift": [13.125, -27.5], "scale": [-0.375, 0.625], "rotation": 37},
        {"shift": [-41.75, 6.25], "scale": [0.25, -0.5], "rotation": -73},
    ]
    for owned in [False, True]:
        for corners, direction in [(4, 0), (4, 1), (4, 2), (8, 3)]:
            label = f"owned={owned} corners={corners} pair={direction}"
            client.call("new_map", {"discard_changes": True})
            bounds = {"mins": [0, 0, 0], "maxs": [64, 64, 64], "material": "brick/brickwall001a"}
            if corners == 4:
                object_id = client.call("create_block", bounds)["id"]
            else:
                object_id = client.call("create_primitive", {**bounds, "shape": "cylinder", "segments": corners})["objects"][0]["id"]
            solid = int(object_id.split(":")[1])
            if owned:
                object_id = client.call("tie_to_entity", {"ids": [object_id], "classname": "func_detail"})["id"]
            source = detail(client, [object_id])[0]
            brush = source["brushes"][0] if owned else source
            side = max(brush["faces"], key=lambda face: face["normal"][2])["side"]
            selected = client.call("select_brush_components", {
                "components": [{"solid": solid, "sides": [side]}], "snapshot": inspect()["snapshot"],
            })
            edges = selected["partition_choices"][0]["pairs"][direction]["edges"]

            def stage():
                return client.call("preview_component_partition", {
                    "pairs": [{"solid": solid, "edges": edges}], "snapshot": inspect()["snapshot"],
                })["preview"]

            draft = stage()
            refs = [{"solid": solid, "partition": index} for index in range(2)]
            if render:
                client.call("set_camera", {"projection": "top", "center": [32, 32, 64], "ortho_height": 96})
                before = client.render(output / f"{int(owned)}-{corners}-{direction}-before.png")
            draft = style(draft, [refs[0]], textures[0])
            loops = draft["partition"]["partitions"][0]["loops"]
            check(loops[0]["texture_plan"] == textures[0] and loops[1]["texture_inherited"], f"{label}: one independent projection and inherited sibling")
            previous = draft["token"]
            draft = style(draft, [refs[1]], textures[1])
            for bad_refs, bad_texture in [([], textures[0]), ([refs[0], refs[0]], textures[0]), ([refs[0]], {**textures[0], "scale": [0, 1]}), ([refs[0]], {**textures[0], "rotation": 65537})]:
                client.call("update_component_partition_texture", {"token": draft["token"], "partitions": bad_refs, "texture": bad_texture}, error=True)
                check(inspect()["preview"]["token"] == draft["token"], f"{label}: invalid update retains draft")
            client.call("update_component_partition_texture", {"token": previous, "partitions": [refs[0]], "texture": None}, error=True)
            check(detail(client, [object_id])[0] == source, f"{label}: flat plans leave source geometry unchanged")
            if render:
                check(client.render(output / f"{int(owned)}-{corners}-{direction}-planned.png") != before, f"{label}: shared viewport renders planned UVs")
            client.call("cancel_component_edit", {"token": draft["token"]})
            draft = stage()
            if render:
                check(client.render(output / f"{int(owned)}-{corners}-{direction}-cancel.png") == before, f"{label}: cancellation restores exact pixels")
            draft = style(draft, [refs[0]], textures[0])
            draft = style(draft, [refs[1]], textures[1])
            client.call("store_component_partition", {"token": draft["token"]})
            saved = client.call("get_saved_component_partitions")["entries"][0]
            check(saved["valid"] and saved["top_textures"] == textures, f"{label}: Keep reports both bounded plans")
            path = output / f"{int(owned)}-{corners}-{direction}.vmf"
            client.call("save_map", {"path": str(path)})
            client.call("open_map", {"path": str(path), "discard_changes": True})
            check(client.call("get_saved_component_partitions")["entries"][0]["top_textures"] == textures, f"{label}: VMF round trip retains plans")
            kept = detail(client, [object_id])[0]
            draft = resume(solid, side)
            draft = style(draft, [refs[1]], None)
            check(draft["partition"]["partitions"][0]["loops"][1]["texture_inherited"], f"{label}: source placement restored independently")
            draft = style(draft, [refs[1]], textures[1])
            if draft["partition"]["ridge_supported"]:
                draft = client.call("raise_component_partition", {"token": draft["token"], "rise": 16.125, "roof_texture_lock": True})["preview"]
                client.call("update_component_partition_texture", {"token": draft["token"], "partitions": [refs[0]], "texture": None}, error=True)
                client.call("commit_component_edit", {"token": draft["token"]})
                check(not client.call("get_saved_component_partitions")["entries"], f"{label}: raised Apply consumes intent")
                client.call("undo")
                check(detail(client, [object_id])[0] == kept, f"{label}: undo restores source and saved plans")
                draft = resume(solid, side)
            draft = client.call("preview_partition_extrusion", {"token": draft["token"], "height": 32.125, "texture_lock": True})["preview"]
            check(len(draft["extrusion"]["pieces"]) == 2, f"{label}: both planned tops extrude")
            client.call("commit_component_edit", {"token": draft["token"]})
            client.call("undo")
            check(client.call("get_saved_component_partitions")["entries"][0]["top_textures"] == textures, f"{label}: extrusion undo retains plans")
            draft = resume(solid, side)
            draft = client.call("update_component_partition_cut", {"token": draft["token"], "edge_parameters": [0.2, 0.8]})["preview"]
            sizes = []
            for ref, material in zip(refs, ["brick/brickwall001a", "de_cbble/woodfloor01"], strict=True):
                preview = client.call("preview_material", {"material": material})
                check(not preview["missing"], f"{label}: actual dimensions available for {material}")
                sizes.append([preview["width"], preview["height"]])
                draft = client.call("update_component_partition_material", {"token": draft["token"], "partitions": [ref], "material": material})["preview"]
            previous = draft["token"]
            draft = place(draft, list(reversed(refs)), fit=True)
            fitted = [piece["texture_plan"] for piece in draft["partition"]["partitions"][0]["loops"]]
            for index, planned in enumerate(fitted):
                check(planned["rotation"] == textures[index]["rotation"] and all(a * b > 0 for a, b in zip(planned["scale"], textures[index]["scale"], strict=True)), f"{label}: Fit preserves rotation and mirroring for piece {index}")
            draft, bounds = piece_bounds(draft)
            for index, (lo, hi) in bounds.items():
                check(all(abs(value) < 1e-6 for value in lo) and all(abs(a - b) < 1e-6 for a, b in zip(hi, sizes[index], strict=True)), f"{label}: adjusted piece {index} fits one actual material tile")
            client.call("place_component_partition_texture", {"token": previous, "partitions": refs, "fit": True}, error=True)
            for bad_refs, settings in [([], {"fit": True}), ([refs[0], refs[0]], {"fit": True}), ([{"solid": solid, "partition": 9}], {"fit": True}), (refs, {})]:
                client.call("place_component_partition_texture", {"token": draft["token"], "partitions": bad_refs, **settings}, error=True)
                check(inspect()["preview"]["token"] == draft["token"], f"{label}: rejected Fit/Justify keeps current token")
            for index in range(2):
                draft = style(draft, [refs[index]], textures[index])
            sibling = draft["partition"]["partitions"][0]["loops"][1]["texture_plan"]
            draft = place(draft, [refs[0]], justify_u="end")
            one = draft["partition"]["partitions"][0]["loops"]
            check(one[0]["texture_plan"]["shift"][1] == textures[0]["shift"][1] and one[1]["texture_plan"] == sibling, f"{label}: one-axis justification preserves other axis and sibling")
            for placement in ["start", "center", "end"]:
                draft = place(draft, refs, justify_u=placement, justify_v=placement)
                planned = draft["partition"]["partitions"][0]["loops"]
                check(all(piece["texture_plan"]["scale"] == textures[index]["scale"] and piece["texture_plan"]["rotation"] == textures[index]["rotation"] for index, piece in enumerate(planned)), f"{label}: {placement} justification preserves scales and rotations")
                draft, bounds = piece_bounds(draft)
                for index, (lo, hi) in bounds.items():
                    actual = lo if placement == "start" else hi if placement == "end" else [(a + b) / 2 for a, b in zip(lo, hi, strict=True)]
                    expected = [0, 0] if placement == "start" else sizes[index] if placement == "end" else [size / 2 for size in sizes[index]]
                    check(all(abs(a - b) < 1e-6 for a, b in zip(actual, expected, strict=True)), f"{label}: piece {index} {placement} uses its material tile")
            draft = client.call("update_component_partition_material", {"token": draft["token"], "partitions": [refs[1]], "material": "missing/partition_fit_dimensions"})["preview"]
            client.call("place_component_partition_texture", {"token": draft["token"], "partitions": refs, "fit": True}, error=True)
            check(inspect()["preview"]["token"] == draft["token"], f"{label}: unavailable dimensions preserve all plans")
            client.call("cancel_component_edit", {"token": draft["token"]})
            check(client.call("get_saved_component_partitions")["entries"][0]["top_textures"] == textures, f"{label}: Cancel leaves kept placement intact")
            draft = resume(solid, side)
            draft = place(draft, refs, fit=True, justify_u="center", justify_v="center")
            fitted = [piece["texture_plan"] for piece in draft["partition"]["partitions"][0]["loops"]]
            client.call("store_component_partition", {"token": draft["token"]})
            check(client.call("get_saved_component_partitions")["entries"][0]["top_textures"] == fitted, f"{label}: Keep persists fitted texture metadata")
            client.call("undo")
            check(client.call("get_saved_component_partitions")["entries"][0]["top_textures"] == textures, f"{label}: Undo restores previous texture metadata atomically")

    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-partition-textures"))
    parser.add_argument("--render", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output, args.render)
        (args.output / "results.json").write_text(json.dumps({"checks": len(checks), "passed": checks}, indent=2) + "\n")
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
