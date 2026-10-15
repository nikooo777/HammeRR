import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client
from validate_material_replacement import preserved_map


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    with preserved_map(client):
        tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
        check(
            {
                "load_pointfile",
                "get_pointfile",
                "navigate_pointfile",
                "frame_pointfile",
                "clear_pointfile",
            }
            <= tools,
            "Pointfile tools discoverable",
        )
        path = output / "route.lin"
        path.write_text("0 0 32\n128 0 32\n128 0 32\n128 128 64\n256 128 96\n")
        camera = {
            "projection": "perspective",
            "position": [340, -360, 280],
            "look_at": [96, 64, 48],
            "fov": 75,
        }
        client.call("set_camera", camera)
        before = client.call("get_map_info")
        baseline = client.render(output / "before.png")
        report = client.call("load_pointfile", {"path": str(path)})
        token = report["token"]
        check(
            report["total"] == 5 and report["source"] == "file" and report["current"],
            "Load a finite path",
        )
        check(
            report["bounds"] == [[0, 0, 32], [256, 128, 96]] and report["length"] > 380,
            "Bounds and length inspectable",
        )
        check(
            client.call("get_map_info")["camera"] == before["camera"],
            "Loading does not move the camera",
        )
        check(
            client.render(output / "loaded.png") != baseline,
            "Loaded path appears in shared renderer",
        )
        page = client.call("get_pointfile", {"offset": 1, "limit": 2})
        check(
            page["points"] == [[128, 0, 32], [128, 0, 32]] and page["next_offset"] == 3,
            "Bounded path pagination",
        )
        for limit in [0, 201]:
            client.call("get_pointfile", {"limit": limit}, error=True)
            checks.append(f"Reject page size {limit}")
        for text in [
            "",
            "0 0 0",
            "0 0 0\nNaN 1 2",
            "0 0 0\n1 2 3 4",
            "0 0 0\n1000001 0 0",
        ]:
            invalid = output / "invalid.lin"
            invalid.write_text(text)
            client.call("load_pointfile", {"path": str(invalid)}, error=True)
            check(
                client.call("get_pointfile")["token"] == token,
                "Invalid file preserves prior path",
            )
        oversized = output / "oversized.lin"
        oversized.write_text(" " * (2 * 1024 * 1024 + 1))
        client.call("load_pointfile", {"path": str(oversized)}, error=True)
        checks.append("Oversized file rejected")
        client.call("load_pointfile", {"path": str(output)}, error=True)
        checks.append("Directory rejected")
        moved = client.call("navigate_pointfile", {"index": 1, "token": token})
        check(
            moved["camera"]["position"] == [128, 0, 32]
            and abs(moved["camera"]["yaw"] - 90) < 0.001
            and moved["camera"]["fov"] == 75,
            "Perspective skips repeated points and preserves FOV",
        )
        last = client.call("navigate_pointfile", {"index": 4, "token": token})
        check(
            abs(last["camera"]["yaw"]) < 0.001 and last["camera"]["pitch"] > 0,
            "Last point continues final segment",
        )
        client.call("navigate_pointfile", {"index": 5, "token": token}, error=True)
        check(
            client.call("get_map_info")["camera"] == last["camera"],
            "Out-of-range index preserves camera",
        )
        for projection in ["top", "front", "side"]:
            client.call(
                "set_camera",
                {"projection": projection, "center": [0, 0, 0], "ortho_height": 384},
            )
            moved = client.call("navigate_pointfile", {"index": 3, "token": token})
            check(
                moved["camera"]["center"] == [128, 128, 64]
                and moved["camera"]["ortho_height"] == 384,
                f"{projection} centers on point and preserves zoom",
            )
            client.call("frame_pointfile", {"aspect": 0.5, "token": token})
            checks.append(f"{projection} frames path")
        client.call("frame_pointfile", {"aspect": 0, "token": token}, error=True)
        checks.append("Invalid framing aspect rejected")
        replacement = client.call("load_pointfile", {"path": str(path)})
        client.call("clear_pointfile", {"token": token}, error=True)
        client.call("navigate_pointfile", {"index": 0, "token": token}, error=True)
        check(
            client.call("get_pointfile")["token"] == replacement["token"],
            "Stale token cannot clear or navigate replacement",
        )
        after = client.call("get_map_info")
        check(
            all(
                before[key] == after[key]
                for key in ["selection", "undo", "redo", "unsaved_changes"]
            ),
            "Pointfile operations preserve map history and selection",
        )
        client.call("clear_pointfile", {"token": replacement["token"]})
        check(
            client.call("get_pointfile")["source"] is None and path.exists(),
            "Unload leaves source file intact",
        )
        client.call("set_camera", camera)
        check(
            client.render(output / "cleared.png") == baseline,
            "Unload removes rendered path",
        )
        client.call("load_pointfile", {"path": str(path)})
        client.call(
            "create_block",
            {
                "mins": [-128, -128, -16],
                "maxs": [128, 128, 0],
                "material": "dev/dev_measuregeneric01",
            },
        )
        check(
            not client.call("get_pointfile")["current"],
            "Map edits mark external path stale",
        )
        client.call("navigate_pointfile", {"index": 0}, error=True)
        client.call("frame_pointfile", {}, error=True)
        checks.append("Stale paths reject navigation and framing")
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [0, 0, 64]},
        )
        client.call("compile_map", {"path": str(output / "leak.bsp")}, error=True)
        leak = client.call("get_pointfile")
        check(
            leak["source"] == "compile" and leak["current"] and leak["total"] > 1,
            "Current compiler leak replaces external path",
        )
        client.call("navigate_pointfile", {"index": 0, "token": leak["token"]})
        checks.append("Compiler path supports same navigation")
        client.call("clear_pointfile", {"token": leak["token"]})
        check(
            client.call("get_map_info")["leak"] is None,
            "Unload clears compiled path overlay",
        )
        client.call("load_pointfile", {"path": str(path)})
        client.call("new_map", {"discard_changes": True})
        check(
            client.call("get_pointfile")["total"] == 0,
            "Map replacement clears external path",
        )
        maximum = output / "maximum.lin"
        maximum.write_text("".join(f"{index} 0 32\n" for index in range(65536)))
        large = client.call("load_pointfile", {"path": str(maximum)})
        check(
            large["total"] == 65536 and len(large["points"]) == 50,
            "Maximum-size path loads with bounded response",
        )
        last_page = client.call("get_pointfile", {"offset": 65535, "limit": 200})
        check(
            last_page["points"] == [[65535, 0, 32]]
            and last_page["next_offset"] is None,
            "Last page of maximum path is bounded",
        )
        client.call(
            "set_camera",
            {"projection": "top", "center": [0, 0, 0], "ortho_height": 512},
        )
        before_frame = client.call("get_map_info")["camera"]
        client.call("frame_pointfile", {"aspect": 0.5}, error=True)
        check(
            client.call("get_map_info")["camera"] == before_frame,
            "Oversized planar framing preserves camera",
        )
        client.call("frame_pointfile", {"aspect": 1})
        check(
            client.call("get_map_info")["camera"]["ortho_height"] > 65535,
            "Maximum path frames when it fits planar zoom",
        )
        with maximum.open("a") as stream:
            stream.write("65536 0 32\n")
        client.call("load_pointfile", {"path": str(maximum)}, error=True)
        check(
            client.call("get_pointfile")["token"] == large["token"],
            "Over-limit point count preserves maximum-size path",
        )
        client.call("new_map", {"discard_changes": True})
    checks.append("Original map and desktop-independent settings restored")
    result = {
        "transport": "http" if client.url else "stdio",
        "tools": len(tools),
        "checks": len(checks),
        "passed": checks,
    }
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key: value for key, value in result.items() if key != "passed"}))


def main():
    parser = argparse.ArgumentParser(
        description="Validate pointfile loading/navigation through MCP; HTTP requires a clean idle map with empty history."
    )
    parser.add_argument("--url")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    client = Client(args.url, args.binary.resolve(), output)
    try:
        validate(client, output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
