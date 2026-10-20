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

| Surface-picked pivots |
| --- |
| ![Rotation handles centered on a picked brush surface point](docs/images/surface-picked-transform-pivot-viewport.png) |
| Pick an exact brush, terrain or model surface point as the selection's transform pivot. |

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
| ![A model aligned against a wall with a normal offset](docs/images/surface-placement.png) | ![Choosing a source corner for several selected objects](docs/images/multi-owner-corner-snap.png) |
| Drop selections to floors or ceilings, or align them against picked brush, terrain and model surfaces. | Move selected objects together by matching a chosen brush corner to a visible brush, terrain or model vertex. |

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
| Browse thumbnails by category, track used/recent materials, and reload shared favorites. | Sample a face, then brush its material or its whole projection across other faces. |

| Material reload | UV vectors and skew |
| --- | --- |
| ![Material reload controls and asset diagnostics](docs/images/material-reload.png) | ![U and V texture vector controls](docs/images/uv-vectors.png) |
| Refresh changed textures and material choices without reopening the map. | Set texture directions and skew directly, with mixed-value inputs and reviewed previews. |

| Texture variation |
| --- |
| ![Seeded U/V texel shifts with a reviewed texture preview](docs/images/uv-variation.png) |
| Break up repeated textures with reproducible per-face offsets, then Apply or Cancel the preview. |

| Texture wrapping | One texture across many faces |
| --- | --- |
| ![A texture continued around a faceted column](docs/images/connected-wrapping-viewport.png) | ![One texture fitted across adjoining faces](docs/images/grouped-textures-viewport.png) |
| Continue a texture seamlessly across shared edges and around columns. | Fit a texture across a group of faces, or project it from the current view. |

| Material replacement | Face lighting |
| --- | --- |
| ![Material replacement with match counts and preview](docs/images/material-replacement.png) | ![Lightmap density on brushes with different scales](docs/images/face-lighting-density.png) |
| Find and replace a material across the map or the selection, with a preview. | Set lightmap scales and smoothing groups, and inspect them in dedicated views. |

[Materials and textures →](docs/textures.md)

| Material source | Material alpha |
| --- | --- |
| ![Mounted VMT source, Patch includes and resolved declarations](docs/images/material-source.png) | ![Selected image alpha with source identity and mip details](docs/images/material-alpha-preview.png) |
| Inspect a material's original text, mounted origin and resolved Patch declarations. | Switch between Color and opaque grayscale Alpha from the selected display image. |

[Material source →](docs/material-source.md)

| Atlas-region texturing | Normal-map preview |
| --- | --- |
| ![Atlas image with a reviewed pixel rectangle](docs/images/atlas-face-fit.png) | ![Normal-mapped brush surface under editor shading](docs/images/normal-map-preview.png) |
| Fit selected faces into a rectangular part of a material image. | Preview supported brush normal maps without compiling. |

[Atlas-region texturing →](docs/atlas-texturing.md) · [Normal-map preview →](docs/normal-maps.md)

### Select and view

| Paint selection | Volume selection |
| --- | --- |
| ![Painting a selection with a circular brush](docs/images/paint-selection.png) | ![A blue volume box and its matching brush](docs/images/volume-preview.png) |
| Brush over the objects or faces you want. | Select everything inside exact XYZ bounds. |

| Overlapping objects |
| --- |
| ![Rear brush selected among three overlapping objects](docs/images/object-hit-cycling.png) |
| Alt+click to cycle through objects and groups in depth order. |

| Selection filters | Temporary editor visibility |
| --- | --- |
| ![Inline filters for world brushes, brush entities and point entities](docs/images/selection-filters.png) | ![Temporary hiding and the visible recovery action](docs/images/editor-visibility.png) |
| Limit object picking by type while keeping objects visible and selected groups whole. | Hide distractions from the editor while keeping saved visibility and builds unchanged. |

| Viewport modes | Saved camera views |
| --- | --- |
| ![Wireframe view with a light helper](docs/images/wireframe-view.png) | ![Named camera views](docs/images/saved-views.png) |
| Choose a viewport mode and show or hide selection outlines in every projection. | Name camera positions and jump back to them. They are saved in the map. |

| Grid defaults | Preview defaults |
| --- | --- |
| ![Saved grid size, snapping and visibility controls](docs/images/grid-defaults.png) | ![Saved entity guide and viewport preview choices](docs/images/preview-defaults.png) |
| Save your startup grid while keeping temporary grid changes independent. | Save startup outlines, entity images, model guides, ropes and sky visibility. |

| Keyboard shortcuts | Navigation defaults |
| --- | --- |
| ![Filtered command bindings with retained shortcut drafts](docs/images/keyboard-shortcuts-search.png) | ![Shared fly speed and explicit startup defaults](docs/images/navigation-defaults.png) |
| Search commands and bind editing actions, projection views and selection modes to your preferred keys. | Set camera fly speed from the editor or MCP and explicitly save startup defaults. |

[Keyboard shortcuts →](docs/keyboard-shortcuts.md) · [Navigation defaults →](docs/navigation-defaults.md)

| Working region | Fog preview |
| --- | --- |
| ![Working-region bounds and visibility controls](docs/images/working-region.png) | ![Authored fog controller and preview settings](docs/images/fog-preview.png) |
| Focus the editor on whole objects touching a temporary box, with shared visibility reasons. | Choose an authored controller to preview static linear fog in perspective Textured views. |

| Authored lighting preview |
| --- |
| ![Sun and sky ambient alongside warm point and cool spot lighting](docs/images/authored-lighting-sky-ambient.png) |
| Preview unshadowed point, spot and sun lighting, with optional sky ambient, on brushes, terrain and models. |

[Authored lighting preview →](docs/lighting-preview.md)

| LDR and HDR skies | Sky asset inspection |
| --- | --- |
| ![Mounted HDR sky through a brick arch](docs/images/hdr-sky-preview.png) | ![Six-face LDR and HDR asset status with selected-face details](docs/images/sky-assets.png) |
| Preview mounted skies and adjust HDR sky exposure without changing map lighting. | Inspect six-face LDR/HDR assets, mounted sources and actionable issues. |

| 3D sky preview |
| --- |
| ![Miniature skyline beyond a foreground courtyard](docs/images/sky3d-preview.png) |
| Prepare an explicit sky-camera region and preview scaled brush scenery beyond the playable map. |

| Measurement guides | Rope preview |
| --- | --- |
| ![Ruler points attached in a rigid entity-local frame](docs/images/measurement-local-attachment.png) | ![Authored rope spans with a static sag preview](docs/images/rope-preview.png) |
| Pick exact ruler endpoints or attach them to entity origins and local frames. | Inspect linked rope spans with authored slack, width and subdivision. |

[Editor basics →](docs/basics.md) · [Selecting →](docs/selection.md) · [Preview defaults →](docs/preview-defaults.md) · [Measurements →](docs/measurements.md)

### Entities and logic

| Place entities on surfaces | FGD entity helpers |
| --- | --- |
| ![Reviewing a new point entity on a picked surface](docs/images/entity-placement-controls.png) | ![Selected point entity with property-radius guides](docs/images/fgd-spheres.png) |
| Pick a surface and review snapped contact, bounds and orientation before creating an entity. | Class boxes share picking and framing; selected radius guides use named properties or the class’s radius field. |

| Entity connection guides | Particle effects |
| --- | --- |
| ![Colored guides between imported authored node IDs](docs/images/fgd-node-lines.png) | ![Particle definitions and effect-name choices](docs/images/particle-browser.png) |
| Show colored links between unique named points or imported node IDs. | Search active particle systems, inspect their definitions and choose effects for entities. |

| Projector and frustum guides | Entity icons |
| --- | --- |
| ![Property-bound FGD frustum with square near and far planes](docs/images/fgd-frustum-guide.png) | ![Mounted sound, target and relay entity icons](docs/images/fgd-icons.png) |
| Inspect projector and class-defined fields of view with exact near and far planes. | Show mounted entity images that follow the viewport camera. |

| Entity model guides | Guide visibility |
| --- | --- |
| ![Player-start, camera and constraint resource models](docs/images/fgd-model-guides.png) | ![The same entities with their resource guides hidden and ordinary boxes retained](docs/images/entity-resource-guides-hidden.png) |
| Show mounted class models with flat shading while retaining their ordinary selection boxes. | Hide editor images and model guides while keeping their boxes selectable. |

| Sprite resource guides |
| --- |
| ![Authored Additive Sprite guide over colored geometry](docs/images/additive-sprite-guide.png) |
| Place scaled Sprite resource images and preview supported authored Additive mode. |

[Sprite resource guides →](docs/sprite-guides.md)

| Connections | Safe renaming |
| --- | --- |
| ![Connection form with suggestions](docs/images/entity-connections.png) | ![Rename preview listing every reference to update](docs/images/entity-rename.png) |
| Wire outputs to inputs, with suggestions from the map and the game's entity definitions. | Rename an entity and update every reference to it in one step. |

| Linked paths |
| --- |
| ![Reviewing an ordered linked path](docs/images/linked-path.png) |
| Place path points on surfaces, then insert, remove, reorder or move nodes while preserving links. |

[Linked paths →](docs/path-authoring.md)

| Edit many entities |
| --- |
| ![Shared property editing with mixed values](docs/images/batch-properties.png) |
| Change shared properties, flags and colors across a selection. |

| RGB, HSV and HSL colors | Saved user colors |
| --- | --- |
| ![HSL color controls with separate light brightness](docs/images/entity-color-hsl.png) | ![Named saved colors in the picker](docs/images/entity-color-palette.png) |
| Pick colors in RGB, HSV or HSL and adjust light brightness independently. | Save, rename and reuse named colors across open maps without changing light brightness. |

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
| Find, select, frame, hide and isolate objects. | Keep automatic snapshots, manual checkpoints and save backups; reload settings and recover safe copies. |

| Map statistics | Leak tracing |
| --- | --- |
| ![Map totals and material usage](docs/images/map-statistics.png) | ![A red leak path through the map](docs/images/pointfile-viewport.png) |
| Counts and breakdowns of brushes, entities, materials, models and terrain. | Follow a leak's pointfile through the map, point by point. |

| Cut, copy and paste |
| --- |
| ![Paste placement and link handling](docs/images/clipboard-paste.png) |
| Transfer brushes and entities between maps with previewed placement, groups, notes and checked links. |

| Prefab library | Referenced brush assemblies |
| --- | --- |
| ![Textured prefab gallery with placement controls](docs/images/prefab-gallery.png) | ![Resolved brush instance geometry](docs/images/instance-reference-contents.png) |
| Browse flat or textured assembly galleries, then place rotated or brush-scaled copies. | Extract, place, preview and compile reusable world/detail VMFs with guarded dependencies. |

| Open documents and Save all |
| --- |
| ![Per-map save results in the Open documents list](docs/images/save-all-named-maps.png) |
| Keep two independent maps open and save named maps together with explicit per-map results. |

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

| Recent maps | Save as |
| --- | --- |
| ![Recent map paths and retained build settings](docs/images/recent-maps.png) | ![Save as and reachable file, build and editing controls](docs/images/save-as-controls.png) |
| Reopen recent maps with retained compatible build settings. | Choose another source file with the toolbar or Ctrl+Shift+S. |

[Organizing and checking maps →](docs/map-tools.md)

### Compile and play

| Native compiler | Play in CS:S |
| --- | --- |
| ![Build settings, statistics and diagnostics](docs/images/build-status.png) | ![Soft sun shadows in Counter-Strike: Source](docs/images/soft-sun.png) |
| Compile playable maps with baked lighting, CS:S engine physics, water, props and terrain. | **Run Map** compiles and launches the game. Builds run in the background and never overwrite good output with a failed one. |

| Named build recipes | Portable sky assets |
| --- | --- |
| ![Named build recipes and their compiler settings](docs/images/build-recipes.png) | ![Sky asset packing enabled in Build settings](docs/images/build-pack-sky-assets.png) |
| Save, recall and reload named compiler settings alongside Fast preview and Final lighting. | Include supported LDR/HDR sky materials and textures in the BSP with their original names and bytes. |

| Baked lightmaps | Static-prop vertex lighting |
| --- | --- |
| ![Baked face lightmap inspection](docs/images/baked-lightmaps.png) | ![Static-prop vertex lighting in Build settings](docs/images/build-prop-vertex-lighting.png) |
| Inspect baked samples and trace compiled fragments to their retained authored contributors. | Bake independent LDR/HDR colors into supported static models across every draw LOD. |

[Baked lightmaps →](docs/baked-lightmaps.md)

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

| Source models | Model opacity |
| --- | --- |
| ![Model browser and bodygroup preview](docs/images/bodygroup-model-browser.png) | ![Dynamic model opacity preview](docs/images/model-opacity.png) |
| Browse models, skins and named bodygroups with reference poses and dynamic-model tints. | Preview Texture render mode with authored opacity while retaining texture alpha and depth. |

| Dynamic model scale |
| --- |
| ![Dynamic models with authored uniform scales](docs/images/model-scale.png) |
| Resize supported dynamic meshes with matching picks, bounds, framing and reference attachments. |

[Source models →](docs/models.md)

| Model detail props |
| --- |
| ![Decorative detail models in Counter-Strike: Source](docs/images/detail-props.png) |
| Place decorative models and compile them into native detail records without collision. |

[Model detail props →](docs/model-detail-props.md)

| Explicit detail sprites |
| --- |
| ![Individual atlas sprites in Counter-Strike: Source](docs/images/explicit-detail-sprites.png) |
| Place scaled atlas cards or fixed cross/tri shapes, with optional client sway. |

[Explicit detail sprites →](docs/detail-sprites.md)

| Material detail scattering |
| --- |
| ![Material-generated foliage on displaced terrain in Counter-Strike: Source](docs/images/terrain-detail-scattering.png) |
| Generate decorative models, billboards and fixed cross/tri foliage from brush and terrain materials, with blend alpha, baked lighting and a map-wide tint. |

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

| Visibility quality | Authored visibility clusters |
| --- | --- |
| ![Detailed portal clipping selected in Build settings](docs/images/build-settings.png) | ![Build details with solved leaf and emitted cluster counts](docs/images/build-visclusters.png) |
| Build conservative or tighter visibility with parallel portal processing. | Group dry open leaves while retaining conservative visibility and separate ambient samples. |

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
| Preview and override which reflection sample a face uses. | Capture local LDR or paired LDR/HDR reflections during Compile and keep them working when you play. |

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
| ![Viewport overlay corner handles](docs/images/overlay-corner-handles-viewport.png) |
| Fit, rebind or drag projection corners on world/detail faces while retaining settings and entity links. |

[Overlays →](docs/overlays.md)

| Model ambient lighting |
| --- |
| ![Sky and surface tint on a weapon model in Counter-Strike: Source](docs/images/model-ambient.png) |
| Models pick up reflected light from nearby world surfaces and visible sky. |

## Status

The editor covers most everyday brush work. Notable gaps:

- **Terrain**: creation, resolution edits, live sculpt/blend strokes, vertex handles and full/half-edge subdivision work; other fractional edge tools remain planned.
- **Models**: static meshes, reference poses, skins, bodygroups and visual browsing work; animation remains planned.
- **Compiler**: Fast and Detailed visibility, independent LDR/HDR lighting, diffuse bounces, static-prop vertex lighting and reflection generation are available; exposure and per-texel prop lightmaps remain planned.

The full picture is in [Status and roadmap](docs/status.md).

## Documentation

The [manual](docs/README.md) covers every feature, step by step:

[Getting started](docs/getting-started.md) · [Editor basics](docs/basics.md) · [Selecting](docs/selection.md) · [Brushes and transforms](docs/brushes.md) · [Hollow, carve and clip](docs/brush-tools.md) · [Vertices, edges and faces](docs/components.md) · [Flat partitions](docs/partitions.md) · [Materials and textures](docs/textures.md) · [Entities and logic](docs/entities.md) · [Models](docs/models.md) · [Organizing and checking maps](docs/map-tools.md) · [Compile and play](docs/compile-and-play.md) · [AI agents and MCP](docs/mcp.md) · [Development](docs/development.md)

## Credits

- The point-light helper uses a [CC0 light bulb model by Josh Dean / Poly Haven](https://polyhaven.com/a/lightbulb_led) ([provenance](crates/render/assets/lightbulb/SOURCE.txt)).
- Counter-Strike: Source and its assets are not included.
