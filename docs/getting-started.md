[← Manual](README.md)

# Getting started

Create, edit and compile Counter-Strike: Source maps on Linux or Windows.

![Textured brush courtyard and model props](images/showcase-editor.png)

## Requirements

- Counter-Strike: Source installed through Steam.
- A compatible graphics driver for the editor and rendered views (Vulkan on Linux; Direct3D 12 or Vulkan on Windows).
- FFmpeg and ffplay for [sound previews](sounds.md).
- On Linux: Cage, grim, Bubblewrap and running Steam for [reflection capture](reflections.md).
- For source builds: Rust 1.92 or later, and a C++20 compiler. On Windows, use the MSVC Rust toolchain and Visual Studio C++ Build Tools with a Windows SDK.

Editing and compilation without rendered views do not need a GPU.

## Download a build

1. Open [Releases](https://github.com/nikooo777/HammeRR/releases) and select a CalVer release (`vYYYY.M.PATCH`).
2. Download the Linux x86_64 `.tar.gz` or Windows x86_64 `.zip` archive. Compare its SHA-256 hash with `SHA256SUMS` before extraction.
3. Extract the whole archive. Keep the programs together in that folder.
4. Start `./editor` on Linux or `editor.exe` on Windows. Rust is not required for these downloads.

Linux builds need glibc 2.39 or later, libstdc++, Fontconfig and the Wayland/X11 libraries. If Windows reports a missing VC++ runtime, install the [Microsoft Visual C++ v14 x64 runtime](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist). The game and optional external tools are not included.

Windows executables have timestamped, self-signed Authenticode signatures. The ZIP includes the public certificate, `HammeRR-signing.cer`. Self-signing does not remove publisher-trust or SmartScreen warnings.

Each release includes a complete `CHANGELOG.md`. The release page shows up to 40 recent commits. See [Windows limits](#windows) before compiling a map.

## Build and launch

1. Build the programs:

   ```sh
   cargo build --release -j8 -p editor -p editor-core -p compiler -p native-runtime
   ```

2. Open a map:

   ```sh
   ./target/release/editor path/to/map.vmf
   ```

   Omit the filename to start an empty map.

3. Click **Save as…** to choose a `.vmf` destination.
4. Use **Save** or <kbd>Ctrl</kbd>+<kbd>S</kbd> for later changes.

The editor finds the Steam game installation and SDK entity definitions automatically.
On Linux, keep `hammerr-physics`, `hammerr-native-runtime` and `libnative_runtime.so`
beside the other programs. They support engine collision, reflection capture and native observation.

## Windows

1. Install Rust with the MSVC toolchain, Visual Studio C++ Build Tools and a Windows SDK.
2. Open PowerShell in the project folder and run:

   ```powershell
   .\run-windows.ps1
   .\run-windows.ps1 path\to\map.vmf
   ```

   The script builds and starts `target\debug\editor.exe`. Add `-Release` for `target\release\editor.exe`. After building, you can start the executable directly.
3. In **Build → Build settings…**, keep **Physics bodies** set to **No physics bodies** and **Generate reflections** off.
4. Connect agents through the desktop [HTTP MCP endpoint](mcp.md#connect-to-the-editor).

Steam is located through the current user's registry setting and standard installation folders. Additional Steam libraries are read from `libraryfolders.vdf`. Set `STEAM_DIR` for a custom Steam root or pass `--game` to choose content directly. The 64-bit Windows CS:S client is preferred when launching a map.

Editing, VMF saves, preferences, recovery, prefabs, instance extraction, rendered previews and BSP compilation are available. State uses `%LOCALAPPDATA%\hammerr\recovery` unless overridden. Sound previews need FFmpeg/ffplay on `PATH`.

> [!NOTE]
> Engine physics, native reflection capture, native observation and headless stdio MCP are unavailable on Windows. **Show silently** requires Hyprland; Windows document display requires an explicit focused-show request.

## Command-line options

| Option | Use |
| --- | --- |
| `--game <gameinfo.txt>` | Choose the game content search paths. |
| `--fgd <file.fgd>` | Choose entity definitions. |
| `--mcp-port <port>` | Change the default MCP port, `7377`. |
| `--no-mcp` | Disable the desktop MCP server. |
| `--headless` | Use stdio MCP without a window on Linux. |

## Custom content

Add custom content through the search paths in `gameinfo.txt`.
The reader supports directories and VPK archives.
Use **File → Content profiles…** to save and switch content, FGD, build and game paths
for each document. See [Content profiles](content-profiles.md).

## Limits

- VPK directories allow 64 MiB per encoded tree, 1,000,000 declarations, 1,024 bytes per logical path and 128 MiB of decoded metadata per index. Mounted indexes share a 256 MiB budget, and repeated declarations count toward it. Excess metadata causes an explicit load error.
- Native reflection capture, native observation and engine physics require Linux; see the Windows notes above for platform limits.
- Invalid imports leave the current map open. Nesting is limited to 256 blocks.
- An active saved camera needs finite, representable coordinates and a distinct look target. Invalid selected camera data refuses Open.
- Save retains imported properties and extra blocks. It uses standard VMF formatting.
- A changed or conflicting destination blocks Save. Use **Save as…** to review the destination again.
- Failed saves retain unsaved changes. A reported write-verification error can mean the file was already written.
- Other-editor round trips are unverified.

Next: [Editor basics](basics.md), [AI agents](mcp.md) or [Compile and play](compile-and-play.md).

<!-- page-nav -->

---

[Manual](README.md) · [Editor basics and saved views →](basics.md)
