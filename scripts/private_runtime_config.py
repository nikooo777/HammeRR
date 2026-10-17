import ctypes
import hashlib
import json
import math
import os
import re
import subprocess
import sys
import time
from contextlib import contextmanager
from pathlib import Path

PLAYABLE_ENGINE_SHA256 = (
    "965b90157cbec58187c553ce8bb65907e186c990b08453cb4abd75e909ec794a"
)
STEAM_API_SHA256 = "c0cc3d2802e5f2463bfa0046c41d2f65a6335baaeefbba6c7dbd5681d5ca7c46"
PRIVATE_CVARS = frozenset(
    {
        "bot_quota",
        "cl_detail_avoid_force",
        "cl_detail_avoid_radius",
        "cl_detail_max_sway",
        "cl_drawhud",
        "con_logfile",
        "fps_max",
        "joystick",
        "mat_autoexposure_max",
        "mat_autoexposure_min",
        "mat_bloomscale",
        "mat_disable_bloom",
        "mat_force_tonemap_scale",
        "mat_fullbright",
        "mat_hdr_level",
        "r_drawothermodels",
        "r_drawvgui",
        "r_drawviewmodel",
        "servercfgfile",
        "sv_cheats",
        "sv_lan",
    }
)


def read_vdf(path):
    if path.stat().st_size > 16 * 1024 * 1024:
        raise ValueError(f"Steam configuration exceeds the read limit: {path}")
    tokens = re.findall(r'"((?:\\.|[^"\\])*)"|([{}])', path.read_text())
    position = 0

    def block(nested=False, depth=0):
        nonlocal position
        if depth > 64:
            raise ValueError(f"Steam configuration exceeds the nesting limit: {path}")
        result = {}
        while position < len(tokens):
            key, mark = tokens[position]
            position += 1
            if mark == "}" and nested:
                return result
            if mark or position == len(tokens):
                raise ValueError(f"Malformed Steam configuration: {path}")
            value, mark = tokens[position]
            position += 1
            if mark == "{":
                result[key.lower()] = block(True, depth + 1)
            elif mark:
                raise ValueError(f"Malformed Steam configuration: {path}")
            else:
                result[key.lower()] = value
        if nested:
            raise ValueError(f"Unterminated Steam configuration: {path}")
        return result

    return block()


def vdf_value(data, *keys, default=None):
    for key in keys:
        if not isinstance(data, dict) or key not in data:
            return default
        data = data[key]
    return data


def cloud_cache_state(steam, account):
    userdata = steam / "userdata" / account
    cache = userdata / "240"
    remote = cache / "remote/cfg/config.cfg"
    manifest = read_vdf(cache / "remotecache.vdf")["240"]
    entry = manifest.get("cfg/config.cfg", {})
    if not remote.is_file() or not remote.stat().st_size:
        raise RuntimeError(
            "Existing App240 Cloud config is required; a missing remote config can enable Cloud"
        )
    if (
        entry.get("sha") != hashlib.sha1(remote.read_bytes()).hexdigest()
        or entry.get("syncstate") != "1"
    ):
        raise RuntimeError(
            "App240 Cloud config must match its synchronized cache entry"
        )
    shared = read_vdf(userdata / "7/remote/sharedconfig.vdf")
    local = read_vdf(userdata / "config/localconfig.vdf")
    app_flag = vdf_value(
        shared,
        "userroamingconfigstore",
        "software",
        "valve",
        "steam",
        "apps",
        "240",
        "cloudenabled",
    )
    if app_flag not in (None, "1"):
        raise RuntimeError(
            "Private validation requires the existing App240 Cloud setting to be enabled"
        )
    sync = vdf_value(
        local,
        "userlocalconfigstore",
        "software",
        "valve",
        "steam",
        "apps",
        "240",
        "cloud",
        "last_sync_state",
    )
    if sync != "synchronized":
        raise RuntimeError("App240 Steam Cloud must already be synchronized")
    semantic = {"change_number": manifest.get("changenumber"), "entries": {}}
    for name, record in manifest.items():
        if isinstance(record, dict):
            semantic["entries"][name] = {
                key: value
                for key, value in record.items()
                if key not in {"localtime", "time"}
            }
    return {
        "app_cloud_enabled": True,
        "app_cloud_flag_present": app_flag is not None,
        "sync": sync,
        "remote_cache": semantic,
    }


def reject_configuration_writes(path, *, imported=False):
    if path.stat().st_size > 1024 * 1024:
        raise ValueError(f"Oversized private configuration: {path}")
    source = path.read_text()
    forbidden = (
        r"\b(?:host_writeconfig|writeconfig|cl_cloud_settings|SetCloudEnabledForApp)\b"
    )
    if imported:
        forbidden += r'|(?:^|[;\r\n])\s*"?(?:exec|alias|stuffcmds)"?(?=\s|;|$)'
    if re.search(forbidden, source, re.IGNORECASE):
        raise ValueError(
            f"Configuration cannot write preferences or change Cloud settings: {path}"
        )


def validate_private_game_paths(root, output):
    root = root.resolve(strict=True)
    game = output.resolve() / "cstrike"
    if game.is_relative_to(root) or root.is_relative_to(game):
        raise ValueError("Native validation requires a separate private game directory")
    if game.is_symlink():
        raise ValueError("Private game directories cannot be symlinks")
    if game.exists():
        info = game / "gameinfo.txt"
        if (
            not info.is_file()
            or info.is_symlink()
            or info.stat().st_size > 16 * 1024
            or 'game "HammeRR validation"' not in info.read_text()
        ):
            raise ValueError(
                "Refusing to overwrite an existing non-validation game directory"
            )
        if any(path.is_symlink() for path in game.rglob("*")):
            raise ValueError(
                "Private game files and writable directories cannot be symlinks"
            )
    return game


def preservation_snapshot(root, steam, account, *, strict=True):
    userdata = steam / "userdata" / account
    files = [
        root / "cstrike/cfg/config.cfg",
        root / "cstrike/videoconfig_linux.cfg",
        userdata / "240/remote/cfg/config.cfg",
        userdata / "240/remotecache.vdf",
        userdata / "7/remote/sharedconfig.vdf",
        userdata / "config/localconfig.vdf",
    ]
    files += sorted((userdata / "240/remote/materials/vgui/logos").glob("spray.*"))
    cloud_log = steam / "logs/cloud_log.txt"
    try:
        cloud = cloud_cache_state(steam, account)
    except (KeyError, OSError, ValueError, RuntimeError) as error:
        if strict:
            raise
        cloud = {"state_error": str(error)}
    return {
        "files": {
            str(path): hashlib.sha256(path.read_bytes()).hexdigest()
            if path.is_file()
            else None
            for path in files
        },
        "cloud": cloud,
        "cloud_log_size": cloud_log.stat().st_size,
        "steam": str(steam),
        "account": account,
        "root": str(root),
    }


def steam_identity_worker(root):
    library = root / "bin/linux64/libsteam_api.so"
    if hashlib.sha256(library.read_bytes()).hexdigest() != STEAM_API_SHA256:
        raise RuntimeError(
            "Re-verify the installed Steam flat identity API before launching"
        )
    if ctypes.sizeof(ctypes.c_void_p) != 8 or sys.byteorder != "little":
        raise RuntimeError(
            "Steam identity query requires the verified Linux x86-64 ABI"
        )
    if (
        os.environ.get("SteamAppId") != "240"
        or os.environ.get("SteamGameId") != "240"
        or os.environ.get("LD_PRELOAD")
    ):
        raise RuntimeError(
            "Steam identity query requires private App240 environment without preload"
        )
    api = ctypes.CDLL(str(library), mode=os.RTLD_NOW | os.RTLD_LOCAL)
    signatures = {
        "SteamAPI_Init": ([], ctypes.c_bool),
        "SteamAPI_Shutdown": ([], None),
        "SteamAPI_SteamUser_v021": ([], ctypes.c_void_p),
        "SteamAPI_ISteamUser_BLoggedOn": ([ctypes.c_void_p], ctypes.c_bool),
        "SteamAPI_ISteamUser_GetSteamID": ([ctypes.c_void_p], ctypes.c_uint64),
        "SteamAPI_SteamUtils_v010": ([], ctypes.c_void_p),
        "SteamAPI_ISteamUtils_GetAppID": ([ctypes.c_void_p], ctypes.c_uint32),
    }
    for name, (arguments, result) in signatures.items():
        function = getattr(api, name)
        function.argtypes = arguments
        function.restype = result
    if not api.SteamAPI_Init():
        raise RuntimeError("Read-only Steam identity initialization failed")
    try:
        user = api.SteamAPI_SteamUser_v021()
        utils = api.SteamAPI_SteamUtils_v010()
        if not user or not utils or api.SteamAPI_ISteamUtils_GetAppID(utils) != 240:
            raise RuntimeError(
                "Read-only Steam identity interfaces or App240 identity unavailable"
            )
        if not api.SteamAPI_ISteamUser_BLoggedOn(user):
            raise RuntimeError("Steam must already have a live logged-on user")
        steam_id = api.SteamAPI_ISteamUser_GetSteamID(user)
        account = steam_id & 0xFFFFFFFF
        if (
            not account
            or steam_id >> 56 != 1
            or (steam_id >> 52) & 15 != 1
            or (steam_id >> 32) & 0xFFFFF != 1
        ):
            raise RuntimeError("Steam did not return a public individual user identity")
        if api.SteamAPI_ISteamUser_GetSteamID(user) != steam_id:
            raise RuntimeError("Steam user changed during the identity query")
        result = {
            "steam_id64": str(steam_id),
            "account": str(account),
            "app_id": 240,
            "user_interface": "SteamUser021",
            "utils_interface": "SteamUtils010",
            "steam_api_sha256": STEAM_API_SHA256,
        }
    finally:
        api.SteamAPI_Shutdown()
    return result


def live_steam_identity(root, game):
    if os.environ.get("LD_PRELOAD"):
        raise RuntimeError(
            "Read-only Steam identity preflight cannot inherit LD_PRELOAD"
        )
    environment = {
        **os.environ,
        "SteamAppId": "240",
        "SteamGameId": "240",
        "LD_LIBRARY_PATH": str(root / "bin/linux64"),
    }
    command = [
        sys.executable,
        "-I",
        "-B",
        str(Path(__file__).resolve()),
        "--steam-identity",
        str(root),
    ]
    log_path = game.parent / "steam-identity.log"
    with log_path.open("w") as log:
        process = subprocess.Popen(
            command,
            cwd=game.parent,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
        )
        try:
            code = process.wait(timeout=15)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
    if code:
        raise RuntimeError(f"Read-only Steam identity query failed; inspect {log_path}")
    if log_path.stat().st_size > 1024 * 1024:
        raise RuntimeError("Read-only Steam identity output exceeded its limit")
    prefix = "HAMMERR_STEAM_IDENTITY "
    records = [
        json.loads(line[len(prefix) :])
        for line in log_path.read_text().splitlines()
        if line.startswith(prefix)
    ]
    if len(records) != 1:
        raise RuntimeError(
            f"Read-only Steam identity query returned no unique identity; inspect {log_path}"
        )
    identity = records[0]
    steam = root.parent.parent.parent
    users = read_vdf(steam / "config/loginusers.vdf")["users"]
    if identity["steam_id64"] not in users:
        raise RuntimeError("The live Steam user has no matching cached login profile")
    started = time.monotonic()
    stable_since = None
    previous = None
    while True:
        try:
            state = cloud_cache_state(steam, identity["account"])
        except (KeyError, OSError, ValueError, RuntimeError):
            state = None
        now = time.monotonic()
        if state is None or state != previous:
            stable_since = now if state is not None else None
            previous = state
        if (
            state is not None
            and stable_since is not None
            and now - started >= 4
            and now - stable_since >= 3
        ):
            break
        if now - started >= 12:
            raise RuntimeError(
                "Steam Cloud did not settle after the read-only identity query"
            )
        time.sleep(0.5)
    (game.parent / "steam-identity.json").write_text(
        json.dumps(identity, indent=2) + "\n"
    )
    return identity


def cloud_log_activity(path, offset):
    errors = set()
    appended = b""
    if path.stat().st_size < offset:
        errors.add("Steam Cloud log rotated during private validation")
    else:
        with path.open("rb") as stream:
            stream.seek(offset)
            appended = stream.read(8 * 1024 * 1024 + 1)
        if len(appended) > 8 * 1024 * 1024:
            errors.add("Steam Cloud log exceeded the private validation read limit")
    app_lines = [
        line
        for line in appended.decode(errors="replace").splitlines()
        if "[AppID 240]" in line
    ]
    uploads = [
        line
        for line in app_lines
        if re.search(
            r"Need to upload file|HTTP upload|Upload batch initiated|Upload OK",
            line,
        )
    ]
    return app_lines, uploads, errors


def begin_private_launch(root, game):
    steam = root.parent.parent.parent
    cloud_log = steam / "logs/cloud_log.txt"
    cloud_log_size = cloud_log.stat().st_size
    identity = live_steam_identity(root, game)
    account = identity["account"]
    before = preservation_snapshot(root, steam, account)
    _, uploads, errors = cloud_log_activity(cloud_log, cloud_log_size)
    if uploads or errors:
        raise RuntimeError(
            "Steam identity preflight uploaded App240 Cloud data or lost its audit log; "
            f"uploads={uploads}, errors={sorted(errors)}"
        )
    before["cloud_log_size"] = cloud_log_size
    before["steam_identity"] = identity
    reject_configuration_writes(
        steam / "userdata" / account / "240/remote/cfg/config.cfg", imported=True
    )
    for directory in [game / "cfg", game / "scripts/vscripts"]:
        for path in directory.rglob("*"):
            if path.is_file() and path.suffix in {".cfg", ".rc", ".nut"}:
                reject_configuration_writes(path)
    (game.parent / "private-preservation-before.json").write_text(
        json.dumps(before, indent=2) + "\n"
    )


def finish_private_launch(game, *, observer="worker"):
    path = game.parent / "private-preservation-before.json"
    before = json.loads(path.read_text())
    previous_path = game.parent / "private-preservation.json"
    try:
        previous = (
            json.loads(previous_path.read_text()) if previous_path.exists() else None
        )
    except (OSError, ValueError):
        previous = {
            "success": False,
            "errors": ["Previous audit was interrupted or unreadable"],
        }
    started = time.monotonic()
    settled_at = started
    last_signature = None
    changed = set()
    cloud_changed = False
    errors = set()
    cloud_before = before["cloud"].copy()
    cloud_before.pop("app_cloud_flag_present", None)
    cloud_log = Path(before["steam"]) / "logs/cloud_log.txt"
    while True:
        after = preservation_snapshot(
            Path(before["root"]), Path(before["steam"]), before["account"], strict=False
        )
        current_changed = sorted(
            name
            for name in before["files"].keys() | after["files"].keys()
            if not name.endswith(
                ("/remotecache.vdf", "/sharedconfig.vdf", "/localconfig.vdf")
            )
            and after["files"].get(name) != before["files"].get(name)
        )
        changed.update(current_changed)
        cloud_after = after["cloud"].copy()
        cloud_after.pop("app_cloud_flag_present", None)
        busy = (
            cloud_after.get("state_error")
            == "App240 Steam Cloud must already be synchronized"
        )
        if not busy and cloud_after != cloud_before:
            cloud_changed = True
        app_lines, uploads, log_errors = cloud_log_activity(
            cloud_log, before["cloud_log_size"]
        )
        errors.update(log_errors)
        signature = json.dumps(
            [current_changed, cloud_after, app_lines], sort_keys=True
        )
        now = time.monotonic()
        if signature != last_signature or busy:
            settled_at = now
            last_signature = signature
        elapsed = now - started
        settled = now - settled_at >= 3 and not busy
        if elapsed >= 6 and settled:
            break
        if elapsed >= 12:
            errors.add(
                "Steam Cloud state did not settle within the post-exit observation window"
            )
            break
        time.sleep(0.5)
    report = {
        "observer": observer,
        "before": before,
        "after": after,
        "changed_preference_files": sorted(changed),
        "cloud_preferences_unchanged": not cloud_changed
        and cloud_before == cloud_after,
        "app240_cloud_uploads": uploads,
        "observation_seconds": elapsed,
        "settled_seconds": now - settled_at,
        "errors": sorted(errors),
        "previous_audit_success": previous.get("success") if previous else None,
        "success": not changed
        and not cloud_changed
        and cloud_before == cloud_after
        and not uploads
        and not errors
        and (previous is None or previous["success"]),
    }
    serialized = json.dumps(report, indent=2) + "\n"
    (game.parent / f"private-preservation-{observer}.json").write_text(serialized)
    previous_path.write_text(serialized)
    if not report["success"]:
        raise RuntimeError(
            f"Private validation changed shared settings or uploaded Cloud data; inspect {game.parent}"
        )
    return report


def private_observer(source):
    names = json.dumps(sorted(PRIVATE_CVARS))
    dispatch = """
function HammeRRPrivateConsole(commands) {
    local cvars = __CVARS__;
    foreach (part in split(commands, ";")) {
        part = strip(part);
        local words = split(part, " ", true);
        if (words.len() == 0) continue;
        if (cvars.find(words[0]) != null) {
            part = words.len() == 1 ? "help " + words[0] : "toggle " + part;
        }
        SendToConsole(part);
    }
}
""".replace("__CVARS__", names)
    if "HammeRRPrivateConsole" in source:
        raise ValueError("Private observer dispatcher is already installed")
    return source.replace("SendToConsole(", "HammeRRPrivateConsole(") + dispatch


def private_launch_options(
    root,
    game,
    *,
    hdr_level=0,
    map_name="hammerr_runtime",
    log_name="hammerr-runtime.log",
    controls=None,
):
    root = root.resolve(strict=True)
    game = game.resolve(strict=True)
    if game.is_relative_to(root) or not (game / "gameinfo.txt").is_file():
        raise ValueError("Native validation requires a separate private game directory")
    engine_sha = hashlib.sha256(
        (root / "bin/linux64/engine.so").read_bytes()
    ).hexdigest()
    if engine_sha != PLAYABLE_ENGINE_SHA256:
        raise RuntimeError(
            "Re-verify this engine's -default configuration-write guard before launching"
        )
    if isinstance(hdr_level, bool) or hdr_level not in (0, 2):
        raise ValueError("Private validation supports fresh LDR or HDR launches")
    for token in [map_name, log_name]:
        if (
            not re.fullmatch(r"[A-Za-z0-9_/-]+(?:\.[A-Za-z0-9_]+)?", token)
            or ".." in token
        ):
            raise ValueError("Invalid private launch file or map name")
    values = {
        "con_logfile": log_name,
        "sv_cheats": 1,
        "sv_lan": 1,
        "mat_hdr_level": hdr_level,
        "fps_max": 60,
        **(controls or {}),
    }
    if values["sv_lan"] != 1 or values["mat_hdr_level"] != hdr_level:
        raise ValueError(
            "Private launch controls cannot override LAN or selected HDR mode"
        )
    lines = []
    for name, value in values.items():
        if name not in PRIVATE_CVARS:
            raise ValueError(f"Unsupported private launch control: {name}")
        if isinstance(value, bool):
            raise TypeError(
                f"Boolean private launch controls require an explicit 0 or 1: {name}"
            )
        if isinstance(value, (int, float)):
            if not math.isfinite(value):
                raise ValueError(f"Nonfinite private launch control: {name}")
            value = str(value)
        elif (
            not isinstance(value, str)
            or not re.fullmatch(r"[A-Za-z0-9_./-]+", value)
            or ".." in value
        ):
            raise ValueError(f"Invalid private launch control: {name}")
        lines.append(f"toggle {name} {value}")
    lines += [f"help {name}" for name in values]
    lines += ["echo HAMMERR_PRIVATE_PREMAP_READY", f"map {map_name}"]
    profile = game / "cfg/hammerr_private_launch.cfg"
    profile.write_text("\n".join(lines) + "\n")
    (game / "cfg/valve.rc").write_text("exec hammerr_private_launch.cfg\n")
    report = {
        "engine_sha256": engine_sha,
        "automatic_configuration_writes_disabled": True,
        "cloud_reads": "allowed into private game files",
        "cloud_setting_commands": False,
        "configuration_write_guard": "-default; no named host_writeconfig commands",
        "profile": str(profile),
        "startup": "Host_ReadConfiguration, private valve.rc, profile, first map",
        "controls": values,
        "pre_map_marker": "HAMMERR_PRIVATE_PREMAP_READY",
    }
    (game.parent / "private-launch.json").write_text(
        json.dumps(report, indent=2) + "\n"
    )
    begin_private_launch(root, game)
    return ["-default", "-insecure"]


def private_readonly_launch(command, output, environment):
    output = output.resolve(strict=True)
    temporary = output / "tmp"
    temporary.mkdir(mode=0o700)
    arguments = [
        "/usr/bin/bwrap", "--die-with-parent", "--ro-bind", "/", "/",
        "--dev-bind", "/dev", "/dev", "--proc", "/proc",
        "--bind", str(output), str(output), "--", *command,
    ]
    return arguments, {**environment, "TMPDIR": str(temporary)}


@contextmanager
def private_native_process(
    command, root, game, output, environment, report,
    *, preservation_key="configuration_preservation",
):
    process = None
    failure = None
    cleanup_failures = []
    try:
        arguments, environment = private_readonly_launch(command, output, environment)
        report["command"] = arguments
        with (output / "launcher.log").open("w") as stream:
            process = subprocess.Popen(
                arguments, cwd=root, env=environment, stdin=subprocess.DEVNULL,
                stdout=stream, stderr=stream,
            )
            yield process
    except BaseException as error:
        failure = error
        report["success"] = False
        report.setdefault("error", str(error))
    finally:
        try:
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
        except BaseException as error:
            cleanup_failures.append(error)
            try:
                if process is not None and process.poll() is None:
                    process.kill()
                    process.wait(timeout=5)
            except BaseException as error:
                cleanup_failures.append(error)
        report["exit_code"] = None if process is None else process.returncode
        try:
            report[preservation_key] = finish_private_launch(game)
        except BaseException as error:
            report["preservation_error"] = str(error)
            cleanup_failures.append(error)
        if cleanup_failures:
            report["success"] = False
            report["cleanup_failures"] = [str(error) for error in cleanup_failures]
        if failure is not None:
            for error in cleanup_failures:
                failure.add_note(f"Private native cleanup also failed: {error}")
            raise failure
        if cleanup_failures:
            raise BaseExceptionGroup("Private native cleanup failed", cleanup_failures)


def verify_private_launch(game, text):
    marker = "HAMMERR_PRIVATE_PREMAP_READY"
    if (
        marker not in text
        or "HAMMERR_MAPSPAWN" not in text
        or text.index(marker) > text.index("HAMMERR_MAPSPAWN")
    ):
        raise RuntimeError(
            "Private settings were not observed before the first map spawn"
        )
    if "Host_WriteConfiguration: Wrote" in text:
        raise RuntimeError(
            "Private validation unexpectedly wrote archived configuration"
        )
    return json.loads((game.parent / "private-launch.json").read_text())


if __name__ == "__main__":
    if len(sys.argv) != 3 or sys.argv[1] != "--steam-identity":
        raise SystemExit(
            "Usage: private_runtime_config.py --steam-identity INSTALLED_GAME_ROOT"
        )
    print(
        "HAMMERR_STEAM_IDENTITY "
        + json.dumps(steam_identity_worker(Path(sys.argv[2]).resolve(strict=True))),
        flush=True,
    )
