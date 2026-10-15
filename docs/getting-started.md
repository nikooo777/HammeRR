[← Manual](README.md)

# Getting started

## Requirements

- **Linux.**
- **Rust 1.92 or newer.**
- **A Vulkan-capable graphics driver** for the editor window and rendered views. Headless editing and compiling work without one.
- **FFmpeg and ffplay** on `PATH` for optional [sound audition and audio export](sounds.md).
- **Counter-Strike: Source** installed through Steam. HammeRR uses its materials and entity definitions, and launches the Linux client to play compiled maps.

## Build

```sh
cargo build --release -p editor -p editor-core -p compiler
```

This produces four programs in `target/release/`:

| Program | Purpose |
| --- | --- |
| `editor` | The map editor, including its MCP server for AI agents. |
| `hammerr-headless` | The same editing session and stdio MCP tools, with a GUI-independent build. See [Without a window](mcp.md#without-a-window). |
| `hammerr-compile` | The standalone map compiler. |
| `hammerr-physics` | A helper used when building maps with CS:S engine physics. Keep it in the same folder as the other programs. |

## Launch

```sh
./target/release/editor path/to/map.vmf
```

Leave out the filename to start with an empty map. HammeRR finds Counter-Strike: Source through Steam and loads its materials and SDK entity definitions automatically.

Saving increments the VMF's map version and uses standard VMF formatting while retaining imported properties and extra blocks. Round trips through other editors haven't been verified yet.

Saving through a file symlink updates its target and preserves the link and target permissions. Broken or looping links are refused. Failed saves preserve unsaved changes; repair the path or choose another destination and save again.

| Option | Purpose |
| --- | --- |
| `--game <gameinfo.txt>` | Load content from a specific game installation instead of the one found through Steam. |
| `--fgd <file.fgd>` | Load another entity definition file, including the files it includes. |
| `--mcp-port <port>` | Serve MCP on another port. The default is `7377`. |
| `--no-mcp` | Don't start the MCP server. |
| `--headless` | Run without a window and serve MCP over stdin/stdout. Graphics start only when a render is requested. See [AI agents and MCP](mcp.md#without-a-window). |

## Game content

HammeRR reads materials from the search paths listed in `gameinfo.txt`, including VPK archives.

`--game` accepts absolute paths, relative paths and a filename in the current directory. Search-path tokens resolve from the supplied gameinfo location, including when that file is a symlink.

- To use extra content, add its folder to the `gameinfo.txt` search paths. The folder itself may be a symlink.
- Links inside a content folder must point somewhere inside that same folder. Directory loops are not followed repeatedly.

## Next steps

- Learn the window, previews and navigation in [Editor basics](basics.md).
- Connect an AI agent with [AI agents and MCP](mcp.md).
- Build and play your map with [Compile and play](compile-and-play.md).
