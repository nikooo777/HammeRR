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
