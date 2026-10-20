# HammeRR manual

Start with [Getting started](getting-started.md) and [Editor basics](basics.md).

## Using the editor

| Page | What it covers |
| --- | --- |
| [Getting started](getting-started.md) | Requirements, building, launching, game content |
| [Editor basics](basics.md) | The window, previews and undo, navigation, viewport modes, saved camera views |
| [Preview defaults](preview-defaults.md) | Save, apply and recover startup visibility for existing images and guides |
| [Fly speed and navigation defaults](navigation-defaults.md) | Shared camera flight speed and explicit startup defaults |
| [Keyboard shortcuts](keyboard-shortcuts.md) | Editing, projection and Objects/Faces command bindings |
| [Open documents](open-documents.md) | Two independent map windows, Save all named maps, explicit MCP targeting and reviewed Close |
| [Recent maps](recent-maps.md) | Reopening retained paths with compatible build settings, removing and clearing entries |
| [Transaction history](history.md) | Inspecting human/agent edits and navigating retained undo/redo states |
| [Measurement guides](measurements.md) | Persistent rulers, exact surface picks and entity-origin/local attachments |
| [Selecting](selection.md) | Object-type filters, temporary editor hiding, cycling and area/volume/paint selection |
| [Cut, copy and paste](clipboard.md) | Detached object transfer, placement previews, native VMF text and safe references |
| [Prefab library](prefabs.md) | Save, search, inspect Flat/Textured thumbnails and place reusable assemblies |
| [Layers](layers.md) | Nested colored layers, membership, scoped selection and map visibility |
| [Object groups](groups.md) | Nested groups, membership editing, group selection and temporary bypass |
| [Brushes and transforms](brushes.md) | Drawing brushes, handles, exact transforms, surface-picked pivots, mirror, shear, brush shapes |
| [Polygon brushes](polygon-brushes.md) | Drawing convex or concave outlines and extruding editable brushes |
| [Surface placement](placement.md) | Floor/ceiling alignment and brush corner snaps to brush, terrain or model vertices |
| [Repeated placement](repeated-placement.md) | Previewing translation, rotation and brush-scale arrays with copied names and internal links |
| [Hollow, carve and clip](brush-tools.md) | Turning solids into rooms, subtracting shapes, cutting brushes with a plane |
| [Merge brushes](merge.md) | Joining brushes whose combined volume is convex, with retained outer face settings |
| [Vertices, edges and faces](components.md) | Moving, scaling, rotating, welding, splitting and roof ridges |
| [Flat partitions](partitions.md) | Dividing a face into pieces to raise, extrude or restyle them |
| [Materials and textures](textures.md) | Material browser, texture editing, sampling, wrapping, painting, replacement, face lighting |
| [Material source](material-source.md) | Mounted VMT source, Patch declarations and bounded Water dependencies |
| [Atlas-region texturing](atlas-texturing.md) | Fit flat faces into a numeric rectangle of an existing material image |
| [Normal-map preview](normal-maps.md) | Preview supported flat-brush normal textures under editor directional shading |
| [Authored lighting preview](lighting-preview.md) | Unshadowed point/spot diffuse light, quality, exposure and supported receivers |
| [Working region](working-region.md) | Temporary whole-object display bounds and visibility reasons |
| [Fog preview](fog.md) | Choosing an authored static fog controller for perspective Textured views |
| [Sky previews and assets](sky.md) | Previewing LDR/HDR skies and explicit 3D sky regions, and inspecting mounted sky assets |
| [Rope preview](ropes.md) | Static linked rope curves, diagnostics and source-entity picking |
| [Terrain](terrain.md) | Creating, sculpting, blending, sewing, shading and selecting terrain triangles |
| [Overlays](overlays.md) | Fit, rebind and edit projection corners; compile imported overlays and fades |
| [Cubemap faces](cubemap-faces.md) | Inspecting sample choices and previewing manual reflection bindings |
| [Generated reflections](reflections.md) | Capturing LDR or paired LDR/HDR samples during a build and packing them for play |
| [Models](models.md) | Static mesh previews, skin materials, bounds, picking and model inspection |
| [Sprite resource guides](sprite-guides.md) | Static mounted Sprite images, physical scale, safe resource choices and limits |
| [Particle effects](particles.md) | Active manifest definitions, metadata and guarded effect-name choices |
| [Sounds](sounds.md) | Sound files and script names, audition, volume and entity assignment |
| [Entities and logic](entities.md) | Placing entities, lights, brush entities, inputs/outputs, safe renaming, editing many entities |
| [Linked paths](path-authoring.md) | Ordered open or closed path_corner/path_track chains with preview and Undo |
| [Organizing and checking maps](map-tools.md) | Finding objects, visibility, statistics, map checks, design notes, autosave and recovery |
| [Compile and play](compile-and-play.md) | Building BSPs, portable sky assets, lighting, physics, leaks and launching Counter-Strike: Source |
| [HDR exposure controllers](hdr-exposure.md) | Fixed game exposure through ordinary entities and connections |
| [Baked lightmaps](baked-lightmaps.md) | Actual compiled LDR/HDR face samples, style and directional-map inspection |
| [Baked prop vertices](baked-prop-vertices.md) | Native prop ordinals, LOD hardware sections and exact packed LDR/HDR color bytes |
| [Water](water.md) | Flat world pools, underwater materials, fog and floating objects |
| [Instance authoring](instance-authoring.md) | Place and edit saved VMF references with readable path and transform controls |
| [Extract brush assemblies](instance-extraction.md) | Export selected brushes and replace them with a guarded instance reference |
| [Instance contents](instance-contents.md) | Resolve, inspect, select and frame a flat reference layer |
| [Referenced instances](instances.md) | Compile saved world/detail assemblies with guarded dependencies |
| [Compile regions](compile-regions.md) | Previewing and saving bounds for sealed partial-map builds |
| [Physics and mass](physics.md) | Engine collision bodies and material-derived brush-model masses |
| [Static props](static-props.md) | Model placement, collision choices and rotated visibility bounds |
| [Model detail props](model-detail-props.md) | Explicit decorative models compiled without collision |
| [Explicit detail sprites](detail-sprites.md) | Place scaled atlas cards or fixed cross/tri shapes with optional sway |
| [Material detail scattering](material-detail-scattering.md) | Generate decorative models and sprites from ground materials when building |
| [Static-prop shadows](prop-shadows.md) | Opaque, alpha-test and requested translucent model shadows, shadow flags and material limits |
| [Occluders](occluders.md) | Compiling static visibility blockers and their native activation records |
| [Visibility clusters](visclusters.md) | Group dry open leaves conservatively without merging geometry or ambient samples |
| [Build recipes](build-recipes.md) | Recalling Fast preview, Final lighting and named custom compiler settings |

## AI agents

| Page | What it covers |
| --- | --- |
| [AI agents and MCP](mcp.md) | Connecting an agent, how agents edit alongside you, tool reference |

## Project

| Page | What it covers |
| --- | --- |
| [Status and roadmap](status.md) | What works, what's partial, what's next |
