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
- FGD entity classes with inherited defaults, class search, typed properties,
  choices and spawn flags. Humans and agents can place lights and spawns.
- Built-in MCP server. Agents share the map, selection, camera and undo
  history with the user, and edits show up live.
- Native BSP compilation with leak detection, pointfiles, detail brushes
  and brush entity models, available in the UI, CLI and MCP.
- Compile and play in Linux CS:S, inspect game logs, and stop the managed
  game process. Leak paths appear in the viewport after failed compiles.

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
`run_map` compiles and launches CS:S; `launch_game` launches the latest
successful compile. `game_log` reads engine output and `stop_game` stops
the managed client. `set_view` controls tool textures and leak paths.
`search_entity_classes` finds point and brush classes; `get_entity_class`
returns their defaults, property types, choices, flags and I/O signatures.
`load_entity_definitions` loads an FGD and its includes for both front ends.
`create_entity`, `set_entity_properties`, `tie_to_entity` and `move_to_world`
use the same creation and validation operations as the GUI.

For example, an agent can place a warm light with:

```json
{ "name": "create_entity", "arguments": { "classname": "light", "origin": [0, 0, 128], "properties": { "_light": "255 220 180 400" } } }
```

| `render_view` of a map built entirely by an agent | `render_view` framing an agent-placed block |
| --- | --- |
| ![Agent-built test map](docs/images/agent-render.png) | ![Agent-placed block in de_cbble](docs/images/agent-block.png) |

## Building and running

Requires Rust 1.88+ and a Vulkan-capable GPU.

```sh
cargo run --release -p editor -- [map.vmf] [--game <gameinfo.txt>] [--fgd <file.fgd>] [--mcp-port N | --no-mcp] [--headless]
cargo test --workspace
```

Fly with the right mouse button held and WASD/QE. Click selects, Ctrl+click
toggles, arrows and PgUp/PgDn nudge, Ctrl+D duplicates, Del deletes,
Ctrl+Z/Y undo and redo.

## Entities

Click **Entity…** in the selection panel to search point classes and place
an entity at the shown coordinates. The position starts 128 units in front
of the camera, snapped to grid 16; MCP uses the same position when `origin`
is omitted. New entities receive inherited FGD defaults. Select the entity
to edit its properties, choose enumerated values and toggle spawn flags.
Invalid typed values are rejected without changing the map.

Select world brushes and click **Tie to entity…** to make a brush entity.
Select a brush entity and click **Move to world** to return its brushes.
These operations share the map's undo history with agent edits.

The editor discovers `cstrike.fgd` in the game directory or installed
Source SDK. Use `--fgd` or **Load FGD…** in the class picker for another
definition file. Custom classes and keys remain editable, and loading
definitions preserves existing map data. Definitions are read from your
installation; game assets are not bundled.

Lights are saved to VMF and carried into BSP entities. The compiler
currently produces fullbright maps; placed lights will affect baked
lighting once lighting compilation is implemented.

## Compiling maps

Click **Compile** to build the current map into a `.bsp` beside its VMF.
Compilation runs in the background and includes unsaved edits. Unsaved
maps prompt for an output filename.

Click **Run Map** to compile and play the current snapshot in the installed
Linux CS:S client. **Game Log** shows its console and launcher output;
**Stop Map** closes the process started by this editor. A changed map must
be compiled again before launching it. Every launch passes `-insecure`.

Each run uses a uniquely named copy of the BSP and its own log files. The
copied map is removed when the game exits; logs remain available until the
next run or editor shutdown. Closing the editor stops its managed game
process. Existing maps and unrelated game processes are left untouched.

The standalone compiler uses the game's materials and compile flags:

```sh
cargo run --release -p compiler --bin hammerr-compile -- map.vmf --game /path/to/cstrike/gameinfo.txt
```

Use `-o /path/to/map.bsp` to choose the output. A leak produces a `.lin`
pointfile and leaves any existing BSP untouched. Maps need sealed world
geometry and at least one point entity in open space.
Leaks also appear as red lines through geometry in the editor and MCP
renders. The **Leak path** checkbox toggles them. Editing the map hides
the previous compile's path until the next compilation.

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
| `source-formats` | KeyValues, VMF, FGD, VPK, VTF, VMT, BSP, game filesystem |
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

Planned editor work: I/O connections, brush tools, 2D views, displacement
editing and model rendering.

This is a clean-room project: no Valve or Hammer++ code is used.
Counter-Strike: Source and its assets are not included.
