import argparse
import json
import os
import signal
import sys
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

STUB = r'''
import json
import os
import sys
import time
from pathlib import Path

mode = os.environ["STUB_MODE"]
Path(os.environ["STUB_PID"]).write_text(str(os.getpid()))
for line in sys.stdin:
    request = json.loads(line)
    if request["method"] == "initialize":
        response = {"jsonrpc": "2.0", "id": request["id"], "result": {}}
        if mode == "initialize_error":
            response = {"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32000, "message": "stub initialize failed"}}
        output = json.dumps(response) + "\n"
        if mode == "prefetched":
            output += '{"unexpected":"already buffered after initialize"}\n'
        os.write(1, output.encode())
    elif request["method"] == "notifications/initialized":
        if mode == "broken_pipe":
            os.close(0)
            print("stdin is closed", flush=True)
            time.sleep(0.1)
            sys.exit(0)
    elif request["method"] == "tools/call":
        name = request["params"]["name"]
        if mode == "operation_error":
            response = {"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32000, "message": "stub refuses " + name}}
        else:
            response = {"jsonrpc": "2.0", "id": request["id"], "result": {"content": [{"type": "text", "text": '{"job":null}'}]}}
        print(json.dumps(response), flush=True)
if mode == "timeout":
    while True:
        time.sleep(1)
if mode == "operation_error":
    print("unexpected output after failed status RPC", flush=True)
    sys.exit(7)
if mode == "recovery_error":
    sys.exit(2)
'''


def messages(error):
    if isinstance(error, BaseExceptionGroup):
        return "\n".join([str(error), *(messages(child) for child in error.exceptions)])
    return f"{type(error).__name__}: {error}"


def reaped(pid):
    try:
        os.waitpid(pid, os.WNOHANG)
    except ChildProcessError:
        return
    raise AssertionError(f"Owned stub child {pid} was not reaped")


def main():
    parser = argparse.ArgumentParser(description="Validate owned MCP client cleanup with safe local protocol children; Python>=3.11, Unix pipes, no HammeRR/native game/GPU process.")
    parser.add_argument("--scripts", type=Path, default=Path(__file__).resolve().parent, help="Repository scripts directory (default: beside this script)")
    parser.add_argument("--output", type=Path, required=True, help="New directory below repository target/validation")
    args = parser.parse_args()
    args.scripts, args.output = args.scripts.resolve(strict=True), args.output.resolve()
    assert args.output.is_relative_to(args.scripts.parent / "target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    sys.path.insert(0, str(args.scripts))
    import validate_instance_props as props
    from validate_face_lighting import Client
    from validate_recovery_exit import close_with_status, require_clean_eof

    stub = args.output / "private-child"
    stub.write_text(f"#!{sys.executable}\n" + STUB)
    stub.chmod(0o700)
    owned, pids, passed = [], [], []

    def start(name, mode):
        directory = args.output / name
        directory.mkdir()
        pid_file = directory / "pid"
        try:
            client = Client(None, stub, directory, environment={"STUB_MODE": mode, "STUB_PID": str(pid_file)})
            owned.append(client)
            return client
        finally:
            if pid_file.exists():
                pids.append(int(pid_file.read_text()))

    def refused(action, expected):
        try:
            action()
        except BaseException as error:
            assert expected in messages(error), messages(error)
            return error
        raise AssertionError(f"Expected protocol/cleanup refusal containing {expected!r}")

    try:
        refused(lambda: start("constructor", "initialize_error"), "stub initialize failed")
        reaped(pids[-1])
        passed.append("Fallible constructor initialization closes and reaps its own child")

        client = start("prefetched", "prefetched")
        process = client.process
        refused(lambda: require_clean_eof(client), "already buffered after initialize")
        assert client.process is None and process.stdin.closed and process.stdout.closed
        reaped(process.pid)
        passed.append("Strict EOF retains trailing JSON already prefetched by RPC readline")

        client = start("broken-pipe", "broken_pipe")
        process = client.process
        assert process.stdout.readline() == "stdin is closed\n"
        process.stdin.buffer.write(b"pending buffered input")
        refused(lambda: require_clean_eof(client), "BrokenPipeError")
        assert client.process is None and process.stdin.closed and process.stdout.closed
        reaped(process.pid)
        passed.append("BrokenPipe while closing stdin is reported after child reap")

        client = start("timeout", "timeout")
        process = client.process
        refused(lambda: client.close_with_status(timeout=0.1), "failed to drain")
        assert client.process is None and process.returncode == -signal.SIGKILL and process.stdout.closed
        reaped(process.pid)
        passed.append("EOF timeout kills and reaps the child without accepting a partial protocol")

        first, second = start("first-cleanup-error", "operation_error"), start("second-cleanup", "normal")
        processes = [first.process, second.process]
        report = {}
        errors = props.cleanup_clients([first, second], report)
        assert len(errors) == 2 and [(row["client"], row["stage"]) for row in report["cleanup_errors"]] == [(0, "stop"), (0, "eof")]
        assert all(client.process is None for client in (first, second))
        for process in processes:
            reaped(process.pid)
        passed.append("Failed status RPC and nonzero/trailing EOF do not skip either owned child")

        client = start("original-and-cleanup", "operation_error")
        process = client.process
        output = args.output / "validation"
        output.mkdir()
        game = output / "game"
        game.mkdir()
        fixture_args = SimpleNamespace(binary=Path(sys.executable), output=output, baked_fixture_game=None)
        with patch.object(props, "game_assets", return_value=game), patch.object(props, "private_client", return_value=client):
            error = refused(lambda: props.validate(fixture_args), "stub refuses inspect_model")
        assert "stub refuses get_compile_status" in messages(error) and "exit=7" in messages(error)
        saved = json.loads((output / "results.json").read_text())
        assert "inspect_model" in saved["error"] and len(saved["cleanup_errors"]) == 2 and saved["status"] == "failed"
        assert client.process is None
        reaped(process.pid)
        passed.append("Validator preserves the original failure plus both cleanup errors in exception/report")

        client = start("expected-recovery-exit", "recovery_error")
        process = client.process
        assert close_with_status(client) == (2, "")
        reaped(process.pid)
        client = start("strict-nonzero-exit", "recovery_error")
        process = client.process
        refused(lambda: require_clean_eof(client), "exit=2")
        reaped(process.pid)
        passed.append("Recovery callers can inspect exit2 while strict EOF still rejects it")
        (args.output / "results.json").write_text(json.dumps({"status": "passed", "checks": passed}, indent=2) + "\n")
        print(json.dumps({"status": "passed", "checks": len(passed)}))
    finally:
        for client in owned:
            if client.process is not None:
                try:
                    client.close_with_status(timeout=0.1)
                except BaseException:
                    pass
        for pid in pids:
            try:
                child, _status = os.waitpid(pid, os.WNOHANG)
                if child == 0:
                    os.kill(pid, signal.SIGKILL)
                    os.waitpid(pid, 0)
            except ChildProcessError:
                pass


if __name__ == "__main__":
    main()
