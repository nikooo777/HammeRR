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
cargo build --release -p editor -p editor-core -p compiler -p native-runtime
./target/release/editor path/to/map.vmf
```

Leave out the filename to start a new map. HammeRR finds CS:S through Steam and loads its materials and entity definitions automatically. See [Getting started](docs/getting-started.md) for launch options and custom content.

## Built for AI agents

While the editor runs, it serves MCP at `http://127.0.0.1:7377/mcp`. Point your agent at it:

```json
{ "mcpServers": { "hammerr": { "url": "http://127.0.0.1:7377/mcp" } } }
```

The agent works in your session: its edits appear live and land in your undo history, and it can set up a preview for you to approve. It can render any view to check its own work, then compile the map and launch the game. Run `hammerr-headless` for a private session without a window.

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
| Move, rotate, scale or snap selections to the grid, with shared previews and texture controls. | Reflect across any axis or slant one coordinate by another, with textures attached. |

| Polygon brushes |
| --- |
| ![A concave polygon extruded into editable brushes](docs/images/polygon-brushes.png) |
| Draw convex or concave outlines and extrude them into editable map geometry. |

| Merge brushes |
| --- |
| ![Brush sources and a convex merge preview](docs/images/brush-merging.png) |
| Join adjacent or overlapping brushes into one convex brush while keeping outer face settings. |

| Surface placement | Corner snapping |
| --- | --- |
| ![A model aligned against a wall with a normal offset](docs/images/surface-placement.png) | ![Reviewed source and target brush corners](docs/images/vertex-snap.png) |
| Drop selections to floors or ceilings, or align them against picked brush, terrain and model surfaces. | Move a brush owner so its chosen corner meets a corner on a picked face. |

| Repeated placement |
| --- |
| ![Repeated placement array preview and controls](docs/images/repeat-array.png) |
| Repeat selections in world or source-local axes, with brush scale factors and one Undo step. |

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
| Divide a face, then raise, extrude outward, cut inward or restyle each piece. | Faces with three to eight corners, with per-piece materials and texture placement. Keep the plan in the map for later. |

[Vertices, edges and faces →](docs/components.md) · [Flat partitions →](docs/partitions.md)

### Materials and textures

| Material browser | Texture painting |
| --- | --- |
| ![Material thumbnails, categories and favorites](docs/images/material-browser.png) | ![A sampled wood floor painted across stair treads](docs/images/texture-strokes.png) |
| Thumbnails, search, categories, materials used in the map, recent materials and favorites. | Sample a face, then brush its material or its whole projection across other faces. |

| Material reload | UV vectors and skew |
| --- | --- |
| ![Material reload controls and asset diagnostics](docs/images/material-reload.png) | ![U and V texture vector controls](docs/images/uv-vectors.png) |
| Refresh changed textures and material choices without reopening the map. | Set texture directions and skew directly, with mixed-value inputs and reviewed previews. |

| Texture wrapping | One texture across many faces |
| --- | --- |
| ![A texture continued around a faceted column](docs/images/connected-wrapping-viewport.png) | ![One texture fitted across adjoining faces](docs/images/grouped-textures-viewport.png) |
| Continue a texture seamlessly across shared edges and around columns. | Fit a texture across a group of faces, or project it from the current view. |

| Material replacement | Face lighting |
| --- | --- |
| ![Material replacement with match counts and preview](docs/images/material-replacement.png) | ![Lightmap density on brushes with different scales](docs/images/face-lighting-density.png) |
| Find and replace a material across the map or the selection, with a preview. | Set lightmap scales and smoothing groups, and inspect them in dedicated views. |

[Materials and textures →](docs/textures.md)

| Material source |
| --- |
| ![Mounted VMT source, Patch includes and resolved declarations](docs/images/material-source.png) |
| Inspect a material's original text, mounted origin and resolved Patch declarations. |

[Material source →](docs/material-source.md)

### Select and view

| Paint selection | Volume selection |
| --- | --- |
| ![Painting a selection with a circular brush](docs/images/paint-selection.png) | ![A blue volume box and its matching brush](docs/images/volume-preview.png) |
| Brush over the objects or faces you want. | Select everything inside exact XYZ bounds. |

| Overlapping objects |
| --- |
| ![Rear brush selected among three overlapping objects](docs/images/object-hit-cycling.png) |
| Alt+click to cycle through objects and groups in depth order. |

| Viewport modes | Saved camera views |
| --- | --- |
| ![Wireframe view with a light helper](docs/images/wireframe-view.png) | ![Named camera views](docs/images/saved-views.png) |
| Choose a viewport mode and show or hide selection outlines in every projection. | Name camera positions and jump back to them. They are saved in the map. |

| Grid defaults |
| --- |
| ![Saved grid size, snapping and visibility controls](docs/images/grid-defaults.png) |
| Save your startup grid while keeping temporary grid changes independent. |

| Working region | Fog preview |
| --- | --- |
| ![Working-region bounds and visibility controls](docs/images/working-region.png) | ![Authored fog controller and preview settings](docs/images/fog-preview.png) |
| Focus the editor on whole objects touching a temporary box, with shared visibility reasons. | Choose an authored controller to preview static linear fog in perspective Textured views. |

| 2D sky preview |
| --- |
| ![The selected sky rendered through sky surfaces](docs/images/sky-preview.png) |
| Preview the selected mounted sky through sky surfaces in perspective Textured views. |

| Measurement guides | Rope preview |
| --- | --- |
| ![Exact named rulers and coordinate controls](docs/images/measurements.png) | ![Authored rope spans with a static sag preview](docs/images/rope-preview.png) |
| Save named rulers with exact endpoints, distances and XYZ components. | Inspect linked rope spans with authored slack, width and subdivision. |

[Editor basics →](docs/basics.md) · [Selecting →](docs/selection.md)

### Entities and logic

| Place entities on surfaces |
| --- |
| ![Reviewing a new point entity on a picked surface](docs/images/entity-placement-controls.png) |
| Pick a surface and review snapped contact, bounds and orientation before creating an entity. |

| Particle effects |
| --- |
| ![Particle definitions and effect-name choices](docs/images/particle-browser.png) |
| Search active particle systems, inspect their definitions and choose effects for entities. |

| Connections | Safe renaming |
| --- | --- |
| ![Connection form with suggestions](docs/images/entity-connections.png) | ![Rename preview listing every reference to update](docs/images/entity-rename.png) |
| Wire outputs to inputs, with suggestions from the map and the game's entity definitions. | Rename an entity and update every reference to it in one step. |

| Linked paths |
| --- |
| ![Reviewing an ordered linked path](docs/images/linked-path.png) |
| Create named path chains or reorder and move existing nodes while retaining their identities. |

[Linked paths →](docs/path-authoring.md)

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

| Cut, copy and paste |
| --- |
| ![Paste placement and link handling](docs/images/clipboard-paste.png) |
| Transfer brushes and entities between maps with previewed placement, groups, notes and checked links. |

| Prefab library |
| --- |
| ![Reusable assembly rotation, scale and source-pivot controls](docs/images/prefab-transformed-placement.png) |
| Save and rename assemblies, then place rotated or brush-scaled copies with checked links. |

| Transaction history |
| --- |
| ![Inspecting transaction authors, changes and retained states](docs/images/history.png) |
| Inspect human and agent edits, then return to a retained state through Undo and Redo. |

| Layers |
| --- |
| ![Nested layers with color and membership controls](docs/images/layers.png) |
| Organize objects into colored layers, select their members and control map visibility. |

| Object groups |
| --- |
| ![Nested object groups and membership controls](docs/images/object-groups.png) |
| Organize brushes and entities into nested groups, select them together and edit membership with Undo. |

| Recent maps |
| --- |
| ![Recent map paths and retained build settings](docs/images/recent-maps.png) |
| Reopen recent maps with retained compatible build settings. |

[Organizing and checking maps →](docs/map-tools.md)

### Compile and play

| Native compiler | Play in CS:S |
| --- | --- |
| ![Build settings, statistics and diagnostics](docs/images/build-status.png) | ![Soft sun shadows in Counter-Strike: Source](docs/images/soft-sun.png) |
| Compile playable maps with baked lighting, CS:S engine physics, water, props and terrain. | **Run Map** compiles and launches the game. Builds run in the background and never overwrite good output with a failed one. |

| Named build recipes |
| --- |
| ![Named build recipes and their compiler settings](docs/images/build-recipes.png) |
| Recall Fast preview, Final lighting or named custom combinations of native compiler settings. |

| Compile regions |
| --- |
| ![Orange compile-region bounds inside the full source map](docs/images/compile-region-preview.png) |
| Build a sealed map region with terrain, water, overlays, visibility helpers and shared previews. |

[Compile and play →](docs/compile-and-play.md)

| Areaportals |
| --- |
| ![Two rooms connected by an open areaportal](docs/images/areaportal-open.png) |
| Control visibility between areas through named portals and door connections. |

| Distance-fading windows |
| --- |
| ![Opaque distance-fading window](docs/images/areaportal-window-closed.png) |
| Fade rendered windows to opaque with distance and hide the area beyond. |

| Terrain authoring |
| --- |
| ![Painted grass and dirt on terrain](docs/images/terrain-blending.png) |
| Create, sculpt and blend terrain, sew borders or corners, and preview edits with Undo. |

| Source models |
| --- |
| ![Model browser and bodygroup preview](docs/images/bodygroup-model-browser.png) |
| Browse models, skins and named bodygroups with reference poses and dynamic-model tints. |

| Model detail props |
| --- |
| ![Decorative detail models in Counter-Strike: Source](docs/images/detail-props.png) |
| Place decorative models and compile them into native detail records without collision. |

[Model detail props →](docs/model-detail-props.md)

| Material detail scattering |
| --- |
| ![Material-generated foliage on displaced terrain in Counter-Strike: Source](docs/images/terrain-detail-scattering.png) |
| Generate decorative models, billboards and fixed cross/tri foliage from brush and terrain materials, with blend alpha and baked lighting. |

[Material detail scattering →](docs/material-detail-scattering.md)

| Sound exploration |
| --- |
| ![Sound browser and audition controls](docs/images/sound-browser.png) |
| Find sound files and script names, audition variants and assign them to entities. |

| Model attachment points |
| --- |
| ![Named attachment inspection](docs/images/model-attachments.png) |
| Inspect named reference-pose attachment points, show their axes and frame them. |

| Terrain triangles |
| --- |
| ![Terrain triangle selection and classifications](docs/images/terrain-triangle-picking.png) |
| Pick or box-select terrain triangles, then classify, sculpt or blend their vertices. |

| Terrain vertices |
| --- |
| ![Terrain vertex move handles](docs/images/terrain-move-handles.png) |
| Pick and drag terrain vertices with shared axis and plane handles, or enter exact world offsets. |

| Terrain subdivision |
| --- |
| ![Rounded connected terrain with whole-to-half borders](docs/images/subdivided-terrain-half-edge.png) |
| Round compatible full and half-edge cages into editable terrain with shared seam samples. |

| Terrain brush strokes |
| --- |
| ![Live terrain sculpting with a brush-radius guide](docs/images/terrain-stroke-guide.png) |
| Drag to sculpt or paint terrain, review the draft, then keep the stroke in one Undo step. |

| Connected terrain lighting |
| --- |
| ![Continuous light across rotated terrain patches](docs/images/terrain-seams.png) |
| Matching full edges share smooth lighting normals in the viewport and compiled map. |

| Visibility quality |
| --- |
| ![Detailed portal clipping selected in Build settings](docs/images/build-settings.png) |
| Build conservative or tighter visibility with parallel portal processing. |

| Brush smoothing |
| --- |
| ![Authored smoothing on a brush cube in Counter-Strike: Source](docs/images/compiled-brush-smoothing.png) |
| Blend selected brush faces and edge junctions with shared smoothing groups. |

| Angle smoothing |
| --- |
| ![Automatic brush smoothing on a cylinder in Counter-Strike: Source](docs/images/compiled-angle-smoothing.png) |
| Smooth unassigned brush faces with a shared build angle. |

| Model smoothing |
| --- |
| ![Smooth baked lighting on a rotating brush model](docs/images/compiled-model-smoothing.png) |
| Smooth each brush model with face groups or a build angle. |

| Normal-map lighting |
| --- |
| ![Directional shading on a normal-mapped floor](docs/images/compiled-normal-maps.png) |
| Bake directional lightmaps for normal-mapped world, terrain and brush-model surfaces. |

| Brush entity lightmaps |
| --- |
| ![Reflected world light on a brush entity beneath a shelf](docs/images/compiled-model-bounce.png) |
| Carry direct and reflected world light with doors and moving brushes. |

| Switchable lights |
| --- |
| ![Named lights switched on in Counter-Strike: Source](docs/images/light-styles-on.png) |
| Bake direct and reflected light that switches, flickers or follows custom patterns. |

| Bounced light |
| --- |
| ![Four diffuse bounces beneath a shelf in Counter-Strike: Source](docs/images/four-bounce-on.png) |
| Add up to four diffuse light bounces from world surfaces and terrain. |

| LDR and HDR lighting |
| --- |
| ![Independent LDR and HDR build controls](docs/images/build-hdr-controls.png) |
| Bake separate light colors, styles, bounces and ambient probes for LDR and HDR. |

| Reflection face bindings | Generated reflections |
| --- | --- |
| ![Reviewing reflection sample assignments](docs/images/cubemap-faces.png) | ![Local reflections on a compiled floor in Counter-Strike: Source](docs/images/compiled-reflections.png) |
| Preview and override which reflection sample a face uses. | Capture local LDR reflections during Compile and keep them working when you play. |

[Cubemap faces →](docs/cubemap-faces.md) · [Generated reflections →](docs/reflections.md)

| Static-prop cutout shadows |
| --- |
| ![Baked light passing through a fence model's cutout texture](docs/images/prop-shadow-cutouts.png) |
| Bake shadows through alpha-test cutouts on supported static props. |

| Hint planes |
| --- |
| ![Hint face and skip bounds in the editor](docs/images/hint-planes.png) |
| Guide structural and visibility partitions with finite hint surfaces and skip bounds. |

| Overlays |
| --- |
| ![Fitted overlay controls with material preview and source corners](docs/images/overlay-authoring.png) |
| Fit, refit or explicitly rebind markings on world/detail faces while retaining settings and entity links. |

[Overlays →](docs/overlays.md)

| Model ambient lighting |
| --- |
| ![Sky and surface tint on a weapon model in Counter-Strike: Source](docs/images/model-ambient.png) |
| Models pick up reflected light from nearby world surfaces and visible sky. |

## Status

The editor covers most everyday brush work. Notable gaps:

- **Terrain**: creation, resolution edits, live sculpt/blend strokes, vertex handles and full/half-edge subdivision work; other fractional edge tools remain planned.
- **Models**: static meshes, reference poses, skins, bodygroups and visual browsing work; animation and scaling remain planned.
- **Compiler**: Fast and Detailed visibility, independent LDR/HDR lighting, diffuse bounces and LDR reflections work; exposure, HDR cubemap capture and prop lighting remain planned.

The full picture is in [Status and roadmap](docs/status.md).

## Documentation

The [manual](docs/README.md) covers every feature, step by step:

[Getting started](docs/getting-started.md) · [Editor basics](docs/basics.md) · [Selecting](docs/selection.md) · [Brushes and transforms](docs/brushes.md) · [Hollow, carve and clip](docs/brush-tools.md) · [Vertices, edges and faces](docs/components.md) · [Flat partitions](docs/partitions.md) · [Materials and textures](docs/textures.md) · [Entities and logic](docs/entities.md) · [Models](docs/models.md) · [Organizing and checking maps](docs/map-tools.md) · [Compile and play](docs/compile-and-play.md) · [AI agents and MCP](docs/mcp.md) · [Development](docs/development.md)

## Credits

- The point-light helper uses a [CC0 light bulb model by Josh Dean / Poly Haven](https://polyhaven.com/a/lightbulb_led) ([provenance](crates/render/assets/lightbulb/SOURCE.txt)).
- Counter-Strike: Source and its assets are not included.
