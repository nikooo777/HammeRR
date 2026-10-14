# HammeRR

**A modern, AI-first map editor for Counter-Strike: Source, native on Linux.**

HammeRR opens and saves Hammer's VMF files, renders your map with the game's own materials, and compiles it into a playable BSP with its own compiler. Every tool is also available to AI agents over MCP, and agents work in the same live session you're looking at.

![HammeRR editing a courtyard built by an AI agent](docs/images/editor.png)

> [!NOTE]
> HammeRR is in active development. The editor is ready for building, texturing and wiring up brush maps; the compiler is experimental. See [Status](docs/status.md).

**[Quick start](#quick-start) · [AI agents](#built-for-ai-agents) · [Feature tour](#feature-tour) · [Manual](docs/README.md) · [Status](docs/status.md)**

## Why HammeRR

- **Native on Linux.** Written in Rust and rendered with Vulkan. No Wine and no Windows compile tools.
- **AI-first.** Every feature has an MCP tool. Agents see what you see, edit what you edit and share your undo history.
- **Hammer-compatible.** Reads and writes VMF, retaining imported properties and extra blocks.
- **Preview before you commit.** Tools show their result before changing anything. Apply is one undo step; Cancel leaves the map untouched.
- **Build and play in one click.** A native BSP compiler with lighting, physics, water, props and terrain, and a direct launch into CS:S.

## Quick start

You need Linux, Rust 1.92 or newer, a Vulkan graphics driver, and Counter-Strike: Source installed through Steam.

```sh
cargo build --release -p editor -p compiler
./target/release/editor path/to/map.vmf
```

Leave out the filename to start a new map. HammeRR finds CS:S through Steam and loads its materials and entity definitions automatically. See [Getting started](docs/getting-started.md) for launch options and custom content.

## Built for AI agents

While the editor runs, it serves MCP at `http://127.0.0.1:7377/mcp`. Point your agent at it:

```json
{ "mcpServers": { "hammerr": { "url": "http://127.0.0.1:7377/mcp" } } }
```

The agent works in your session: its edits appear live and land in your undo history, and it can set up a preview for you to approve. It can render any view to check its own work, then compile the map and launch the game. To give an agent a private session without a window, run the editor with `--headless`.

| Built by an agent | Checked by the agent |
| --- | --- |
| ![Agent render of a courtyard with walls, stairs, crates and lights](docs/images/agent-render.png) | ![Agent close-up of stacked crates with one face selected](docs/images/agent-block.png) |

Setup details, conventions and the full tool list are in [AI agents and MCP](docs/mcp.md).

## Feature tour

### Build geometry

| Brush shapes | Hollow |
| --- | --- |
| ![Wedge, cylinder, cone, sphere and arch](docs/images/brush-primitives.png) | ![A hollowed room with an open roof and front](docs/images/hollow-open-shell.png) |
| Wedges, cylinders, cones, spheres and arches from exact dimensions. | Turn solids into rooms, with walls inside or outside, and any faces left open. |

| Carve | Clip |
| --- | --- |
| ![Two cutter shapes leave a stepped cavity](docs/images/multiple-cutter-cavities.png) | ![A three-point cut through a brush](docs/images/three-point-cut.png) |
| Subtract one or more shapes from other brushes. | Cut by drawing a line, typing a plane or picking three points, then fine-tune with handles. |

| Transforms and texture lock | Mirror and shear |
| --- | --- |
| ![A resized brush with its texture stretched](docs/images/object-texture-stretch.png) | ![A sheared brush](docs/images/brush-shear-after.png) |
| Move, rotate and scale with handles or exact values. Textures follow, stretch or stay fixed. | Reflect across any axis or slant one coordinate by another, with textures attached. |

[Brushes and transforms →](docs/brushes.md) · [Hollow, carve and clip →](docs/brush-tools.md)

### Reshape vertices, edges and faces

| Move, scale and rotate | Weld |
| --- | --- |
| ![A rotated top face makes a sloped roof](docs/images/component-rotate-preview.png) | ![Four corners welded into a pyramid](docs/images/component-weld-preview.png) |
| Edit faces, edges and corners directly. A rotated top face becomes a sloped roof. | Merge corners at their average, an existing corner or an exact position. |

| Split and raise | Roof ridge |
| --- | --- |
| ![A face split into four triangles around a raised corner](docs/images/component-face-split.png) | ![Two roof slopes on a box](docs/images/component-roof-ridge.png) |
| Add a corner in the middle of a face or edge and push it outward. | Raise a four-sided face into two roof slopes. |

| Flat partitions | Any convex face |
| --- | --- |
| ![Two pieces of a face extruded to different heights](docs/images/partition-extrusion-preview.png) | ![Two styled prisms above an octagonal face](docs/images/polygon-partition-preview.png) |
| Divide a face, then raise, extrude or restyle each piece. | Faces with three to eight corners, with per-piece materials and texture placement. Keep the plan in the map for later. |

[Vertices, edges and faces →](docs/components.md) · [Flat partitions →](docs/partitions.md)

### Materials and textures

| Material browser | Texture painting |
| --- | --- |
| ![Material thumbnails, categories and favorites](docs/images/material-browser.png) | ![A sampled wood floor painted across stair treads](docs/images/texture-strokes.png) |
| Thumbnails, search, categories, materials used in the map, recent materials and favorites. | Sample a face, then brush its material or its whole projection across other faces. |

| Texture wrapping | One texture across many faces |
| --- | --- |
| ![A texture continued around a faceted column](docs/images/connected-wrapping-viewport.png) | ![One texture fitted across adjoining faces](docs/images/grouped-textures-viewport.png) |
| Continue a texture seamlessly across shared edges and around columns. | Fit a texture across a group of faces, or project it from the current view. |

| Material replacement | Face lighting |
| --- | --- |
| ![Material replacement with match counts and preview](docs/images/material-replacement.png) | ![Lightmap density on brushes with different scales](docs/images/face-lighting-density.png) |
| Find and replace a material across the map or the selection, with a preview. | Set lightmap scales and smoothing groups, and inspect them in dedicated views. |

[Materials and textures →](docs/textures.md)

### Select and view

| Paint selection | Volume selection |
| --- | --- |
| ![Painting a selection with a circular brush](docs/images/paint-selection.png) | ![A blue volume box and its matching brush](docs/images/volume-preview.png) |
| Brush over the objects or faces you want. | Select everything inside exact XYZ bounds. |

| Viewport modes | Saved camera views |
| --- | --- |
| ![Wireframe view with a light helper](docs/images/wireframe-view.png) | ![Named camera views](docs/images/saved-views.png) |
| Textured, flat, wireframe, lightmap density and smoothing groups, in every projection. | Name camera positions and jump back to them. They are saved in the map. |

[Editor basics →](docs/basics.md) · [Selecting →](docs/selection.md)

### Entities and logic

| Connections | Safe renaming |
| --- | --- |
| ![Connection form with suggestions](docs/images/entity-connections.png) | ![Rename preview listing every reference to update](docs/images/entity-rename.png) |
| Wire outputs to inputs, with suggestions from the map and the game's entity definitions. | Rename an entity and update every reference to it in one step. |

| Edit many entities |
| --- |
| ![Shared property editing with mixed values](docs/images/batch-properties.png) |
| Change shared properties, flags and colors across a selection. |

Also: a typed inspector with color pickers, brightness sliders and spawn flags; light, spotlight and sun helpers; brush entities; and world settings.

[Entities and logic →](docs/entities.md)

### Organize and check

| Map checks | Design notes |
| --- | --- |
| ![Map issues with an explained repair](docs/images/map-checks.png) | ![Design notes with a region](docs/images/design-context.png) |
| Find problems, jump to them, and apply reviewed repairs. | Keep briefs, object notes and regions in the map. Agents read and write them too. |

| Object report | Autosave and recovery |
| --- | --- |
| ![Object search with visibility actions](docs/images/object-report.png) | ![Recovery snapshots and settings](docs/images/recovery.png) |
| Find, select, frame, hide and isolate objects. | Periodic snapshots, manual checkpoints and save backups, opened as safe copies. |

| Map statistics | Leak tracing |
| --- | --- |
| ![Map totals and material usage](docs/images/map-statistics.png) | ![A red leak path through the map](docs/images/pointfile-viewport.png) |
| Counts and breakdowns of brushes, entities, materials, models and terrain. | Follow a leak's pointfile through the map, point by point. |

[Organizing and checking maps →](docs/map-tools.md)

### Compile and play

| Native compiler | Play in CS:S |
| --- | --- |
| ![Build settings, statistics and diagnostics](docs/images/build-status.png) | ![Soft sun shadows in Counter-Strike: Source](docs/images/soft-sun.png) |
| Baked light from point lights, spotlights, sun and sky, with soft sun shadows, plus CS:S engine physics, water, cubemaps, static props and terrain. | **Run Map** compiles and launches the game. Builds run in the background and never overwrite good output with a failed one. |

[Compile and play →](docs/compile-and-play.md)

| Areaportals |
| --- |
| ![Two rooms connected by an open areaportal](docs/images/areaportal-open.png) |
| Control visibility between areas through named portals and door connections. |

| Distance-fading windows |
| --- |
| ![Opaque distance-fading window](docs/images/areaportal-window-closed.png) |
| Fade rendered windows to opaque with distance and hide the area beyond. |

| Connected terrain lighting |
| --- |
| ![Continuous light across rotated terrain patches](docs/images/terrain-seams.png) |
| Matching full edges share smooth lighting normals in the viewport and compiled map. |

| Visibility quality |
| --- |
| ![Detailed portal clipping selected in Build settings](docs/images/build-settings.png) |
| Choose a quick conservative build or parallel portal clipping for tighter visibility. |

| Brush smoothing |
| --- |
| ![Authored smoothing on a brush cube in Counter-Strike: Source](docs/images/compiled-brush-smoothing.png) |
| Blend baked lighting across selected brush faces with shared smoothing groups. |

| Bounced light |
| --- |
| ![Reflected light beneath a shelf in Counter-Strike: Source](docs/images/compiled-bounce.png) |
| Add one diffuse light bounce from world surfaces and terrain. |

| Hint planes |
| --- |
| ![Hint face and skip bounds in the editor](docs/images/hint-planes.png) |
| Guide visibility partitions with invisible hint surfaces and skip bounds. |

| Imported overlays |
| --- |
| ![Graffiti projected onto the floor in Counter-Strike: Source](docs/images/overlays.png) |
| Compile surface markings with material, render order, distance fade and preserved face bindings. |

[Overlays →](docs/overlays.md)

| Model ambient lighting |
| --- |
| ![Sky and surface tint on a weapon model in Counter-Strike: Source](docs/images/model-ambient.png) |
| Models pick up reflected light from nearby world surfaces and visible sky. |

## Status

The editor covers most everyday brush work. Notable gaps:

- **Terrain**: imported terrain displays and compiles, but can't be created or edited yet.
- **Models**: static props compile but aren't drawn in the viewport yet.
- **Compiler**: Fast and Detailed visibility are available; lighting covers direct light, sky ambient and one optional diffuse bounce; HDR remains unfinished.

The full picture is in [Status and roadmap](docs/status.md).

## Documentation

The [manual](docs/README.md) covers every feature, step by step:

[Getting started](docs/getting-started.md) · [Editor basics](docs/basics.md) · [Selecting](docs/selection.md) · [Brushes and transforms](docs/brushes.md) · [Hollow, carve and clip](docs/brush-tools.md) · [Vertices, edges and faces](docs/components.md) · [Flat partitions](docs/partitions.md) · [Materials and textures](docs/textures.md) · [Entities and logic](docs/entities.md) · [Organizing and checking maps](docs/map-tools.md) · [Compile and play](docs/compile-and-play.md) · [AI agents and MCP](docs/mcp.md) · [Development](docs/development.md)

## Credits

- The point-light helper uses a [CC0 light bulb model by Josh Dean / Poly Haven](https://polyhaven.com/a/lightbulb_led) ([provenance](crates/render/assets/lightbulb/SOURCE.txt)).
- Counter-Strike: Source and its assets are not included.
