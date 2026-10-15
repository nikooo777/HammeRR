import argparse
import json
import math
from pathlib import Path

from validate_authored_planes import records
from validate_face_lighting import ROOT, Client, detail
from validate_terrain_lod import build
from validate_visibility import ROOM


OUTLINE = [[-128, -128], [128, -128], [128, -64], [-64, -64], [-64, 128], [-128, 128]]


def validate(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call("create_block", {"mins": low, "maxs": high, "material": "tools/toolsnodraw"})
        client.call("create_entity", {"classname": "info_player_terrorist", "origin": [192, 192, 8]})
        client.call("set_view", {"tool_textures": False})
        camera = client.call("get_camera")
        history = client.call("get_map_info")["undo"]
        source = args.output / "source.vmf"
        client.call("save_map", {"path": str(source)})
        original = source.read_bytes()
        request = {"outline": OUTLINE, "base": 0, "depth": 64, "snap": False, "material": "dev/dev_measuregeneric01", "preview": True}
        preview = client.call("create_polygon_brush", request)
        state = preview["state"]["preview"]
        assert len(state["brushes"]) == 4 and state["spec"]["outline"] == OUTLINE
        for key, expected in {"mins": [-128, -128, 0], "maxs": [128, 128, 64]}.items():
            assert all(math.isclose(a,b,abs_tol=1e-6) for a,b in zip(state["bounds"][key],expected,strict=True)), state["bounds"]
        assert client.call("get_map_info")["undo"] == history and source.read_bytes() == original
        client.render(args.output / "polygon-brushes.png", camera={"position": [-330, -350, 290], "look_at": [0, 0, 32]})
        assert client.call("get_camera") == camera
        for outline in [
            [[0, 0], [64, 64], [0, 64], [64, 0]],
            [[0, 0], [64, 0], [64, 64], [0, 0]],
            [[0, 0], [32, 0], [64, 0], [64, 64]],
            [[0, 0], [65537, 0], [0, 64]],
        ]:
            client.call("create_polygon_brush", {**request, "outline": outline}, error=True)
            assert client.call("get_primitive_preview")["preview"]["token"] == state["token"]
        client.call("create_polygon_brush", {**request, "depth": -1}, error=True)
        client.call("create_polygon_brush", {**request, "unknown": True}, error=True)
        client.call("commit_primitive_preview", {"token": state["token"]})
        ids = [brush["id"] for brush in state["brushes"]]
        applied = detail(client, ids)
        assert all(len(brush["faces"]) == 5 for brush in applied)
        client.call("undo")
        client.call("get_objects", {"ids": ids}, error=True)
        client.call("redo")
        assert detail(client, ids) == applied
        saved = args.output / "polygon-brushes.vmf"
        client.call("save_map", {"path": str(saved)})
        client.call("open_map", {"path": str(saved), "discard_changes": True})
        assert detail(client, ids) == applied
        bsp, stats = build(client, args.output, "polygon-brushes")
        assert len(records(bsp.read_bytes(), 18, "<3i")) == 10

        for axis in ["x", "y", "z"]:
            convex = client.call("create_polygon_brush", {**request, "axis": axis, "outline": [[0.25, 0.125], [32.25, 0.125], [16.25, 32.125]], "base": -16.5, "depth": 8.25})["state"]["preview"]
            assert len(convex["brushes"]) == 1
            client.call("cancel_primitive_preview", {"token": convex["token"]})
        client.call("set_spatial_settings", {"grid_size": 16, "snap": True})
        snapped = client.call("create_polygon_brush", {**request, "snap": True, "outline": [[1, 1], [65, 1], [1, 65]]})["state"]["preview"]
        assert snapped["spec"]["outline"] == [[0, 0], [64, 0], [0, 64]]
        replacement = client.call("create_primitive", {"shape": "wedge", "mins": [0, 0, 0], "maxs": [32, 32, 32], "preview": True})["state"]["preview"]
        client.call("cancel_primitive_preview", {"token": snapped["token"]}, error=True)
        assert client.call("get_primitive_preview")["preview"]["token"] == replacement["token"]
        client.call("cancel_primitive_preview", {"token": replacement["token"]})
        (args.output / "validation.json").write_text(json.dumps({"polygon": state, "stats": stats, "axis_checks": 3}, indent=2) + "\n")
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-polygon-brushes"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"polygon_brushes": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
