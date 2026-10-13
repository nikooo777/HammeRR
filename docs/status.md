[← Manual](README.md)

# Status and roadmap

HammeRR is in active development.

- **The editor** is usable today for building, texturing and wiring up brush-based maps, by hand or with an AI agent.
- **The compiler is experimental.** Compiled maps load and play in Counter-Strike: Source, but visibility is conservative and lighting covers direct light and sky ambient only, without bounced light or HDR.

## Editor

| Area | State |
| --- | --- |
| Opening and saving VMFs | Works. Unmodified files are saved back byte for byte. Round trips through other editors haven't been verified yet. |
| Brushes, transforms and shapes | Works. |
| Hollow, carve and clip | Works for brushes. Terrain is rejected. |
| Vertices, edges and faces | Works for brushes. Terrain is rejected. |
| Flat partitions | Works. Inward extrusion, free-form dividing lines and per-piece texture axes, fit and justification are planned. |
| Materials and textures | Works. |
| Entities, connections and renaming | Works. |
| Map checks | Geometry, IDs, textures, entity logic, asset paths and spawns. Models, terrain, objectives, full asset dependencies and engine limits are planned. |
| Terrain (displacements) | Imported terrain displays, can be selected and compiles. Creating and editing terrain is planned. |
| Models | Static props compile, but models aren't drawn in the viewport yet. |

## Compiler

| Feature | Works | Not yet |
| --- | --- | --- |
| Geometry and collision | BSP version 20, with checked brush planes and collision bevels | — |
| Build settings | Lighting, physics and sky quality available before the first build; shared with agents and frozen during builds | — |
| Visibility | Conservative visible and audible sets from portals; sky visibility | Tight portal clipping (VVIS quality), areaportals |
| Lighting | Fullbright; Direct LDR for static point lights, spotlights, one sun and sky ambient; soft sun, Fast/Balanced/Detailed sampling, percentage falloff, opaque shadows and terrain | Adaptive sky/sun sampling, bounced world light, HDR, transparent shadows, smoothing groups and bump maps on brush faces, baked prop and moving-model lightmaps |
| Model ambient | LDR cubes from reflected world-face averages and sky, with one safe probe per open leaf; legacy and indexed LDR/HDR data preservation | Local lightmap sampling, adaptive probe placement, static-prop occlusion |
| Physics | World, clips, brush entities, water and terrain, using CS:S's own collision library | Surface materials, authored masses, broader collision with moving bodies |
| Brush entities | Explicit and origin-brush pivots; sliding and rotating doors, rotating brushes and triggers | Broader brush-entity behavior coverage |
| Water | Flat world water, fog regions, floating objects | Depth and below-water materials, currents, water in detail brushes or brush entities |
| Cubemaps | Sample positions and sizes | Baked reflections |
| Static props | Models, transforms, collision, skins, fading | Prop lighting and shadows, exact placement in the BSP tree |
| Terrain | Power 2/3/4 surfaces, collision, lighting with smooth normals within each patch, links between matching neighbors | Normal sewing across patches, LOD between different powers or partial edges, corner neighbors, subdivision, removed triangles |
| Overlays | — | Not started |

## Next

**Editor:** free-form flat partitions, inward extrusion, per-piece texture editing, terrain clipping and authoring, model rendering and broader map checks.

**Compiler:** tighter visibility, bounced light and HDR, broader physics collision, terrain sewing, prop lighting, areaportals and overlays.
