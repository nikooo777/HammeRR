# HammeRR

**A modern, AI-first Source map editor for Linux.**

HammeRR is written in Rust for Counter-Strike: Source. It reads and writes
Hammer's VMF format, renders the game's own materials, and gives humans and
AI agents the same editing operations through a shared session and MCP.

The aim is a new editor with integrated tooling and a modern interface.
Development is active; see [current limitations](#status) before compiling
maps for play.

![Current editor showing an MCP-built courtyard and the light inspector](docs/images/editor.png)

*A courtyard built through MCP, with textured brushes, detailed light helpers,
and the shared entity inspector and activity panel.*

[Features](#features) · [Quick start](#quick-start) · [Editing](#editing) ·
[MCP](#mcp) · [Compile and play](#compile-and-play) · [Status](#status)

## Features

| Area | Available now |
| --- | --- |
| Maps and content | VMF editing and saving; byte-for-byte round trips for unmodified files; game materials from Steam libraries, search paths and VPK archives. |
| Viewport and brushes | Textured perspective and planar views, grids, fractional snapping, brush drawing, move/rotate/scale handles, exact transforms, and texture locking. |
| Selection | Object and face marquee selection, intersecting or fully contained matches, and replace/add/remove/toggle rules. |
| Brush clipping | Planar cut gestures or exact plane inputs, keep either side or both, cap materials, cancellable previews and one-step undo. |
| Face materials | Individual face selection, UV offset/scale/rotation, texture fitting, cancellable previews, and a grouped material browser with thumbnails. |
| Entities | FGD class search, typed properties, choices, spawn flags, lights and player spawns, plus brush entity conversion. |
| Entity logic | Connection creation, editing and deletion; target and input suggestions; incoming links; delay and firing rules; cancellable drafts and undo. |
| Humans and agents | Shared map, selection, camera, previews and undo history. MCP edits appear live in the editor. |
| Build and play | Native BSP compilation, leak detection and viewport paths, CS:S launch controls, and game logs. |

## Quick start

Requires **Rust 1.92+** and a **Vulkan-capable GPU**. Install Counter-Strike:
Source to use its materials; HammeRR discovers the game through Steam.

```sh
cargo build --release -p editor
./target/release/editor path/to/map.vmf
```

Omit the filename to start an empty map. The editor also discovers the
installed CS:S SDK entity definitions.

<details>
<summary>Launch options</summary>

| Option | Purpose |
| --- | --- |
| `--game <gameinfo.txt>` | Use a specific game content installation. |
| `--fgd <file.fgd>` | Load another entity definition file and its includes. |
| `--mcp-port <port>` | Change the HTTP MCP port; the default is `7377`. |
| `--no-mcp` | Run the GUI without the MCP server. |
| `--headless` | Serve MCP over stdio without opening a window. |

```sh
cargo test --workspace
```

</details>

## Editing

### Navigation and selection

| Action | Control |
| --- | --- |
| Fly in perspective | Hold right mouse; move with WASD and Q/E. |
| Change projection | Choose **Perspective**, **Top · XY**, **Front · XZ** or **Side · YZ**. |
| Pan / zoom in planar views | Right-drag / mouse wheel; zoom follows the cursor. |
| Frame selection or map | **Frame · F**; frames the map when nothing is selected. |
| Select / toggle selection | Click / Ctrl+click. |
| Select an area | Left-drag away from a handle; choose **Intersects** or **Fully inside** and a selection rule. Ctrl+drag toggles. |
| Transform with handles | Choose **Move handles**, **Rotate handles** or **Scale handles**; drag a handle and release to apply. Escape cancels. |
| Nudge selection | Arrows and PgUp/PgDn; Shift uses at most one unit. |
| Duplicate / delete | Ctrl+D / Del. |
| Undo / redo | Ctrl+Z / Ctrl+Y. |

**Grid** accepts spacing from 0.125 to 4096 units. **Snap** and **Show grid**
are independent. In planar views, arrows move within the displayed plane;
PgUp/PgDn move along its depth axis.

### Brushes and transforms

In a planar view, click **Draw brush**, set **Start Z/Y/X** and **Depth Z/Y/X**,
then drag a footprint. The preview shows snapped dimensions; releasing
places the brush in one undo step. Escape cancels, and **Done** returns to
selection. **Add brush** places a default block in perspective.

Choose a handle tool above the viewport for movement arrows and plane handles,
rotation rings, or axis and uniform scale handles. **World axes** and **Local
axes** control the frame; local axes follow the first selected object's brush
orientation or entity angles. **Snap** uses the grid for relative movement and
scaled dimensions, and **Angle snap** sets rotation increments. Releasing a
drag applies one undo step; Escape or **Cancel** discards the draft.

Enable **Transform selection** in the inspector for exact XYZ inputs. Choose
the selection center, world origin or custom coordinates as the pivot. These
settings also control handles. **Keep textures attached** preserves surface
UVs. Numeric transforms remain exact, independent of grid snapping.

Area selection works in perspective, Top, Front and Side, for objects or
faces. It includes geometry through the view's depth and skips hidden brushes
and hidden tool materials. A brush entity is selected as a whole. **Fully
inside** requires all its visible brushes to fit; **Intersects** accepts any
intersection. Selection previews highlight candidates without changing the
map or adding undo history.

Imported terrain uses its supporting brush faces for area queries. Volume
and paint selection are still pending.

### Brush clipping

Select brushes or a brush entity, then click **Clip selected brushes**.
Draw a cut in Top, Front or Side, or enter a plane normal and distance.
Choose **Arrow side**, **Opposite side** or **Both sides**, and optionally
browse for the cut-face material. **Apply cut** commits the preview in one
undo step; **Cancel cut** or Escape discards it.

Existing faces keep their texture projections. Brushes the plane does not
cross stay unchanged. Intersected displacements cannot be clipped yet.

### Face textures and materials

Choose **Faces · materials & textures**, then select surfaces with click
and Ctrl+click. **Apply** assigns the chosen material to those faces.
**Edit face textures…** controls offsets, scales, rotation and one-tile
fitting. **Preview textures** shows a draft; **Apply textures** commits it
in one undo step. **Cancel**, Escape or closing the dialog discards it.

**Browse materials…** offers thumbnail pages, a larger preview, search,
folder/category groups, and map-used, recent and favorite materials.
Choosing a thumbnail selects a candidate; **Use material** accepts it.
**Cancel** keeps the previous choice.

| Material browser | Face texture controls |
| --- | --- |
| ![Material thumbnails, categories and favorites](docs/images/material-browser.png) | ![Face material, offset, scale, rotation and preview controls](docs/images/face-textures.png) |

Blank texture fields preserve existing values when selections are mixed.
Fitting uses the material's real dimensions. Categories are inferred from
material paths; favorites and recent applications last for the session.

### Entities and lights

Click **Entity…** to search point classes and place lights, spawns or other
entities. The initial position uses the camera or planar workplane and
shared snapping settings. Select an entity to edit its typed properties,
choose enumerated values and toggle spawn flags. Invalid values leave the
map unchanged.

The inspector puts common settings first: position and rotation components,
color pickers, brightness sliders, numeric inputs and material previews.
**?** shows descriptions; **Advanced properties** exposes engine keys and
custom fields. Entity, color, material and texture dialogs all have Cancel
buttons and Escape dismissal.

Point lights use a bundled [CC0 bulb model by Josh Dean / Poly Haven](https://polyhaven.com/a/lightbulb_led).
Spotlights and sunlight have direction helpers; selected spotlights show
cone guides. These are editor markers, visible in both the viewport and MCP
renders. They add no models or geometry to the exported map. See
[asset provenance](crates/render/assets/lightbulb/SOURCE.txt).

Select world brushes and use **Tie to entity…** to create a brush entity.
**Move to world** converts it back. Both operations share undo history with
agent edits. **Load FGD…** in the class picker loads other definitions;
custom classes and unknown keys remain editable.

### Entity connections

Select a point or brush entity and open **Connections…**. **Outgoing** lists
its outputs; **Incoming** shows map connections targeting it and lets you
edit their source. Click **Add connection** or **Edit connection**, then
choose an output, target and input. Suggestions use the map's named entities
and inherited FGD definitions; custom names and I/O are also supported.

Set the parameter, delay in seconds, and **Unlimited**, **Once** or a
specific firing count. **Apply connection** creates one undo step. **Cancel**,
Escape or closing the dialog discards unapplied fields. Deletion is undoable.
MCP can inspect and change the same draft while the dialog is open.

Missing targets, undeclared I/O and parameter type mismatches show warnings.
Invalid encoding, negative delays and invalid counts prevent Apply. Map edits
invalidate an open draft, so it cannot overwrite a shifted connection index.
Incoming links resolve named targets, prefix wildcards and `!self`; other
procedural targets resolve in game. Renaming an entity currently leaves its
existing references unchanged.

New connections use CS:S's comma-separated format, so their fields cannot
contain commas, quotes or control characters. Imported escape-separated
connections retain their delimiter; other imported records stay untouched
until explicitly edited or deleted.

![Entity connection form with readable fields, suggestions and Cancel](docs/images/entity-connections.png)

## MCP

The GUI serves MCP at **`http://127.0.0.1:7377/mcp`**. Connect an agent with:

```json
{
  "mcpServers": {
    "hammerr": {
      "url": "http://127.0.0.1:7377/mcp"
    }
  }
}
```

For an isolated session without a window:

```sh
./target/release/editor path/to/map.vmf --headless
```

Human controls and MCP calls use the same Session operations. Agents can
build geometry, place lights, edit individual faces, preview changes,
compile maps and launch the game. `render_view` returns a PNG from any
viewpoint; `pick_at_pixel` resolves a rendered pixel to an object and face.

For example, place a warm light:

```json
{
  "name": "create_entity",
  "arguments": {
    "classname": "light",
    "origin": [0, 0, 128],
    "properties": { "_light": "255 220 180 400" }
  }
}
```

| Courtyard built through MCP | Face selection and fitted crate textures |
| --- | --- |
| ![MCP render of textured walls, stairs, crates and light helpers](docs/images/agent-render.png) | ![MCP close-up of stacked crates with one face selected](docs/images/agent-block.png) |

<details>
<summary>Tool reference by task</summary>

| Task | Tools |
| --- | --- |
| Files and inspection | `new_map`, `open_map`, `save_map`, `get_map_info`, `find_objects`, `get_objects` |
| Selection and history | `select`, `get_selection`, `set_selection_mode`, `undo`, `redo` |
| Object edits | `move_objects`, `duplicate_objects`, `delete_objects` |
| Spatial authoring | `set_spatial_settings`, `draw_block`, `cancel_block_preview`, `create_block`, `transform_objects` |
| Transform handles and previews | `set_transform_tool`, `get_transform_handles`, `begin_transform_drag`, `update_transform_drag`, `commit_transform_preview`, `cancel_transform_preview` |
| Area selection | `set_area_selection_options`, `select_area`, `get_area_selection`, `commit_area_selection`, `cancel_area_selection` |
| Brush clipping | `set_clip_tool`, `clip_brushes`, `get_clip_preview`, `commit_clip_preview`, `cancel_clip_preview` |
| Entity authoring | `create_entity`, `set_entity_properties`, `tie_to_entity`, `move_to_world` |
| Entity connections | `get_entity_connections`, `get_connection_choices`, `begin_connection_edit`, `update_connection_draft`, `commit_connection_edit`, `cancel_connection_edit`, `delete_entity_connection` |
| Entity definitions and controls | `search_entity_classes`, `get_entity_class`, `load_entity_definitions`, `get_entity_inspector`, `set_entity_color`, `set_entity_component` |
| Face textures | `select_faces`, `get_face_selection`, `set_material`, `edit_faces`, `commit_face_preview`, `cancel_face_preview` |
| Material browsing | `search_materials`, `browse_materials`, `preview_material`, `set_material_favorite` |
| Camera and visual checks | `get_camera`, `set_camera`, `frame_selection`, `set_view`, `render_view`, `pick_at_pixel`, `raycast` |
| Compile and play | `compile_map`, `run_map`, `launch_game`, `game_log`, `stop_game` |

`edit_faces` supports material, UV offsets/scales, rotation, fitting and
non-destructive previews. Commit or cancel the shared preview from either
front end. Material browsing and inspector tools return the same catalog,
labels and control types used by the GUI.

`clip_brushes` accepts a plane normal/distance, three points, or a snapped
planar line. Use `preview: true` to inspect the shared cut before committing
or cancelling it. `get_clip_preview` reports the resulting brushes and faces;
`render_view` and picking use the same preview geometry as the viewport.

`transform_objects` accepts `space: "world"` or `"local"` and `preview: true`.
Handle tools support numeric amounts and viewport pointers, with updates
measured from the gesture's start. `get_transform_handles` returns the rendered
paths in world coordinates and pixels, and can hit-test a pixel. Shared drafts
can be inspected, applied or cancelled from the editor or MCP.

`select_area` takes pixel corners, viewport dimensions and an optional camera
override, plus `contained`/`intersecting` and selection rules. Use `preview:
true` to inspect candidates before applying. Camera overrides leave the user's
view unchanged; face mode returns stable solid/side IDs.

`begin_connection_edit` starts an addition or edits an outgoing `index`.
`update_connection_draft` accepts readable fields; `delay` and `times` are
strings so unfinished values can be shared with the UI. Inspect feedback,
then commit or cancel. Pass the draft's `token` to guard against another
operation replacing the form. Starting an edit or deleting supports optional
`generation` and `revision` guards from `get_entity_connections`.

</details>

## Compile and play

| Control | Behavior |
| --- | --- |
| **Compile** | Build the current snapshot, including unsaved edits, into a BSP beside the VMF. Unsaved maps prompt for an output filename. |
| **Run Map** | Compile and launch the snapshot in the installed Linux CS:S client with `-insecure`. |
| **Game Log** | Read console and launcher output. |
| **Stop Map** | Stop the game process started by this editor. |
| **Leak path** | Toggle the red leak path drawn through geometry after a failed compile. |

Compilation runs in the background. Maps need sealed world geometry and
at least one point entity in open space. A leak produces a `.lin` pointfile
and preserves any existing BSP. Editing hides the previous leak path until
the next compilation.

Each game run uses a uniquely named BSP copy and its own logs. The copy is
removed when the game exits; logs remain until the next run or editor
shutdown. Closing the editor stops its managed game process. Existing maps
and unrelated game processes remain untouched.

<details>
<summary>Standalone compiler and room example</summary>

```sh
cargo run --release -p compiler --bin hammerr-compile -- \
  map.vmf --game /path/to/cstrike/gameinfo.txt
```

Use `-o /path/to/map.bsp` to choose the output. To generate a sealed room
with both player spawns:

```sh
cargo run -p compiler --example room -- room.vmf
```

After compiling, copy the BSP to `cstrike/maps/` and run `map room` in the
CS:S console. The room has been compiled and loaded in Linux CS:S with a
player spawned inside and fullbright rendering enabled by the engine.

</details>

## Status

**The compiler is experimental.** It produces fullbright BSP v20 maps with
a single visibility cluster. Lights are saved in VMF and carried into BSP
entities; baked lighting is still pending.

VVIS, VRAD, physics collision, water volumes, compiled displacements,
static props, areaportals, overlays, cubemaps and origin brushes remain
unfinished. Unsupported geometry is reported in diagnostics. The BSP
reader/writer is verified against stock CS:S maps.

Editor work ahead includes safe reference renaming, more brush tools, displacement
editing, model rendering and further texture alignment tools.

## Project layout

| Crate | Responsibility |
| --- | --- |
| `source-formats` | Valve file formats and the game filesystem. |
| `map-core` | Geometry, displacements, undoable documents, edits and picking. |
| `render` | wgpu renderer, materials, camera and offscreen snapshots. |
| `editor` | Slint UI, shared Session and MCP server. |
| `compiler` | Native VMF-to-BSP compilation and the `hammerr-compile` CLI. |

This is a clean-room project: no Valve or Hammer++ code is used.
Counter-Strike: Source and its assets are not included.
