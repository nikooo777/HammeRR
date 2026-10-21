import argparse
import base64
import hashlib
import json
import os
import select
import subprocess
import threading
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_BINARY = ROOT / "target/debug/hammerr-headless"


class Client:
    def __init__(self, url, binary, output, gameinfo=None, *, environment=None):
        self.url = url
        self.sequence = 0
        self.session = None
        self.process = None
        self.log = None
        try:
            if not url:
                self.log = (output / "editor.log").open("w")
                self.process = subprocess.Popen(
                    [
                        str(binary),
                        "--headless",
                        *(["--game", str(gameinfo)] if gameinfo else []),
                    ],
                    cwd=ROOT,
                    env={**os.environ, "HAMMERR_RECOVERY_DIR": str(output / "recovery"),
                         **(environment or {})},
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=self.log,
                    text=True,
                    bufsize=1,
                )
            self.rpc(
                "initialize",
                {
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": {"name": "lighting-validation", "version": "1"},
                },
            )
            self.rpc("notifications/initialized", {}, notification=True)
        except BaseException as error:
            try:
                code, trailing = self.close_with_status(timeout=10)
                if code not in (None, 0) or trailing.strip():
                    raise RuntimeError(f"Client initialization EOF failed: exit={code}, trailing={trailing[:512]!r}")
            except BaseException as cleanup:
                raise BaseExceptionGroup("Client initialization and cleanup failed", [error, cleanup]) from None
            raise

    def rpc(self, method, params, notification=False):
        self.sequence += 1
        payload = {"jsonrpc": "2.0", "method": method, "params": params}
        if not notification:
            payload["id"] = self.sequence
        if self.url:
            headers = {
                "Content-Type": "application/json",
                "Accept": "application/json, text/event-stream",
                "MCP-Protocol-Version": "2025-03-26",
            }
            if self.session:
                headers["Mcp-Session-Id"] = self.session
            request = urllib.request.Request(
                self.url, data=json.dumps(payload).encode(), headers=headers
            )
            with urllib.request.urlopen(request, timeout=90) as response:
                self.session = response.headers.get("Mcp-Session-Id", self.session)
                data = response.read().decode()
            if notification:
                return None
            messages = [
                json.loads(line[5:])
                for line in data.splitlines()
                if line.startswith("data:") and line[5:].strip()
            ]
            message = next((m for m in messages if m.get("id") == self.sequence), None)
            if message is None:
                message = json.loads(data)
        else:
            self.process.stdin.write(json.dumps(payload) + "\n")
            self.process.stdin.flush()
            if notification:
                return None
            while True:
                ready, _, _ = select.select([self.process.stdout], [], [], 90)
                if not ready:
                    raise TimeoutError(method)
                line = self.process.stdout.readline()
                if not line:
                    raise RuntimeError("Editor exited; inspect editor.log")
                message = json.loads(line)
                if message.get("id") == self.sequence:
                    break
        if "error" in message:
            raise RuntimeError(message["error"])
        return message["result"]

    def call(self, name, arguments=None, error=False):
        result = self.rpc("tools/call", {"name": name, "arguments": arguments or {}})
        if bool(result.get("isError")) != error:
            raise AssertionError(f"{name}: {result}")
        if error:
            return result
        content = [c["text"] for c in result["content"] if c["type"] == "text"]
        return json.loads(content[0]) if content else None

    def render_result(self, **arguments):
        result = self.rpc(
            "tools/call",
            {
                "name": "render_view",
                "arguments": {"width": 1100, "height": 700, **arguments},
            },
        )
        assert not result.get("isError"), result
        image = next(c for c in result["content"] if c["type"] == "image")
        data = base64.b64decode(image["data"])
        assert data.startswith(b"\x89PNG\r\n\x1a\n")
        metadata = json.loads(next(c["text"] for c in result["content"] if c["type"] == "text"))
        return data, metadata

    def render(self, path, **arguments):
        data, _ = self.render_result(**arguments)
        path.write_bytes(data)
        return hashlib.sha256(data).hexdigest()

    def close(self):
        self.close_with_status(timeout=10)

    def close_with_status(self, timeout=15):
        process = self.process
        if process is None:
            if self.log is not None:
                self.log.close()
                self.log = None
            return None, ""
        errors, trailing, read_errors = [], [], []

        def drain_stdout():
            try:
                trailing.append(process.stdout.read())
            except BaseException as error:
                read_errors.append(error)
            finally:
                try:
                    process.stdout.close()
                except BaseException as error:
                    read_errors.append(error)

        reader = threading.Thread(target=drain_stdout, daemon=True)
        started = False
        try:
            try:
                reader.start()
                started = True
            except BaseException as error:
                errors.append(error)
            try:
                process.stdin.close()
            except BaseException as error:
                errors.append(error)
            try:
                process.wait(timeout=timeout)
            except BaseException as error:
                errors.append(TimeoutError(f"Headless EOF failed to drain within {timeout} seconds")
                              if isinstance(error, subprocess.TimeoutExpired) else error)
                try:
                    process.kill()
                except ProcessLookupError:
                    pass
                except BaseException as error:
                    errors.append(error)
                try:
                    process.wait(timeout=5)
                except BaseException as error:
                    errors.append(error)
        finally:
            if not process.stdin.closed:
                try:
                    process.stdin.close()
                except BaseException as error:
                    errors.append(error)
            if started:
                try:
                    reader.join(timeout=5)
                    if reader.is_alive():
                        errors.append(TimeoutError("Headless stdout remained open after child cleanup"))
                    else:
                        errors.extend(read_errors)
                except BaseException as error:
                    errors.append(error)
            else:
                try:
                    process.stdout.close()
                except BaseException as error:
                    errors.append(error)
            if process.returncode is not None:
                self.process = None
            if self.log is not None:
                try:
                    self.log.close()
                except BaseException as error:
                    errors.append(error)
                finally:
                    self.log = None
        output = "".join(trailing)
        if errors:
            raise BaseExceptionGroup(f"Client cleanup failed: exit={process.returncode}, trailing={output[:512]!r}", errors)
        return process.returncode, output


def detail(client, ids):
    if not ids:
        return []
    return client.call("get_objects", {"ids": ids})["objects"]


def face_ref(brush, face):
    return {"solid": brush["solid"], "side": face["side"]}


def validate(client, output):
    checks = []

    def check(condition, name):
        if not condition:
            raise AssertionError(name)
        checks.append(name)

    tools = client.rpc("tools/list", {})["tools"]
    schemas = {tool["name"]: tool["inputSchema"] for tool in tools}
    fields = schemas["edit_faces"]["properties"]
    check(
        all(
            k in fields
            for k in [
                "lightmap_scale",
                "smoothing_groups",
                "smoothing_add",
                "smoothing_remove",
            ]
        ),
        "lighting schema",
    )
    original = client.call("get_map_info")
    check(
        not original["unsaved_changes"]
        and not original["undo"]
        and not original["redo"],
        "clean initial map and history required",
    )
    pending = [
        original["face_selection"]["preview"],
        original["primitive"]["preview"],
        original["clipping"]["enabled"],
        original["clipping"]["preview"],
        original["transform"]["drag"],
        original["transform"]["preview"],
        original["area_selection"]["preview"],
        original.get("volume_selection", {}).get("preview"),
        original["block_preview"],
        original["spatial"]["draw_block"],
        original["connection_draft"],
        original["rename_draft"],
        original["property_draft"],
        original["context"]["draft"],
        original["context"]["focused_region"],
        original["map_checks"]["report"],
        original["compiling"],
        original["launching"],
        original["game_running"],
    ]
    if "get_material_search" in schemas:
        pending.append(client.call("get_material_search"))
    check(
        not any(pending),
        "idle session without previews, reports or running jobs required",
    )
    original_hash = (
        hashlib.sha256(Path(original["path"]).read_bytes()).hexdigest()
        if original["path"]
        else None
    )
    original_selection = detail(client, original["selection"])
    try:
        client.call("new_map")
        shell = [
            ([-256, -256, -16], [256, 256, 0]),
            ([-256, -256, 240], [256, 256, 256]),
            ([-256, -256, 0], [-240, 256, 240]),
            ([240, -256, 0], [256, 256, 240]),
            ([-240, -256, 0], [240, -240, 240]),
            ([-240, 240, 0], [240, 256, 240]),
        ]
        for mins, maxs in shell:
            client.call("create_block", {"mins": mins, "maxs": maxs})
        ids = []
        for x, scale, groups in [
            (-176, 4, [1, 25]),
            (-80, 16, [2, 32]),
            (16, 32, [3]),
            (112, 64, [4]),
        ]:
            block = client.call(
                "create_block", {"mins": [x, -32, 0], "maxs": [x + 64, 32, 80]}
            )
            ids.append(block["id"])
            brush = detail(client, [block["id"]])[0]
            refs = [face_ref(brush, face) for face in brush["faces"]]
            client.call(
                "edit_faces",
                {"faces": refs, "lightmap_scale": scale, "smoothing_groups": groups},
            )
        for x, color in [(-110, "255 220 170 400"), (110, "150 200 255 400")]:
            client.call(
                "create_entity",
                {
                    "classname": "light",
                    "origin": [x, 70, 145],
                    "properties": {"_light": color},
                },
            )
        client.call(
            "create_entity",
            {"classname": "info_player_counterterrorist", "origin": [-192, -160, 24]},
        )
        client.call("select", {"ids": []})
        client.call(
            "set_camera", {"position": [-215, -215, 190], "look_at": [0, 15, 65]}
        )
        brushes = detail(client, ids)
        faces = [
            face_ref(
                brush,
                next(face for face in brush["faces"] if face["normal"] == [0, 0, 1]),
            )
            for brush in brushes[:2]
        ]
        client.call("select_faces", {"faces": faces})
        initial = client.call("get_face_selection")
        check(
            [f["lightmap_scale"] for f in initial["faces"]] == [4, 16],
            "mixed scales exposed",
        )
        check(
            [f["smoothing_groups"] for f in initial["faces"]] == [[1, 25], [2, 32]],
            "mixed groups exposed",
        )
        baseline = detail(client, ids)
        history = client.call("get_map_info")["undo"]
        client.call("set_view", {"surface": "lightmap_density"})
        density = client.render(output / "lightmap-density.png")
        first = client.call(
            "edit_faces",
            {
                "lightmap_scale": 8,
                "smoothing_add": [3],
                "smoothing_remove": [1, 32],
                "preview": True,
            },
        )
        token = first["state"]["preview_token"]
        check(bool(token), "preview token exposed")
        check(detail(client, ids) == baseline, "preview preserves committed faces")
        check(
            client.call("get_map_info")["undo"] == history, "preview preserves history"
        )
        check(
            [f["smoothing_groups"] for f in first["state"]["preview_faces"]]
            == [[3, 25], [2, 3]],
            "preview preserves unrelated groups",
        )
        check(
            all(f["lightmap_scale"] == 8 for f in first["state"]["preview_faces"]),
            "preview scales",
        )
        preview = client.render(output / "lightmap-preview.png")
        check(preview != density, "density image changes with preview")
        client.call("cancel_face_preview", {"token": token})
        check(
            client.render(output / "lightmap-cancelled.png") == density,
            "cancel restores exact image",
        )
        for invalid in [
            {"lightmap_scale": 0},
            {"lightmap_scale": 1025},
            {"smoothing_groups": [0]},
            {"smoothing_groups": [33]},
            {"smoothing_add": [1], "smoothing_remove": [1]},
            {
                "faces": [faces[0], {**faces[1], "side": 4294967295}],
                "lightmap_scale": 4,
            },
        ]:
            client.call("edit_faces", invalid, error=True)
            check(detail(client, ids) == baseline, f"atomic rejection: {invalid}")
        check(
            client.call("get_map_info")["undo"] == history,
            "invalid edits preserve history",
        )
        old = client.call("edit_faces", {"lightmap_scale": 4, "preview": True})[
            "state"
        ]["preview_token"]
        new = client.call("edit_faces", {"lightmap_scale": 8, "preview": True})[
            "state"
        ]["preview_token"]
        check(new != old, "replacement gets new token")
        for tool in ["commit_face_preview", "cancel_face_preview"]:
            client.call(tool, {"token": old}, error=True)
            check(
                client.call("get_face_selection")["preview_token"] == new,
                f"stale {tool} preserves replacement",
            )
        client.call("cancel_face_preview")
        check(
            not client.call("get_face_selection")["preview"],
            "legacy tokenless cancellation",
        )
        preview = client.call(
            "edit_faces",
            {
                "lightmap_scale": 8,
                "smoothing_add": [3],
                "smoothing_remove": [1, 32],
                "preview": True,
            },
        )
        client.call("commit_face_preview", {"token": preview["state"]["preview_token"]})
        committed = detail(client, ids)
        for before, after, selected in zip(baseline[:2], committed[:2], faces):
            for a, b in zip(before["faces"], after["faces"]):
                check(
                    a == b
                    if a["side"] != selected["side"]
                    else b["lightmap_scale"] == 8,
                    "targeted face edit",
                )
        check(client.call("get_map_info")["undo"][1:] == history[:9], "one undo entry")
        client.call("undo")
        check(detail(client, ids) == baseline, "undo restores metadata")
        client.call("redo")
        check(detail(client, ids) == committed, "redo restores metadata")
        client.call("edit_faces", {"smoothing_groups": []})
        check(
            all(
                f["smoothing_groups"] == []
                for f in client.call("get_face_selection")["faces"]
            ),
            "clear groups",
        )
        client.call("undo")
        client.call("edit_faces", {"preview": True, "smoothing_add": [32]})
        client.call("select_faces", {"faces": faces[:1]})
        client.call("commit_face_preview", error=True)
        check(detail(client, ids) == committed, "selection invalidates preview")
        client.call("select_faces", {"faces": []})
        image = client.render(
            output / "face-lighting-density.png", surface="lightmap_density"
        )
        groups = client.render(
            output / "face-lighting-groups.png",
            surface="smoothing_groups",
            smoothing_group=32,
        )
        textured = client.render(
            output / "face-lighting-textured.png", surface="textured"
        )
        check(len({image, groups, textured}) == 3, "distinct diagnostic images")
        check(
            client.call("get_map_info")["view"]["surface"] == "lightmap_density",
            "render overrides preserve shared view",
        )
        for invalid in [
            {"surface": "unknown"},
            {"surface": "textured", "smoothing_group": 0},
            {"smoothing_group": 33},
        ]:
            before = client.call("get_map_info")["view"]
            client.call("set_view", invalid, error=True)
            check(
                client.call("get_map_info")["view"] == before,
                "invalid view preserves settings",
            )
            client.call("render_view", invalid, error=True)
        client.call("set_view", {"surface": "smoothing_groups", "smoothing_group": 32})
        check(
            client.call("get_map_info")["view"]["smoothing_group"] == 32,
            "shared hard-edge group view",
        )
        top = {"projection": "top", "center": [0, 0, 0], "ortho_height": 600}
        planar_renders = {
            client.render(output / f"top-{mode}.png", camera=top, surface=mode)
            for mode in ["textured", "wireframe", "lightmap_density"]
        }
        check(len(planar_renders) == 3, "planar camera honors explicit viewport modes")
        client.call("save_map", {"path": str(output / "lighting.vmf")})
        client.call("open_map", {"path": str(output / "lighting.vmf")})
        check(
            detail(client, ids) == committed, "VMF save/reopen preserves face settings"
        )
        compiled = client.call("compile_map", {"path": str(output / "lighting.bsp")})
        check(
            compiled["success"] and Path(compiled["bsp"]).is_file(),
            "sealed fixture compiles",
        )
        (output / "compile.json").write_text(json.dumps(compiled, indent=2) + "\n")
    finally:
        if original["path"]:
            client.call("open_map", {"path": original["path"], "discard_changes": True})
        else:
            client.call("new_map", {"discard_changes": True})
        if original["face_selection"]["mode"] == "faces":
            client.call(
                "select_faces",
                {
                    "faces": [
                        {k: f[k] for k in ["solid", "side"]}
                        for f in original["face_selection"]["faces"]
                    ]
                },
            )
        else:
            client.call("set_selection_mode", {"mode": "objects"})
            client.call("select", {"ids": original["selection"]})
        camera = original["camera"]
        client.call(
            "set_camera",
            {
                k: camera[k]
                for k in [
                    "position",
                    "projection",
                    "fov",
                    "ortho_height",
                    "yaw",
                    "pitch",
                ]
                if k in camera
                and (camera["projection"] == "perspective" or k not in ["yaw", "pitch"])
            },
        )
        client.call("set_view", original["view"])
        restored = client.call("get_map_info")
        for key in [
            "path",
            "unsaved_changes",
            "world_brushes",
            "point_entities",
            "brush_entities",
            "camera",
            "view",
            "selection",
            "undo",
            "redo",
        ]:
            check(restored[key] == original[key], f"restore {key}")
        check(
            detail(client, original["selection"]) == original_selection,
            "restore selected object details",
        )
        if original_hash:
            check(
                hashlib.sha256(Path(original["path"]).read_bytes()).hexdigest()
                == original_hash,
                "original file hash unchanged",
            )
    result = {
        "transport": "http" if client.url else "stdio",
        "tools": len(tools),
        "checks": len(checks),
        "passed": checks,
    }
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k != "passed"}))


def main():
    parser = argparse.ArgumentParser(
        description="Validate face lighting over MCP; HTTP requires a clean map with empty undo/redo history."
    )
    parser.add_argument(
        "--url", help="Existing editor MCP URL; otherwise launch a headless process"
    )
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output",
        type=Path,
        required=True,
        help="New directory for fixture, images and reports",
    )
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
