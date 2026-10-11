import argparse
import hashlib
import json
import time
from contextlib import contextmanager
from pathlib import Path

from validate_face_lighting import ROOT, Client
from validate_material_replacement import preserved_map


def wait_job(client):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        job = client.call("get_compile_status")["job"]
        if job["status"] not in ["running", "cancelling"]:
            return job
        time.sleep(0.01)
    raise AssertionError("Compile job did not finish within 30 seconds")


@contextmanager
def stopped_on_exit(client):
    try:
        yield
    finally:
        job = client.call("get_compile_status")["job"]
        if job and job["status"] in ["running", "cancelling"]:
            if job["cancellable"]:
                client.call("cancel_compile", {"token": job["token"]})
            wait_job(client)


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    with preserved_map(client), stopped_on_exit(client):
        check(
            "job" in client.call("get_compile_status"),
            "Compile status is inspectable before starting",
        )
        launch = client.call("get_compile_status")["launch"]
        check(
            not launch["ready"] and launch["reason"],
            "No build explains why play is unavailable",
        )
        walls = []
        for mins, maxs in [
            ([-272, -272, -16], [272, 272, 0]),
            ([-272, -272, 256], [272, 272, 272]),
            ([-272, -272, 0], [-256, 272, 256]),
            ([256, -272, 0], [272, 272, 256]),
            ([-256, -272, 0], [256, -256, 256]),
            ([-256, 256, 0], [256, 272, 256]),
        ]:
            walls.append(
                client.call(
                    "create_block",
                    {
                        "mins": mins,
                        "maxs": maxs,
                        "material": "dev/dev_measuregeneric01",
                    },
                )["id"]
            )
        client.call(
            "create_entity", {"classname": "info_player_terrorist", "origin": [0, 0, 8]}
        )
        client.call("save_map", {"path": str(output / "room.vmf")})
        history = client.call("get_map_info")["undo"]
        bsp = output / "room.bsp"
        started = client.call("start_compile", {"path": str(bsp)})["job"]
        check(bool(started["token"]), "Start returns a job token")
        completed = wait_job(client)
        check(
            completed["token"] == started["token"], "Status identifies the started job"
        )
        check(completed["status"] == "completed", "Sealed room compiles asynchronously")
        check(completed["stage"] == "writing", "Final stage reports output writing")
        check(
            completed["result"]["success"] and bsp.read_bytes().startswith(b"VBSP"),
            "Successful BSP published",
        )
        check(
            not completed["cancellable"] and not completed["stale"],
            "Completed job is current and cannot be cancelled",
        )
        client.call("cancel_compile", {"token": started["token"]}, error=True)
        check(
            client.call("get_map_info")["undo"] == history,
            "Compilation leaves edit history unchanged",
        )
        original_hash = hashlib.sha256(bsp.read_bytes()).hexdigest()
        launch = client.call("get_compile_status")["launch"]
        check(
            launch["ready"] and launch["bsp"] == str(bsp),
            "Current successful build is ready to play",
        )
        backup = bsp.with_suffix(".validation-backup.bsp")
        bsp.rename(backup)
        try:
            launch = client.call("get_compile_status")["launch"]
            check(
                not launch["ready"] and "missing" in launch["reason"],
                "Missing BSP disables play with a readable reason",
            )
            client.call("launch_game", error=True)
            check(
                not client.call("get_map_info")["launching"],
                "Rejected missing-output launch leaves no pending launch",
            )
        finally:
            backup.rename(bsp)
        client.call("compile_map", {"path": str(output / "blocking.bsp")})
        check(
            hashlib.sha256((output / "blocking.bsp").read_bytes()).hexdigest()
            == original_hash,
            "Blocking and background compiles emit identical BSP bytes",
        )
        old_token = client.call("get_compile_status")["job"]["token"]
        client.call("start_compile", {"path": str(output / "bad.vmf")}, error=True)
        check(
            client.call("get_compile_status")["job"]["token"] == old_token,
            "Invalid output preserves job status",
        )
        client.call("start_compile", {"path": str(output / "missing/room.bsp")})
        failed = wait_job(client)
        check(
            failed["status"] == "failed" and bool(failed["result"]["error"]),
            "Output write failures have terminal status",
        )
        launch = client.call("get_compile_status")["launch"]
        check(
            launch["ready"] and launch["bsp"] == str(output / "blocking.bsp"),
            "Failed rebuild retains the last current successful launch target",
        )
        client.call("delete_objects", {"ids": [walls[-1]]})
        check(
            client.call("get_compile_status")["job"]["stale"],
            "Edits mark previous job snapshots stale",
        )
        launch = client.call("get_compile_status")["launch"]
        check(
            not launch["ready"] and "changed" in launch["reason"],
            "Map edits disable launch with a readable reason",
        )
        client.call("start_compile", {"path": str(bsp)})
        leaked = wait_job(client)
        check(
            leaked["status"] == "failed" and bool(leaked["result"]["leak"]),
            "Leak failures expose structured diagnostics and pointfile",
        )
        check(bsp.with_suffix(".lin").exists(), "Leak pointfile published")
        check(
            hashlib.sha256(bsp.read_bytes()).hexdigest() == original_hash,
            "Leak failure preserves previous BSP",
        )
        client.call("undo")
        for i in range(64):
            x, y = (i % 8) * 56 - 224, (i // 8) * 56 - 224
            client.call(
                "create_primitive",
                {
                    "shape": "cylinder",
                    "mins": [x, y, 16],
                    "maxs": [x + 28, y + 28, 100],
                    "segments": 24,
                    "material": "dev/dev_measuregeneric01",
                },
            )
        client.call("save_map", {"path": str(output / "cancellation.vmf")})
        before = client.call("get_map_info")
        lin = bsp.with_suffix(".lin").read_bytes()
        started = client.call("start_compile", {"path": str(bsp)})["job"]
        client.call("cancel_compile", {"token": old_token}, error=True)
        client.call("start_compile", {"path": str(bsp)}, error=True)
        observed = client.call("get_compile_status")["job"]
        check(
            observed["status"] == "running" and observed["stage"] != "queued",
            "Active compiler exposes a real stage",
        )
        cancellation = client.call("cancel_compile", {"token": started["token"]})["job"]
        check(cancellation["status"] == "cancelling", "Cancellation request is visible")
        cancelled = wait_job(client)
        check(
            cancelled["status"] == "cancelled", "Cancellation reaches a terminal state"
        )
        check(
            hashlib.sha256(bsp.read_bytes()).hexdigest() == original_hash,
            "Accepted cancellation preserves existing BSP",
        )
        check(
            bsp.with_suffix(".lin").read_bytes() == lin,
            "Accepted cancellation preserves existing pointfile",
        )
        after = client.call("get_map_info")
        check(
            not after["compiling"] and not after["game_running"],
            "Cancelled worker releases the session without launching a game",
        )
        check(
            all(
                before[key] == after[key]
                for key in [
                    "undo",
                    "redo",
                    "unsaved_changes",
                    "selection",
                    "world_brushes",
                ]
            ),
            "Cancellation leaves the map and history unchanged",
        )
        check(
            not any(path.name.startswith(".tmp") for path in output.iterdir()),
            "No temporary output files remain",
        )
        (output / "jobs.json").write_text(
            json.dumps(
                {
                    "completed": completed,
                    "write_failure": failed,
                    "leak": leaked,
                    "running": observed,
                    "cancelled": cancelled,
                },
                indent=2,
            )
            + "\n"
        )
    checks.append("Original map, selection and camera restored")
    result = {
        "transport": "http" if client.url else "stdio",
        "checks": len(checks),
        "passed": checks,
    }
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key: value for key, value in result.items() if key != "passed"}))


def main():
    parser = argparse.ArgumentParser(
        description="Validate background compilation and cancellation through MCP."
    )
    parser.add_argument("--url")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
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
