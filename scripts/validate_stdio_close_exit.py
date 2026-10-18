import argparse
import json
from pathlib import Path

from validate_face_lighting import Client


def validate(binary, output):
    output.mkdir(parents=True, exist_ok=True)
    client = Client(None, binary, output)
    try:
        review = client.call("get_map_info")["document_close"]
        result = client.rpc("tools/call", {
            "name": "close_document",
            "arguments": {"review_token": review["review_token"]},
        })
        assert not result.get("isError"), result
        assert result["structuredContent"]["closed"], result
        assert result["_meta"]["io.hammerr/terminal-attempt"] == result["structuredContent"]["attempt"]
        assert not client.process.stdin.closed
        assert client.process.wait(timeout=10) == 0
        assert not client.process.stdin.closed
        return {"binary": str(binary), "exit_with_stdin_open": True}
    finally:
        if client.process.poll() is None:
            client.process.kill()
            client.process.wait(timeout=10)
        client.process.stdin.close()
        client.process.stdout.close()
        client.log.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", action="append", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    results = [validate(binary.resolve(), args.output / str(index)) for index, binary in enumerate(args.binary)]
    args.output.joinpath("results.json").write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps(results))


if __name__ == "__main__":
    main()
