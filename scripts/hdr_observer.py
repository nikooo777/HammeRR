import json

ENGINE_SHA256 = "965b90157cbec58187c553ce8bb65907e186c990b08453cb4abd75e909ec794a"
HARDWARE_VERSION = "MaterialSystemHardwareConfig012"
HDR_CONTROLS = {
    "mat_force_tonemap_scale": 0.25,
    "mat_autoexposure_min": 0.25,
    "mat_autoexposure_max": 0.25,
    "mat_disable_bloom": 1,
    "mat_bloomscale": 0,
}


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def read_probe(path):
    if not path.exists():
        return []
    lines = path.read_text().split("\n")
    return [json.loads(line) for line in lines[:-1] if line]


def verify_hardware(records, level):
    failures = [row for row in records if row["event"].startswith("invalid_") or row["event"] in ["acquisition_failed", "thread_failed", "ready_timeout"]]
    require(not failures, f"Native hardware observer failed: {failures}")
    if not records or records[-1]["event"] != "complete":
        return None
    samples = [row for row in records if row["event"] == "sample"]
    require(len(samples) == 8 and all(row["dx_support_level"] >= 90 and row["hardware_hdr_type"] in (1, 2) for row in samples), "Hardware HDR capability was not measured")
    require(all(row["hdr_type"] in (0, 1, 2) and row["hdr_enabled"] == (level == 2) and (row["hdr_type"] != 0) == (level == 2) for row in samples), "Native hardware selected the wrong HDR state")
    require(len({(row["hdr_type"], row["hdr_enabled"]) for row in samples}) == 1, "Native HDR state changed during sampling")
    factory = [row for row in records if row["event"] == "hardware_factory" and row["present"] and row["code"] == 0 and row["version"] == HARDWARE_VERSION]
    require(factory, "The verified native hardware interface was not acquired")
    starts = [row for row in records if row["event"] == "start"]
    require(len(starts) == 1, "Native hardware observer process identity is missing")
    return {"samples": samples, "factory": factory[-1], "native_pid": starts[0]["pid"]}



def probe_environment(root, game, library, ready, report):
    if ready.exists() or report.exists():
        raise ValueError("Use fresh native HDR probe paths")
    return {
        "LD_PRELOAD": str(library.resolve(strict=True)),
        "HAMMERR_HDR_EXPECTED_EXE": str((root / "cstrike_linux64").resolve(strict=True)),
        "HAMMERR_HDR_LIBRARY_DIR": str((root / "bin/linux64").resolve(strict=True)),
        "HAMMERR_HDR_PRIVATE_GAME": str(game),
        "HAMMERR_HDR_READY": str(ready),
        "HAMMERR_HDR_REPORT": str(report),
    }
