# HammeRR

**An AI-first Counter-Strike: Source map editor and compiler for Linux and Windows.**

Edit VMF maps with native game assets. Build playable BSPs with integrated visibility, lighting and engine physics.
Humans and AI agents share the same editing operations, previews and Undo history.

[![Estimated implementation: 89.92% across 107 equally weighted capability groups](docs/images/roadmap-progress.svg)](docs/status.md#group-completion)

![A HammeRR courtyard running in Counter-Strike: Source](docs/images/showcase-native-pool.png)

[Manual](docs/README.md) · [Status and limits](docs/status.md) · [MCP setup](docs/mcp.md)

## Quick start

[Download a tagged build](https://github.com/nikooo777/HammeRR/releases) or build from source with Rust 1.92+ and a C++20 toolchain. Requires a compatible GPU driver and Counter-Strike: Source installed through Steam.

```sh
cargo build --release -j8 -p editor -p editor-core -p compiler -p native-runtime
./target/release/editor path/to/map.vmf
```

Omit the map path for a new map. Game content is found through Steam.
See [Getting started](docs/getting-started.md) for custom content and launch options.
The compiler is experimental; see [Status](docs/status.md).

On Windows, use Rust MSVC and Visual Studio C++ Build Tools with a Windows SDK:

```powershell
.\run-windows.ps1
```

See [Windows setup and limits](docs/getting-started.md#windows).

## MCP

Connect to the live editor at `http://127.0.0.1:7377/mcp`:

```json
{"mcpServers":{"hammerr":{"url":"http://127.0.0.1:7377/mcp"}}}
```

Agents can edit, render, compile and launch maps. On Linux, use `hammerr-headless` for a private session without a window.
See the [tool index and setup](docs/mcp.md).

## The editor

![HammeRR with detail brushes selected: menus, tool rail, viewport, Inspector and Agent activity](docs/images/editor-window.png)

Menus, a tool rail, a selection-driven Inspector and a live Agent feed. See [Editor basics](docs/basics.md).

## Feature tour

| [Brush shapes](docs/brushes.md) | [Hollow rooms](docs/brush-tools.md) |
| --- | --- |
| ![Wedges, cylinders, cones, spheres and an arch](docs/images/primitive-gallery.png) | ![A hollow masonry room](docs/images/hollow-room.png) |
| Build exact shapes or import block batches. | Turn solids into rooms with selected openings. |

| [Carved openings](docs/brush-tools.md#carve) | [Clip planes](docs/brush-tools.md#clip) |
| --- | --- |
| ![An arch cut through a brick wall](docs/images/carved-doorway.png) | ![Two painted curved-terrain halves moved apart](docs/images/terrain-surface-cut.png) |
| Subtract shapes to form doors and passages. | Cut brushes, planar surfaces and folded terrain. |

| [Component editing](docs/components.md) | [Arrays and surface scatter](docs/repeated-placement.md) |
| --- | --- |
| ![Free face cuts with independent texture placement](docs/images/free-face-cuts.png) | ![Scaled barrels follow a sculpted terrain surface](docs/images/surface-scatter.png) |
| Cut face patterns and reshape components with attached textures. | Repeat or scatter assemblies with checked pivots, scale and references. |

| [Materials](docs/textures.md) | [Normal maps](docs/normal-maps.md) |
| --- | --- |
| ![Different materials on one brush](docs/images/face-materials.png) | ![Normal-map surface detail on brick](docs/images/normal-maps-on.png) |
| Filter native materials and inspect both blend-image layers. | Inspect supported surface detail before compiling. |

| [Atlas texturing](docs/atlas-texturing.md) | [Grid snapping](docs/brushes.md#snap-objects-to-grid) |
| --- | --- |
| ![Face islands automatically packed into one Source trim sheet](docs/images/atlas-packing.png) | ![Separate objects aligned to the grid](docs/images/independent-grid-snapping.png) |
| Pack face islands and fit named, padded atlas regions. | Align objects together or snap each object separately. |

| [Terrain](docs/terrain.md) | [Compiled terrain](docs/terrain.md#compile-split-borders) |
| --- | --- |
| ![Sculpted terrain with painted blends](docs/images/terrain-blended.png) | ![Terrain running in CS:S](docs/images/terrain-native.png) |
| Subdivide, sculpt, paint, sew, mirror and shear terrain. | Build mixed-grid terrain with lighting and collision. |

| [Lighting preview](docs/lighting-preview.md) | [Baked lighting](docs/compile-and-play.md#lighting) |
| --- | --- |
| ![HDR authored lighting on brick and concrete](docs/images/authored-lighting-hdr.png) | ![Red and blue translucent terrain casts colored baked shadows](docs/images/texture-shadows-colored.png) |
| Inspect unshadowed LDR/HDR point, spot and sun lighting. | Bake LDR/HDR, colored shadows, emission and brush/prop bounces. |

| [Models and bulk editing](docs/bulk-entities.md) | [Surface placement](docs/placement.md) |
| --- | --- |
| ![Six props replaced together](docs/images/bulk-entities.png) | ![A barrel aligned with a sloping surface](docs/images/slope-placement.png) |
| Choose model sets, skins, bodygroups, scale, tint and opacity. | Place objects on floors, ceilings or picked surfaces. |

| [Point-entity gallery](docs/entity-gallery.md) | [Selection modes](docs/selection.md#choose-the-object-selection-unit) |
| --- | --- |
| ![Real FGD light, player and prop examples before placement](docs/images/entity-gallery.png) | ![One child brush selected inside a brush entity](docs/images/solid-selection.png) |
| Preview loaded classes and place reviewed batches in one Undo step. | Pick groups, whole objects or individual brushes inside entities. |

| [Entity property guides](docs/entities.md#inspect-property-guides) | [Content refresh](docs/textures.md#reload-mounted-content-and-definitions) |
| --- | --- |
| ![Path cylinders, wire boxes and coordinate guides](docs/images/entity-property-guides.png) | ![Updated loose-file and VPK materials on the same panels](docs/images/asset-reload-after.png) |
| Inspect bound properties and linked endpoints beside brush and model geometry. | Watch for changes and refresh mounts, definitions and assets. |

| [Reference images](docs/reference-images.md) | [Magnetic snapping](docs/placement.md#magnetic-translation) |
| --- | --- |
| ![A blueprint reference plane standing behind the compound](docs/images/reference-images.png) | ![Brush corners joined through magnetic capture](docs/images/magnetic-snapping.png) |
| Trace PNG references with shared pose and display controls. | Capture visible brush, terrain and model vertices while moving. |

| [Water and slime](docs/water.md) | [Ropes](docs/ropes.md) |
| --- | --- |
| ![A compiled reflecting pool](docs/images/water-native.png) | ![A textured rope placed between two surfaces](docs/images/rope-placement.png) |
| Build water and opaque slime pools with captured reflections and fluid physics. | Place ropes between surfaces and preview their motion, materials and world contact. |

| [3D sky terrain](docs/sky.md#add-terrain-to-the-miniature) |
| --- |
| ![A miniature hill fills the sky behind foreground props](docs/images/miniature-terrain.png) |
| Preview whole terrain patches with their authored mapping and smooth normals. |

| [Fog](docs/fog.md) | [Spatial guides](docs/measurements.md) |
| --- | --- |
| ![Distance fog in a metal hall](docs/images/fog-on.png) | ![An oblique reference plane and attached ruler](docs/images/spatial-guides.png) |
| Preview world and sky fog with timed histories and one HDR scene exposure. | Attach rulers to surfaces and snap picks to 3D reference planes. |

| [Color correction](docs/color-correction.md) | [Visibility profiles](docs/working-region.md#saved-rule-profiles) |
| --- | --- |
| ![A map viewport with color correction](docs/images/color-correction-preview.png) | ![A saved Architecture profile showing detail owners](docs/images/visibility-profile.png) |
| Preview one Source color LUT with an adjustable weight. | Save queries that isolate the parts of a map you need. |

| [Overlays](docs/overlays.md) | [Assemblies](docs/instances.md) |
| --- | --- |
| ![One projection spans two panels with a clear gap](docs/images/overlay-native.png) | ![Two references to one arch use different baked light colors](docs/images/instance-light-colors.png) |
| Fit projections across flat faces and separate planar terrain patches. | Reuse assemblies with local extraction pivots, scoped lights and static fog. |

| [Compiler visibility](docs/visibility.md) | [Editable instances](docs/instances.md#collapse-references-into-editable-geometry) |
| --- | --- |
| ![Complete PVS destination bounds in a three-room map](docs/images/visibility-pvs.png) | ![Editable brush arches after instance collapse](docs/images/collapsed-assembly.png) |
| Inspect native leaves, portals and complete PVS/PAS bounds. | Convert all references or a selected subset into editable map objects. |

| [Prop leaf bounds](docs/baked-prop-vertices.md) | [Final compiled details](docs/compiled-details.md) |
| --- | --- |
| ![Complete native static-prop leaf bounds](docs/images/prop-leaf-bounds.png) | ![Final details use baked HDR style colours](docs/images/compiled-detail-radiance.png) |
| Show every compiled leaf bound assigned to a static prop. | Preview final models and cards with baked LDR/HDR styles. |

| [Baked surfaces](docs/baked-surfaces.md) | [Map comparison](docs/map-comparison.md) |
| --- | --- |
| ![Baked lighting on arches, terrain, stairs and wooden props](docs/images/baked-surfaces.png) | ![Changed objects selected after an external-map comparison](docs/images/external-map-comparison.png) |
| View final baked surfaces and locate exact prop-lighting samples. | Compare stored objects and select or frame current matches. |

| [Document tabs and workspaces](docs/open-documents.md) | [Content profiles](docs/content-profiles.md) |
| --- | --- |
| ![Two persistent tabs and independent map views](docs/images/document-tabs.png) | ![The same map rendered with a mod material profile](docs/images/content-profile-mod.png) |
| Keep independent maps open and restore saved content, cameras and layouts. | Switch content per document and restore it when reopening maps. |

| [Prefab library](docs/prefabs.md) | [Coordinated views](docs/basics.md#coordinated-views) |
| --- | --- |
| ![A reusable brush and model assembly at three scales](docs/images/prefab-assemblies.png) | ![Four interactive views share the selected bridges](docs/images/coordinated-views.png) |
| Organize saved assemblies, replace their content and place scaled copies. | Edit through independent cameras with linked orthographic navigation. |

| [Linked paths and trains](docs/path-authoring.md) | [Entity graphs](docs/entities.md#inspect-connected-logic) |
| --- | --- |
| ![Directed train path with a node move handle](docs/images/linked-path-handles.png) | ![Directed logic with prepared instance lights](docs/images/entity-graph-canvas.png) |
| Edit path nodes, configure trains and duplicate linked assemblies. | Trace logic, arrange nodes and inspect instance sources. |

Counter-Strike: Source and its assets are not included.
