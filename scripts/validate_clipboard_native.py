import argparse
import sys
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, ROOT
from validate_native_runtime import private_compositor


def main():
    parser = argparse.ArgumentParser(description="Check native clipboard transfer on a private headless Wayland display.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    command = [sys.executable, "-B", str(ROOT / "scripts/validate_clipboard.py"), "--binary", str(args.binary.resolve()), "--output", str(args.output / "checks"), "--native"]
    private_compositor(command, args.output, timeout=180, extra_environment={"HAMMERR_ISOLATED_CLIPBOARD": "1"})
    print((args.output / "checks/validation.json").read_text())


if __name__ == "__main__":
    main()
