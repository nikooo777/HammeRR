[← Manual](README.md)

# Development

## Project layout

| Path | Contents |
| --- | --- |
| `crates/source-formats` | Valve file formats (VMF, BSP, VPK, VMT, VTF, FGD, MDL, KeyValues) and the game filesystem. |
| `crates/map-core` | Geometry, terrain, the undoable map document, edits and picking. |
| `crates/render` | The wgpu renderer, materials, camera and offscreen snapshots. |
| `crates/editor` | The Slint UI, the shared editing session and the MCP server. |
| `crates/compiler` | The native VMF-to-BSP compiler, `hammerr-compile` and the `hammerr-physics` helper. |
| `scripts/` | End-to-end validation scripts. |
| `docs/` | This manual and its screenshots. |

The editor UI and the MCP server call the same session operations, so every feature behaves identically for people and agents. New features need both a UI control and an MCP tool.

## Build and test

```sh
cargo build --release -p editor -p compiler
cargo test --workspace
```

Builds use a lot of memory. On smaller machines, limit parallel jobs with `-j`.

### Build settings UI

```sh
cargo test -p editor compile_controls_share_cancel_status_and_guard_replaced_jobs
```

This software-rendered check opens **Build settings…** from a fresh session, changes lighting, physics and sky quality before a build, and checks cancellation and job replacement. It uses a private testing backend and sends no desktop input. Set `HAMMERR_UI_SNAPSHOTS` to an existing directory to save its window captures.

### Native BSP formats

```sh
cargo test -p source-formats --test ambient --test areas --test overlays --test bsp
```

Checks embedded, dense legacy and indexed LDR/HDR ambient cubes, leaf aliases, sample positions, native area/areaportal/clip geometry, overlay projections/fades and malformed records. Overlay checks include packed render order, zero-face records, inactive face slots, negative fade sentinels and a bounded 1,024-record payload. The BSP test checks installed CS:S maps when available; use `CSS_DIR` to choose another installation. These format checks run without the editor or game.

### Compiler face provenance

```sh
cargo test -p compiler --test face_provenance
cargo test -p compiler --lib faces::tests
```

Checks source side IDs through merged and subdivided faces, shared metadata, hidden source exclusion and the compiler's bounded provenance work budget.

### Authored overlays

```sh
cargo test -p compiler --test overlays
python3 -B scripts/validate_overlays.py --output /tmp/hammerr-overlays-check
```

Checks imported projection fields, overlay-aware face merging, final native face indices, terrain bindings, fade packing, accessors, hidden sources and previous-output preservation. The MCP validator compares foreground/background builds and preserved source/history; `--runtime` adds private CS:S rendering, distance fade and named-accessor checks.

## Validation scripts

The `scripts/validate_<name>.py` scripts drive the editor through MCP and check features end to end. They need only Python 3.

```sh
cargo build -p editor -p compiler
python -B scripts/validate_hollow.py --output /tmp/hammerr-hollow-check
```

- By default each script starts its own `target/debug/editor --headless`. Use `--binary` to choose another build.
- Results go to `--output`: fixture maps, renders where relevant, and a JSON report. Use a new directory for each run.

| Area | Scripts |
| --- | --- |
| Viewing and selection | `view_modes`, `saved_views`, `sky_preview`, `volume_selection`, `paint_selection`, `map_statistics`, `pointfiles` |
| Brushes and transforms | `geometry_tools`, `texture_lock_modes`, `brush_reflection`, `brush_shear`, `transform_display`, `precision_cycles`, `polygon_brushes`, `surface_placement`, `repeat_array` |
| Hollow, carve, merge and clip | `hollow`, `carving`, `carve_many`, `merge`, `clip_handles` |
| Vertices, edges and faces | `components`, `component_sources`, `component_picking`, `component_topology`, `component_transforms`, `component_creases`, `weld_texture_attachment`, `split_texture_attachment`, `roof_texture_attachment` |
| Flat partitions | `component_partitions`, `partition_picking`, `saved_partitions`, `partition_material_metadata`, `partition_materials`, `partition_textures`, `adjusted_partition_ridges`, `boundary_partitions`, `polygon_partitions`, `partition_extrusion`, `partition_visibility` |
| Materials and textures | `face_lighting`, `material_replacement`, `material_favorites`, `surface_group`, `texture_strokes`, `texture_chain` |
| Map checks | `spawn_clearance`, `audit_fixes` |
| Local preferences | `recent_maps`, `build_recipes` |
| Compiler | `compile_jobs`, `authored_planes`, `visibility`, `portal_flow`, `parallel_visibility`, `hints`, `sky_visibility`, `origin_brushes`, `cubemap_samples`, `water_metadata`, `direct_lighting`, `bounced_lighting`, `brush_smoothing`, `smoothing_angle`, `spotlights`, `direct_sky`, `shadow_index`, `engine_physics`, `physics_surfaces`, `static_props`, `terrain`, `terrain_creation`, `terrain_normals` |
| In game | `native_runtime`, `brush_runtime`, `vphysics_backend` |

### Extra options

- **`--url http://127.0.0.1:7377/mcp`** runs the check against an open editor instead of a private one. The editor must be idle, with a clean map and empty undo history. The script opens its fixture, then restores the original file, camera, selection and view. Supported by `clip_handles`, `compile_jobs`, `component_picking`, `components`, `face_lighting`, `geometry_tools`, `hollow`, `map_statistics`, `material_replacement`, `paint_selection`, `pointfiles`, `saved_views`, `sky_preview`, `surface_group`, `texture_chain`, `texture_strokes`, `view_modes` and `volume_selection`.
- **`--render`** adds offscreen GPU pixel checks without opening a window. Supported by `adjusted_partition_ridges`, `boundary_partitions`, `brush_reflection`, `brush_shear`, `carve_many`, `carving`, `component_creases`, `component_partitions`, `component_topology`, `component_transforms`, `merge`, `partition_extrusion`, `partition_materials`, `partition_picking`, `partition_textures`, `polygon_partitions`, `roof_texture_attachment`, `saved_partitions`, `split_texture_attachment`, `texture_lock_modes` and `weld_texture_attachment`.
- **`precision_cycles --operation scale|rotate|reflection|shear`** checks repeated edit/save/reopen cycles for an operation other than translation. Drift must stay below `1e-5` units and `1e-4` texels, and undo/redo must be exact.
- **`partition_extrusion --adjusted`** extrudes pieces from a moved dividing line.
- **`water_metadata --physics engine`** also checks fluid bodies.
- **`shadow_index --reference <dir>`** compares the BSP and lighting work against an earlier run's output directory.
- **`direct_sky --reference <dir>`** compares zero-ambient BSPs against an earlier compatible run. **`--quality-tests`** checks every sampling quality, settings frozen during a build, and a narrow sky opening. **`--soft-sun-tests`** checks clear, enclosed, brush-shadow and terrain-shadow cases at every quality through both build paths. **`--local-ambient-tests`** checks probe counts, spatial separation and model ambient on a point-light gradient at all three qualities while direct world lightmaps stay unchanged.

### Lighting and terrain checks

Use the existing debug build to run these headless checks; rebuilding between scripts is unnecessary:

```sh
python3 -B scripts/validate_direct_lighting.py --output /tmp/hammerr-falloff-check
python3 -B scripts/validate_direct_sky.py --quality-tests --output /tmp/hammerr-sky-check
python3 -B scripts/validate_terrain.py --output /tmp/hammerr-terrain-check
python3 -B scripts/validate_terrain_normals.py --output /tmp/hammerr-normals-check
python3 -B scripts/validate_terrain_seams.py --output /tmp/hammerr-seams-check
python3 -B scripts/validate_areaportals.py --output /tmp/hammerr-areas-check
python3 -B scripts/validate_window_portals.py --output /tmp/hammerr-windows-check
python3 -B scripts/validate_overlay_capacity.py --output /tmp/hammerr-overlay-capacity-check
python3 -B scripts/validate_portal_flow.py --output /tmp/hammerr-portal-flow-check
python3 -B scripts/validate_hints.py --output /tmp/hammerr-hints-check
python3 -B scripts/validate_parallel_visibility.py --output /tmp/hammerr-parallel-check
python3 -B scripts/validate_bounced_lighting.py --output /tmp/hammerr-bounce-check
python3 -B scripts/validate_bounced_lighting.py --model-receiver --output /tmp/hammerr-model-bounce-check
python3 -B scripts/validate_brush_smoothing.py --output /tmp/hammerr-smoothing-check
python3 -B scripts/validate_smoothing_angle.py --output /tmp/hammerr-angle-check
python3 -B scripts/validate_brush_junctions.py --output /tmp/hammerr-junction-check
python3 -B scripts/validate_light_styles.py --output /tmp/hammerr-styles-check
python3 -B scripts/validate_model_smoothing.py --output /tmp/hammerr-model-smoothing-check
python3 -B scripts/validate_light_culling.py --output /tmp/hammerr-light-culling-check
python3 -B scripts/validate_bump_lighting.py --output /tmp/hammerr-bump-check
python3 -B scripts/validate_physics_surfaces.py --output /tmp/hammerr-physics-surfaces-check
python3 -B scripts/validate_terrain_creation.py --output /tmp/hammerr-terrain-creation-check
```

`direct_lighting` covers legacy and percentage-distance point and spotlight falloff, including hard fades. `direct_sky` covers sun and ambient visibility, opaque occlusion, shared sampling settings and compiled ambient-cube records and counters. `terrain` covers power-2/3/4 lightmaps and matching full-edge neighbors. `terrain_seams` checks rotated curved seams, hidden-neighbor exclusion, exact foreground/background output and unchanged native geometry/physics. `terrain_normals` checks that baking and viewport shading use the same smooth patch normals while preserving collision geometry.

`light_culling` checks shared counts and exact foreground/background builds in connected/disconnected rooms with finite-range named lights and fractional brush-model pivots. Compiler reference regressions compare complete BSP bytes with culling disabled, including terrain, styles and both visibility modes.

## In-game checks

These scripts compile fixtures through MCP and run Counter-Strike: Source with `-insecure` on a private headless display. They need [Cage](https://github.com/cage-kiosk/cage) and [grim](https://sr.ht/~emersion/grim/). Maps, config, scripts and logs stay in private folders, and no mouse or keyboard input reaches your desktop.

```sh
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 \
  python3 -B scripts/validate_native_runtime.py
```

| Script | Checks |
| --- | --- |
| `bump_lighting --runtime` | Shared material/face authoring, native base/directional map order, equal base samples, frozen jobs and exact builds; private flat/tilted normal textures check engine brightness and directional shading. |
| `native_runtime` | Player spawn and teleport, baked red lightmaps, a physics crate with and without collision bodies. `--fluid` adds a pool and checks that the crate floats. |
| `brush_runtime` | Sliding and rotating doors, a continuously rotating brush, trigger outputs, pivot-aware model lightmaps, exact shared builds, and unchanged geometry/collision. |
| `physics_surfaces --runtime` | Shared face materials and rotated mixed-surface hulls, checked native material tables, exact foreground/background builds and unchanged source/history; paired ice/rubber contact on the same incline. |
| `static_props --runtime` | Static prop collision settings. |
| `terrain_corners --runtime` | Reciprocal native corner-only records, rotated start indices, mixed powers, exact shared builds, hidden omission and private native loading/contact. |
| `terrain_queries` | Raised/draft terrain ray and pixel hits, visible bounds and matching draft/committed framing, read-only history, picked-point brush placement and imported-entity cleanup/undo. |
| `terrain_sewing_topology --runtime` | MCP-authored mixed grids and half-edge junctions, one-time averaging and fine border/alpha interpolation, owned previews, invalid updates/cancellation, preserved interiors, exact Undo and repeated no-op edits, save/reopen, exact native builds and seam/player/prop contact. |
| `terrain_sewing_collision --runtime` | Shared seam and alpha sewing, exact cancel/undo, native neighbor metadata and build pairs; independent physics/player/ray toggle records, dynamic-object support, player support and trace behavior. |
| `detail_terrain --runtime` | Consumed func_detail power-2/3/4 mixed/half terrain, native detail contents, preserved grids/tags/source/history, exact MCP builds, hidden owners, rejected runtime owners and private seam/player/prop contact. |
| `terrain_clearance` | Private synthetic-content MCP checks for actual terrain sheets, excluded parent volumes/previews, player flags and visibility, uncertain materials, malformed-angle issue/repair, precise unsupported owners and preserved source state. |
| `terrain_smoothing --runtime` | Shared border rules, analytic hard/Automatic samples, two-sided angle limits, preview/cancel and invalid-update retention, malformed-field repair, native fields, preserved geometry/physics, exact restored defaults and foreground/background builds, paired native seam shading and grounding. |
| `subdivided_terrain [--reference /path/to/map.bsp]` | Imported subdivided markers on curved power-2/3/4 grids, exact native foreground/background bytes compared with the same explicit rows, marker save/reopen, unchanged source/history and preserved editor cache guards. An optional reference also requires identical bytes to a separately validated BSP. |
| `terrain_lod --runtime [--split-edge]` | Rotated power-2/4 full edges, native masks and geometry snapping, reduced contact meshes, exact MCP builds, unchanged source/history, hidden/mismatched neighbors and private near/far rendering with seam ray heights and player/prop contact. `--split-edge` adds a three-patch full/half junction, both native slots, independently hidden neighbors and coupled LOD masks. |
| `terrain_brushes --runtime` | MCP-authored terrain, radial raise/lower, flatten/smoothing cancellation, deterministic noise, atomic invalid updates, blend paint/fill/invert and preserved geometry, save/reopen, exact native builds, player/prop contact and paired native two-texture endpoints. |
| `terrain_editing --runtime` | Shared resolution previews and full grid inspection, asymmetric heights/blends, shared samples, cancellation, atomic rejection, coarsening/undo, save/reopen and exact native foreground/background builds, with contact on the refined surface. |
| `terrain_creation --runtime` | Shared quad conversion at powers 2/3/4 and all four queried start corners, checked native start positions, atomic invalid creation/removal batches, restored-parent compilation and exact grid undo, saved source/selection and exact foreground/background builds, offscreen grid rendering and native contact at the created surface height. |
| `terrain --runtime` | Terrain with uneven heights, power-4 and mixed-size lighting, player collision, ray flags and moving objects. `--runtime-cases power-four mixed` limits the game checks to those two fixtures; `--runtime-cases neighbors` runs the linked-edge fixture. |
| `areaportals --runtime` | Native area/portal bindings, hidden and invalid boundaries, exact MCP build output, and visible-area/client portal bits through close/reopen from both sides. |
| `window_portals --runtime` | Rendered and foreground brush models, distance closing, fade-distance inputs, and server/client visibility bits. |
| `overlays --runtime` | Authored graffiti rendering against a control scene, distance fading and the named accessor's native overlay ID. |
| `portal_flow --runtime` | Fast/Detailed settings, PVS/PAS rows, independent clear rays, exact MCP builds and paired native rendering through staggered openings. |
| `hints --runtime` | Shared face/material authoring, retained initial hint priority and finite scope, unchanged physics payload, atomic invalid-helper rejection, paired rendering and grounding inside/across the helper. |
| `smoothing_angle --runtime` | Zero/below/boundary angles on a cylinder, shared settings and frozen/stale jobs, exact MCP builds and unchanged collision, with paired native shading. |
| `model_smoothing --runtime` | Model-local angle thresholds, shared settings and exact builds, unchanged collision/source/history, paired curved-model shading and native rotation. |
| `light_styles --runtime [--bounced-model --bounces 1|2|3|4]` | Named channels and averages, shared relay authoring, exact MCP builds and atomic failures; native Initially dark, TurnOn, TurnOff, custom-pattern brightness and alternating animation. `--bounced-model` checks reflected named light on a shadowed model, including the same native switches and patterns across one through four chosen bounces. |
| `brush_junctions --runtime` | Partial-edge normals, shared smooth/hard masks, exact MCP builds and unchanged source/history/collision, with paired native shading. |
| `brush_smoothing --runtime` | Shared face mask authoring, smooth and hard-group normals, native short indices, exact MCP builds and unchanged collision, with paired native shading. |
| `bounced_lighting --runtime [--model-receiver]` | Shared zero/one-bounce settings, reflected color beneath a shelf, exact MCP builds and unchanged geometry/collision, with paired native rendering. `--model-receiver` adds pivot-aware brush-model receivers and native top-face samples. |
| `parallel_visibility --runtime [--visibility fast|detailed]` | An authored 10×10 hint grid in Fast or Detailed mode, all-cell PVS/PAS, deterministic worker work and exact shared builds, with native grounding/rendering. |
| `overlay_capacity --runtime` | Current-engine record capacity: 1,024 native records with only the last one drawing, checked against the same scene without overlays. This patches a private MCP-built BSP to isolate capacity; `overlays` covers authored helper compilation. |
| `terrain_seams --runtime` | Rotated curved neighbors share continuous baked lighting and retain seam contact heights. |
| `terrain_normals --runtime` | A curved terrain ridge with smooth normals, shared with the viewport, keeps its collision height in game. |

Pass `--game-dir` if CS:S isn't in `~/.local/share/Steam/steamapps/common/Counter-Strike Source`.

### Physics library probe

```sh
python3 -B scripts/validate_vphysics_backend.py \
  --game-dir "/path/to/Counter-Strike Source"
```

Checks the installed CS:S Linux64 **VPhysicsCollision007** library that engine physics builds rely on: convex construction, serialization, compound geometry and solid loading. It runs in a timed child process with a 24 GiB address-space limit, rejects library builds it doesn't recognize, and doesn't launch the game.

### Terrain triangle checks

```sh
python3 -B scripts/validate_terrain_triangles.py --runtime --output /tmp/hammerr-terrain-triangles-check
```

Checks native triangle order at powers 2–4, arbitrary subset classification overrides, source/BSP bit conversion, invalid-batch preservation, Cancel, Undo/Redo, save/reopen, automatic restoration and exact foreground/background builds. Other BSP lump payloads must stay unchanged. `--runtime` loads the classified map and checks player/prop contact in a private CS:S display with `-insecure`. The editor's `terrain_triangle_controls_preserve_owned_cancel_and_apply_subsets` test exercises the software-rendered dialog without desktop input; `HAMMERR_UI_SNAPSHOTS` saves its capture.

To recheck the current CS:S removal limitation, reuse a flat engine-physics BSP produced by `terrain_lod`:

```sh
python3 -B scripts/validate_terrain_removal_probe.py --bsp /tmp/hammerr-terrain-lod-check/flat.bsp --output /tmp/hammerr-removal-probe-check
```

This private runtime probe adds the native removal bit to every terrain triangle while retaining all other data, then compares the render, ray hits and player positions against the original. A changed engine behavior requires reviewing hole support before changing the compiler.

```sh
python3 -B scripts/validate_terrain_triangle_picking.py --output /tmp/hammerr-terrain-picking-check
```

Checks native ray picks at powers2–4, normalized directions, draft geometry, offscreen highlighted/cleared renders, invalid-batch preservation, replaced highlight tokens, changed-power guards, framing, hidden detail owners, mixed tool faces and occlusion. Source/history remain unchanged during the read/view operations. The software UI test `terrain_triangle_batch_controls_pick_frame_and_keep_foreign_guides` covers retained surface subsets, viewport picking cancellation, batch Apply/Undo and foreign guide ownership.

```sh
python3 -B scripts/validate_terrain_vertex_masks.py --output /tmp/hammerr-terrain-masks-check
```

Checks deduplicated masks across power2/3/4 grids, exact unselected geometry/alpha, untouched faces, owned previews/Cancel, mask reuse after Apply, replaced-token failures preserving a valid draft, saved source and native geometry/alpha through exact foreground/background builds. The software UI check `terrain_mask_controls_retain_selection_and_confine_sculpt_and_blend` covers **Use selection**, mask counts, sculpt/fill previews, Cancel and Undo. Core tests check destination-only writes and immutable-neighbor smoothing.

```sh
python3 -B scripts/validate_terrain_regions.py --output /tmp/hammerr-terrain-regions-check
```

Checks elevated-sheet volume matches in object/face modes, excluded parent volumes/faces, native triangle clipping/containment, atomic box-based highlights/masks, replaced/canceled tokens, side-view marquee height matching, source/history and hidden-owner/tool filtering. Core region tests include interior peaks beyond contained corners. The software UI test `terrain_volume_control_selects_triangles_without_changing_the_map` covers **Select in volume** and **Use selection**.

```sh
python3 -B scripts/validate_indexed_terrain.py --output /tmp/hammerr-indexed-terrain-check
```

Checks power2/3/4 paint strokes against raised sheets, direct-pick parity, excluded parent-volume hits, object/face scopes, Cancel, owner selection and hidden owners. Source/history remain unchanged during stroke previews. Core picking tests also compare displaced/offset sheets outside parent bounds and preserve ordinary brush/entity behavior; the existing `paint_selection` script checks the broader stroke workflow.

```sh
python3 -B scripts/validate_terrain_corner_sewing.py --output /tmp/hammerr-terrain-corner-sew-check
```

Checks mixed-power rotated corner-only contacts, one original-value height/alpha average, untouched vertices, owned preview/Cancel, Undo/Redo/reopen, exact repeated no-ops, reciprocal native corner links and exact foreground/background builds. The existing `terrain_sewing_topology` check covers full/half borders; core sewing tests verify the shared junction average and metadata preservation.

```sh
python3 -B scripts/validate_terrain_vertices.py --output /tmp/hammerr-terrain-vertices-check
```

Checks exact vertex IDs and masks on rotated power-2/3/4 grids, single-sample sculpt/blend writes, picking, framing, guarded box selection, unknown arguments, stale topology, selection replacement, Cancel/Undo/Redo/reopen and native sample agreement through both build paths. Existing triangle picking and mask scripts cover the other selection kind; software UI tests exercise the selection-mode switch and brush flow.

```sh
python3 -B scripts/validate_terrain_strokes.py --output /tmp/hammerr-terrain-strokes-check
```

Checks whole/grouped paths at powers 2–4 for raise, flatten, smooth, noise and blend paint, unchanged source/history during preview, stationary pointers, ray-miss segment breaks, invalid update limits, explicit vertex masks, Apply/Cancel, one-step Undo, foreign preview preservation, saved rows and native sample agreement through exact foreground/background builds. Core sampling checks include turns and movement along the brush direction; software UI tests drive the viewport-stroke lifecycle and existing controls.

```sh
python3 -B scripts/validate_terrain_vertex_moves.py --output /tmp/hammerr-terrain-vertex-moves-check
```

Checks exact lateral/vertical sample moves on rotated powers 2–4, untouched neighbors/alpha/flags, zero no-ops, invalid-coordinate and unknown-argument guards, preview/source/history ownership, masks and triangle unions, Cancel/Undo/Redo/reopen, native XYZ reconstruction and exact foreground/background builds. Core tests cover metadata/absent rows; software UI drives the exact-offset controls. The stroke check also confirms that stationary updates retain the existing preview token.

```sh
python3 -B scripts/validate_terrain_handles.py --output /tmp/hammerr-terrain-handles-check
```

Checks all seven numeric handle constraints on rotated powers 2–4, shared snapping, unchanged neighbors/source/history, invalid updates, pointer hit-testing and resized viewport guards, owned review, one-step Undo, foreign preview preservation and save/reopen. Session tests cover axis/plane/screen pointer movement in all four projections; software UI tests exercise compact resizing, handle review and silent cancellation. The numeric vertex-move check covers native XYZ output from the shared preview operation.

```sh
python3 -B scripts/validate_terrain_subdivision.py --output /tmp/hammerr-terrain-subdivision-check
python3 -B scripts/validate_subdivided_terrain.py --output /tmp/hammerr-subdivided-compatibility-check
```

Checks connected folded power-2/3/4 cages, shared samples and pinned base boundaries, preserved metadata, repeated-generation no-ops, invalid previews, generated-grid editing, Cancel/Undo/Redo/reopen and active native positions with LOD snapping through both build paths. The saved-grid check compares marked/unmarked native output and exercises supported sewing/resizing on imported marked rows. Core checks cover subdivision stencils, manifold/half-edge rejection and actual cache guards; software UI drives the operation and Cancel.

```sh
python3 -B scripts/validate_model_previews.py --output /tmp/hammerr-model-previews-check
cargo test -p source-formats --test css decodes_css_ -- --nocapture
```

Checks CPU model inspection, invalid paths/skins/arguments, three real model meshes, model-aware area/volume contained/intersecting rules, object/query response schemas and bounds, pixel picks beyond helper bounds, skin-render differences, draft transforms, cache reload, helper fallbacks, source/history preservation, native static-prop positions/angles/skins through both builds and save/reopen. Decoder tests cover fixups, mesh-relative indices, triangle strips, checksums, malformed ranges and nonfinite data. Bounded-read tests reject oversized directory/VPK entries before allocation. Software UI checks inspection, reload and Cancel. The CSS format check uses the local install (`CSS_DIR` overrides its location).

```sh
python3 -B scripts/validate_model_browser.py --output /tmp/hammerr-model-browser-check
```

Checks catalog search/groups/pages, isolated model/skin image changes, unchanged active scene/camera/selection/history, atomic model+skin Undo/Redo, stale and replaced choices, foreign geometry preview preservation, native static-prop fields through both compile paths and save/reopen. Shared Session and software UI checks cover empty model fields, pose validation and Cancel ownership.

The model browser check also covers three real skeletal/weapon reference poses, compatible entity choices, static-prop rejection without replacing prior BSP output, native dynamic-model paths, owned Apply/Undo and save/reopen. Decoder checks cover parent chains, inverse binds, weighted positions/normals and measured float-weight tolerance.

The browser check includes Kleiner’s blank/visible clipboard bodygroup, integer mapping, isolated-image differences, active/resumed/stale choice queries, one-step model/skin/body Undo and native body-zero rejection preserving prior output. Core fixtures cover an entirely blank body, signed relative tables and malformed groups; renderer checks retain helpers for blank choices. Software UI checks mirror agent updates and reject an Apply that has not yet displayed the current choice.

```sh
python3 -B scripts/validate_model_attachments.py --output /tmp/hammerr-model-attachments-check
```

Checks real named attachment positions under Source entity rotation, shared guide renders and clearing, source/history preservation, camera framing, invalid/stale/foreign tokens, hidden entities, geometry-preview guards and save/reopen. Format and renderer fixtures independently check parent composition without inverse binds and native world-aligned orientation. The software model-details test exercises attachment choice, guides, framing, reload and ownership-preserving Cancel.

### Sound exploration

```sh
python3 -B scripts/validate_sounds.py --output /tmp/hammerr-sounds-check
```

Requires FFmpeg/ffplay. Uses `SDL_AUDIODRIVER=dummy` for silent playback checks; it does not use the desktop or speakers. Checks real WAV/MP3 files and manifest wave variants, PCM export and volume amplitude, ten-second bounds, playback ownership, invalid/stale/replaced choices, unchanged source/history/camera, Apply/Undo/reopen and native entity sound names through both compile paths. Core fixtures cover malformed manifests, ambiguous names, nesting/path/chunk guards, oversized-file rejection before allocation and cached draft reads with fresh Apply validation. The software UI check exercises search, variants, shared volume, resumed/changed choices, Apply and foreign Cancel.

```sh
python3 -B scripts/validate_model_tints.py --output /tmp/hammerr-model-tints-check
```

Checks real model RGB image changes, common color-control metadata, fourth-component preservation, invalid intensity, Undo/Redo/reopen, native dynamic rendercolor fields through both build paths and colored-static rejection preserving prior BSP output. Renderer checks cover exact vertex modulation and legible selection for black models; shared Session checks keep native alpha separate from light brightness.

```sh
python3 -B scripts/validate_surface_placement.py --output /tmp/hammerr-surface-placement-check
```

Checks shared floor/ceiling and ray placement, actual model support/contact and model targets, hidden-target filtering, terrain triangle normals, helper fallback, source/history/camera preservation, failed-preview retention, stale/foreign ownership, Cancel/Undo/Redo/reopen and native entity transforms through identical foreground/background builds. Core checks cover brush/terrain/model normals; software UI checks drive picking, changed-settings review, Apply and foreign-preview Cancel without a desktop.

```sh
python3 -B scripts/validate_polygon_brushes.py --output /tmp/hammerr-polygon-brushes-check
```

Checks concave extrusion, unchanged source/history/camera during preview, invalid-outline retention, fractional axes, shared snapping, replacement ownership, Cancel/Undo/Redo/reopen and identical foreground/background native builds. Core checks cover both windings, translated outlines, exact volume/concavity and invalid intersections; software UI drives the pad, exact controls, review and foreign Cancel. Existing primitive geometry and controls share regression coverage.

### Convex brush merging

```sh
cargo test -p map-core merge::
cargo test -p editor merge
python3 -B scripts/validate_merge.py --binary target/debug/editor --output /tmp/hammerr-merge-check --render
```

Checks complete convex unions, deterministic outer-face/metadata inheritance, world/entity ownership, unrelated children, gap/concavity/cavity rejection, preview replacement, exact Undo/Redo and VMF reopen. The headless MCP script also checks fractional tiled unions and compiles a sealed room with a merged floor. `--render` adds preview/Apply pixel agreement and restoration after Discard. The software UI test `merge_controls_keep_sources_and_share_preview_discard_cancel_apply` writes `brush-merging.png` when `HAMMERR_UI_SNAPSHOTS` names an existing directory.

### Repeated placement arrays

```sh
cargo test -p map-core repeat::tests
cargo test -p editor session::repeat::tests
python3 -B scripts/validate_repeat_array.py --binary target/debug/editor --output /tmp/hammerr-repeat-array-check
```

Checks nth-copy transforms from immutable originals, generated IDs/names, supported internal references, external-link preservation, copied notes and bounds. The private headless MCP script includes offscreen renders, source/history/camera preservation, invalid/stale tokens, competing previews, Undo/Redo and VMF save/reopen. The software UI test `repeat_ui::tests::repeat_controls_preview_apply_cancel_and_preserve_foreign_drafts` covers previewed-setting review and exact Cancel ownership; `HAMMERR_UI_SNAPSHOTS` saves `repeat-array.png`.

### 2D sky preview

```sh
cargo test -p render sky::tests
cargo test -p render surface_and_sky_shaders_parse_and_validate_without_a_gpu
cargo test -p render sky::tests -- --ignored
cargo test -p editor sky_preview_tracks_world_settings_and_undo_without_view_history
python3 -B scripts/validate_sky_preview.py --binary target/debug/editor --gameinfo /path/to/cstrike/gameinfo.txt --sky sky_dust --output /tmp/hammerr-sky-preview-check
```

CPU tests check six-face loading, VMT patches, dimensions, LDR selection, supported transforms and reported failures. The explicitly ignored GPU tests check cardinal/quadrant orientation, camera translation, depth/occlusion, fallback and unchanged view modes/filters. The MCP script always performs offscreen image checks, temporary and shared view settings, picking, world-setting Undo and save/reopen. Repeat with `--sky sky_day01_09` to cover stock vertical scaling. `--url` uses the preserved-map workflow described above; private headless sessions are the default.

### Recent maps and build recipes

```sh
cargo test -p editor recent_maps::tests
cargo test -p editor session::recent::tests
cargo test -p editor session::compilation::recipes::tests
python3 -B scripts/validate_recent_maps.py --binary target/debug/editor --output /tmp/hammerr-recent-maps-check
python3 -B scripts/validate_build_recipes.py --binary target/debug/editor --output /tmp/hammerr-build-recipes-check
```

Recent-map checks use isolated preferences and multiple private stdio processes to cover MRU ordering/eviction, canonical aliases, missing/malformed files, dirty refusal, primary and auxiliary write failures, compatible six-field build context, incompatible content, restart and storage repair. Remove/clear preserve map files and Undo. The script can omit `--output` to create a fresh temporary directory.

Recipe checks cover immutable presets, bounded custom CRUD/apply, stale revisions, manual-value provenance invalidation, active/cancelling job guards, frozen completed-job names, output freshness, corrupt/oversized storage, conflict detection and atomic failure/retry. The headless script isolates recipe/recovery storage and verifies native compilation plus persistence across restart. Its output directory must not already exist; optional `--gameinfo` chooses a fixture game installation. Rebuild the editor before script validation when code changed; Cargo tests alone do not refresh the executable.

Software UI tests `recent_map_controls_share_reopen_and_guard_discard_remove_clear_and_cancel` and `recipe_controls_share_crud_apply_cancel_and_job_guards` exercise the same Session operations and write `recent-maps.png` and `build-recipes.png` under `HAMMERR_UI_SNAPSHOTS`. They use the private testing backend without desktop input. These preference checks do not launch the game.

### Prop shadows and native occluders

```sh
python3 -B scripts/validate_prop_shadows.py --binary target/debug/editor --output /tmp/hammerr-prop-shadows-check
python3 -B scripts/validate_occluders.py --binary target/debug/editor --output /tmp/hammerr-occluders-check
```

The prop fixture mounts synthetic bounded model companions, compares actual BSP light samples through both compile paths, checks shadow flags and selected-material warnings, and verifies malformed-asset failure preserves output/source/history. Compiler `prop_shadows`, `direct_lighting`, `static_props` and `bounced_lighting` integration tests cover lighting interaction and probe relocation outside a closed prop; `prop_occlusion` unit tests check before/on/behind-source indirect rays. Source-format checks cover bounded patch-material includes.

The occluder fixture checks native polygon planes/global vertices, dense entity indices, active state, metadata preservation, foreground/background equality, hidden helpers and atomic rejection of unsupported geometry. Source-format `occlusion` and compiler `occluders` integration tests cover malformed/versioned records, empty native sentinels, area ambiguity, pruned-solid adjacency and world bounds. These checks do not establish in-game culling or input-transition behavior.

### Clipboard transfer

```sh
cargo test -p source-formats --lib bounded_tests
cargo test -p map-core --lib transfer::tests
cargo test -p editor clipboard
python3 -B scripts/validate_clipboard.py --binary target/debug/editor --output /tmp/hammerr-clipboard-check
python3 -B scripts/validate_clipboard_native.py --binary target/debug/editor --output /tmp/hammerr-clipboard-native-check
```

Core checks cover detached source/destination identity collisions, exact internal references, external policies, texture placement, groups, owned-brush notes, terrain, native round trips and atomic history. Parser checks bound nesting/nodes before typed conversion. Session/UI checks cover publication failure before Cut, unchanged-map name receipts, invalid import retention, stale/foreign previews and parameter review; `HAMMERR_UI_SNAPSHOTS` writes `clipboard-paste.png` through the software testing backend.

The default validator uses two private stdio Sessions and exported text, with source replacement, offscreen preview/Apply/Cancel agreement, malformed payloads, metadata, save/reopen and identical foreground/background compilation. It never reads or writes the system clipboard. The native wrapper starts a separate Cage headless Wayland compositor with a private runtime directory, removes inherited display connections and runs the same transfer with explicit native publication/read. Its output directory must be new. It requires Cage and leaves desktop focus, input and the user's clipboard untouched; it does not launch the game.

### Occluder runtime fixture

```sh
python3 -B scripts/validate_occluder_runtime.py --binary target/debug/editor --output /tmp/hammerr-occluder-fixture
python3 -B scripts/validate_occluder_runtime.py --binary target/debug/editor --output /tmp/hammerr-occluder-client --runtime
python3 -B scripts/validate_occluder_runtime.py --binary target/debug/editor --output /tmp/hammerr-occluder-inactive --runtime --start-inactive
python3 -B scripts/validate_occluder_runtime.py --binary target/debug/editor --output /tmp/hammerr-occluder-islands --runtime --multi-brush
```

The default checks a sealed lit fixture, native occluder records and identical foreground/background builds. `--runtime` launches CS:S with `-insecure` and `sv_lan 1` on a private headless Cage display, and confirms the post-map LAN value. This installed client still requires Steam running to spawn a local player. `--start-inactive` checks the native inactive flag and inverse sequence. `--multi-brush` checks separated strips sharing one index, with side-prop culling and an unchanged center-gap prop. It observes initial/Deactivate/Activate/Toggle netprops and compares stable prop-culling frames. Server loading alone does not establish client culling. Use a fresh output directory for each run.


### Native layers and clipboard memberships

```sh
cargo test -p map-core layers::tests
cargo test -p editor session::layers::tests
cargo test -p editor layers_ui::tests
python3 -B scripts/validate_layers.py --binary target/debug/editor --output /tmp/hammerr-layers-check
```

Core checks cover nested record/member transactions, reparenting/deletion,
unknown metadata, independent ID allocation, rollback/Undo, owned-brush links
and clipboard remapping. Session and software UI checks cover stale versions,
parameter review, cancel ownership, visibility and transfer. Set
`HAMMERR_UI_SNAPSHOTS` to write `layers.png` without desktop input. The MCP script
uses private stdio sessions for hierarchy, multiple memberships, owned-brush
selection, hidden/compiler scope, stale requests, cross-map clipboard transfer
and native save/reopen. Use a new output directory; it does not launch the game
or touch the desktop clipboard.


### Moving brush contact

```sh
python3 -B scripts/validate_moving_contact.py --binary target/debug/editor --output /tmp/hammerr-moving-contact-check
python3 -B scripts/validate_moving_contact.py --binary target/debug/editor --output /tmp/hammerr-moving-contact-client --runtime
```

The default authors a linear platform, physics crate and endpoint connections
through MCP, checks native model counts and exact foreground/background builds,
and preserves VMF/source/history. `--runtime` uses the shared private Cage helper
with -insecure and sv_lan 1. It checks three stationary samples in five phases,
vertical contact through Open/Close and changed-speed SetPosition cycles, endpoint
outputs and the grounded player. Use a fresh output directory; the probe uses no
desktop input or focus changes.


### Brush-model physics surfaces

```sh
cargo test -p compiler --lib physics::tests
cargo test -p compiler --test physics
python3 -B scripts/validate_physics_surfaces.py --binary target/debug/editor --output /tmp/hammerr-model-surfaces-check --brush-model
python3 -B scripts/validate_physics_surfaces.py --binary target/debug/editor --output /tmp/hammerr-model-surfaces-client --brush-model --runtime
python3 -B scripts/validate_physics_surfaces.py --binary target/debug/editor --output /tmp/hammerr-mixed-model-surfaces-client --mixed-model --runtime
```

The model variants assign one VMT property across a tilted `func_brush` or only
to its top face (`--mixed-model`), check
native body key data and distinct property counts, and compare exact shared
builds while preserving source/history. Native mode uses a private Cage display,
`-insecure` and `sv_lan 1` to compare ice/rubber crate motion. The ordinary world
variant retains per-face mappings. Compiler regressions cover default collision
bytes, uniform keys, mixed triangle mappings, safe name bounds, invalid body
roles and native serialization/reload. Use a fresh output directory per run.

### Prefab library

```sh
mkdir -p /tmp/hammerr-prefab-ui
HAMMERR_UI_SNAPSHOTS=/tmp/hammerr-prefab-ui cargo test -p editor --bin editor prefabs
python3 -B scripts/validate_prefabs.py --binary target/debug/editor --output /tmp/hammerr-prefab-check
```

Storage checks cover atomic unique creation, filename scope, stale metadata,
symlink rejection and preservation of unrelated files. Session/software UI checks
cover source preservation, capture/preview/Apply/Cancel, stale source requests,
failed load/placement retention and deletion review; the UI capture is
`prefab-library.png`. The MCP script uses private Sessions and a private library
for cross-map IDs/links/groups/layers/notes, exact centered placement, Undo/Redo,
native save/reopen, restart, malformed files and deletion after loading a detached
draft. Use a fresh output directory. These checks use no desktop input, native
clipboard or game launch.
