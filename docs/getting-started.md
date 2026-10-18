[← Manual](README.md)

# Getting started

## Requirements

- **Linux.**
- **Rust 1.92 or newer.**
- **A Vulkan-capable graphics driver** for the editor window and rendered views. Headless editing and compiling work without one.
- **FFmpeg and ffplay** on `PATH` for optional [sound audition and audio export](sounds.md).
- **Counter-Strike: Source** installed through Steam. HammeRR uses its materials and entity definitions, and launches the Linux client to play compiled maps.
- **Cage, grim and Bubblewrap** for optional [generated reflections](reflections.md). Capture also needs a running Steam client and usable Linux user namespaces.

## Build

```sh
cargo build --release -p editor -p editor-core -p compiler -p native-runtime
```

This produces five programs in `target/release/`:

| Program | Purpose |
| --- | --- |
| `editor` | The map editor, including its MCP server for AI agents. |
| `hammerr-headless` | The same editing session and stdio MCP tools, with a GUI-independent build. See [Without a window](mcp.md#without-a-window). |
| `hammerr-compile` | The standalone map compiler. |
| `hammerr-physics` | A helper used when building maps with CS:S engine physics. Keep it in the same folder as the other programs. |
| `hammerr-native-runtime` | The private client helper for [generated reflections](reflections.md). Keep it beside the editor, headless server and compiler. |

## Launch

```sh
./target/release/editor path/to/map.vmf
```

Leave out the filename to start with an empty map. HammeRR finds Counter-Strike: Source through Steam and loads its materials and SDK entity definitions automatically.

Saving increments the VMF's map version and uses standard VMF formatting while retaining imported properties and extra blocks. Round trips through other editors haven't been verified yet.

Imports accept at most 256 nested blocks. An excessive or malformed import
leaves the current map intact. Raw property keys/values, classnames, material
names and group names cannot contain literal double quotes; invalid edits are
rejected. Quoted unknown block names, including names containing spaces, remain
quoted when saved. Serialization failures leave the existing file intact and
appear as save or recovery errors.

Saving through a file symlink updates its target and preserves the link and target permissions. Broken or looping links are refused. Failed saves preserve unsaved changes; repair the path or choose another destination and save again.

Open maps and build outputs reserve their physical files inside the application,
including aliases and hard links. A conflicting Open, Save or Compile fails
without replacing the current map or output. If a source file or its directory
changes outside HammeRR, an ordinary Save rejects the stale path. Use **Save as…**
to choose the intended destination again, including the same filename.

Opening, saving and compiler publication require Linux. Writes retain the destination
directory while writing; manual writers and separate application processes do
not participate in the application's reservations. A warning after publication
means the file was written but its path could not be verified; inspect the
destination before choosing it again.

Discard confirmations apply to the map state you reviewed. If the map changes
before New, Open, Recent maps or recovery replaces it, the action asks you to
review again. Save and Compile file choosers also reject a buffer replaced while
the chooser was open; edits within the same buffer remain allowed.

## Save to another file

1. Click **Save as…** in the toolbar, or press **Ctrl+Shift+S**.
2. Choose a folder and filename in **Save map as**. The chooser starts with the
   current filename and folder; unnamed maps start with `untitled.vmf`.
3. Confirm the destination. It becomes the current source file for later
   **Save** / **Ctrl+S**. Cancel leaves the map and current source unchanged.

![Save as and the two toolbar rows](images/save-as-controls.png)

Choosing a destination again does not merge external edits. A chooser opened
before New or Open replaces the buffer cannot save the replacement map.

## Launch options

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
