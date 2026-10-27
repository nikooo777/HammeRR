[← Manual](README.md)

# Status

HammeRR supports brush-map authoring through the editor and MCP.
The integrated compiler is experimental. File operations, BSP publication and game launching require Linux.

## Editor

| Area | Works | Not yet / limits |
| --- | --- | --- |
| [MCP](mcp.md) | Live desktop and private headless sessions; shared editing, rendering and builds. | Select a document explicitly when two maps are open. |
| [Documents](open-documents.md) | Two map windows, silent display, named Close blockers and per-map Save results. | Tabs, bulk Quit and workspace restoration. |
| [VMF and recovery](map-tools.md) | Preserved imported fields, checked Save, recovery snapshots and map checks. | Other-editor round trips; general automatic repairs. |
| [History](history.md) | Shared Undo/Redo and change summaries. | Persistent history and full property diffs. |
| [External map comparison](map-comparison.md) | Bounded authored world-brush/whole-entity comparison by stored IDs, categorized pages and complete filtered current selection/framing. | No merge, map-settings comparison, recursive-instance expansion or geometric-equivalence claim. |
| [Selection](selection.md) | Type filters, independent selection tint/outlines, startup defaults, named profiles, overlap cycling, area, volume and paint selection. | Separate solid/group modes. |
| [Keyboard shortcuts](keyboard-shortcuts.md) | Configurable viewport commands, including temporary editor hide/show. | File, clipboard, physical-key and multi-key bindings. |
| [Brushes](brushes.md) | Shapes, handles, exact transforms, selection or per-object grid snapping, pivots with grid snapping and saved defaults, mirror and shear. | Magnetic snapping and surface-attached pivots. |
| [Brush tools](brush-tools.md) | Hollow, carve, clip, convex merge and polygon extrusion. | Concave merge and holes. |
| [Components](components.md) | Move, rotate, scale, weld, split, ridges and partitions. | General topology editing and arbitrary dividing lines. |
| [Materials](textures.md) | Grouped browsing, favorites, mapping, painting, wrapping, replacement, saved named atlas regions, padded image fitting with coplanar continuity and face lighting. | Full native shader preview. |
| [Normal maps](normal-maps.md) | Supported flat-brush normal textures. | Terrain/model normal maps and special encodings. |
| [Lighting preview](lighting-preview.md) | Unshadowed point, spot and sun lighting with checked receivers. | Shadows, sky visibility, bounce and native brightness match. |
| [Color correction](color-correction.md) | One explicit Source RGB8 `.raw` LUT, shared enable/weight controls and complete viewport display-color preview. | Native HDR, tone mapping, entities/volumes and multiple-LUT composition. |
| [Terrain](terrain.md) | Creation, sculpting, blending, masks, handles, resizing, sewing and full/half-edge subdivision. | Holes, triangle removal and general fractional joins. |
| [Entities](entities.md) | Typed properties, exact/substring class selection, colors, FGD helpers, connections, bounded I/O graph queries/connected selection/framing, batch edits and safe rename. | General graph canvas layouts and runtime simulation. |
| [Models](models.md) | Meshes, reference poses, skins, bodygroups, dynamic scale, tint, opacity and attachments. | Animation, procedural poses, attachment parenting and physics preview. |
| [Resource previews](sprite-guides.md) | Supported sprites, [static ropes with sag-aware framing and selection bounds](ropes.md), fog and LDR/HDR/3D skies. | Complete native effects and runtime behavior. |
| [Particles](particles.md) | Active manifest metadata and effect assignment. | Particle rendering and animation. |
| [Sounds](sounds.md) | File/script browsing, short audition and assignment. | Game mixing, soundscapes and sentence synthesis. |
| [Organization](layers.md) | Groups, layers, clipboard, prefabs, notes and saved views. | Automatic category rules and embedded prefab assets. |
| [Measurements](measurements.md) | Saved rulers, surface picks and origin/local entity attachments. | Surface attachments, parenting and animated bones. |
| [Instances](instance-authoring.md) | Saved references, checked brush/terrain and static-prop mesh previews, nested light bindings, static child fog, saved child atlas regions and bounded extraction with chosen local pivots. | In-place child editing, child illumination and general replacement parameters. |

## Compiler

| Area | Works | Not yet / limits |
| --- | --- | --- |
| [World and visibility](compile-and-play.md) | Structural/detail geometry, Fast and Detailed visibility, hints, portals and fading windows. | General unsupported portal/model combinations. |
| [Visibility inspection](visibility.md) | Owned world-leaf/cluster joins, original portal rings, native PVS/PAS pages, world-position/Perspective-camera lookup, framing, selected guides and complete bounded PVS/PAS destination bounds. | Declared f32 classification has no engine boundary bit-parity claim; destination guides cap at 512 leaves and native bounds are not exact cells; no compiled-cell picking or PRT import; freshness guards consumed compiler inputs, not a complete runtime material closure. |
| [Lighting](compile-and-play.md#lighting) | LDR/HDR, styles with located overflow errors, point/spot/sun/sky, soft sun, ambient probes and up to four diffuse bounces. | Material emission, per-texel prop lightmaps and general shader transport. |
| [Material lighting](compile-and-play.md#normal-map-lighting) | Checked normal maps, albedo, reflectivity and [binary texture shadows](brush-texture-shadows.md), including explicitly requested stationary root and child brush models. | Self shadows need a lit receiver in the same model; general translucent effects remain unsupported. |
| [Physics](physics.md) | Engine world, terrain and supported brush-model collision, contact materials and derived mass. Builds track consumed surface definitions. | Custom density and broader moving-body behavior. |
| [Brush entities](compile-and-play.md#brush-entities-and-pivots) | Checked pivots, initial rotation, moving models and attached lightmaps. | General parenting, arbitrary model classes and initial poses. |
| [Terrain](terrain.md) | Mixed-grid full/half neighbors, LOD snapping, lighting and collision. | Holes and runtime brush-model terrain. |
| [Water and slime](water.md) | Flat structural-world pools, inward surfaces, depth, fluid physics, map-wide current/damping and static liquid asset packing; complete volumes in compile regions. | Sloped or detail/model liquids, per-pool controls and moving rendered water. Slime packing requires supported static non-reflective materials; other assets must remain mounted. |
| [Static props](static-props.md) | Models, checked collision choices, shadows, ambient/vertex lighting and nested material edits for packing and cutout receivers. Builds track consumed shadow assets even when packing and vertex lighting are off. | Exact hull leaf membership and general materials/bodygroups. |
| [Detail props](material-detail-scattering.md) | Explicit and scattered models, cards, cross/tri shapes, supported lighting and a [final compiled-detail preview](compiled-details.md) with camera-facing cards. | Preview motion, fading, baked illumination and native appearance parity. |
| [Reflections](reflections.md) | Private LDR or paired LDR/HDR captures, verified packing and root-face binding drafts with assembled instance sample choices. | Recursive reflection bounces and unsupported capture scenes. |
| [Overlays](overlays.md) | Coplanar flat, isolated planar-terrain and mixed-group fitting, refit, rebind and corner editing; native projections, face splitting and water transitions. | Joined or nonplanar terrain authoring; projected-material viewport preview. |
| [Assemblies](instances.md) | Checked transforms, stationary child poses, local spotlight targets, static fog and nested light-value bindings; collapse all visible references or a selected leading set into checked editable world/detail/terrain, props, lights and cubemaps with captured source routes. | Prop collapse needs empty lightingorigin. Arbitrary-subset collapse, other runtime-entity collapse, scripting and broader name fixups. |
| [Visibility helpers](occluders.md) | Occluders and eligible visibility-cluster volumes. | Unsupported memberships and dynamic child control. |
| [Inspection](baked-lightmaps.md) | Owned face lightmaps, prop vertex bytes, native light records and captured point/spot, static-prop and baked face source identities with complete nested helper routes; [frame current point/spot origins](compiled-lights.md) and [native static-prop origins](baked-prop-vertices.md) in the viewport; select the root entity or containing reference for a current prop or light; frame flat compiled faces and show complete native prop leaf bounds. | Unreportable physical paths show stale provenance; occurrence routes describe captured sources only; no full baked-output viewport rendering. |
| [Publication](compile-regions.md) | Captured build scope, consumed material-source guards, cancellation, checked output writes and private game launch. | General cross-platform file/runtime support. |

Unsupported input and exceeded resource budgets fail explicitly.
A failed build preserves the previous published output.
