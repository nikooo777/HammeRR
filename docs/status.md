[← Manual](README.md)

# Status and roadmap

HammeRR is in active development.

- **The editor** is usable today for building, texturing and wiring up brush-based maps, by hand or with an AI agent.
- **The compiler is experimental.** Compiled maps load and play in Counter-Strike: Source. Fast and Detailed visibility are available; lighting covers direct light, sky ambient and up to four optional diffuse bounces; HDR remains unfinished.

## Editor

| Area | State |
| --- | --- |
| Opening and saving VMFs | Works. Imported properties and extra blocks are retained; saves increment the map version and use standard VMF formatting. Round trips through other editors haven't been verified yet. |
| Brushes, transforms and shapes | Works. |
| Hollow, carve and clip | Works for brushes. Terrain is rejected. |
| Vertices, edges and faces | Works for brushes. Terrain is rejected. |
| Flat partitions | Works. Inward extrusion, free-form dividing lines and per-piece texture axes, fit and justification are planned. |
| Materials and textures | Works. |
| Entities, connections and renaming | Works. |
| Map checks | Geometry, IDs, textures, entity logic, asset paths and spawns. Models, terrain, objectives, full asset dependencies and engine limits are planned. |
| Terrain (displacements) | Creation, chosen corners, resolution edits, sculpting, blend paint, full/half-edge sewing across grid resolutions and collision controls work with guarded previews and Undo. Viewport center picking and framing follow displaced terrain. Compatible mixed-power full and whole-to-half edges compile with native LOD masks and snapping. Live strokes, subdivision, other partial spans and restricted-topology remapping remain planned. |
| Models | Static props compile, but models aren't drawn in the viewport yet. |

## Compiler

| Feature | Works | Not yet |
| --- | --- | --- |
| Geometry and collision | BSP version 20, with checked brush planes and collision bevels | — |
| Build settings | Lighting, physics, visibility and sky quality available before the first build; shared with agents and frozen during builds | — |
| Visibility | Fast conservative sets or Detailed portal separator clipping with bounded parallel portal-facing rows and source flow; authored finite hint priority and remaining open-leaf cuts; visible/audible sets, sky visibility, areaportals and distance-fading windows | Occluders and broader map coverage |
| Lighting | Fullbright; Direct LDR for static, switchable and animated point/spot lights, one sun and sky ambient; directional normal-map lightmaps, soft sun, Fast/Balanced/Detailed sampling, percentage falloff, opaque shadows, terrain, pivot-aware direct brush-entity maps with self shadows, scoped world/model brush groups or optional angle smoothing, including partial-edge junctions and up to four style-preserving diffuse world/terrain-source bounces into world/model receivers; conservative point/spot visibility and exact-range culling | Adaptive sampling, HDR, transparent shadows, second-normal-map-only materials, model relighting/reflected sources and baked prop lightmaps |
| Model ambient | LDR cubes from filtered local world/terrain lightmaps and sky, with up to 1/4/8 safe probes per open leaf by quality; legacy and indexed LDR/HDR data preservation | Adaptive refinement, switching ambient cubes, static-prop occlusion, probes below terrain sheets |
| Physics | World, clips, brush entities, water and terrain, using CS:S's own collision library; world/terrain triangle surface properties | Model surface assignments, density and compiler-derived masses, broader collision with moving bodies |
| Brush entities | Explicit and origin-brush pivots; sliding and rotating doors, rotating brushes and triggers | Broader behavior/contact coverage and independently rotated initial lighting poses |
| Water | Flat world water, fog regions, floating objects | Depth and below-water materials, currents, water in detail brushes or brush entities |
| Cubemaps | Sample positions and sizes | Baked reflections |
| Static props | Models, transforms, collision, skins, fading, named lighting origins | Prop lighting and shadows, exact placement in the BSP tree |
| Terrain | Power 2/3/4 surfaces, collision, matching full-edge and corner-only links, full/half-edge LOD masks/snapping and compiled common-vertex normal sewing | Authored smoothing angles, other fractional spans, subdivision, removed triangles |
| Overlays | Imported `info_overlay` projections on world, detail and terrain surfaces; materials, render order, distance fading and named accessors | Placement, corner editing, viewport projection, geometry-edit rebinding, brush-model targets, water transitions and decals |

## Next

**Editor:** free-form flat partitions, inward extrusion, per-piece texture editing, terrain clipping and authoring, model rendering and broader map checks.

**Compiler:** HDR, broader physics collision, broader terrain topology, prop lighting, brush-model overlays and water transitions.
