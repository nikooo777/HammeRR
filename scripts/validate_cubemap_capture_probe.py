import argparse
import hashlib
import io
import json
import math
import os
import re
import resource
import shutil
import struct
import subprocess
import sys
import time
import traceback
import zipfile
import zlib
from pathlib import Path

import validate_native_runtime as runtime
from validate_bump_lighting import texture
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM

ENGINE_SHA256 = "965b90157cbec58187c553ce8bb65907e186c990b08453cb4abd75e909ec794a"
MAP_NAME = "hammerr_cubemap_capture_probe"
SAMPLES = [(-96, 0, 64), (96, 0, 64)]
CUBE_SIZE = 32
SIZE_ENUM = 6
WIDTH = 1280
HEIGHT = 720
PAKFILE = 40
MAX_BSP_BYTES = 64 * 1024**2
MAX_PAK_BYTES = 16 * 1024**2
MAX_ENTRY_BYTES = 1024**2
FACE_NAMES = ["right", "left", "back", "front", "up", "down"]
MODULES = ["engine.so", "vtex_dll.so", "bsppack.so"]
CONTROL_VALUES = {"sv_lan": 1, "mat_hdr_level": 0, "mat_fullbright": 0}
READY = "HAMMERR_CUBEMAP_READY 2 100"
BEGIN = "HAMMERR_CUBEMAP_CAPTURE_BEGIN"
RETURNED = "HAMMERR_CUBEMAP_CAPTURE_RETURNED"
DISCONNECTED = "HAMMERR_CUBEMAP_DISCONNECTED"
OBSERVER = """
stage <- 0;
ready_at <- 0.0;
function HammeRRThink() {
    local player = Entities.FindByClassname(null, "player");
    if (player == null) return 0.5;
    if (stage == 0) {
        SendToConsole("sv_cheats 1; sv_lan 1; joystick 0; bot_quota 0; bot_kick; jointeam 2; joinclass 1; hidepanel info; hidepanel team; hidepanel class_ter; hidepanel class_ct; r_drawvgui 0; gameui_hide; cl_drawhud 0; r_drawviewmodel 0; r_drawothermodels 0; mat_fullbright 0");
        ready_at = Time() + 6.0;
        stage = 1;
    }
    if (stage == 1 && Time() > ready_at && player.GetTeam() == 2 && player.GetHealth() == 100) {
        SendToConsole("noclip; setpos -192 -176 64; setang 15 45 0; sv_lan; mat_hdr_level; mat_fullbright");
        ready_at = Time() + 3.0;
        stage = 2;
    }
    if (stage == 2 && Time() > ready_at) {
        printl("HAMMERR_CUBEMAP_READY " + player.GetTeam() + " " + player.GetHealth());
        ready_at = Time() + 3.0;
        stage = 3;
    }
    if (stage == 3 && Time() > ready_at) {
        SendToConsole("exec hammerr_cubemap_capture_once.cfg");
        ready_at = Time() + 1.0;
    }
    return 0.5;
}
"""


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def read_bounded(path, maximum):
    require(path.stat().st_size <= maximum, f"Oversized input: {path}")
    with path.open("rb") as stream:
        data = stream.read(maximum + 1)
    require(len(data) <= maximum, f"Input grew beyond its limit: {path}")
    return data


def write_cfg(path, text):
    temporary = path.with_suffix(".pending")
    temporary.write_text(text)
    temporary.replace(path)


def module_fingerprints(root):
    result = {}
    for name in MODULES:
        path = (root / "bin/linux64" / name).resolve(strict=True)
        data = read_bounded(path, MAX_BSP_BYTES)
        require(data[:5] == b"\x7fELF\x02", f"Expected ELF64 module: {path}")
        result[name] = {"path": str(path), "bytes": len(data), "sha256": digest(data)}
    require(result["engine.so"]["sha256"] == ENGINE_SHA256, "Installed engine changed; repeat the static evidence audit before this probe")
    return result


def bsp_lumps(data):
    require(1036 <= len(data) <= MAX_BSP_BYTES and data[:4] == b"VBSP", "Invalid or oversized BSP")
    result = []
    for index in range(64):
        offset, size, version, compression = struct.unpack_from("<4i", data, 8 + index * 16)
        require(0 <= offset <= offset + size <= len(data), f"Invalid lump span: {index}")
        result.append((version, compression, data[offset:offset + size]))
    return result


def zip_entries(payload):
    require(22 <= len(payload) <= MAX_PAK_BYTES, "Invalid PAK size")
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        infos = archive.infolist()
        require(len(infos) <= 256, "Too many PAK entries")
        names = [info.filename for info in infos]
        require(len(names) == len(set(name.lower() for name in names)), "Duplicate PAK names")
        require(all(not name.startswith(("/", "\\")) and "\\" not in name and ".." not in name.split("/") for name in names), "Unsafe PAK name")
        require(sum(info.file_size for info in infos) <= MAX_PAK_BYTES, "PAK decompression budget exceeded")
        require(all(0 < info.file_size <= MAX_ENTRY_BYTES and not info.flag_bits & 1 for info in infos), "Invalid or encrypted PAK entry")
        spans = []
        for info in infos:
            require(info.compress_type in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED) and info.extract_version <= 20, "Unsupported ZIP method/version")
            offset = info.header_offset
            require(0 <= offset <= offset + 30 <= len(payload), "Truncated ZIP local header")
            local = struct.unpack_from("<4s5H3I2H", payload, offset)
            require(local[0] == b"PK\x03\x04" and local[2] == info.flag_bits and local[3] == info.compress_type, "ZIP local/central header mismatch")
            require(local[2] & 8 or local[6:9] == (info.CRC, info.compress_size, info.file_size), "ZIP local CRC/size mismatch")
            start = offset + 30 + local[9] + local[10]
            end = start + info.compress_size
            require(start <= end <= len(payload), "Truncated ZIP entry")
            spans.append((offset, end))
        spans.sort()
        require(all(a[1] <= b[0] for a, b in zip(spans, spans[1:])), "Overlapping ZIP entry spans")
        require(archive.testzip() is None, "PAK CRC check failed")
        entries = {info.filename: archive.read(info) for info in infos}
        records = [{"name": info.filename, "bytes": info.file_size, "crc32": f"{info.CRC:08x}", "compression": info.compress_type, "sha256": digest(entries[info.filename])} for info in infos]
    return entries, records


def make_zip(entries):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", compression=zipfile.ZIP_STORED) as archive:
        for name, data in sorted(entries.items()):
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_STORED
            archive.writestr(info, data)
    return buffer.getvalue()


def seed_pak(data, entries):
    original, _ = zip_entries(bsp_lumps(data)[PAKFILE][2])
    require(not set(original) & set(entries), "Seed assets collide with original PAK entries")
    payload = make_zip({**original, **entries})
    result = bytearray(data)
    result.extend(b"\0" * (-len(result) % 4))
    struct.pack_into("<4i", result, 8 + PAKFILE * 16, len(result), len(payload), 0, 0)
    result.extend(payload)
    return bytes(result)


def sample_name(origin):
    return f"materials/maps/{MAP_NAME}/c{origin[0]}_{origin[1]}_{origin[2]}.vtf"


def bootstrap_cube():
    header = bytearray(80)
    header[:4] = b"VTF\0"
    for offset, value in [(4, 7), (8, 2), (12, 80), (20, 0x4000 | 0x0c)]:
        struct.pack_into("<I", header, offset, value)
    for offset, value in [(16, CUBE_SIZE), (18, CUBE_SIZE), (24, 1), (26, 0xffff), (63, 1)]:
        struct.pack_into("<H", header, offset, value)
    struct.pack_into("<3f", header, 32, 0.5, 0.0, 0.5)
    struct.pack_into("<f", header, 48, 1.0)
    struct.pack_into("<i", header, 52, 0)
    header[56] = int(math.log2(CUBE_SIZE)) + 1
    struct.pack_into("<i", header, 57, -1)
    pixels = bytearray()
    for mip in reversed(range(header[56])):
        size = max(1, CUBE_SIZE >> mip)
        pixels.extend(bytes([255, 0, 255, 255]) * size * size * 6)
    return bytes(header + pixels)


def image_size(image_format, width, height):
    if image_format in (13, 20):
        return ((width + 3) // 4) * ((height + 3) // 4) * 8
    if image_format in (14, 15):
        return ((width + 3) // 4) * ((height + 3) // 4) * 16
    channels = {0: 4, 1: 4, 2: 3, 3: 3, 11: 4, 12: 4, 16: 4}.get(image_format)
    require(channels is not None, f"Unsupported LDR capture format: {image_format}")
    return width * height * channels


def rgb565(value):
    return ((value >> 11) * 255 // 31, ((value >> 5) & 63) * 255 // 63, (value & 31) * 255 // 31)


def decode_image(data, image_format, width, height):
    if image_format not in (13, 14, 15, 20):
        stride = image_size(image_format, 1, 1)
        pixels = []
        for offset in range(0, len(data), stride):
            row = data[offset:offset + stride]
            if image_format in (2, 0):
                rgb = row[:3]
            elif image_format in (3, 12, 16):
                rgb = row[2::-1]
            elif image_format == 11:
                rgb = row[1:4]
            elif image_format == 1:
                rgb = row[3:0:-1]
            else:
                raise AssertionError(f"Unsupported pixel format: {image_format}")
            pixels.append(tuple(rgb))
        return pixels
    stride = 8 if image_format in (13, 20) else 16
    pixels = [(0, 0, 0)] * width * height
    blocks_x = (width + 3) // 4
    for index, offset in enumerate(range(0, len(data), stride)):
        color_offset = offset if stride == 8 else offset + 8
        c0, c1, bits = struct.unpack_from("<HHI", data, color_offset)
        a, b = rgb565(c0), rgb565(c1)
        if c0 > c1 or image_format in (14, 15):
            palette = [a, b, tuple((2 * x + y) // 3 for x, y in zip(a, b)), tuple((x + 2 * y) // 3 for x, y in zip(a, b))]
        else:
            palette = [a, b, tuple((x + y) // 2 for x, y in zip(a, b)), (0, 0, 0)]
        bx, by = index % blocks_x, index // blocks_x
        for pixel in range(16):
            x, y = bx * 4 + pixel % 4, by * 4 + pixel // 4
            if x < width and y < height:
                pixels[y * width + x] = palette[(bits >> (pixel * 2)) & 3]
    return pixels


def decode_cube(data):
    require(80 <= len(data) <= MAX_ENTRY_BYTES and data[:4] == b"VTF\0", "Captured sample is not a bounded VTF")
    major, minor, header = struct.unpack_from("<3I", data, 4)
    require(major == 7 and 0 <= minor <= 5 and 64 <= header <= len(data), "Unsupported VTF version/header")
    width, height = struct.unpack_from("<2H", data, 16)
    flags = struct.unpack_from("<I", data, 20)[0]
    frames, first = struct.unpack_from("<2H", data, 24)
    depth = struct.unpack_from("<H", data, 63)[0] if minor >= 2 else 1
    image_format = struct.unpack_from("<i", data, 52)[0]
    mips = data[56]
    faces = 7 if minor < 5 and first != 0xffff else 6
    require(width == height == CUBE_SIZE and depth == frames == 1 and flags & 0x4000, "Captured VTF has wrong cube dimensions, frames, depth or envmap flag")
    require(mips == int(math.log2(CUBE_SIZE)) + 1, "Captured VTF lacks a full mip chain")
    require(all(math.isfinite(value) for value in struct.unpack_from("<3f", data, 32)), "Nonfinite VTF reflectivity")
    if minor >= 3:
        require(header >= 80, "Truncated VTF resource header")
        count = struct.unpack_from("<I", data, 68)[0]
        require(count <= 32 and 80 + count * 8 <= header, "Invalid VTF resource count")
        resources = [data[80 + index * 8:88 + index * 8] for index in range(count)]
        high = [row for row in resources if row[:3] == b"\x30\0\0"]
        require(len(high) == 1 and not high[0][3] & 2, "Invalid high-resolution VTF resource")
        start = struct.unpack_from("<I", high[0], 4)[0]
    else:
        low_format = struct.unpack_from("<i", data, 57)[0]
        low_width, low_height = data[61:63]
        thumbnail = image_size(low_format, low_width, low_height) if low_width and low_height else 0
        start = header + thumbnail
    cursor = start
    largest = None
    for mip in reversed(range(mips)):
        size = max(1, width >> mip)
        span = image_size(image_format, size, size)
        require(header <= cursor <= cursor + span * faces <= len(data), "Truncated VTF face/mip span")
        if mip == 0:
            largest = [decode_image(data[cursor + face * span:cursor + (face + 1) * span], image_format, size, size) for face in range(6)]
        cursor += span * faces
    return {"version": [major, minor], "width": width, "height": height, "depth": depth, "frames": frames, "faces": faces, "mips": mips, "format": image_format, "flags": flags, "high_res_offset": start, "image_end": cursor, "bytes": len(data)}, largest


def save_faces(path, faces):
    width, height = CUBE_SIZE * 6, CUBE_SIZE
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for face in faces:
            for pixel in face[y * CUBE_SIZE:(y + 1) * CUBE_SIZE]:
                rows.extend(pixel)

    def chunk(kind, payload):
        return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", zlib.crc32(kind + payload) & 0xffffffff)

    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">2I5B", width, height, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def check_pixels(faces):
    centers = []
    for face in faces:
        patch = [face[y * CUBE_SIZE + x] for y in range(12, 20) for x in range(12, 20)]
        centers.append([sum(pixel[axis] for pixel in patch) / len(patch) for axis in range(3)])
    right, left, back, front, up, _ = centers
    require(right[1] > max(right[0], right[2]) * 2 and right[1] > 30, f"Right cube face missed the green wall: {centers}")
    require(left[0] > max(left[1], left[2]) * 2 and left[0] > 30, f"Left cube face missed the red wall: {centers}")
    require(back[2] > max(back[0], back[1]) * 2 and back[2] > 30, f"Back cube face missed the blue wall: {centers}")
    require(min(front[:2]) > front[2] * 2 and min(front[:2]) > 30, f"Front cube face missed the yellow wall: {centers}")
    require(min(up[1:]) > up[0] * 2 and min(up[1:]) > 30, f"Upper cube face missed the cyan ceiling: {centers}")
    pixels = [pixel for face in faces for pixel in face]
    pink = sum(r > 100 and b > 100 and g < min(r, b) / 3 for r, g, b in pixels)
    require(pink / len(pixels) < 0.25, "Captured images are predominantly missing-texture pixels")
    require(len({digest(bytes(component for pixel in face for component in pixel)) for face in faces}) >= 4, "Capture lacks distinct directional images")
    return {"face_order": FACE_NAMES, "center_rgb": centers, "pink_fraction": pink / len(pixels)}


def make_fixture(args):
    game = runtime.private_game(args.game_dir, args.output / "source-game", engine_physics=True)
    folder = game / "materials/hammerr_cubemap_probe"
    folder.mkdir(parents=True)
    colors = {"gray": (180, 180, 180), "cyan": (16, 200, 200), "red": (220, 24, 16), "green": (16, 220, 32), "yellow": (220, 200, 16), "blue": (16, 32, 220)}
    for name, rgb in colors.items():
        (folder / f"{name}.vtf").write_bytes(texture(bytes([*rgb, 255]) * 32 * 32))
        shader = "LightmappedGeneric" if name == "gray" else "UnlitGeneric"
        extra = ' $envmap "env_cubemap" $envmaptint "[0.35 0.35 0.35]"' if name == "gray" else ""
        (folder / f"{name}.vmt").write_text(f'{shader} {{ $basetexture "hammerr_cubemap_probe/{name}" $surfaceprop concrete{extra} }}\n')
    client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    try:
        ids = []
        for (low, high), material in zip(ROOM, ["gray", "cyan", "red", "green", "yellow", "blue"], strict=True):
            ids.append(client.call("create_block", {"mins": low, "maxs": high, "material": f"hammerr_cubemap_probe/{material}"})["id"])
        for classname, origin, properties in [
            ("info_player_terrorist", [-192, -176, 8], {}),
            ("info_player_counterterrorist", [192, 176, 8], {}),
            ("light", [0, 0, 180], {"_light": "255 255 255 200", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}),
            ("logic_script", [-192, -176, 64], {"targetname": "hammerr_cubemap_observer", "vscripts": "hammerr_cubemap_probe.nut", "thinkfunction": "HammeRRThink"}),
            *[("env_cubemap", list(origin), {"cubemapsize": str(SIZE_ENUM)}) for origin in SAMPLES],
        ]:
            ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
        source = args.output / "source.vmf"
        client.call("save_map", {"path": str(source)})
        before = source.read_bytes(), detail(client, ids), client.call("get_map_info")["undo"]
        client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "none", "bounces": 0})
        bsp = args.output / "source.bsp"
        compiled = client.call("compile_map", {"path": str(bsp)})
        require(compiled["success"] and compiled["stats"]["cubemap_samples"] == len(SAMPLES), compiled)
        require(compiled["stats"]["lighting_bytes"] > 0, "The reflective floor did not receive LDR lighting")
        data = read_bounded(bsp, MAX_BSP_BYTES)
        samples = bsp_lumps(data)[42]
        require(samples[:2] == (0, 0) and list(struct.iter_unpack("<3iB3x", samples[2])) == [(*origin, SIZE_ENUM) for origin in SAMPLES], "MCP fixture has unexpected cubemap records")
        require((source.read_bytes(), detail(client, ids), client.call("get_map_info")["undo"]) == before, "Compilation mutated authored source or history")
        report = {"bsp": str(bsp), "source": str(source), "fixture_game": str(game), "compile": compiled, "authored_through_mcp": True, "source_and_history_preserved": True, "binding_scope": "Unbound env_cubemap material is intentional: this probes native capture, not compiler reflection binding."}
        (args.output / "fixture.json").write_text(json.dumps(report, indent=2) + "\n")
        return bsp, game
    finally:
        client.close()


def verify_controls(text):
    values = {}
    for name, expected in CONTROL_VALUES.items():
        matches = re.findall(r'"' + re.escape(name) + r'" = "([^"]+)"', text)
        require(matches and float(matches[-1]) == expected, f"Post-map {name} does not equal {expected}: {matches}")
        values[name] = float(matches[-1])
    require(READY in text, "Post-map readiness marker missing")
    return values


def observed_modules(pid, expected, seen):
    path = Path(f"/proc/{pid}/maps")
    try:
        lines = path.read_text().splitlines()
    except FileNotFoundError:
        return
    for line in lines:
        parts = line.split(maxsplit=5)
        if len(parts) == 6 and Path(parts[5]).name in expected:
            name = Path(parts[5]).name
            actual = Path(parts[5]).resolve(strict=True)
            require(str(actual) == expected[name]["path"], f"Loaded {name} from an unexpected installation: {actual}")
            seen[name] = str(actual)


def loader_modules(output, root, expected):
    result = {}
    paths = list(output.glob("loader.*"))
    require(len(paths) <= 16 and sum(path.stat().st_size for path in paths) <= 8 * 1024**2, "Loader diagnostic budget exceeded")
    for path in paths:
        for line in read_bounded(path, 8 * 1024**2).decode(errors="replace").splitlines():
            match = re.search(r"file=(.+?) \[\d+\];\s+generating link map|calling init:\s*(.+)$", line)
            if match is None:
                continue
            value = Path((match[1] or match[2]).strip())
            if value.name not in expected:
                continue
            actual = (value if value.is_absolute() else root / value).resolve(strict=True)
            require(str(actual) == expected[value.name]["path"], f"Native loader used an unexpected capture module: {actual}")
            result[value.name] = {"path": str(actual), "method": "native loader files diagnostic", "log": str(path), "event": line.strip()}
    return result


def worker(args):
    output = args.output
    require(not os.environ.get("LD_PRELOAD"), "Do not inherit LD_PRELOAD into the native capture worker")
    require(os.environ.get("WAYLAND_DISPLAY") and os.environ.get("XDG_RUNTIME_DIR") == str(output / "wayland"), "Capture worker requires its private compositor")
    require(WIDTH >= CUBE_SIZE * 4 and HEIGHT >= CUBE_SIZE * 4, "Capture viewport is too small")
    fingerprints = module_fingerprints(args.game_dir)
    game = runtime.private_game(args.game_dir, output, engine_physics=True)
    scripts = game / "scripts/vscripts"
    (scripts / "hammerr_cubemap_probe.nut").write_text(runtime.private_observer(OBSERVER))
    cfg = game / "cfg/hammerr_cubemap_capture_once.cfg"
    write_cfg(cfg, "echo HAMMERR_CUBEMAP_WAITING_FOR_VERIFIED_READY\n")
    post_map = "toggle sv_lan 1\ntoggle mat_fullbright 0\nhelp sv_lan\nhelp mat_hdr_level\nhelp mat_fullbright\n"
    for name in ["server.cfg", "listenserver.cfg", "hammerr_cubemap_postmap.cfg"]:
        (game / "cfg" / name).write_text(post_map)
    assets = {str(path.relative_to(args.fixture_game)): path.read_bytes() for path in sorted((args.fixture_game / "materials").rglob("*")) if path.is_file()}
    sentinel = b"Preserve this preexisting packed asset through native capture.\n"
    assets["hammerr/cubemap-capture-sentinel.txt"] = sentinel
    bootstrap = bootstrap_cube()
    decode_cube(bootstrap)
    assets.update({sample_name(origin): bootstrap for origin in SAMPLES})
    staged = game / "maps" / f"{MAP_NAME}.bsp"
    staged.write_bytes(seed_pak(read_bounded(args.bsp, MAX_BSP_BYTES), assets))
    seeded = staged.read_bytes()
    seeded_lumps = bsp_lumps(seeded)
    seeded_entries, seeded_records = zip_entries(seeded_lumps[PAKFILE][2])
    (output / "seeded.bsp").write_bytes(seeded)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    private_settings = output / "settings"
    for name in ["config", "cache", "data"]:
        (private_settings / name).mkdir(parents=True)
    environment = {**os.environ, "LD_LIBRARY_PATH": str(args.game_dir / "bin/linux64"), "LD_DEBUG": "files", "LD_DEBUG_OUTPUT": str(output / "loader"), "SteamAppId": "240", "SteamGameId": "240", "SDL_VIDEODRIVER": "x11", "XDG_CONFIG_HOME": str(private_settings / "config"), "XDG_CACHE_HOME": str(private_settings / "cache"), "XDG_DATA_HOME": str(private_settings / "data")}
    launch_options = runtime.private_launch_options(args.game_dir, game, hdr_level=0, map_name=MAP_NAME, log_name="hammerr-cubemap.log", controls={"servercfgfile": "hammerr_cubemap_postmap.cfg", "mat_fullbright": 0})
    command = [str(args.game_dir / "cstrike_linux64"), "-game", str(game), "-windowed", "-w", str(WIDTH), "-h", str(HEIGHT), "-novid", *launch_options, "-nomaster", "-nojoy", "-console", "-port", str(args.port)]
    report = {"command": command, "cwd": str(args.game_dir), "module_fingerprints": fingerprints, "viewport": [WIDTH, HEIGHT], "capture_size": CUBE_SIZE, "sample_origins": SAMPLES, "seeded_pak": seeded_records, "bootstrap_sha256": digest(bootstrap), "capture_trigger": "buildcubemaps 1 in a private cfg after verified post-map readiness", "display": os.environ.get("DISPLAY"), "wayland_display": os.environ.get("WAYLAND_DISPLAY"), "desktop_input": False, "success": False}
    (output / "command.json").write_text(json.dumps(report, indent=2) + "\n")
    console = game / "hammerr-cubemap.log"
    seen = {}
    process = None
    try:
        with (output / "launcher.log").open("w") as log:
            process = subprocess.Popen(command, cwd=args.game_dir, env=environment, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
            started = time.monotonic()
            armed = False
            disarmed = False
            while time.monotonic() - started < args.timeout:
                text = read_bounded(console, 2 * 1024**2).decode(errors="replace") if console.exists() else ""
                observed_modules(process.pid, fingerprints, seen)
                if not armed and READY in text:
                    report["private_launch"] = runtime.verify_private_launch(game, text)
                    report["post_map_controls"] = verify_controls(text)
                    screenshot = output / "ready.png"
                    subprocess.run(["grim", str(screenshot)], check=True, timeout=5)
                    image = read_bounded(screenshot, MAX_PAK_BYTES)
                    require(image[:8] == b"\x89PNG\r\n\x1a\n" and image[12:16] == b"IHDR", "Private compositor returned an invalid screenshot")
                    dimensions = struct.unpack_from(">2I", image, 16)
                    require(min(dimensions) >= CUBE_SIZE * 4, "Private capture display is too small")
                    report["private_display_pixels"] = dimensions
                    write_cfg(cfg, f"echo {BEGIN}\nbuildcubemaps 1\necho {RETURNED}\nhelp sv_lan\nhelp mat_hdr_level\nhelp mat_fullbright\ndisconnect\necho {DISCONNECTED}\nquit\n")
                    armed = True
                    report["armed_seconds"] = time.monotonic() - started
                if armed and not disarmed and BEGIN in text:
                    write_cfg(cfg, "echo HAMMERR_CUBEMAP_RELOADED_WITHOUT_RECAPTURE\n")
                    disarmed = True
                code = process.poll()
                if code is not None:
                    report["exit_code"] = code
                    break
                time.sleep(0.02)
            else:
                raise TimeoutError(f"Native capture exceeded {args.timeout} seconds; inspect {output}")
            text = read_bounded(console, 2 * 1024**2).decode(errors="replace") if console.exists() else ""
            require(process.returncode == 0, f"Native client exited with {process.returncode}")
            require(all(marker in text for marker in [READY, BEGIN, RETURNED, DISCONNECTED]), "Capture/return/disconnect handshake did not complete")
            require(text.index(READY) < text.index(BEGIN) < text.index(RETURNED) < text.index(DISCONNECTED), "Capture ran outside the post-ready transaction")
            require(text.count(BEGIN) == 1, "Capture was accidentally repeated after native restart")
            require(all(re.search(rf"bounce: 1/1 sample: {index}/{len(SAMPLES)}", text) for index in range(1, len(SAMPLES) + 1)), "Native sample progress is incomplete")
            require(not any(message in text for message in ["vtex failed to compile cubemap", "Can't load vtex_dll", "Can't load bsppack", "Cube map buffer size"]), "Native texture compiler/packing reported a capture failure")
            report["private_launch"] = runtime.verify_private_launch(game, text)
            modules = {name: {"path": path, "method": "owned process /proc maps"} for name, path in seen.items()}
            modules.update(loader_modules(output, args.game_dir, fingerprints))
            require(set(modules) == set(MODULES), f"Did not observe every fingerprinted capture module in the owned process: {modules}")
            report["post_capture_controls"] = verify_controls(text)
            captured = read_bounded(staged, MAX_BSP_BYTES)
            after_lumps = bsp_lumps(captured)
            require(captured[4:8] == seeded[4:8] and captured[1032:1036] == seeded[1032:1036], "Native capture changed BSP version/revision")
            require(all(before == after for index, (before, after) in enumerate(zip(seeded_lumps, after_lumps, strict=True)) if index != PAKFILE), "Native capture changed a non-PAK BSP lump")
            final_entries, final_records = zip_entries(after_lumps[PAKFILE][2])
            sample_paths = {sample_name(origin) for origin in SAMPLES}
            for name, data in seeded_entries.items():
                if name not in sample_paths:
                    require(final_entries.get(name) == data, f"Native capture lost or changed packed asset: {name}")
            default_path = f"materials/maps/{MAP_NAME}/cubemapdefault.vtf"
            require(set(final_entries) - set(seeded_entries) <= {default_path}, "Native capture added an unexpected PAK path")
            samples = []
            for origin in SAMPLES:
                name = sample_name(origin)
                require(name in final_entries and final_entries[name] != bootstrap, f"Native capture left a bootstrap sample: {name}")
                metadata, faces = decode_cube(final_entries[name])
                save_faces(output / f"c{origin[0]}_{origin[1]}_{origin[2]}-faces.png", faces)
                pixels = check_pixels(faces)
                samples.append({"name": name, "sha256": digest(final_entries[name]), "vtf": metadata, "pixels": pixels})
            require(len({sample["sha256"] for sample in samples}) == len(SAMPLES), "Distinct sample positions produced identical complete captures")
            (output / "captured.bsp").write_bytes(captured)
            report.update({"success": True, "samples": samples, "final_pak": final_records, "unchanged_non_pak_lumps": [index for index in range(64) if index != PAKFILE], "packed_source_assets_preserved": True, "capture_seconds": time.monotonic() - started, "observed_modules": modules, "post_capture_console_tail": text[-12000:]})
    except BaseException as error:
        report.update({"error": str(error), "traceback": traceback.format_exc(), "observed_modules": seen})
        raise
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        try:
            report["configuration_preservation"] = runtime.finish_private_launch(game)
        except BaseException as error:
            report.update({"success": False, "preservation_error": str(error), "preservation_traceback": traceback.format_exc()})
            raise
        finally:
            report["final_exit_code"] = None if process is None else process.returncode
            (output / "runtime.json").write_text(json.dumps(report, indent=2) + "\n")


def protected_files(args, bsp, game):
    paths = [args.output / "source.vmf", bsp]
    paths += [path for path in (game / "materials").rglob("*") if path.is_file()]
    paths += [args.game_dir / "bin/linux64" / name for name in MODULES]
    paths += [path for path in (args.game_dir / "cstrike/cfg").rglob("*") if path.is_file()]
    paths.append(args.game_dir / "cstrike/maps" / f"{MAP_NAME}.bsp")
    for prefix in [args.game_dir, args.game_dir / "cstrike"]:
        for folder in ["materials/maps", "materialsrc/maps"]:
            paths += [path for path in (prefix / folder / MAP_NAME).rglob("*") if path.is_file()]
    return {str(path): digest(read_bounded(path, MAX_BSP_BYTES)) if path.exists() else None for path in sorted(set(paths))}


def main():
    parser = argparse.ArgumentParser(description="Prepare a private MCP-authored room and prove native LDR cubemap capture/packing; does not implement compiler reflection bindings.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=150)
    parser.add_argument("--port", type=int, default=27593)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--fixture-game", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.game_dir = args.game_dir.resolve(strict=True)
    args.binary = args.binary.resolve(strict=True)
    require(30 <= args.timeout <= 180 and 1024 <= args.port <= 65535, "Timeout or private port is out of bounds")
    if args.worker:
        worker(args)
        return
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": "Private native LDR cubemap capture proof only; compiler bindings remain unsupported.", "success": False, "desktop_input": False}
    before = None
    try:
        require(shutil.which("cage") and shutil.which("grim"), "Cage and grim are required for the private headless display")
        require(not os.environ.get("LD_PRELOAD"), "Do not inherit LD_PRELOAD into the capture probe")
        report["module_fingerprints"] = module_fingerprints(args.game_dir)
        bsp, game = make_fixture(args)
        before = protected_files(args, bsp, game)
        destination = args.output / "native"
        command = [sys.executable, "-B", str(Path(__file__).resolve()), "--worker", "--binary", str(args.binary), "--game-dir", str(args.game_dir), "--output", str(destination), "--bsp", str(bsp), "--fixture-game", str(game), "--timeout", str(args.timeout), "--port", str(args.port)]
        runtime.private_compositor(command, destination, timeout=args.timeout + 20)
        result = json.loads((destination / "runtime.json").read_text())
        require(result["success"], "Native worker did not verify captured data")
        report.update({"success": True, "runtime": result, "public_sources_and_assets_preserved": True})
    except BaseException as error:
        report.update({"error": str(error), "traceback": traceback.format_exc()})
        raise
    finally:
        if before is not None:
            after = protected_files(args, bsp, game)
            report["protected_file_sha256"] = before
            report["public_sources_and_assets_preserved"] = before == after
            if before != after:
                report["success"] = False
                report["changed_protected_files"] = [path for path in sorted(set(before) | set(after)) if before.get(path) != after.get(path)]
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        require(report.get("public_sources_and_assets_preserved", before is None), "Probe changed a protected source, asset, module or installed cfg")
    print(json.dumps({"success": True, "output": str(args.output), "samples": len(SAMPLES), "desktop_input": False}))


if __name__ == "__main__":
    main()
