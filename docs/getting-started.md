[← Manual](README.md)

# Getting started

Create, edit and compile Counter-Strike: Source maps on Linux.

## Requirements

- Linux, Rust 1.92 or later, and a C++20 compiler.
- Counter-Strike: Source installed through Steam.
- A Vulkan driver for the editor and rendered views.
- FFmpeg and ffplay for [sound previews](sounds.md).
- Cage, grim, Bubblewrap and running Steam for [reflection capture](reflections.md).

Editing and compilation without rendered views do not need a GPU.

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
4. Use **Save** or **Ctrl+S** for later changes.

The editor finds the Steam game installation and SDK entity definitions automatically.
Keep `hammerr-physics` and `hammerr-native-runtime` beside the other programs.
These helpers support engine collision and reflection capture.

![Textured brush courtyard and model props](images/showcase-editor.png)

## Other content

| Option | Use |
| --- | --- |
| `--game <gameinfo.txt>` | Choose the game content search paths. |
| `--fgd <file.fgd>` | Choose entity definitions. |
| `--mcp-port <port>` | Change the default MCP port, `7377`. |
| `--no-mcp` | Disable the desktop MCP server. |
| `--headless` | Use stdio MCP without a window. |

Add custom content through the search paths in `gameinfo.txt`.
The reader supports directories and VPK archives.

## Limits

- File operations and game launching require Linux.
- Invalid imports leave the current map open. Nesting is limited to 256 blocks.
- Save retains imported properties and extra blocks. It uses standard VMF formatting.
- A changed or conflicting destination blocks Save. Use **Save as…** to review the destination again.
- Failed saves retain unsaved changes. A reported write-verification error can mean the file was already written.
- Other-editor round trips are unverified.

Next: [Editor basics](basics.md), [AI agents](mcp.md) or [Compile and play](compile-and-play.md).
