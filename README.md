# HammeRR

A Linux-native map editor for Counter-Strike: Source, written in Rust as a
modern replacement for Hammer++. It reads and writes Hammer's VMF format,
renders maps with the game's own materials, and exposes every editing
feature to AI agents over MCP.

![The editor with sdk_de_cbble open](docs/images/editor.png)

*sdk_de_cbble in HammeRR. The crate in the courtyard was placed by an agent
over MCP; the activity panel shows who did what.*

## Features

- Opens, edits and saves VMF files; unmodified files round-trip byte for byte.
- Loads game content from `gameinfo.txt` search paths and VPK archives
  (VMT materials, VTF textures), finding CS:S through the Steam libraries.
- wgpu viewport with textures, translucency and alpha test, selection
  highlights and fly camera.
- Undoable editing: blocks, entities, key/values, move with texture lock,
  duplicate, delete, materials, tie to entity / move to world.
- Built-in MCP server. Agents share the map, selection, camera and undo
  history with the user, and edits show up live.

## Agent integration

The GUI serves MCP at `http://127.0.0.1:7377/mcp`; `--headless` serves it
over stdio without a window.

```json
{ "mcpServers": { "hammerr": { "url": "http://127.0.0.1:7377/mcp" } } }
```

Tools cover files, queries (`find_objects`, `get_objects`), selection,
editing, undo/redo, the camera, `search_materials`, and visual checks:
`render_view` returns a PNG of any viewpoint and `pick_at_pixel` maps a pixel
of that image back to an object and face.

| `render_view` of a map built entirely by an agent | `render_view` framing an agent-placed block |
| --- | --- |
| ![Agent-built test map](docs/images/agent-render.png) | ![Agent-placed block in de_cbble](docs/images/agent-block.png) |

## Building and running

Requires Rust 1.88+ and a Vulkan-capable GPU.

```sh
cargo run --release -p editor -- [map.vmf] [--game <gameinfo.txt>] [--mcp-port N | --no-mcp] [--headless]
cargo test --workspace
```

Fly with the right mouse button held and WASD/QE. Click selects, Ctrl+click
toggles, arrows and PgUp/PgDn nudge, Ctrl+D duplicates, Del deletes,
Ctrl+Z/Y undo and redo.

## Layout

| Crate | Role |
| --- | --- |
| `source-formats` | KeyValues, VMF, VPK, VTF, VMT, BSP, game filesystem |
| `map-core` | Brush geometry, displacements, undoable document, edit operations, picking |
| `render` | wgpu renderer, materials, camera, offscreen snapshots |
| `editor` | Slint UI, shared session, MCP server |
| `compiler` | VMF to BSP compiler (in progress) |

## Status

Map compiling is being built from scratch as native, parallel replacements
for VBSP, VVIS and VRAD; the BSP reader/writer is done and verified against
every stock map. Planned next: entity classes from FGDs, I/O connections,
brush tools, 2D views, displacement editing and model rendering.

This is a clean-room project: no Valve or Hammer++ code is used.
Counter-Strike: Source and its assets are not included.
