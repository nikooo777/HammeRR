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
| Viewing and selection | `view_modes`, `saved_views`, `volume_selection`, `paint_selection`, `map_statistics`, `pointfiles` |
| Brushes and transforms | `geometry_tools`, `texture_lock_modes`, `brush_reflection`, `brush_shear`, `transform_display`, `precision_cycles` |
| Hollow, carve and clip | `hollow`, `carving`, `carve_many`, `clip_handles` |
| Vertices, edges and faces | `components`, `component_sources`, `component_picking`, `component_topology`, `component_transforms`, `component_creases`, `weld_texture_attachment`, `split_texture_attachment`, `roof_texture_attachment` |
| Flat partitions | `component_partitions`, `partition_picking`, `saved_partitions`, `partition_material_metadata`, `partition_materials`, `partition_textures`, `adjusted_partition_ridges`, `boundary_partitions`, `polygon_partitions`, `partition_extrusion`, `partition_visibility` |
| Materials and textures | `face_lighting`, `material_replacement`, `material_favorites`, `surface_group`, `texture_strokes`, `texture_chain` |
| Map checks | `spawn_clearance`, `audit_fixes` |
| Compiler | `compile_jobs`, `authored_planes`, `visibility`, `portal_flow`, `parallel_visibility`, `hints`, `sky_visibility`, `origin_brushes`, `cubemap_samples`, `water_metadata`, `direct_lighting`, `bounced_lighting`, `brush_smoothing`, `smoothing_angle`, `spotlights`, `direct_sky`, `shadow_index`, `engine_physics`, `physics_surfaces`, `static_props`, `terrain`, `terrain_creation`, `terrain_normals` |
| In game | `native_runtime`, `brush_runtime`, `vphysics_backend` |

### Extra options

- **`--url http://127.0.0.1:7377/mcp`** runs the check against an open editor instead of a private one. The editor must be idle, with a clean map and empty undo history. The script opens its fixture, then restores the original file, camera, selection and view. Supported by `clip_handles`, `compile_jobs`, `component_picking`, `components`, `face_lighting`, `geometry_tools`, `hollow`, `map_statistics`, `material_replacement`, `paint_selection`, `pointfiles`, `saved_views`, `surface_group`, `texture_chain`, `texture_strokes`, `view_modes` and `volume_selection`.
- **`--render`** adds offscreen GPU pixel checks without opening a window. Supported by `adjusted_partition_ridges`, `boundary_partitions`, `brush_reflection`, `brush_shear`, `carve_many`, `carving`, `component_creases`, `component_partitions`, `component_topology`, `component_transforms`, `partition_extrusion`, `partition_materials`, `partition_picking`, `partition_textures`, `polygon_partitions`, `roof_texture_attachment`, `saved_partitions`, `split_texture_attachment`, `texture_lock_modes` and `weld_texture_attachment`.
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
| `terrain_sewing_collision --runtime` | Shared seam and alpha sewing, exact cancel/undo, native neighbor metadata and build pairs; independent physics/player/ray toggle records, dynamic-object support, player support and trace behavior. |
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
