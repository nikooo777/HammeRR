[← Manual](README.md)

# Status and roadmap

HammeRR is in active development.

- **The editor** is usable today for building, texturing and wiring up brush-based maps, by hand or with an AI agent.
- **The compiler is experimental.** Compiled maps load and play in Counter-Strike: Source. Fast and Detailed visibility are available; lighting covers direct light, sky ambient and up to four optional diffuse bounces; HDR remains unfinished.

## Editor

| Area | State |
| --- | --- |
| Opening and saving VMFs | Works. Imported properties and extra blocks are retained; saves increment the map version and use standard VMF formatting. Round trips through other editors haven't been verified yet. |
| Recent maps | Up to twenty paths retain compatible build settings, with shared reopen, remove and clear controls. Game content and custom FGD files do not switch automatically; automatic relocation and coordinated writes from multiple editors are unavailable. |
| Object groups | Native nested groups for world brushes and whole entities support membership/name/parent edits, group selection, temporary bypass and Undo. Imported entity-owned child-brush memberships are preserved but cannot be edited; malformed group graphs cannot be authored. |
| Brushes, transforms and shapes | Works. |
| Polygon brushes | Convex and simple concave outlines, grid/exact corners, axis-aligned extrusion, materials and shared previews/Undo work. Oblique drawing planes and holes are planned. |
| Surface placement | Shared floor/ceiling and picked-surface previews, mesh/terrain contact, normal alignment, offsets and one-step Undo work. Independent object settling, overlap resolution and physics placement are planned. |
| Repeated placement | Fixed world translation/rotation arrays share previews, unique generated names, checked internal-reference remapping and one-step Undo. Uncertain internal links and named source sets with scripts or dynamic I/O reject; local/scale arrays and surface sprinkle are unavailable. |
| Hollow, carve and clip | Works for brushes. Terrain is rejected. |
| Merge brushes | Guarded previews join 2–16 brushes into one convex brush, retaining outer face settings and one-step Undo. World brushes or children of one brush entity can merge; gaps, concave unions, mixed owners and terrain reject. |
| Vertices, edges and faces | Brush component editing works. Terrain has separate vertex controls. |
| Flat partitions | Works. Inward extrusion, free-form dividing lines and per-piece texture axes, fit and justification are planned. |
| Materials and textures | Works. |
| 2D sky preview | Six mounted static LDR faces render through authored sky surfaces in perspective Textured views, with shared controls and load diagnostics. Axis-aligned texture scale/translation and unequal face sizes work. 3D sky-camera regions, HDR, fog, exposure, animation and compiled visibility fidelity are unavailable. |
| Entities, connections and renaming | Works. |
| Map checks | Geometry, IDs, textures, entity logic, asset paths, terrain metadata and authored spawn clearance against static brushes/world/detail terrain. Models, compiler LOD changes, runtime placement, objectives, full dependencies and engine limits are planned. |
| Terrain (displacements) | Creation, chosen corners, resolution edits, sculpting, blend paint, full/half-edge and corner-only sewing across grid resolutions and collision/border-shading controls and per-triangle walkability/buildability work with guarded previews and Undo. Viewport center picking and framing follow displaced terrain. Native triangle subsets retain checks across surfaces, with viewport picks, yellow outlines and framing. Retained triangle unions or exact checked vertices can mask sculpting and blend edits, with cyan vertex guides, picking, framing and box selection. Volume/marquee matching follows actual sheets, and volume boxes select native triangle subsets. Indexed paint/texture strokes also pick displaced surfaces instead of parent boxes. Compatible mixed-power full and whole-to-half edges compile with native LOD masks and snapping. Live sculpt/blend strokes share bounded paths, masks, radius guides, owned review and one-step Undo. Exact world XYZ vertex movement and viewport axis/plane/screen handles share owned previews, masks, grid snapping and Undo. Connected full-edge base-cage subdivision generates editable rounded grids. Half-edge cage subdivision, other partial spans and restricted-topology remapping remain planned. |
| Models | Searchable folder/used groups, isolated model/skin previews and one-step Apply/Cancel share Session/MCP. Static meshes and checked reference poses draw with materials, skins, Source origins/angles, selection and hiding; inspection/reload, mesh picking, bounds-based marquee/volume matching, object reports, framing and local transform bounds work. Named bodygroups share previews, atomic Apply/Undo and selectable helpers for blank choices. Local attachment names and reference-pose world positions/axes have shared inspection, owned guides and framing; animated/virtual attachments and attachment parenting remain planned. Animation playback, procedural/IK poses, scaling and opacity/render-mode overrides remain planned. Dynamic-model RGB tints share color controls, viewport/MCP rendering and alpha-preserving Undo; colored static props are rejected by the current writer. |
| Sounds | Mounted WAV/MP3 search, folder/script/used groups, manifest sound-script metadata and wave variants, isolated audition/volume, agent audio export and atomic sound-field choices work through shared Session/MCP operations. Game mixing, soundscapes, voice/sentence synthesis, map-specific overrides and ambiguous script resolution are planned. |

## Compiler

| Feature | Works | Not yet |
| --- | --- | --- |
| Geometry and collision | BSP version 20, with checked brush planes and collision bevels | — |
| Build settings | Lighting, physics, visibility and sky quality available before the first build; shared with agents and frozen during builds | — |
| Build recipes | Fast preview and Final lighting presets, up to 32 persistent named custom settings, shared create/edit/delete/apply controls and recipe identity captured at build start | Arbitrary commands, extra or reordered compiler stages and output-path recipes; the compiler's existing limits still apply |
| Visibility | Fast conservative sets or Detailed portal separator clipping with bounded parallel portal-facing rows and source flow; authored finite hint priority and remaining open-leaf cuts; visible/audible sets, sky visibility, areaportals, distance-fading windows and checked native single-brush occluder polygons/indices/areas | Occluder runtime culling and activation transitions, broader occluder geometry and broader map coverage |
| Lighting | Fullbright; Direct LDR for static, switchable and animated point/spot lights, one sun and sky ambient; directional normal-map lightmaps, soft sun, Fast/Balanced/Detailed sampling, percentage falloff, opaque brush/terrain/static-prop mesh shadows across direct, sky, bounced and ambient rays, terrain, pivot-aware direct brush-entity maps with self shadows, scoped world/model brush groups or optional angle smoothing, including partial-edge junctions and up to four style-preserving diffuse world/terrain-source bounces into world/model receivers; conservative point/spot visibility and exact-range culling | Adaptive sampling, HDR, transparent shadows, second-normal-map-only materials, model relighting/reflected sources and baked prop lightmaps |
| Model ambient | LDR cubes from filtered local world/terrain lightmaps and sky with opaque static-prop mesh occlusion, with up to 1/4/8 safe probes per open leaf by quality; legacy and indexed LDR/HDR data preservation | Adaptive refinement, switching ambient cubes, probes below terrain sheets |
| Physics | World, clips, brush entities, water and terrain, using CS:S's own collision library; world/terrain triangle surface properties | Model surface assignments, density and compiler-derived masses, broader collision with moving bodies |
| Brush entities | Explicit and origin-brush pivots; sliding and rotating doors, rotating brushes and triggers | Broader behavior/contact coverage and independently rotated initial lighting poses |
| Water | Flat world water, fog regions, floating objects | Depth and below-water materials, currents, water in detail brushes or brush entities |
| Cubemaps | Sample positions and sizes | Baked reflections |
| Static props | Models, transforms, collision, skins, fading, named lighting origins; bounded [opaque LOD0 mesh shadows](prop-shadows.md) with native disabled-shadow flags, selected-material warnings and checked asset failures | Texture-alpha/translucent shadows, per-vertex/per-texel prop lighting, reflected prop sources, animated-prop shadows and exact placement in the BSP tree |
| Terrain | World/func_detail power 2/3/4 surfaces, collision, matching full-edge and corner-only links, full/half-edge LOD masks/snapping, saved subdivided grids, authored border shading and shared vertex normals | Other fractional spans, half-edge cage subdivision, removed triangles |
| Overlays | Imported `info_overlay` projections on world, detail and terrain surfaces; materials, render order, distance fading and named accessors | Placement, corner editing, viewport projection, geometry-edit rebinding, brush-model targets, water transitions and decals |

## Next

**Editor:** shared cut/copy/paste with safe cross-document transfer, free-form flat partitions, inward extrusion, per-piece texture editing, terrain clipping and authoring, broader model formats and poses and broader map checks.

**Compiler:** HDR, broader physics collision, broader terrain topology, prop lighting, brush-model overlays and water transitions.
