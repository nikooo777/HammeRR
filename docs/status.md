[← Manual](README.md)

# Status

HammeRR supports brush-map authoring through the editor and MCP.
The integrated compiler is experimental. File operations, BSP publication and game launching require Linux.

## Editor

| Area | Works | Not yet / limits |
| --- | --- | --- |
| [MCP](mcp.md) | Live desktop and private headless sessions; shared editing, rendering and builds. | Select a document explicitly when two maps are open. |
| [Documents](open-documents.md) | Two map windows, reviewed Close, per-map Save results. | Tabs, bulk Quit and workspace restoration. |
| [VMF and recovery](map-tools.md) | Preserved imported fields, checked Save, recovery snapshots and map checks. | Other-editor round trips; general automatic repairs. |
| [History](history.md) | Shared Undo/Redo and change summaries. | Persistent history and full property diffs. |
| [Selection](selection.md) | Type filters, saved startup defaults, overlap cycling, area, volume and paint selection. | Named filter profiles. |
| [Brushes](brushes.md) | Shapes, handles, exact transforms, pivots, mirror, shear and grid snapping. | Magnetic snapping and persistent custom pivots. |
| [Brush tools](brush-tools.md) | Hollow, carve, clip, convex merge and polygon extrusion. | Concave merge and holes. |
| [Components](components.md) | Move, rotate, scale, weld, split, ridges and partitions. | General topology editing and arbitrary dividing lines. |
| [Materials](textures.md) | Grouped browsing, favorites, mapping, painting, wrapping, replacement, padded atlas rectangles and face lighting. | Full native shader preview. |
| [Normal maps](normal-maps.md) | Supported flat-brush normal textures. | Terrain/model normal maps and special encodings. |
| [Lighting preview](lighting-preview.md) | Unshadowed point, spot and sun lighting with checked receivers. | Shadows, sky visibility, bounce and native brightness match. |
| [Terrain](terrain.md) | Creation, sculpting, blending, masks, handles, resizing, sewing and full/half-edge subdivision. | Holes, triangle removal and general fractional joins. |
| [Entities](entities.md) | Typed properties, colors, FGD helpers, connections, batch edits and safe rename. | General runtime simulation. |
| [Models](models.md) | Meshes, reference poses, skins, bodygroups, dynamic scale, tint, opacity and attachments. | Animation, procedural poses, attachment parenting and physics preview. |
| [Resource previews](sprite-guides.md) | Supported sprites, static ropes, fog and LDR/HDR/3D skies. | Complete native effects and runtime behavior. |
| [Particles](particles.md) | Active manifest metadata and effect assignment. | Particle rendering and animation. |
| [Sounds](sounds.md) | File/script browsing, short audition and assignment. | Game mixing, soundscapes and sentence synthesis. |
| [Organization](layers.md) | Groups, layers, clipboard, prefabs, notes and saved views. | Automatic category rules and embedded prefab assets. |
| [Measurements](measurements.md) | Saved rulers, surface picks and origin/local entity attachments. | Surface attachments, parenting and animated bones. |
| [Instances](instance-authoring.md) | Saved references, checked contents, light color/HDR gain bindings and bounded extraction. | In-place child editing and general replacement parameters. |

## Compiler

| Area | Works | Not yet / limits |
| --- | --- | --- |
| [World and visibility](compile-and-play.md) | Structural/detail geometry, Fast and Detailed visibility, hints, portals and fading windows. | General unsupported portal/model combinations. |
| [Lighting](compile-and-play.md#lighting) | LDR/HDR, styles, point/spot/sun/sky, soft sun, ambient probes and up to four diffuse bounces. | Material emission, per-texel prop lightmaps and general shader transport. |
| [Material lighting](compile-and-play.md#normal-map-lighting) | Checked normal maps, albedo, reflectivity and [alpha texture shadows](brush-texture-shadows.md). | Unsupported effects reject explicitly. |
| [Physics](physics.md) | Engine world, terrain and supported brush-model collision, contact materials and derived mass. | Custom density and broader moving-body behavior. |
| [Brush entities](compile-and-play.md#brush-entities-and-pivots) | Checked pivots, initial rotation, moving models and attached lightmaps. | General parenting, arbitrary model classes and initial poses. |
| [Terrain](terrain.md) | Mixed-grid full/half neighbors, LOD snapping, lighting and collision. | Holes and runtime brush-model terrain. |
| [Water](water.md) | Flat world pools, inward surfaces, depth, fluid physics, current and damping. | Sloped inward surfaces, slime, per-pool current and moving rendered water. |
| [Static props](static-props.md) | Models, checked collision choices, shadows, ambient/vertex lighting and packing with nested material edits. | Exact hull leaf membership and general materials/bodygroups. |
| [Detail props](material-detail-scattering.md) | Explicit and generated decorative models, cards, cross/tri shapes and supported lighting. | Editor display of final generated details. |
| [Reflections](reflections.md) | Private LDR or paired LDR/HDR captures and verified packing. | Recursive reflection bounces and unsupported capture scenes. |
| [Overlays](overlays.md) | Coplanar face-group fitting, native projections, face splitting, supported terrain targets and water transitions. | Full projected-material viewport preview; multi-face refit and corner editing. |
| [Assemblies](instances.md) | Checked nested transforms, stationary child initial poses, per-reference light colors/HDR gains and supported geometry/entities. | General runtime entities, scripting and name fixups. |
| [Visibility helpers](occluders.md) | Occluders and eligible visibility-cluster volumes. | Unsupported memberships and dynamic child control. |
| [Inspection](baked-lightmaps.md) | Owned face lightmaps, prop vertex bytes and native light records. | Full baked-output viewport rendering. |
| [Publication](compile-regions.md) | Captured build scope, cancellation, checked output writes and private game launch. | General cross-platform file/runtime support. |

Unsupported input and exceeded resource budgets fail explicitly.
A failed build preserves the previous published output.
