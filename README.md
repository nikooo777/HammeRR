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
- Switchable perspective, top, front and side views with a visible grid,
  fractional snapping, cursor-centered zoom and selection framing.
- Draw brushes in planar views with a footprint preview, elevation and depth;
  each placement is one undoable edit shared with MCP.
- Exact numeric movement, rotation and scale with pivots and texture locking.
- Select individual brush faces and edit their material, texture offset,
  scale and rotation, or fit one tile to each face. Preview, apply and cancel
  through the UI or MCP, with one-step undo.
- Material thumbnails grouped by folder and category, with search, paging,
  map-used materials, recent applications and session-local favorites.
- Cancel buttons and Escape dismissal in entity, color, material and texture
  dialogs; cancelling texture previews leaves the map and history unchanged.
- Undoable editing: blocks, entities, key/values, move with texture lock,
  duplicate, delete, materials, tie to entity / move to world.
- FGD entity classes with inherited defaults, class search, typed properties,
  choices and spawn flags. Humans and agents can place lights and spawns.
- Detailed bulb models, spotlight and sunlight shapes with color and
  direction helpers.
  Readable property controls include color pickers, brightness sliders,
  separate position/rotation inputs, and searchable material previews.
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
`get_entity_inspector` returns the panel's labels, common/advanced fields
and control types. `set_entity_color` changes RGB and optional brightness;
`set_entity_component` edits one coordinate or brightness component.
`preview_material` returns a texture thumbnail and material details from
the same loader used by **Browse materials…**.
`set_spatial_settings` controls grid spacing, snapping, workplane and brush
depth. `set_camera` selects perspective/top/front/side and `frame_selection`
frames the current selection or map. `draw_block` previews or creates a
snapped footprint; `cancel_block_preview` clears it without an edit.
`transform_objects` moves, rotates and scales a selection atomically, with
an optional pivot and texture locking.
`set_selection_mode`, `select_faces` and `get_face_selection` share face
selection and highlighting with the GUI. `edit_faces` assigns materials and
changes UV parameters, with optional fitting and a non-destructive preview.
`commit_face_preview` applies that preview; `cancel_face_preview` discards it.
`browse_materials` exposes groups, counts and paged material metadata;
`set_material_favorite` manages the same favorites shown in the browser.

For example, an agent can place a warm light with:

```json
{ "name": "create_entity", "arguments": { "classname": "light", "origin": [0, 0, 128], "properties": { "_light": "255 220 180 400" } } }
```

| `render_view` of a map built entirely by an agent | `render_view` framing an agent-placed block |
| --- | --- |
| ![Agent-built test map](docs/images/agent-render.png) | ![Agent-placed block in de_cbble](docs/images/agent-block.png) |

## Building and running

Requires Rust 1.92+ and a Vulkan-capable GPU.

```sh
cargo run --release -p editor -- [map.vmf] [--game <gameinfo.txt>] [--fgd <file.fgd>] [--mcp-port N | --no-mcp] [--headless]
cargo test --workspace
```

Fly with the right mouse button held and WASD/QE. Click selects, Ctrl+click
toggles, arrows and PgUp/PgDn nudge, Ctrl+D duplicates, Del deletes,
Ctrl+Z/Y undo and redo.

Choose **Top · XY**, **Front · XZ** or **Side · YZ** in the viewport controls.
Right-drag pans and the wheel zooms around the cursor. **Frame · F** frames
the selection, or the whole map when nothing is selected. **Grid** accepts
spacing from 0.125 to 4096 units; **Snap** and **Show grid** are independent.
Arrows move in the displayed plane and PgUp/PgDn move along its depth axis,
using the current grid step; Shift uses at most one unit.

Click **Draw brush**, enter **Start Z/Y/X** and **Depth Z/Y/X**, then drag a
footprint. The preview shows its snapped dimensions; releasing places it,
and Esc cancels the drag. **Done** returns to selection. The perspective
**Add brush** button still places a default block.

Select objects and enable **Transform selection** in the inspector for exact
XYZ movement, world-axis rotation or positive scale factors. Rotation and
scale use the selection center, world origin or custom coordinates as a
pivot. **Keep textures attached** preserves surface UVs. Each application
is one undo step; point entities move and rotate with the group.

Choose **Faces · materials & textures** in the selection panel, then click
a surface; Ctrl+click toggles additional faces. **Apply** assigns a material
to the chosen faces. **Edit face textures…** offers horizontal/vertical
offsets and scales, rotation and **Fit one texture tile to each face**.
Blank fields preserve each face's current value when selections are mixed.
**Preview textures** shows a draft; **Apply textures** records one undo step.
**Cancel**, Escape or closing the texture dialog discards the preview.
Changing the selection or map invalidates an obsolete preview.

**Browse materials…** shows thumbnail pages with folder/category groups and
map-used, recently applied and favorite materials. Categories are inferred
from material paths; favorites and recent applications last for the session.
Selecting a thumbnail only chooses a candidate. **Use material** accepts it;
**Cancel** leaves the previous choice unchanged. Texture fitting uses actual
material dimensions and rejects materials whose texture cannot be loaded.

| Grouped material browser | Face texture controls |
| --- | --- |
| ![Material thumbnails, categories and favorites](docs/images/material-browser.png) | ![Face material, offset, scale, rotation and preview controls](docs/images/face-textures.png) |

## Entities

Click **Entity…** in the selection panel to search point classes and place
an entity at the shown coordinates. The position starts 128 units in front
of the perspective camera, or at the planar view's center and workplane
elevation, using the shared snapping settings. MCP uses the same position
when `origin` is omitted. New entities receive inherited FGD defaults. Select the entity
to edit its properties, choose enumerated values and toggle spawn flags.
Invalid typed values are rejected without changing the map.

Common settings appear first, with descriptions behind **?** buttons.
**Advanced properties** reveals engine keys, types, stored values and
custom fields. Light colors have a swatch and picker; brightness has a
slider and an exact numeric input. Position and rotation have separate
component inputs. Selected spotlights show inner and outer cone guides.

Point lights use a bundled [CC0 bulb model by Josh Dean, Poly Haven](https://polyhaven.com/a/lightbulb_led),
with a colored globe, textured body and threaded metal base. These editor
markers appear in both the viewport and MCP renders; they add no game
models or geometry to the exported map. Asset provenance and regeneration
details are in [SOURCE.txt](crates/render/assets/lightbulb/SOURCE.txt).

Select a brush and click **Browse materials…** to search game materials
and preview the selected texture. **Use material** chooses the material;
**Apply** paints the selected brushes. Entity material fields apply the
chosen value to that property.

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
