import argparse
import os
import signal
import subprocess
import sys
from pathlib import Path

from validate_face_lighting import ROOT


def main():
    parser = argparse.ArgumentParser(description="Check native clipboard transfer on a private headless Wayland display.")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    runtime = args.output / "runtime"
    runtime.mkdir(mode=0o700)
    environment = {
        **os.environ,
        "WLR_BACKENDS": "headless",
        "WLR_HEADLESS_OUTPUTS": "1",
        "WLR_RENDERER": "pixman",
        "XDG_RUNTIME_DIR": str(runtime),
        "HAMMERR_ISOLATED_CLIPBOARD": "1",
        "SDL_AUDIODRIVER": "dummy",
    }
    environment.pop("DISPLAY", None)
    environment.pop("WAYLAND_DISPLAY", None)
    command = ["cage", "--", sys.executable, "-B", str(ROOT / "scripts/validate_clipboard.py"), "--binary", str(args.binary.resolve()), "--output", str(args.output / "checks"), "--native"]
    with (args.output / "compositor.log").open("w") as stream:
        process = subprocess.Popen(command, cwd=ROOT, env=environment, stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            result = process.wait(timeout=180)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            raise
    if result:
        print((args.output / "compositor.log").read_text())
        raise SystemExit(result)
    print((args.output / "checks/validation.json").read_text())


if __name__ == "__main__":
    main()
