import argparse
import hashlib
import html
import json
import os
import re
import shutil
import subprocess
import tomllib
from pathlib import Path
from typing import NamedTuple


class Platform(NamedTuple):
    archive_format: str
    archive_suffix: str
    binaries: tuple[str, ...]


CALVER = re.compile(r"v([1-9][0-9]{3})\.([1-9]|1[0-2])\.(0|[1-9][0-9]*)")
PLATFORMS = {
    "linux-x86_64": Platform(
        "gztar",
        ".tar.gz",
        (
            "editor",
            "hammerr-compile",
            "hammerr-headless",
            "hammerr-physics",
            "hammerr-native-runtime",
        ),
    ),
    "windows-x86_64": Platform("zip", ".zip", ("editor.exe", "hammerr-compile.exe")),
}
STAGE = Path("dist/stage")
PACKAGES = Path("dist/packages")


def run(*arguments):
    return subprocess.run(
        arguments, check=True, stdout=subprocess.PIPE, text=True
    ).stdout


def version(tag):
    match = CALVER.fullmatch(tag)
    return tuple(map(int, match.groups())) if match else None


def checked_tag():
    tag = os.environ["GITHUB_REF_NAME"]
    with open("Cargo.toml", "rb") as manifest:
        package_version = tomllib.load(manifest)["workspace"]["package"]["version"]
    if os.environ["GITHUB_REF_TYPE"] != "tag" or version(tag) is None:
        raise ValueError("Use a CalVer tag vYYYY.M.PATCH without leading zeroes")
    if tag != f"v{package_version}":
        raise ValueError("Release tag must match the workspace package version")
    return tag


def package_name(tag, platform):
    return f"HammeRR-{tag}-{platform}"


def checksums(paths, destination):
    lines = []
    for path in sorted(paths):
        with path.open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        lines.append(f"{digest}  {path.name}\n")
    destination.write_text("".join(lines), encoding="utf-8", newline="\n")


def stage(tag):
    platform = os.environ["RELEASE_PLATFORM"]
    STAGE.mkdir(parents=True)
    for name in PLATFORMS[platform].binaries:
        shutil.copy2(Path("target/release") / name, STAGE / name)
    shutil.copyfile("crates/editor/ui/icons/LICENSE", STAGE / "LUCIDE-LICENSE.txt")
    manual = f"https://github.com/{os.environ['GITHUB_REPOSITORY']}/blob/{tag}/docs/getting-started.md"
    instructions = (
        "Start ./editor. Keep all executables in this folder.\n"
        "Requires glibc 2.39 or newer, libstdc++, Fontconfig, Wayland/X11 libraries and a GPU driver.\n"
        if platform == "linux-x86_64"
        else "Start editor.exe. Install the Microsoft Visual C++ v14 x64 runtime if it is missing.\n"
        "The executables have timestamped, self-signed Authenticode signatures.\n"
        "HammeRR-signing.cer is the public certificate, not a private key.\n"
        "Self-signing does not remove Windows publisher-trust or SmartScreen warnings.\n"
        "Engine physics, native reflection capture and headless stdio MCP require Linux.\n"
    )
    (STAGE / "README.txt").write_text(
        f"HammeRR {tag} ({platform})\n\n{instructions}\n"
        "Install Counter-Strike: Source through Steam. No game assets are included.\n"
        f"Setup and platform limits: {manual}\n",
        encoding="utf-8",
        newline="\n",
    )


def archive(tag):
    platform = os.environ["RELEASE_PLATFORM"]
    spec = PLATFORMS[platform]
    expected = set(spec.binaries) | {"README.txt", "LUCIDE-LICENSE.txt"}
    if platform == "windows-x86_64":
        expected.add("HammeRR-signing.cer")
    if {path.name for path in STAGE.iterdir()} != expected:
        raise ValueError("Staged package has missing or unexpected files")
    checksums(STAGE.iterdir(), STAGE / "SHA256SUMS")
    name = package_name(tag, platform)
    bundle = STAGE.rename(STAGE.with_name(name))
    PACKAGES.mkdir(parents=True)
    shutil.make_archive(
        str(PACKAGES / name), spec.archive_format, bundle.parent, bundle.name
    )


def publish(tag):
    repository = os.environ["GITHUB_REPOSITORY"]
    url = f"https://github.com/{repository}"
    assets = [
        PACKAGES / f"{package_name(tag, platform)}{spec.archive_suffix}"
        for platform, spec in PLATFORMS.items()
    ]
    if set(PACKAGES.iterdir()) != set(assets) or any(
        not path.is_file() for path in assets
    ):
        raise ValueError(
            "Both platform archives are required; unexpected assets are not published"
        )
    pages = json.loads(
        run("gh", "api", "--paginate", "--slurp", f"repos/{repository}/releases")
    )
    releases = [release for page in pages for release in page]
    existing = next(
        (release for release in releases if release["tag_name"] == tag), None
    )
    if existing and not existing["draft"]:
        raise ValueError("This release is already published; use a new version")
    previous = None
    candidates = [
        release["tag_name"]
        for release in releases
        if not release["draft"]
        and not release["prerelease"]
        and version(release["tag_name"]) is not None
        and version(release["tag_name"]) < version(tag)
    ]
    for candidate in sorted(candidates, key=version, reverse=True):
        ancestry = subprocess.run(
            (
                "git",
                "merge-base",
                "--is-ancestor",
                f"refs/tags/{candidate}",
                f"refs/tags/{tag}",
            ),
            check=False,
        )
        if ancestry.returncode == 0:
            previous = candidate
            break
        if ancestry.returncode != 1:
            ancestry.check_returncode()
    revision = (
        f"refs/tags/{previous}..refs/tags/{tag}" if previous else f"refs/tags/{tag}"
    )
    entries = []
    for line in run("git", "log", "--format=%H%x00%s", revision).splitlines():
        commit, subject = line.split("\0", 1)
        subject = re.sub(r"([\\`*_\[\]])", r"\\\1", html.escape(subject))
        entries.append(f"- {subject} ([{commit[:8]}]({url}/commit/{commit}))")
    scope = (
        f"Changes since {previous}."
        if previous
        else "Initial CalVer release; includes the repository history."
    )
    changelog = PACKAGES / "CHANGELOG.md"
    changelog.write_text(
        f"# HammeRR {tag}\n\n{scope}\n\n" + "\n".join(entries) + "\n", encoding="utf-8"
    )
    assets.append(changelog)
    checksums(assets, PACKAGES / "SHA256SUMS")
    assets.append(PACKAGES / "SHA256SUMS")
    notes = Path("dist/release-notes.md")
    summary = (
        f"{scope}\n\n"
        "Downloads: Linux x86_64 (`.tar.gz`) and Windows x86_64 (`.zip`), built in release mode.\n\n"
        "Windows executables are self-signed and timestamped. The ZIP includes the public certificate; "
        "Windows trust and SmartScreen warnings can still appear.\n\n"
        f"[Setup and platform limits]({url}/blob/{tag}/docs/getting-started.md). "
        "`SHA256SUMS` covers both archives and `CHANGELOG.md`.\n\n"
        f"## Changes\n\n[Complete changelog]({url}/releases/download/{tag}/CHANGELOG.md) "
        f"({len(entries)} commits).\n\n"
    )
    if len(entries) > 40:
        summary += "Newest 40 commits shown below.\n\n"
    notes.write_text(summary + "\n".join(entries[:40]) + "\n", encoding="utf-8")
    if existing:
        if {asset["name"] for asset in existing["assets"]} - {
            path.name for path in assets
        }:
            raise ValueError(
                "The draft contains unexpected assets; review it before retrying"
            )
        run(
            "gh",
            "release",
            "edit",
            tag,
            "--title",
            f"HammeRR {tag}",
            "--notes-file",
            str(notes),
        )
    else:
        run(
            "gh",
            "release",
            "create",
            tag,
            "--draft",
            "--verify-tag",
            "--title",
            f"HammeRR {tag}",
            "--notes-file",
            str(notes),
        )
    run("gh", "release", "upload", tag, "--clobber", *(str(path) for path in assets))
    uploaded = json.loads(run("gh", "release", "view", tag, "--json", "assets"))[
        "assets"
    ]
    if {asset["name"]: asset["size"] for asset in uploaded} != {
        path.name: path.stat().st_size for path in assets
    }:
        raise ValueError(
            "Release assets do not match the completed packages; leaving the draft unpublished"
        )
    print(run("gh", "release", "edit", tag, "--draft=false"), end="")


def main():
    parser = argparse.ArgumentParser(
        description="Build and publish HammeRR's tagged release packages."
    )
    parser.add_argument("command", choices=("check", "stage", "archive", "publish"))
    command = parser.parse_args().command
    tag = checked_tag()
    if command == "stage":
        stage(tag)
    elif command == "archive":
        archive(tag)
    elif command == "publish":
        publish(tag)


if __name__ == "__main__":
    main()
