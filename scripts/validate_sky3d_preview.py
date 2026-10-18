import argparse
import hashlib
import json
import math
import os
import struct
import sys
import time
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))

from validate_face_lighting import DEFAULT_BINARY, Client
from validate_sky_preview import rgba_png, set_sky
from validate_recovery_exit import close_with_status, digest

WIDTH = 401
HEIGHT = 301
FOV = 60
SKY_ORIGIN = (4096, 2048, 1024)
BACKGROUND = (23, 47, 89)
PALETTE = {
    "red": (255, 0, 0),
    "green": (0, 255, 0),
    "yellow": (255, 255, 0),
    "cyan": (0, 255, 255),
    "magenta": (255, 0, 255),
    "foreground": (255, 128, 0),
    "tool": (113, 113, 113),
}
PANELS = [
    ("red", (56, -42, -28), (60, -10, 30), False),
    ("green", (72, 10, -10), (76, 42, 34), False),
    ("yellow", (40, -9, -25), (44, 8, -5), False),
    ("cyan", (94, -50, 36), (98, 50, 44), False),
    ("magenta", (86, -58, -48), (90, -38, -18), True),
]
FOREGROUND = ((96, -25, -30), (112, -9, 30))
POSES = [
    {"position": [0, 0, 0], "yaw": 0, "pitch": 0},
    {"position": [48, 32, 16], "yaw": 0, "pitch": 0},
    {"position": [-40, -24, -16], "yaw": -12, "pitch": 7},
]


def texture(rgb, size=4, dxt1=False):
    header = bytearray(80)
    header[:4] = b"VTF\0"
    struct.pack_into("<IIIHH", header, 4, 7, 2, 80, size, size)
    struct.pack_into("<I", header, 20, 0x0C)
    struct.pack_into("<H", header, 24, 1)
    struct.pack_into("<fI", header, 48, 1, 13 if dxt1 else 0)
    header[56] = 1
    struct.pack_into("<i", header, 57, -1)
    struct.pack_into("<H", header, 63, 1)
    if dxt1:
        rgb565 = ((rgb[0] * 31 // 255) << 11) | ((rgb[1] * 63 // 255) << 5) | (rgb[2] * 31 // 255)
        payload = struct.pack("<HHI", rgb565, rgb565, 0) * ((size // 4) ** 2)
    else:
        payload = bytes([*rgb, 255]) * (size * size)
    return bytes(header) + payload


def create_assets(output):
    game = output / "game"
    materials = game / "materials"
    samples = materials / "acceptance"
    sky = materials / "skybox"
    tools = materials / "tools"
    for directory in [samples, sky, tools]:
        directory.mkdir(parents=True)
    for name, rgb in PALETTE.items():
        (samples / f"{name}.vtf").write_bytes(texture(rgb))
        (samples / f"{name}.vmt").write_text(
            f"LightmappedGeneric {{ $basetexture acceptance/{name} }}\n"
        )
    for name, size in [("red", 2048), ("green", 1024), ("yellow", 2048)]:
        (samples / f"large_{name}.vtf").write_bytes(texture(PALETTE[name], size=size, dxt1=True))
        (samples / f"large_{name}.vmt").write_text(
            f"LightmappedGeneric {{ $basetexture acceptance/large_{name} }}\n"
        )
    for name, flag in [("toolsskybox", "%compilesky"), ("toolsskybox2d", "%compile2dsky")]:
        (tools / f"{name}.vmt").write_text(
            f"LightmappedGeneric {{ {flag} 1 $basetexture acceptance/tool }}\n"
        )
    (samples / "water.vmt").write_text(
        "Water { %compilewater 1 $basetexture acceptance/cyan }\n"
    )
    for suffix in ["rt", "bk", "lf", "ft", "up", "dn"]:
        (sky / f"hammerr_acceptance{suffix}.vmt").write_text(
            "Sky { $basetexture skybox/acceptance }\n"
        )
    (sky / "acceptance.vtf").write_bytes(texture(BACKGROUND))
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text("GameInfo { FileSystem { SearchPaths { game |gameinfo_path|. } } }\n")
    return gameinfo, samples


def plus(first, second):
    return tuple(a + b for a, b in zip(first, second, strict=True))


def ray(pose, x, y):
    yaw = math.radians(pose["yaw"])
    pitch = math.radians(pose["pitch"])
    forward = (math.cos(pitch) * math.cos(yaw), math.cos(pitch) * math.sin(yaw), math.sin(pitch))
    right = (math.sin(yaw), -math.cos(yaw), 0)
    up = (-math.sin(pitch) * math.cos(yaw), -math.sin(pitch) * math.sin(yaw), math.cos(pitch))
    slope = math.tan(math.radians(FOV / 2))
    horizontal = (2 * (x + 0.5) / WIDTH - 1) * slope * WIDTH / HEIGHT
    vertical = (1 - 2 * (y + 0.5) / HEIGHT) * slope
    direction = tuple(f + horizontal * r + vertical * u for f, r, u in zip(forward, right, up, strict=True))
    return tuple(pose["position"]), direction


def hit_box(origin, direction, mins, maxs):
    near, far = -math.inf, math.inf
    for axis in range(3):
        if abs(direction[axis]) < 1e-12:
            if not mins[axis] < origin[axis] < maxs[axis]:
                return None
            continue
        first = (mins[axis] - origin[axis]) / direction[axis]
        second = (maxs[axis] - origin[axis]) / direction[axis]
        near = max(near, min(first, second))
        far = min(far, max(first, second))
    if far <= max(near, 0):
        return None
    return near if near > 0 else far


def classify_pixel(rgba):
    rgb = rgba[:3]
    if max(abs(a - b) for a, b in zip(rgb, BACKGROUND, strict=True)) <= 2:
        return "background"
    high = max(rgb)
    if high < 40:
        return "other"
    mask = tuple(channel > high * 0.65 for channel in rgb)
    labels = {
        (True, False, False): "red",
        (False, True, False): "green",
        (True, True, False): "yellow",
        (False, True, True): "cyan",
        (True, False, True): "magenta",
    }
    if rgb[0] > 1.2 * rgb[1] and rgb[1] > max(20, 3 * rgb[2]):
        return "foreground"
    return labels.get(mask, "other")


def expected_label(pose, x, y, scale, split=True, foreground=False, hidden=()):
    origin, direction = ray(pose, x, y)
    if foreground and hit_box(origin, direction, *FOREGROUND) is not None:
        return "foreground"
    room_times = [(256 - origin[axis]) / direction[axis] if direction[axis] > 0 else (-256 - origin[axis]) / direction[axis] if direction[axis] < 0 else math.inf for axis in range(3)]
    exit_axis = min(range(3), key=room_times.__getitem__)
    if split and exit_axis == 0 and direction[0] > 0:
        wall_y = origin[1] + room_times[0] * direction[1]
        if wall_y > 0:
            return "background"
    nearest = math.inf
    label = "background"
    for name, mins, maxs, _ in PANELS:
        if name in hidden:
            continue
        virtual_mins = tuple(value * scale for value in mins)
        virtual_maxs = tuple(value * scale for value in maxs)
        distance = hit_box(origin, direction, virtual_mins, virtual_maxs)
        if distance is not None and distance < nearest:
            nearest, label = distance, name
    return label


def rgba_at(pixels, x, y):
    offset = (y * WIDTH + x) * 4
    return tuple(pixels[offset:offset + 4])


def oracle_measurement(pixels, pose, scale, split=True, foreground=False, hidden=()):
    compared = {}
    failures = []
    for y in range(8, HEIGHT - 8, 4):
        for x in range(8, WIDTH - 8, 4):
            expected = expected_label(pose, x, y, scale, split, foreground, hidden)
            neighbors = [expected_label(pose, x + dx, y + dy, scale, split, foreground, hidden) for dx, dy in [(-3, 0), (3, 0), (0, -3), (0, 3)]]
            if any(label != expected for label in neighbors):
                continue
            actual = classify_pixel(rgba_at(pixels, x, y))
            compared[expected] = compared.get(expected, 0) + 1
            if actual != expected:
                failures.append({"pixel": [x, y], "expected": expected, "actual": actual, "rgba": rgba_at(pixels, x, y)})
    return {"compared": compared, "mismatches": len(failures), "examples": failures[:12]}


class Acceptance:
    def __init__(self, client, output, assets):
        self.client = client
        self.output = output
        self.assets = assets
        self.checks = []
        self.measurements = {}
        self.sequence = 0

    def check(self, condition, label, detail=None):
        if not condition:
            raise AssertionError(f"{label}: {json.dumps(detail, default=str)}")
        self.checks.append(label)

    def state(self):
        return self.client.call("get_3d_sky_preview")

    def action(self, action, **arguments):
        return self.client.call("set_3d_sky_preview", {"action": action, **arguments})

    def select(self, camera, enabled=False):
        return self.action("settings", enabled=enabled, camera=camera)

    def prepare(self, background=False):
        return self.action("prepare", background=background)

    def settled(self, timeout=60):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            state = self.state()
            if not state["worker_active"]:
                return state
            time.sleep(0.02)
        raise TimeoutError("3D sky preparation did not settle")

    def rejected(self, name, arguments):
        try:
            result = self.client.call(name, arguments, error=True)
        except RuntimeError as error:
            return str(error)
        return json.dumps(result)

    def document(self):
        info = self.client.call("get_map_info")
        path = Path(info["path"]) if info["path"] else None
        return {
            "path": info["path"],
            "source_sha256": hashlib.sha256(path.read_bytes()).hexdigest() if path else None,
            "selection": info["selection"],
            "unsaved_changes": info["unsaved_changes"],
            "history": self.client.call("get_history"),
            "camera": self.client.call("get_camera"),
        }

    def render(self, name, pose=None, **arguments):
        self.sequence += 1
        camera = {"projection": "perspective", "fov": FOV, **(pose or POSES[0])}
        data, metadata = self.client.render_result(width=WIDTH, height=HEIGHT, camera=camera, **arguments)
        path = self.output / f"{self.sequence:03d}-{name}.png"
        path.write_bytes(data)
        path.with_suffix(".json").write_text(json.dumps(metadata, indent=2) + "\n")
        self.check(metadata["width"] == WIDTH and metadata["height"] == HEIGHT, f"{name}: exact output dimensions")
        report = metadata["sky3d_preview"]
        requested = arguments.get("sky3d_preview", self.state()["enabled"])
        eligible = requested and camera["projection"] == "perspective" and arguments.get("surface", "textured") == "textured" and arguments.get("tool_textures", True)
        self.check(report["requested"] == requested and report["eligible"] == eligible, f"{name}: render metadata reports actual requested and eligible state", report)
        suppression = "disabled" if not requested else "unsupported_view" if camera["projection"] != "perspective" or arguments.get("surface", "textured") != "textured" else "tool_textures_hidden" if not arguments.get("tool_textures", True) else None
        self.check(report["active"] == eligible and report["suppression_reason"] == suppression, f"{name}: active and suppression metadata follow the actual render override", report)
        return rgba_png(path), metadata

    def oracle(self, name, pixels, pose, scale, split=True, foreground=False, hidden=()):
        measurement = oracle_measurement(pixels, pose, scale, split, foreground, hidden)
        self.measurements[name] = {"pose": pose, "scale": scale, "split_sky2d": split, "foreground": foreground, "hidden": hidden, **measurement}
        self.check(measurement["mismatches"] == 0, f"{name}: analytical ray/box oracle matches every interior sample", measurement)
        self.check(sum(measurement["compared"].values()) > 5000, f"{name}: oracle covers thousands of pixels", measurement)
        return measurement

    def block(self, mins, maxs, material="tools/toolsskybox"):
        return self.client.call("create_block", {"mins": mins, "maxs": maxs, "material": material})["id"]

    def shell(self, origin, radius, split=False):
        walls = {}
        for axis in range(3):
            for sign in [-1, 1]:
                mins, maxs = [-radius - 16] * 3, [radius + 16] * 3
                mins[axis], maxs[axis] = (-radius - 16, -radius) if sign < 0 else (radius, radius + 16)
                if split and axis == 0 and sign == 1:
                    first_maxs = list(maxs)
                    first_maxs[1] = 0
                    walls["3d"] = self.block(plus(origin, mins), plus(origin, first_maxs))
                    second_mins = list(mins)
                    second_mins[1] = 0
                    walls["2d"] = self.block(plus(origin, second_mins), plus(origin, maxs), "tools/toolsskybox2d")
                else:
                    walls[(axis, sign)] = self.block(plus(origin, mins), plus(origin, maxs))
        return walls

    def fixture(self):
        self.client.call("new_map", {"discard_changes": True})
        self.client.call("set_spatial_settings", {"grid_visible": False})
        self.client.call("set_view", {"surface": "textured", "tool_textures": True, "sky_preview": True, "selection_outlines": False, "entity_images": False, "entity_model_guides": False})
        set_sky(self.client, "hammerr_acceptance")
        main_walls = self.shell((0, 0, 0), 256, split=True)
        mini_walls = self.shell(SKY_ORIGIN, 128)
        panel_ids = {}
        for name, mins, maxs, detail in PANELS:
            object_id = self.block(plus(SKY_ORIGIN, mins), plus(SKY_ORIGIN, maxs), f"acceptance/{name}")
            if detail:
                object_id = self.client.call("tie_to_entity", {"ids": [object_id], "classname": "func_detail"})["id"]
            panel_ids[name] = object_id
        cameras = []
        for name, scale, angles in [("selected_scale_16", 16, "0 0 0"), ("scale_4_with_angles", 4, "30 135 25"), ("same_16_different_angles", 16, "30 135 25")]:
            cameras.append(self.client.call("create_entity", {"classname": "sky_camera", "origin": SKY_ORIGIN, "properties": {"targetname": name, "scale": str(scale), "angles": angles}})["id"])
        self.client.call("create_entity", {"classname": "env_cubemap", "origin": [0, 0, -4096], "properties": {"cubemapsize": "not_a_compile_size"}})
        self.client.call("create_entity", {"classname": "info_target", "origin": plus(SKY_ORIGIN, (32, -16, 16)), "properties": {"targetname": "miniature_helper_must_not_render"}})
        self.client.call("select", {"ids": []})
        self.client.call("set_camera", {"projection": "perspective", "fov": FOV, **POSES[2]})
        self.client.call("save_map", {"path": str(self.output / "source.vmf")})
        return {"main": main_walls, "mini": mini_walls, "panels": panel_ids, "cameras": cameras}

    def lifecycle_and_pixels(self, fixture):
        schemas = {tool["name"]: tool["inputSchema"] for tool in self.client.rpc("tools/list", {})["tools"]}
        self.check({"get_3d_sky_preview", "set_3d_sky_preview"} <= schemas.keys(), "Explicit camera workflow is discoverable through production MCP")
        self.check("sky3d_preview" in schemas["render_view"]["properties"], "Temporary 3D sky rendering is discoverable")
        first = self.state()
        self.check(not first["enabled"] and first["camera_entity"] is None and not first["ready"], "New document defaults off with no inferred camera")
        self.check(len(first["cameras"]) == 3, "Every authored camera remains an explicit choice")
        self.check(not self.client.call("get_map_info")["rendering"]["initialized"], "Camera inspection is CPU-only")
        document = self.document()
        rejection = self.rejected("set_3d_sky_preview", {"action": "settings", "enabled": True})
        self.check(bool(rejection) and self.state() == first, "Enabling without an explicit camera is rejected atomically")
        selected = self.select(fixture["cameras"][0])
        self.check(not selected["enabled"] and selected["camera_entity"] == fixture["cameras"][0] and not selected["ready"], "Selecting a camera does not prepare or enable it")
        prepared = self.prepare()
        self.check(prepared["status"] == "ready" and prepared["ready"] and not prepared["enabled"], "Explicit prepare completes before enable", prepared)
        self.check(set(prepared["stats"]["source_owners"]) == set(fixture["panels"].values()), "Prepared non-sky provenance contains exactly the miniature's authored world/detail owners", prepared["stats"])
        self.check(not self.client.call("get_map_info")["rendering"]["initialized"], "Preparing geometry and assets leaves GPU uninitialized")
        _, disabled_meta = self.render("prepared-disabled")
        self.check(not disabled_meta["sky3d_preview"]["enabled"], "Prepared disabled render reports current shared display state")
        self.select(fixture["cameras"][0], enabled=True)
        on, on_meta = self.render("scale16-origin")
        self.check(on_meta["sky3d_preview"]["enabled"] and "off" not in on_meta["sky3d_preview"]["summary"].lower(), "Enabling a cached prepared scene refreshes shared-state render metadata and summary")
        self.select(fixture["cameras"][0], enabled=False)
        temporary_on, temporary_meta = self.render("disabled-shared-temporary-enabled", sky3d_preview=True)
        self.check(not temporary_meta["sky3d_preview"]["enabled"] and temporary_meta["sky3d_preview"]["requested"] and temporary_meta["sky3d_preview"]["eligible"], "Temporary enable reports disabled shared settings separately from enabled rendering")
        self.check(temporary_on == on, "Temporary enable on a disabled shared scene reproduces prepared geometry pixels")
        self.select(fixture["cameras"][0], enabled=True)
        first_measurement = self.oracle("scale16-origin", on, POSES[0], 16)
        self.check(first_measurement["compared"].get("red", 0) > 20 and first_measurement["compared"].get("magenta", 0) > 5, "World and func_detail scenery both appear through SKY")
        self.check(first_measurement["compared"].get("green", 0) == 0, "The SKY2D half masks miniature scenery")
        for index, pose in enumerate(POSES[1:], start=1):
            pixels, _ = self.render(f"scale16-pose{index}", pose=pose)
            self.oracle(f"scale16-pose{index}", pixels, pose, 16)
        shared = self.state()
        off, _ = self.render("temporary-disabled", sky3d_preview=False)
        self.check(all(classify_pixel(rgba_at(off, x, y)) == "background" for x, y in [(40, 40), (200, 150), (360, 260)]), "Temporary disable restores the configured six-face backdrop")
        self.check(self.state() == shared, "Temporary render override preserves shared 3D settings")
        independent, _ = self.render("independent-of-2d-preview", sky_preview=False)
        samples = [(x, y) for y in range(12, HEIGHT - 12, 8) for x in range(12, WIDTH - 12, 8) if expected_label(POSES[0], x, y, 16) != "background" and all(expected_label(POSES[0], x + dx, y + dy, 16) == expected_label(POSES[0], x, y, 16) for dx, dy in [(-3, 0), (3, 0), (0, -3), (0, 3)])]
        self.check(len(samples) > 100 and all(rgba_at(independent, x, y) == rgba_at(on, x, y) for x, y in samples), "Disabling the independent 2D preview preserves all admitted miniature scenery pixels")
        for mode in ["flat", "wireframe", "lightmap_density", "smoothing_groups"]:
            enabled, _ = self.render(f"{mode}-on", surface=mode)
            disabled, _ = self.render(f"{mode}-off", surface=mode, sky3d_preview=False)
            self.check(enabled == disabled, f"{mode} suppresses 3D sky without changing existing pixels")
        for projection in ["top", "front", "side"]:
            camera = {"projection": projection, "center": [0, 0, 0], "ortho_height": 700}
            enabled, _ = self.render(f"{projection}-on", pose=camera)
            disabled, _ = self.render(f"{projection}-off", pose=camera, sky3d_preview=False)
            self.check(enabled == disabled, f"{projection} suppresses 3D sky without changing existing pixels")
        hidden, _ = self.render("tools-hidden-on", tool_textures=False)
        hidden_off, _ = self.render("tools-hidden-off", tool_textures=False, sky3d_preview=False)
        self.check(hidden == hidden_off, "Tool-texture filtering suppresses the 3D sky composite")
        self.select(fixture["cameras"][2])
        self.prepare()
        self.select(fixture["cameras"][2], enabled=True)
        angled, _ = self.render("camera-entity-angles")
        self.check(angled == on, "Authored sky_camera angles do not rotate miniature geometry")
        self.select(fixture["cameras"][1])
        self.prepare()
        self.select(fixture["cameras"][1], enabled=True)
        for index, pose in enumerate(POSES):
            pixels, _ = self.render(f"scale4-pose{index}", pose=pose)
            self.oracle(f"scale4-pose{index}", pixels, pose, 4)
        self.check(self.document() == document, "Camera choice, preparation, enable and all temporary renders preserve source, selection, history and the user's camera")

    def foreground_and_main_leak(self, fixture):
        foreground = self.block(*FOREGROUND, "acceptance/foreground")
        self.client.call("select", {"ids": []})
        stale = self.state()
        self.check(not stale["ready"] and stale["status"] == "stale", "A committed geometry edit immediately invalidates prepared scenery", stale)
        self.prepare()
        pixels, _ = self.render("foreground")
        measurement = self.oracle("foreground", pixels, POSES[0], 4, foreground=True)
        self.check(measurement["compared"].get("foreground", 0) > 50 and measurement["compared"].get("red", 0) > 10, "Foreground occludes projected scenery while neighboring scenery remains visible")
        pick = self.client.call("pick_at_pixel", {"x": 240, "y": 150, "width": WIDTH, "height": HEIGHT, "camera": {"projection": "perspective", "fov": FOV, **POSES[0]}})
        self.check(pick["object"] == foreground, "Projected scenery preserves source foreground picking", pick)
        enabled_pick = self.client.call("pick_at_pixel", {"x": 200, "y": 150, "width": WIDTH, "height": HEIGHT, "camera": {"projection": "perspective", "fov": FOV, **POSES[0]}})
        self.select(fixture["cameras"][1], enabled=False)
        disabled_pick = self.client.call("pick_at_pixel", {"x": 200, "y": 150, "width": WIDTH, "height": HEIGHT, "camera": {"projection": "perspective", "fov": FOV, **POSES[0]}})
        self.check(enabled_pick == disabled_pick, "Sky enable preserves main-surface picking")
        self.select(fixture["cameras"][1], enabled=True)
        self.client.call("delete_objects", {"ids": [foreground, fixture["main"][(0, -1)]]})
        self.client.call("select", {"ids": []})
        self.client.call("save_map", {"path": str(self.output / "main-leak.vmf")})
        document = self.document()
        state = self.prepare()
        self.check(state["ready"], "An unrelated leaking main room does not reject the sealed miniature", state)
        pixels, _ = self.render("unrelated-main-leak")
        self.oracle("unrelated-main-leak", pixels, POSES[0], 4)
        self.check(self.document() == document, "Leaking-main preparation and render preserve committed source and history")
        visibility = self.client.call("get_editor_visibility")
        self.client.call("set_editor_visibility", {"action": "hide", "ids": [fixture["mini"][(2, 1)], fixture["panels"]["magenta"]], "snapshot": visibility["snapshot"]})
        self.check(not self.state()["ready"], "Changing editor visibility invalidates prepared displayed fragments")
        prepared = self.prepare()
        self.check(prepared["ready"], "An editor-hidden sealing wall still participates in structural enclosure")
        self.check(set(prepared["stats"]["source_owners"]) == set(fixture["panels"].values()) - {fixture["panels"]["magenta"]}, "Hidden func_detail provenance is removed before displayed fragment merging")
        pixels, _ = self.render("hidden-detail-and-sealing-wall")
        self.oracle("hidden-detail-and-sealing-wall", pixels, POSES[0], 4, hidden=("magenta",))
        self.check(self.document() == document, "Temporary editor visibility and filtered preparation preserve saved source, history, selection and camera")
        visibility = self.client.call("get_editor_visibility")
        self.client.call("set_editor_visibility", {"action": "show_all", "snapshot": visibility["snapshot"]})
        self.check(self.prepare()["ready"], "Restoring editor visibility rebuilds source-owned displayed fragments")

    def bounded_gpu_replacement(self, fixture):
        for name in ["red", "green"]:
            self.client.call("set_material", {"ids": [fixture["panels"][name]], "material": f"acceptance/large_{name}"})
        self.client.call("select", {"ids": []})
        document = self.document()
        first = self.prepare()
        self.check(first["ready"] and 16 * 1024 * 1024 < first["stats"]["geometry"]["gpu_bytes"] < 32 * 1024 * 1024, "A legal opaque miniature can occupy more than half the GPU asset pool", first)
        self.select(fixture["cameras"][1], enabled=True)
        pixels, _ = self.render("large-legal-assets")
        self.oracle("large-legal-assets", pixels, POSES[0], 4)
        self.check(self.prepare()["ready"], "Preparing the same legal large miniature again completes")
        replacement, _ = self.render("large-legal-assets-reprepared")
        self.check(replacement == pixels, "Replacing a legal large GPU scene retires its previous allocation before upload")
        self.check(self.document() == document, "Repeated preparation and GPU replacement preserve committed source and history")
        self.client.call("set_material", {"ids": [fixture["panels"]["yellow"]], "material": "acceptance/large_yellow"})
        document = self.document()
        exceeded = self.prepare()
        self.check(not exceeded["ready"] and exceeded["status"] == "failed" and "budget" in json.dumps(exceeded).lower(), "An oversized decoded asset set fails its allocation budget explicitly", exceeded)
        self.check(self.document() == document, "Allocation-budget rejection preserves committed source and history")
        for name in ["red", "green", "yellow"]:
            self.client.call("set_material", {"ids": [fixture["panels"][name]], "material": f"acceptance/{name}"})
        self.check(self.prepare()["ready"], "Returning to small authored materials recovers after allocation rejection")

    def unavailable_invalid_and_stale(self, fixture):
        texture_path = self.assets / "red.vtf"
        source = texture_path.read_bytes()
        try:
            texture_path.unlink()
            self.client.call("reload_material_assets")
            self.check(not self.state()["ready"], "Explicit asset reload invalidates prepared material ownership")
            unavailable = self.prepare()
            self.check(not unavailable["ready"] and unavailable["status"] == "failed" and "red" in json.dumps(unavailable).lower(), "Missing admitted miniature texture fails visibly before GPU upload", unavailable)
        finally:
            texture_path.write_bytes(source)
            self.client.call("reload_material_assets")
        self.check(self.prepare()["ready"], "Restoring and reloading the admitted texture permits explicit preparation")
        camera = fixture["cameras"][1]
        for scale in ["0", "-1", "1.5", "NaN", "1000000", None]:
            arguments = {"id": camera, "remove": ["scale"]} if scale is None else {"id": camera, "set": {"scale": scale}}
            self.client.call("set_entity_properties", arguments)
            stale = self.state()
            self.check(not stale["ready"] and stale["camera_entity"] == camera and bool(stale["issue"]), f"Invalid scale {scale!r} keeps the explicit camera and suppresses stale geometry", stale)
            before = self.state()
            rejection = self.rejected("set_3d_sky_preview", {"action": "settings", "enabled": True, "camera": camera})
            self.check(bool(rejection) and self.state() == before, f"Invalid scale {scale!r} fails atomically without choosing another camera")
            self.client.call("undo")
            self.check(self.prepare()["ready"], f"Undo and explicit prepare recover from invalid scale {scale!r}")
        self.client.call("delete_objects", {"ids": [camera]})
        removed = self.state()
        self.check(not removed["ready"] and removed["camera_entity"] == camera and bool(removed["issue"]), "Removing the selected camera never reroutes to another valid camera", removed)
        self.client.call("undo")
        self.check(not self.state()["ready"], "Undo restores camera source but requires an explicit new preparation")
        self.check(self.prepare()["ready"], "Explicit preparation recovers after camera restoration")
        self.client.call("delete_objects", {"ids": [fixture["mini"][(0, -1)]]})
        self.client.call("select", {"ids": []})
        document = self.document()
        leaking = self.prepare()
        self.check(not leaking["ready"] and leaking["status"] == "failed" and "leak" in json.dumps(leaking).lower(), "Opening the selected miniature reports its own leak", leaking)
        self.check(self.document() == document, "Failed leak preparation preserves the committed document")
        self.client.call("undo")
        self.check(self.prepare()["ready"], "Undo and explicit prepare recover the sealed miniature")
        water = self.block(plus(SKY_ORIGIN, (-50, -40, -40)), plus(SKY_ORIGIN, (-30, -20, -20)), "acceptance/water")
        self.client.call("select", {"ids": []})
        unsupported = self.prepare()
        self.check(not unsupported["ready"] and unsupported["status"] == "failed" and "water" in json.dumps(unsupported).lower(), "Intersecting unsupported water has an explicit membership failure", unsupported)
        self.client.call("delete_objects", {"ids": [water]})
        self.check(self.prepare()["ready"], "Removing unsupported miniature content restores readiness")
        unrelated_water = self.block((-80, 20, -50), (-60, 40, -30), "acceptance/water")
        self.check(self.prepare()["ready"], "Provably disjoint unsupported water in the main room does not reject the miniature")
        self.client.call("delete_objects", {"ids": [unrelated_water]})
        for name, origin, diagnostic in [
            ("solid", plus(SKY_ORIGIN, (-136, 0, 0)), "solid"),
            ("outside", (65000, 65000, 65000), "outside"),
            ("boundary", plus(SKY_ORIGIN, (-128, 0, 0)), "boundary"),
        ]:
            invalid = self.client.call("create_entity", {"classname": "sky_camera", "origin": origin, "properties": {"scale": "16", "targetname": name}})["id"]
            self.select(invalid)
            document = self.document()
            failed = self.prepare()
            self.check(not failed["ready"] and failed["status"] == "failed" and diagnostic in json.dumps(failed).lower(), f"A {name} camera seed is visibly rejected", failed)
            self.check(self.document() == document, f"Rejected {name} seed preserves document and history")
        self.select(camera)
        self.check(self.prepare()["ready"], "Explicitly reselecting the valid camera recovers after invalid seeds")

    def cancellation_and_document_replacement(self, fixture):
        for y in range(-96, 97, 24):
            for z in range(-96, 97, 24):
                self.block(plus(SKY_ORIGIN, (-96, y, z)), plus(SKY_ORIGIN, (-88, y + 8, z + 8)), "acceptance/green")
        self.client.call("select", {"ids": []})
        document = self.document()
        started = self.prepare(background=True)
        self.check(started["worker_active"], "Background preparation exposes an owned active worker", started)
        self.action("cancel")
        cancelled = self.settled()
        self.check(cancelled["status"] == "cancelled" and not cancelled["ready"], "Cancellation drains the owned worker without adopting prepared geometry", cancelled)
        self.check(self.document() == document, "Cancellation preserves source, selection, history and camera")
        self.prepare(background=True)
        self.client.call("create_entity", {"classname": "info_target", "origin": [-10, 0, 0]})
        stale = self.settled()
        self.check(stale["status"] == "stale" and not stale["ready"], "A result from an older committed revision cannot become ready", stale)
        self.prepare(background=True)
        self.client.call("new_map", {"discard_changes": True})
        replaced = self.settled()
        self.check(not replaced["enabled"] and replaced["camera_entity"] is None and not replaced["ready"] and not replaced["worker_active"], "New document rejects the previous document's pending result", replaced)
        self.client.call("open_map", {"path": str(self.output / "source.vmf"), "discard_changes": True})
        reopened = self.state()
        self.check(not reopened["enabled"] and reopened["camera_entity"] is None and not reopened["ready"], "Open resets camera choice and prepared state instead of reviving a prior document result", reopened)

    def run(self):
        fixture = self.fixture()
        self.lifecycle_and_pixels(fixture)
        self.foreground_and_main_leak(fixture)
        self.bounded_gpu_replacement(fixture)
        self.unavailable_invalid_and_stale(fixture)
        self.cancellation_and_document_replacement(fixture)
        result = {"checks": len(self.checks), "passed": self.checks, "measurements": self.measurements, "geometry_oracle": "Independent pinhole rays against authored axis-aligned boxes transformed by (point - sky_origin) * scale; SKY2D main-surface mask and foreground tested before miniature intersections."}
        return result


def main():
    parser = argparse.ArgumentParser(description="Independently validate explicitly prepared 3D sky through production MCP and offscreen pixels.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    gameinfo, assets = create_assets(output)
    with patch.dict(os.environ, {
        "HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
        "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"),
        "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
    }):
        client = Client(None, args.binary.resolve(), output, gameinfo)
    try:
        definitions = output / "fixture.fgd"
        definitions.write_text('@PointClass = sky_camera [ scale(string) : "Raw fixture scale" ]\n@PointClass = env_cubemap [ cubemapsize(string) : "Raw fixture sample size" ]\n')
        client.call("load_entity_definitions", {"path": str(definitions)})
        result = Acceptance(client, output, assets).run()
    finally:
        code, trailing = close_with_status(client)
        assert code == 0 and not trailing.strip(), (code, trailing)
    result.update({"status": "passed", "binary": str(args.binary.resolve()), "binary_sha256": digest(args.binary.resolve()), "exit_code": code})
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"checks": result["checks"], "output": str(output)}))


if __name__ == "__main__":
    main()
