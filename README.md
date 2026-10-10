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
- Native BSP compilation with leak detection, pointfiles, detail brushes
  and brush entity models, available in the UI, CLI and MCP.

## Agent integration

The GUI serves MCP at `http://127.0.0.1:7377/mcp`; `--headless` serves it
over stdio without a window.

```json
{ "mcpServers": { "hammerr": { "url": "http://127.0.0.1:7377/mcp" } } }
```

Tools cover files, queries (`find_objects`, `get_objects`), selection,
editing, undo/redo, the camera, `search_materials`, and visual checks:
`render_view` returns a PNG of any viewpoint and `pick_at_pixel` maps a pixel
of that image back to an object and face. `compile_map` compiles the current
snapshot and returns diagnostics, geometry counts and leak paths.

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

## Compiling maps

Click **Compile** to build the current map into a `.bsp` beside its VMF.
Compilation runs in the background and includes unsaved edits. Unsaved
maps prompt for an output filename.

The standalone compiler uses the game's materials and compile flags:

```sh
cargo run --release -p compiler --bin hammerr-compile -- map.vmf --game /path/to/cstrike/gameinfo.txt
```

Use `-o /path/to/map.bsp` to choose the output. A leak produces a `.lin`
pointfile and leaves any existing BSP untouched. Maps need sealed world
geometry and at least one point entity in open space.

To generate a small room with both player spawns:

```sh
cargo run -p compiler --example room -- room.vmf
```

Copy the compiled BSP to `cstrike/maps/` and run `map room` in the CS:S
console. This room has been compiled and loaded in the Linux CS:S client,
with a player spawned inside and fullbright rendering enabled by the engine.

## Layout

| Crate | Role |
| --- | --- |
| `source-formats` | KeyValues, VMF, VPK, VTF, VMT, BSP, game filesystem |
| `map-core` | Brush geometry, displacements, undoable document, edit operations, picking |
| `render` | wgpu renderer, materials, camera, offscreen snapshots |
| `editor` | Slint UI, shared session, MCP server |
| `compiler` | Native VMF to BSP compiler and `hammerr-compile` CLI |

## Status

The compiler produces experimental fullbright BSP v20 maps with a single
visibility cluster. VVIS, VRAD, physics collision, water volumes,
displacements, static props, areaportals, overlays, cubemaps and origin
brushes remain unfinished. Unsupported geometry is reported in diagnostics.
The BSP reader/writer is verified against the stock CS:S maps.

Planned editor work: entity classes from FGDs, I/O connections, brush tools,
2D views, displacement editing and model rendering.

This is a clean-room project: no Valve or Hammer++ code is used.
Counter-Strike: Source and its assets are not included.
