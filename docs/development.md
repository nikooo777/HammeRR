[← Manual](README.md)

# Development

## Project layout

| Path | Contents |
| --- | --- |
| `crates/source-formats` | Valve file formats (VMF, BSP, VPK, VMT, VTF, FGD, MDL, DMX/PCF, KeyValues) and the game filesystem. |
| `crates/map-core` | Geometry, terrain, the undoable map document, edits and picking. |
| `crates/render` | The wgpu renderer, materials, camera and offscreen snapshots. |
| `crates/editor-core` | The shared editing session, application runtime, optional MCP server and `hammerr-headless`. No Slint dependency. |
| `crates/editor-ui` | Generated Slint windows and rows; UI definitions remain in `crates/editor/ui/`. |
| `crates/editor` | UI controllers, desktop startup and software UI tests. |
| `crates/compiler` | The native VMF-to-BSP compiler, `hammerr-compile` and the `hammerr-physics` helper. |
| `crates/native-runtime` | Supervised private CS:S capture, runtime identity and preservation checks, and `hammerr-native-runtime`. |
| `scripts/` | End-to-end validation scripts. |
| `docs/` | This manual and its screenshots. |

The editor UI and the MCP server call the same session operations, so every feature behaves identically for people and agents. New features need both a UI control and an MCP tool.

Shared session locking and compile/game workers live in `editor-core/src/runtime.rs`; object-ID parsing and formatting live in `identity.rs`. Both executables use `startup::Application` for content mounting, recovery and shutdown. MCP tool groups and parameters live under `editor-core/src/mcp/`, while desktop composition and feature bindings live under `editor/src/ui/` and the feature-specific UI modules.

`Session` owns geometry-preview state through `session/previews.rs`. Rendering, picking and tool-texture filtering share its displayed-document selector. Feature modules validate replacement drafts before clearing competitors through the coordinator; their own payloads and ownership tokens remain local.

## Build and test

```sh
cargo build --release -p editor -p editor-core -p compiler -p native-runtime
cargo test --workspace
```

Builds use a lot of memory. On smaller machines, limit parallel jobs with `-j`.

For application changes that do not need the UI, build and test without compiling Slint:

```sh
cargo test -p editor-core --no-default-features --lib
cargo test -p editor-core --features mcp --lib
cargo build -p editor-core --features mcp --bin hammerr-headless
```

Development and test builds leave the generated `editor-ui` crate unoptimized
to reduce compilation work. Slint's testing metadata stays enabled, other
package settings keep their existing optimization levels, and release builds
retain full UI optimization.

The optional `test-support` feature exposes shared fixtures for desktop tests. It is not needed to run either program. `editor --headless` remains compatible with the standalone target.

On the development desktop, serialize Cargo work with eight jobs and a 24 GiB memory limit:

```sh
mkdir -p target
systemd-run --user --scope --quiet -p MemoryMax=24G -p MemorySwapMax=0 \
  env CARGO_BUILD_JOBS=8 flock target/hammerr-cargo.lock \
  cargo test -p editor-core --no-default-features --lib
```

### Content paths and safe saves

```sh
cargo test -p source-formats --lib gamefs::tests
cargo test -p editor-core recovery::tests
cargo test -p editor-core recipe_persistence_roundtrips_and_retries_failed_writes_atomically
cargo test -p map-core --lib displacement::
```

Checks relative and linked gameinfo paths, mount precedence, save links, destination errors, retained unsaved state, recovery backups and file permissions. Terrain checks cover the shared mesh and authoring guards through sculpting, vertex moves, sewing, subdivision and resolution changes. These run without desktop input or a game launch.

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

### Asset names and physics keys

```sh
cargo test -p source-formats --test malformed_assets --test static_props --test physics
```

Checks truncated VPK trees, UTF-8 names, terminated model names with opaque padding, physics key terminators and retained opaque key bytes. Rejected static-prop writes preserve existing game lumps.

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
cargo build -p editor-core -p compiler --features editor-core/mcp
python3 -B scripts/validate_headless.py --output /tmp/hammerr-headless-check
```

- Scripts share `DEFAULT_BINARY` in `validate_face_lighting.py` and start a private `target/debug/hammerr-headless` session. Use `--binary` to choose another build, including `target/debug/editor` for desktop compatibility.
- Results go to `--output`: fixture maps, renders where relevant, and a JSON report. Use a new directory for each run.

| Area | Scripts |
| --- | --- |
| Application startup and shutdown | `headless` (edit/history, save/reopen, compile, lazy graphics and exit recovery; `--desktop-binary` compares both executable tool inventories, and `--baseline` compares a saved `tools/list` array sorted by name) |
| Viewing and selection | `view_modes`, `saved_views`, `sky_preview`, `volume_selection`, `paint_selection`, `object_picking`, `working_region`, `fog_preview`, `map_statistics`, `pointfiles` |
| Brushes and transforms | `geometry_tools`, `texture_lock_modes`, `brush_reflection`, `brush_shear`, `transform_display`, `precision_cycles`, `polygon_brushes`, `surface_placement`, `repeat_array` |
| Hollow, carve, merge and clip | `hollow`, `carving`, `carve_many`, `merge`, `clip_handles` |
| Vertices, edges and faces | `components`, `component_sources`, `component_picking`, `component_topology`, `component_transforms`, `component_creases`, `weld_texture_attachment`, `split_texture_attachment`, `roof_texture_attachment` |
| Flat partitions | `component_partitions`, `partition_picking`, `saved_partitions`, `partition_material_metadata`, `partition_materials`, `partition_textures`, `adjusted_partition_ridges`, `boundary_partitions`, `polygon_partitions`, `partition_extrusion`, `partition_visibility` |
| Materials and textures | `face_lighting`, `material_replacement`, `material_favorites`, `material_reload`, `surface_group`, `texture_strokes`, `texture_chain`, `uv_vectors` |
| Map checks | `spawn_clearance`, `audit_fixes` |
| Shared authoring | `fgd_size_helpers`, `measurement_picking`, `prefab_thumbnails`, `linked_paths`, `overlay_corners` |
| Particle definitions | `particles` (active manifest discovery, bounded binary DMX parsing, metadata, owned choices and retryable errors) |
| Local preferences | `recent_maps`, `build_recipes` |
| Compiler | `compile_jobs`, `authored_planes`, `visibility`, `portal_flow`, `parallel_visibility`, `hints`, `sky_visibility`, `origin_brushes`, `cubemap_samples`, `reflection_settings`, `cubemap_reflections`, `water_metadata`, `water_bottom`, `water_bottom_native`, `water_overlays`, `static_prop_leaves`, `direct_lighting`, `hdr_lighting`, `bounced_lighting`, `brush_smoothing`, `smoothing_angle`, `spotlights`, `direct_sky`, `shadow_index`, `engine_physics`, `physics_surfaces`, `static_props`, `terrain`, `terrain_creation`, `terrain_normals`, `terrain_subdivision` |
| In game | `prop_shadows_native` and `hdr_lighting` (optional `--runtime`), `native_runtime`, `brush_runtime`, `vphysics_backend` |

`transform_display` also checks `preview_snap_to_grid`: fractional negative world bounds-minimum anchors, mixed selections, shared texture-lock policy, source/history preservation, Apply/Cancel, aligned no-ops, model-reload cancellation and exact typed transforms with snapping enabled.

`material_reload` edits private mounted VMT/VTF fixtures and checks explicit cache
refresh, persistent offscreen GPU uploads, thumbnails, added/deleted catalog
entries, failure diagnostics, stale-texture replacement and repair, with unchanged
source and Undo history:

```sh
python3 -B scripts/validate_material_reload.py --binary target/debug/hammerr-headless --output /tmp/hammerr-material-reload-check
```

`object_picking` checks depth order, exact world distances for non-unit rays, stateless cycling and wrapping, source/history/camera preservation, misses, mode/preview guards, visibility, brush owners, nested groups, bypass and nearest-64 truncation. Add `--render` for a private viewport example:

```sh
python3 -B scripts/validate_object_picking.py --binary target/debug/hammerr-headless --output /tmp/hammerr-object-picking-check --render
```

### Extra options

- **`--url http://127.0.0.1:7377/mcp`** runs the check against an open editor instead of a private one. The editor must be idle, with a clean map and empty undo history. The script opens its fixture, then restores the original file, camera, selection and view. Supported by `clip_handles`, `compile_jobs`, `component_picking`, `components`, `face_lighting`, `geometry_tools`, `hollow`, `map_statistics`, `material_replacement`, `paint_selection`, `pointfiles`, `saved_views`, `sky_preview`, `surface_group`, `texture_chain`, `texture_strokes`, `uv_vectors`, `view_modes` and `volume_selection`.
- **`--render`** adds offscreen GPU pixel checks without opening a window. Supported by `adjusted_partition_ridges`, `boundary_partitions`, `brush_reflection`, `brush_shear`, `carve_many`, `carving`, `component_creases`, `component_partitions`, `component_topology`, `component_transforms`, `merge`, `partition_extrusion`, `partition_materials`, `partition_picking`, `partition_textures`, `polygon_partitions`, `roof_texture_attachment`, `saved_partitions`, `split_texture_attachment`, `texture_lock_modes` and `weld_texture_attachment`.
- **`precision_cycles --operation scale|rotate|reflection|shear`** checks repeated edit/save/reopen cycles for an operation other than translation. Drift must stay below `1e-5` units and `1e-4` texels, and undo/redo must be exact.
- **`partition_extrusion --adjusted`** extrudes pieces from a moved dividing line.
  The same script checks inward pockets and through cuts, retained source faces,
  fragment ownership, saved-plan consumption, picking and exact Apply/Undo.
  Use `--render` to compare preview and committed geometry offscreen.
- **`water_metadata --physics engine`** also checks fluid bodies.
- **`shadow_index --reference <dir>`** compares the BSP and lighting work against an earlier run's output directory.
- **`direct_sky --reference <dir>`** compares zero-ambient BSPs against an earlier compatible run. **`--quality-tests`** checks every sampling quality, settings frozen during a build, and a narrow sky opening. **`--soft-sun-tests`** checks clear, enclosed, brush-shadow and terrain-shadow cases at every quality through both build paths. **`--local-ambient-tests`** checks probe counts, spatial separation and model ambient on a point-light gradient at all three qualities while direct world lightmaps stay unchanged.

### Lighting and terrain checks

Typed face-channel checks cover separate LDR/HDR styles and sample offsets,
known native layouts, bounded allocations and unchanged legacy LDR bytes.

```sh
cargo test -p source-formats --test lighting --test bsp --test overlays
cargo test -p compiler --test direct_lighting --test bounced_lighting --test bump_lighting --test light_styles
```

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

### Model detail props

```sh
cargo test -p source-formats --lib bsp::detail_props::tests
cargo test -p compiler --test detail_props
cargo test -p compiler --test region_detail_models
cargo test -p compiler --lib named_model_receiver
python3 -B scripts/validate_detail_props.py --output /tmp/hammerr-detail-props-check
python3 -B scripts/validate_detail_props.py --output /tmp/hammerr-detail-props-regions-check --regions
python3 -B scripts/validate_detail_props.py --output /tmp/hammerr-detail-props-native-check --runtime
python3 -B scripts/validate_detail_origins.py --binary target/debug/hammerr-headless --output /tmp/hammerr-detail-origins-check
python3 -B scripts/validate_material_details.py --output /tmp/hammerr-material-details-check --runtime
```

The MCP check covers native model records, defaults, hidden omission, rejected
overrides, saved source and history, and identical foreground/background output.
`--runtime` additionally compares detail rendering enabled and disabled in a
private installed-engine launch, with read-only source assets and configuration
preservation. It needs the installed CS:S game, SDK FGD, running Steam and private
compositor dependencies. Override `--game-dir`, `--fgd` or `--port` as needed.

`--regions` checks explicit model detail inclusion, outside omission, retained
outside lighting helpers and unchanged retained records. It compares
foreground/background builds and rejected-input publication guards without a
native launch. Compiler regressions cover inclusive bounds, the implicit zero
origin, hidden omission and the normal asset, renderability, bounds, open-leaf
and lighting-receiver checks. Selected origins on sealing boundaries are not
nudged into open space.

`validate_detail_origins` authors named `info_lighting` helpers for explicit
models through the existing property calls. It checks sampled colors with
unchanged placement, reference errors and preserved source/history/output
through foreground/background builds. It uses isolated stdio MCP without a
native launch; `--game-dir` and `--fgd` override the installed asset paths.
Compiler regressions also cover the unchanged stored-up normal, Fullbright
reference validation, ambiguous helpers and region-exterior lit receivers.

`--runtime-bsp <path> --runtime --expected-count <n>` reuses this native check for
an already checked BSP containing the same observer entity; `--fixture-game`
copies its private materials. This mode checks records and runtime rendering,
and explicitly does not repeat authored-source validation.

`validate_material_details` authors a ground material/dictionary fixture through
the existing world and face operations, checks deterministic generated records,
rejections and source/history preservation, and optionally runs the same native
detail ON/OFF observer. The native occluder check accepts
`validate_occluder_runtime.py --union touching|overlapping --runtime`; add
`--start-inactive` for the inverse initial state. Each run needs a fresh output
directory and uses the shared private launch guards.

### Direct UV vectors

```sh
cargo test -p map-core --lib surface::vectors_tests
cargo test -p editor-core --features mcp --lib direct_vector
cargo test -p editor --bin editor direct_vector_dialog
cargo test -p compiler --test direct_uv_vectors
python3 -B scripts/validate_uv_vectors.py --binary target/debug/hammerr-headless --output /tmp/hammerr-uv-vectors-check
```

Checks skewed texel coordinates, partial vector updates across world and owned
faces, preserved metadata, atomic invalid batches, shared preview ownership,
Cancel/Undo/Redo and VMF round trips. The compiler target checks native world
and brush-model texture vectors and origin adjustment. The MCP script always
renders its draft and cancelled state offscreen; it has no `--render` switch.
Use `--gameinfo /path/to/cstrike/gameinfo.txt` for an explicit content mount or
`--url http://127.0.0.1:7377/mcp` for a clean idle editor. The software UI test
writes `uv-vectors.png` when `HAMMERR_UI_SNAPSHOTS` names an existing directory.

### Independent LDR and HDR lighting

```sh
cargo test -p source-formats --test lighting
cargo test -p compiler --lib lighting::input_tests
cargo test -p compiler --lib lighting::sky::tests
cargo test -p compiler --test hdr_lighting --test lighting_reference
cargo test -p editor-core --features mcp --lib build_lighting_settings_are_shared
cargo test -p editor --bin editor hdr_results_keep_channels
python3 -B scripts/validate_hdr_lighting.py --binary target/debug/hammerr-headless --output /tmp/hammerr-hdr-lighting-check
```

Checks selected HDR tuples, overrides, sentinels and scales; independent face
styles, sample offsets, bumped maps, world lights, bounces and ambient cubes;
LDR byte preservation; aggregate work/allocation guards; and atomic failed or
cancelled publication. The MCP fixture uses different channel colors and HDR
radiance above 255, decodes both channels independently, compares synchronous
and background output, and checks source/history preservation. The compiler
integration target also covers terrain, brush models and incomplete native HDR
data without synthetic emitters or probes. Eligibility metadata alone does not
establish the runtime's selected channel.

Build the private read-only native observer before adding `--runtime`:

```sh
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 \
  cc -std=c11 -O2 -Wall -Wextra -Werror -fPIC -shared scripts/hdr_runtime_probe.c \
  -o target/debug/hammerr_hdr_probe.so -ldl -pthread
python3 -B scripts/validate_hdr_lighting.py --binary target/debug/hammerr-headless --runtime --output /tmp/hammerr-hdr-lighting-client
```

Use `--probe-library /path/to/hammerr_hdr_probe.so` for another observer build
and `--game-dir "/path/to/Counter-Strike Source"` for another installation.
Native validation requires Cage, grim, a compatible fingerprinted playable
engine and Steam running. It selects HDR level 0 or 2 before each fresh private
map load, retains `-insecure` and post-map `sv_lan 1`, and verifies fixed
exposure/bloom cvars. The observer acquires `MaterialSystemHardwareConfig012`
and requires consistent `GetHDREnabled` and `GetHDRType` readings before
capturing the red/blue comparison. `LD_PRELOAD` applies only to the private game
command. Reports include acquisition routes, hardware values, module hashes
and frames; this check uses no desktop input or focus changes. Use a fresh
output directory per run. `--runtime-bsp /path/to/hdr.bsp` with
`--fixture-game /path/to/source-game/cstrike --runtime` repeats native checks
against an existing fixture without recompiling it.

### Rotated static-prop leaves and model mass

```sh
cargo build -p compiler
cargo test -p compiler --lib static_props::leaf_box
cargo test -p compiler --test static_props --test physics --test physics_mass
python3 -B scripts/validate_static_prop_leaves.py --binary target/debug/hammerr-headless --output /tmp/hammerr-prop-leaves-check
```

The prop check compares clipped native leaf membership against an independent
transformed box, sampled containment and its former broad-phase bounds, while
requiring identical foreground/background builds and unchanged source/history.
Mass checks use mounted definitions and native collision volumes to cover solid,
shell and mixed-material masses, omitted collision brushes, invalid inputs and
retained output. Build the matching `hammerr-physics` worker before running them.

### Imported water overlays

```sh
cargo test -p source-formats --test water_overlays --test overlays
cargo test -p compiler --test water_overlays --test overlays --test region_overlays
python3 -B scripts/validate_water_overlays.py --binary target/debug/hammerr-headless --output /tmp/hammerr-water-overlays-check
python3 -B scripts/validate_water_overlays.py --binary target/debug/hammerr-headless --runtime --output /tmp/hammerr-water-overlays-client
```

Checks the native water-record layout, final source-face bindings, regular fade
indexing, imported blocks, hidden helpers, region inclusion, source/history and
atomic failures. Native mode compares the transition with its absent control
on separate private displays, requiring a visible projection and no runtime
accessor. The fixture uses a graffiti material on a floor to isolate transition
record loading; water-edge appearance remains material-dependent.

### Water underside materials

```sh
cargo test -p compiler --test water_bottom --test water
python3 -B scripts/validate_water_bottom.py --binary target/debug/hammerr-headless --output /tmp/hammerr-water-bottom-check
python3 -B scripts/validate_water_bottom.py --binary target/debug/hammerr-headless --vertical-sides --output /tmp/hammerr-water-side-check
python3 -B scripts/validate_water_metadata.py --binary target/debug/hammerr-headless --physics engine --output /tmp/hammerr-water-metadata-check
```

Checks patched `$bottommaterial` resolution, separate top/underside bindings,
copied texture vectors and flags, winding, wet-leaf fog references, clipped
surface area, unchanged depth/collision data and overlay exclusion from generated
inward faces.
Missing or malformed explicit bottom assets must preserve the old output;
repair must restore deterministic synchronous/background builds without source
or history edits. `--vertical-sides` also checks side-specific bottom assets,
separate top-material regions, wet/dry leaf bindings and omitted internal water
boundaries. `water_bottom` uses private mounted assets and has no runtime switch.
`water_metadata --physics engine` additionally checks native fluid-body output;
these commands inspect compiled data without rendering it in CS:S.

```sh
python3 -B scripts/validate_water_bottom_native.py --binary target/debug/hammerr-headless --output /tmp/hammerr-water-bottom-native-compile
python3 -B scripts/validate_water_bottom_native.py --binary target/debug/hammerr-headless --runtime --reuse-fixture /tmp/hammerr-water-bottom-native-compile/compile.json --output /tmp/hammerr-water-bottom-native-client
python3 -B scripts/validate_water_bottom_native.py --binary target/debug/hammerr-headless --vertical-sides --output /tmp/hammerr-water-side-native-compile
python3 -B scripts/validate_water_bottom_native.py --binary target/debug/hammerr-headless --vertical-sides --runtime --reuse-fixture /tmp/hammerr-water-side-native-compile/compile.json --output /tmp/hammerr-water-side-native-client
```

The native pair mounts identical runtime material trees and changes only the
compiled underside binding. It requires submerged eye poses, red/blue underside
pixels, stable above/away controls, exact collision bytes and unchanged sources.
`--vertical-sides` keeps the horizontal underside neutral and changes only the
compiled vertical inward material. Both modes use constant refraction textures
through the Water shader to identify the bound surface material; they do not
measure natural reflection or refraction quality.
Output directories must be fresh. The saved compile fixture avoids rebuilding
for a repeated native observation; use the same `--vertical-sides` mode when
reusing it.

### Linked path authoring

```sh
cargo test -p editor-core --features mcp --lib linked_path
cargo test -p editor --bin editor linked_path_window_
python3 -B scripts/validate_linked_paths.py --binary target/debug/hammerr-headless --output /tmp/hammerr-linked-paths-check
```

Checks both path classes and open/closed chains, inherited FGD defaults,
bounded points and safe names, reviewed/stale tokens, source/history
preservation, one Undo/Redo and exact saved/reopened links and IDs. The MCP
validator launches no compiler or game; User/Agent ownership is exercised by
the core and software UI checks. Set `HAMMERR_UI_SNAPSHOTS` to save the dialog.
Existing-path tests additionally insert/remove/reorder/reposition nodes while
keeping surviving IDs, flags, custom values, I/O and external train references.
They check head removal with an explicit seed, safe inserted names and fresh IDs,
FGD defaults only on new nodes, outside-reference/I/O removal guards, atomic
failure and exact Undo/Redo/save-reopen identities. Ambiguous graphs reject and
unchanged paths add no history. The software callbacks
capture `linked-path-existing.png` alongside the creation dialog.

### Generated reflections

```sh
cargo test -p source-formats --test pak
cargo test -p native-runtime --lib
cargo test -p native-runtime --lib isolation::tests::readonly_sources_and_private_writes_retain_host_child_identity -- --ignored --exact --nocapture
cargo test -p compiler --lib cubemap_assets::
cargo test -p editor-core --features mcp --lib reflection_
cargo test -p editor reflection_controls_toggle_review_cancel_and_freeze_with_the_shared_job -- --test-threads=1
cargo test -p editor cubemap_window_keeps_cached_inspection_and_requires_current_owned_review -- --test-threads=1
cargo build -p compiler -p native-runtime -p editor-core --features editor-core/mcp --bins
python3 -B scripts/validate_reflection_settings.py --check-staleness --output /tmp/hammerr-reflection-settings-check
python3 -B scripts/validate_cubemap_reflections.py --output /tmp/hammerr-cubemap-reflections-check
python3 -B scripts/validate_hdr_reflections.py --binary target/debug/hammerr-headless \
  --probe-library target/debug/hammerr_hdr_probe.so --output /tmp/hammerr-hdr-reflections-check
```

The format and library checks cover bounded ZIP/PAK data, source provenance,
capture receipts, registered namespace rebasing, cancellation and publication
guards. Software UI checks exercise shared settings, runtime drafts and owned
binding review without desktop input; `HAMMERR_UI_SNAPSHOTS` saves their windows.
The software capture controls also exercise paired LDR/HDR generation, retained
generation when switching lit modes, atomic Fullbright rejection and the
`build-hdr-reflection-controls.png` snapshot. Runtime inspection reports supported
and selected channels separately from map eligibility. The reflection settings
fixture covers both lit modes without launching a capture client.
Set `HAMMERR_UI_CAPTURE_ENGINE_ROOT` to a supported running Steam installation
for a capture-runtime readiness screenshot. This only configures the runtime;
the software UI check never launches CS:S.
`reflection_settings` checks MCP settings and persistence; `--check-staleness`
also compiles a generation-disabled baseline to check freshness reporting.

`cubemap_reflections` requires the supported running Steam/CS:S installation
and a fresh absent output directory. It authors its own fixture through MCP,
captures and verifies complete native VTFs, and checks reflected pixels with
specular enabled and disabled. The ordinary Run path stages through a private
recording executable; a guarded native client then loads that exact staged BSP
and compares its appearance with the published map. It checks rejection of a
missing generated sample and preserves source assets and shared configuration.
Use `--game-dir` and `--steam-root` for nondefault installations. The native
client always uses the shared private launch and preservation guards below.
`hdr_reflections` uses the hardware observer built in the lighting checks above.
It verifies paired publication and Run rebasing, every mip's directional HDR RGB
against the raw capture, canonical unused fields, actual hardware channel
selection, reflected pixels and absence of HDR fallback. Use `--reuse-fixture`
with a completed fixture's `results.json` to repeat native views without another
capture.
The explicit ignored isolation fixture requires working Bubblewrap/user
namespaces. It checks denied source/cache writes, allowed private output and
the real host child PID used for module verification. Reflection capture and
its native view validator expose external files read-only.

### Terrain collision surfaces

```sh
cargo test -p compiler --lib physics::terrain::tests
cargo test -p compiler --test terrain_physics -- --nocapture
python3 -B scripts/validate_terrain_physics_surfaces.py --binary target/debug/hammerr-headless --output /tmp/hammerr-terrain-physics-surfaces-check
```

Checks actual emitted LOD triangles, strict stored-float blend thresholds,
primary/secondary surface indices, missing-property compatibility, invalid
definitions and native collision serialization/reload. The MCP validator paints
an inclined patch, saves and reopens it, compares foreground/background output
and checks unchanged geometry. It uses the native physics helper; it does not
launch a game or measure friction in a running client.

### Private launch guards and cubemap capture probe

```sh
python3 -B scripts/test_private_runtime_config.py
python3 -B scripts/validate_cubemap_capture_probe.py --binary target/debug/hammerr-headless --output /tmp/hammerr-cubemap-capture-probe --timeout 150
```

The headless regressions cover rejecting installed or symlinked output paths
before writes, imported config command checks, identity-preflight upload/log
guards, and timeout cleanup that retains both the original failure and
preservation errors. The capture probe checks
native capture and packing into a private staged map; it does not implement
compiler reflection bindings. It requires complete sample VTFs with directional
pixels, valid packed assets, unchanged non-PAK lumps and source files.

Native validators share one private compositor and launch profile. They require
running Steam, a synchronized existing CS:S Cloud config and fingerprinted
Linux64 engine/Steam interfaces. A read-only child queries the live account;
`-default` suppresses automatic configuration saves. Controls use `toggle` and
`help` after imported settings and before the first map. Each exit, including
failed or timed-out workers, checks shared config/spray bytes, Cloud preferences,
cache semantics and upload logs. Cloud reads into private files remain allowed;
no Cloud preference commands are issued. Reports retain these checks separately
from feature assertions.

## In-game checks

These scripts compile fixtures through MCP and run Counter-Strike: Source with `-insecure` on a private headless display. They need [Cage](https://github.com/cage-kiosk/cage) and [grim](https://sr.ht/~emersion/grim/). Generated maps, config, scripts and logs use private folders; the shared launch guard audits installed settings and Cloud state after exit. No mouse or keyboard input reaches your desktop.

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
cargo test -p map-core --lib displacement::subdivide
cargo test -p editor-core --features mcp --lib half_edge_subdivision
cargo test -p compiler --test terrain_subdivision --test terrain
python3 -B scripts/validate_terrain_subdivision.py --output /tmp/hammerr-terrain-subdivision-check
python3 -B scripts/validate_terrain_subdivision.py --split-edge --output /tmp/hammerr-terrain-subdivision-half-check
python3 -B scripts/validate_subdivided_terrain.py --output /tmp/hammerr-subdivided-compatibility-check
```

Checks connected folded power-2/3/4 cages, shared samples and pinned base boundaries, preserved metadata, repeated-generation no-ops, invalid previews, generated-grid editing, Cancel/Undo/Redo/reopen and active native positions with LOD snapping through both build paths. `--split-edge` uses a large edge joined to two smaller neighbors, checks both half-edge slots and repeats native builds after save/reopen; it does not launch the game. The saved-grid check compares marked/unmarked native output and exercises supported sewing/resizing on imported marked rows. Core checks cover subdivision stencils, full/half span combinations, two-density seed grids, unsupported density chains, malformed contacts, manifold guards and actual cache guards; software UI drives the operation and Cancel.

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

The same validator also checks a rotated fractional source corner, exact target
coincidence, unchanged committed source/history during preview, rejected options
preserving the token, repeated no-op Apply, Undo and shared corner pick/cancel.
Session regressions cover multi-child owner rigidity, texture policy,
deterministic target ties, visibility filters, unsupported/ambiguous targets and
whole-owner range rejection. Software UI exercises **Snap selected corner**,
captured source review, disabled alignment/offset controls and Apply/Cancel;
`HAMMERR_UI_SNAPSHOTS` writes `vertex-snap.png`.

```sh
python3 -B scripts/validate_polygon_brushes.py --output /tmp/hammerr-polygon-brushes-check
```

Checks concave extrusion, unchanged source/history/camera during preview, invalid-outline retention, fractional axes, shared snapping, replacement ownership, Cancel/Undo/Redo/reopen and identical foreground/background native builds. Core checks cover both windings, translated outlines, exact volume/concavity and invalid intersections; software UI drives the pad, exact controls, review and foreign Cancel. Existing primitive geometry and controls share regression coverage.

### Convex brush merging

```sh
cargo test -p map-core merge::
cargo test -p editor-core -p editor merge
python3 -B scripts/validate_merge.py --binary target/debug/hammerr-headless --output /tmp/hammerr-merge-check --render
```

Checks complete convex unions, deterministic outer-face/metadata inheritance, world/entity ownership, unrelated children, gap/concavity/cavity rejection, preview replacement, exact Undo/Redo and VMF reopen. The headless MCP script also checks fractional tiled unions and compiles a sealed room with a merged floor. `--render` adds preview/Apply pixel agreement and restoration after Discard. The software UI test `merge_controls_keep_sources_and_share_preview_discard_cancel_apply` writes `brush-merging.png` when `HAMMERR_UI_SNAPSHOTS` names an existing directory.

### Repeated placement arrays

```sh
cargo test -p map-core repeat::tests
cargo test -p editor-core session::repeat::tests
python3 -B scripts/validate_repeat_array.py --binary target/debug/hammerr-headless --output /tmp/hammerr-repeat-array-check
```

Checks nth-copy world/source-local transforms from immutable originals, fixed captured frames, malformed orientation rejection, explicit/default world parity, generated IDs/names, supported internal references, external-link preservation, copied notes and bounds. The private headless MCP script includes offscreen renders, source/history/camera preservation, invalid/stale tokens, competing previews, Undo/Redo and VMF save/reopen. The software UI test `repeat_ui::tests::repeat_controls_preview_apply_cancel_and_preserve_foreign_drafts` covers previewed-setting review and exact Cancel ownership; `HAMMERR_UI_SNAPSHOTS` saves `repeat-array.png` with the source-local controls.

### 2D sky preview

```sh
cargo test -p render sky::tests
cargo test -p render surface_and_sky_shaders_parse_and_validate_without_a_gpu
cargo test -p render sky::tests -- --ignored
cargo test -p editor-core sky_preview_tracks_world_settings_and_undo_without_view_history
python3 -B scripts/validate_sky_preview.py --binary target/debug/hammerr-headless --gameinfo /path/to/cstrike/gameinfo.txt --sky sky_dust --output /tmp/hammerr-sky-preview-check
```

CPU tests check six-face loading, VMT patches, dimensions, LDR selection, supported transforms and reported failures. The explicitly ignored GPU tests check cardinal/quadrant orientation, camera translation, depth/occlusion, fallback and unchanged view modes/filters. The MCP script always performs offscreen image checks, temporary and shared view settings, picking, world-setting Undo and save/reopen. Repeat with `--sky sky_day01_09` to cover stock vertical scaling. `--url` uses the preserved-map workflow described above; private headless sessions are the default.

### Sky asset inspection

```sh
cargo test -p editor-core sky_assets::tests
cargo test -p editor-core sky_inspection_tool_has_strict_empty_arguments_and_matches_read_only_session_report
cargo test -p editor sky_assets_ui::tests
python3 -B scripts/validate_sky_assets.py --binary target/debug/hammerr-headless --output /tmp/hammerr-sky-assets-check
```

Core tests cover exact family identity, separate LDR/HDR structure, bounded directory/VPK and Patch reads, malformed/missing assets and unsupported declarations without substituting families. The MCP test checks strict empty arguments and exact Session report parity while preserving an unapplied property draft. The private stdio fixture mounts synthetic LDR/HDR VTFs and material/Patch sources, checks directory/VPK provenance, malformed winning assets, compressed HDR diagnostics, missing faces and invalid names, and compares document/history/camera/selection/clipboard/drafts and mounted bytes around every inspection. It starts no renderer, compiler or game process.

The software UI test `world_sky_inspection_button_refresh_and_close_preserve_applied_map_and_pending_edits` invokes the actual **Inspect applied sky…**, **Refresh sky assets** and **Close** buttons. It verifies that a staged different skyname and a transform draft survive, that the report uses the applied name, and that both closing and a source repair leave the parent edit intact. `HAMMERR_UI_SNAPSHOTS` writes `sky-assets.png` through the offscreen testing backend.

### Recent maps and build recipes

```sh
cargo test -p editor-core recent_maps::tests
cargo test -p editor-core session::recent::tests
cargo test -p editor-core session::compilation::recipes::tests
python3 -B scripts/validate_recent_maps.py --binary target/debug/hammerr-headless --output /tmp/hammerr-recent-maps-check
python3 -B scripts/validate_build_recipes.py --binary target/debug/hammerr-headless --output /tmp/hammerr-build-recipes-check
```

Recent-map checks use isolated preferences and multiple private stdio processes to cover MRU ordering/eviction, canonical aliases, missing/malformed files, dirty refusal, primary and auxiliary write failures, compatible seven-field build context, incompatible content, restart and storage repair. Remove/clear preserve map files and Undo. The script can omit `--output` to create a fresh temporary directory.

Recipe checks cover immutable presets, bounded custom CRUD/apply, stale revisions, manual-value provenance invalidation, active/cancelling job guards, frozen completed-job names, output freshness, corrupt/oversized storage, conflict detection and atomic failure/retry. The headless script isolates recipe/recovery storage and verifies native compilation plus persistence across restart. Its output directory must not already exist; optional `--gameinfo` chooses a fixture game installation. Rebuild the selected executable before script validation when code changed; Cargo tests alone do not refresh the executable.

Software UI tests `recent_map_controls_share_reopen_and_guard_discard_remove_clear_and_cancel` and `recipe_controls_share_crud_apply_cancel_and_job_guards` exercise the same Session operations and write `recent-maps.png` and `build-recipes.png` under `HAMMERR_UI_SNAPSHOTS`. They use the private testing backend without desktop input. These preference checks do not launch the game.

### Prop shadows and native occluders

```sh
python3 -B scripts/validate_prop_shadows.py --binary target/debug/hammerr-headless --output /tmp/hammerr-prop-shadows-check
python3 -B scripts/validate_occluders.py --binary target/debug/hammerr-headless --output /tmp/hammerr-occluders-check
```

The prop fixture mounts synthetic bounded model companions, compares actual BSP light samples through both compile paths, checks shadow flags and selected-material warnings, and verifies malformed-asset failure preserves output/source/history. Compiler `prop_shadows`, `direct_lighting`, `static_props` and `bounced_lighting` integration tests cover lighting interaction and probe relocation outside a closed prop; `prop_occlusion` unit tests check before/on/behind-source indirect rays. Source-format checks cover bounded patch-material includes.

The occluder fixture checks native polygon planes/global vertices, dense entity indices, active state, metadata preservation, foreground/background equality, hidden helpers and atomic rejection of unsupported geometry. Source-format `occlusion` and compiler `occluders` integration tests cover malformed/versioned records, empty native sentinels, area ambiguity, pruned-solid adjacency and world bounds. These checks do not establish in-game culling or input-transition behavior.

### Clipboard transfer

```sh
cargo test -p source-formats --lib bounded_tests
cargo test -p map-core --lib transfer::tests
cargo test -p editor-core -p editor clipboard
python3 -B scripts/validate_clipboard.py --binary target/debug/hammerr-headless --output /tmp/hammerr-clipboard-check
python3 -B scripts/validate_clipboard_native.py --binary target/debug/hammerr-headless --output /tmp/hammerr-clipboard-native-check
```

Core checks cover detached source/destination identity collisions, exact internal references, external policies, texture placement, groups, owned-brush notes, terrain, native round trips and atomic history. Parser checks bound nesting/nodes before typed conversion. Session/UI checks cover publication failure before Cut, unchanged-map name receipts, invalid import retention, stale/foreign previews and parameter review; `HAMMERR_UI_SNAPSHOTS` writes `clipboard-paste.png` through the software testing backend.

The default validator uses two private stdio Sessions and exported text, with source replacement, offscreen preview/Apply/Cancel agreement, malformed payloads, metadata, save/reopen and identical foreground/background compilation. It never reads or writes the system clipboard. The native wrapper starts a separate Cage headless Wayland compositor with a private runtime directory, removes inherited display connections and runs the same transfer with explicit native publication/read. Its output directory must be new. It requires Cage and leaves desktop focus, input and the user's clipboard untouched; it does not launch the game.

### Occluder runtime fixture

```sh
python3 -B scripts/validate_occluder_runtime.py --binary target/debug/hammerr-headless --output /tmp/hammerr-occluder-fixture
python3 -B scripts/validate_occluder_runtime.py --binary target/debug/hammerr-headless --output /tmp/hammerr-occluder-client --runtime
python3 -B scripts/validate_occluder_runtime.py --binary target/debug/hammerr-headless --output /tmp/hammerr-occluder-inactive --runtime --start-inactive
python3 -B scripts/validate_occluder_runtime.py --binary target/debug/hammerr-headless --output /tmp/hammerr-occluder-islands --runtime --multi-brush
```

The default checks a sealed lit fixture, native occluder records and identical foreground/background builds. `--runtime` launches CS:S with `-insecure` and `sv_lan 1` on a private headless Cage display, and confirms the post-map LAN value. This installed client still requires Steam running to spawn a local player. `--start-inactive` checks the native inactive flag and inverse sequence. `--multi-brush` checks separated strips sharing one index, with side-prop culling and an unchanged center-gap prop. It observes initial/Deactivate/Activate/Toggle netprops and compares stable prop-culling frames. Server loading alone does not establish client culling. Use a fresh output directory for each run.


### Native layers and clipboard memberships

```sh
cargo test -p map-core layers::tests
cargo test -p editor-core session::layers::tests
cargo test -p editor layers_ui::tests
python3 -B scripts/validate_layers.py --binary target/debug/hammerr-headless --output /tmp/hammerr-layers-check
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
python3 -B scripts/validate_moving_contact.py --binary target/debug/hammerr-headless --output /tmp/hammerr-moving-contact-check
python3 -B scripts/validate_moving_contact.py --binary target/debug/hammerr-headless --output /tmp/hammerr-moving-contact-client --runtime
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
python3 -B scripts/validate_physics_surfaces.py --binary target/debug/hammerr-headless --output /tmp/hammerr-model-surfaces-check --brush-model
python3 -B scripts/validate_physics_surfaces.py --binary target/debug/hammerr-headless --output /tmp/hammerr-model-surfaces-client --brush-model --runtime
python3 -B scripts/validate_physics_surfaces.py --binary target/debug/hammerr-headless --output /tmp/hammerr-mixed-model-surfaces-client --mixed-model --runtime
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
cargo test -p editor-core prefabs
mkdir -p /tmp/hammerr-prefab-ui
HAMMERR_UI_SNAPSHOTS=/tmp/hammerr-prefab-ui cargo test -p editor --bin editor prefabs
python3 -B scripts/validate_prefabs.py --binary target/debug/hammerr-headless --output /tmp/hammerr-prefab-check
cargo test -p editor-core --features mcp --lib thumbnail
cargo test -p editor-core --features mcp --lib prefab_thumbnail_ -- --ignored --test-threads=1
cargo test -p editor --bin editor prefab_thumbnail_controls_ -- --ignored --test-threads=1
python3 -B scripts/validate_prefab_thumbnails.py --binary target/debug/hammerr-headless --output /tmp/hammerr-prefab-thumbnails-check
```

Storage checks cover atomic unique creation and no-replace renaming, filename scope, stale metadata,
symlink rejection and preservation of unrelated files. Session/software UI checks
cover source preservation, capture/preview/Apply/Cancel, stale source requests,
failed load/placement retention and deletion review; the UI capture is
`prefab-library.png`. The MCP script uses private Sessions and a private library
for cross-map IDs/links/groups/layers/notes, exact centered placement, Undo/Redo,
native save/reopen, restart, malformed files, rotated/brush-scaled placement,
source pivots, renaming and deletion after loading a detached draft. Map-core
`prefab_` tests cover transforms and attached UVs. UI captures also include
`prefab-transformed-placement.png`, `prefab-transformed-paste.png` and
`prefab-rename.png`. Late source-file failures must preserve the previous
clipboard, owned preview and history. Use a fresh output directory. These checks use no desktop input, native
clipboard or game launch.

Thumbnail checks use detached real brush/terrain/model geometry, flat materials
and a separate lazy renderer. They cover exact-byte/identity cache invalidation,
content/FGD epochs, resource caps, hidden sources, explicit model/instance errors,
GPU retry, actual PNG pixels and preservation of active map/clipboard/drafts/
camera/history. The two ignored graphics commands require an adapter; the UI
callback uses a software dialog and captures `prefab-thumbnail.png` through
`HAMMERR_UI_SNAPSHOTS`. The stdio fixture supports `--cpu-only` for inspection and
source-failure checks without graphics. It creates its own library and never
launches the game.

### Compile regions

```sh
cargo test -p compiler --test cordons
cargo test -p compiler --lib cordons::tests
cargo test -p compiler --test region_overlays --test region_occluders --test region_areaportals
cargo test -p map-core --lib compile_region::tests
cargo test -p editor-core --features mcp --lib session::compile_region::tests
cargo test -p editor --bin editor compile_region_ui::tests
python3 -B scripts/validate_compile_cordons.py --binary target/debug/hammerr-headless --output /tmp/hammerr-cordon-check
python3 -B scripts/validate_compile_regions.py --binary target/debug/hammerr-headless --output /tmp/hammerr-region-check --render --runtime
python3 -B scripts/validate_compile_regions.py --binary target/debug/hammerr-headless --output /tmp/hammerr-region-water-check --water --runtime
python3 -B scripts/validate_compile_regions.py --binary target/debug/hammerr-headless --output /tmp/hammerr-region-global-check --water --global-markers --runtime
python3 -B scripts/validate_compile_regions.py --binary target/debug/hammerr-headless --output /tmp/hammerr-region-terrain-check --terrain --global-markers --render --runtime
python3 -B scripts/validate_compile_regions.py --binary target/debug/hammerr-headless --output /tmp/hammerr-region-mixed-check --terrain --water --global-markers --runtime
python3 -B scripts/validate_compile_regions.py --binary target/debug/hammerr-headless --output /tmp/hammerr-region-lighting-origin-check --lighting-origins
python3 -B scripts/validate_region_overlays.py --binary target/debug/hammerr-headless --output /tmp/hammerr-region-overlay-check --runtime
python3 -B scripts/validate_region_helpers.py --binary target/debug/hammerr-headless --output /tmp/hammerr-region-helper-check --runtime
```

Compiler regressions cover native legacy/plural parsing, disabled identity,
whole crossing brushes, origin-brush pivots, fresh IDs, rejected content and
a leaking control with a missing boundary. Session and software UI tests cover
owned drafts, explicit review, Undo/Redo and frozen jobs. The private MCP
workflow checks exact synchronous/background output, filtering across independent
consumers, saved metadata and preserved source/history/selection. `--render`
captures an offscreen region guide; `--runtime` uses a private display with
`-insecure` and `sv_lan 1` for boundary traces and grounded player/prop contact.
`--water` adds a contained world pool, crossing-volume publication guards,
native fluid counters and a 60-second buoyancy observation above which the
boundary traces remain dry. Water in detail/model brushes remains rejected.

The imported-box checks include additional disabled entries and effective-scope
inspection/framing without source or history edits. Region overlay checks retain
whole imported projections and reject unavailable final face bindings. The helper
workflow combines portals/windows with native occluder arrays, geometry-based
inclusion, boundary rejection and missing window render targets. Its `--runtime`
mode uses isolated displays to observe native area controls and window fading.
`--lighting-origins` inspects exact outside static-prop lighting coordinates and
checks hidden/ambiguous references through both publication paths.

`--global-markers` places sun/water controls outside the region, verifies
retained origins and authored distances, and checks exact shared output with
the rendered black boundary. Compiler cases cover unavailable/incompatible
boundary assets without a replacement material.
`--terrain` checks whole cage/grid suggestions, native mesh/collision counters,
exact shared output and crossing-terrain publication guards. Compiler cases
also cover detail terrain, outside omission and a displaced vertex crossing
the boundary while its base remains inside. The dry native case checks prop
contact above the raised sheet; combining `--terrain --water` checks buoyancy
with both geometry types present.
The cordon script separately checks invalid imports and retained prior output.
Use fresh output directories. Neither script sends desktop input.

### Fitted overlay authoring

```sh
cargo test -p map-core --lib overlay::tests
cargo test -p editor-core --features mcp --lib overlay
cargo test -p compiler --test overlay_fit --test overlays --test region_overlays
cargo test -p editor --bin editor overlay_ui::tests
python3 -B scripts/validate_overlay_authoring.py --binary target/debug/hammerr-headless --output /tmp/hammerr-overlay-authoring-check --render --runtime
python3 -B scripts/validate_overlay_corners.py --binary target/debug/hammerr-headless --output /tmp/hammerr-overlay-corners-check --render
python3 -B scripts/validate_overlay_corners.py --binary target/debug/hammerr-headless --output /tmp/hammerr-overlay-corners-native --runtime
```

These checks cover real tilted world/detail corners, shared imported projection
validation, same-binding refit and explicit destination rebinding with raw metadata and entity identity retained,
ownership/review, stale source/selection guards, material browsing and read
errors, Apply/Cancel/Undo and exact shared native output. `--render` captures the
shared outline; `--runtime` compares a fully MCP-created floor projection with absent
and faded cases on private displays, including runtime accessor lookup.

`validate_overlay_corners` edits existing world/detail and tilted projection
frames, checks exact raw-field preservation, local/world outlines, convexity and
native-precision failures, ownership/stale tokens, Undo and save/reopen. `--render`
uses offscreen shared guides; `--runtime` compares edited native projections with
baseline/visible controls and accessor identity on a private display. Software
callbacks capture `overlay-corners.png` through `HAMMERR_UI_SNAPSHOTS`.

### Compile-region shell experiment

```sh
python3 -B scripts/probe_compile_region_shell.py --binary target/debug/hammerr-headless --output /tmp/hammerr-region-shell
python3 -B scripts/probe_compile_region_shell.py --binary target/debug/hammerr-headless --output /tmp/hammerr-region-shell-client --runtime
```

This disposable MCP prototype copies intersecting whole world brushes and
in-bounds point entities into a separate map, then constructs six boundary
brushes. It checks exact shared builds, preserved source, retained crossing
geometry, excluded targets and a leaking control with one boundary removed.
Optional native validation uses a private display, `-insecure` and `sv_lan 1`
for player grounding, stable prop contact and native traces of all four lateral
boundaries. This does not implement region metadata, authoring controls or
general entity/terrain/helper inclusion; it is
an experiment for the planned compiler path. Use a fresh output directory.

### Visibility and material failure checks

```sh
cargo test -p compiler --lib visibility
cargo test -p compiler --test material_failures
cargo test -p editor-core --lib malformed_materials
python3 -B scripts/validate_material_failures.py --binary target/debug/hammerr-headless --output /tmp/hammerr-material-errors
```

Visibility checks require real clipping beyond 128 vertices, small-edge
separators, tiny-aperture preservation and errors for nonfinite or over-budget
inputs. Material checks cover VMT/patch/VTF errors, finite reflectivity, cached
and uncached preview diagnostics, affected-face reports, retained BSPs/source/
history and asset repairs through both MCP build paths.

Native smoothing checks share the private compositor, capture-view setup and
frame comparator in `validate_native_runtime.py`. The clipboard wrapper uses
that compositor too. Each worker has a private runtime directory and no inherited
desktop display; failed or timed-out workers are stopped and reaped. Feature
fixtures and assertions remain independent. The frame comparator checks complete
RGB captures before comparing pixels. Whole-frame change counts are diagnostics,
not lighting pass/fail criteria: HUD changes and unrelated noise can change those
pixels too. Retain the exact BSP checks and inspect the native captures.

### Content discovery and terrain preview errors

```sh
python3 -B scripts/validate_content_errors.py --binary target/debug/hammerr-headless --output /tmp/hammerr-content-errors --render
```

The private headless fixture uses isolated content mounts and deliberate
symlink loops. It checks discovery/read errors, mount precedence, retryable
material/model/sound catalogs, affected-face reports, retained BSPs and source/
history. Imported invalid smoothing, unpaired deformation and nonfinite terrain
exercise shared preview/check/compiler handling. `--render` adds offscreen PNGs,
cached `preview_issues` and error clearing after restoring healthy source;
omit it for GPU-independent checks. No desktop input or game launch is used.

The shared script `Client.render_result` returns PNG bytes and metadata from one
render response. `Client.render` continues to save the PNG and return its
checksum for existing checks.

### Working regions, fog and particle definitions

```sh
cargo test -p editor-core --features mcp --lib working_region
cargo test -p editor-core --features mcp --lib fog_
cargo test -p source-formats --lib particle
cargo test -p editor-core --features mcp --lib particles
python3 -B scripts/validate_working_region.py --binary target/debug/hammerr-headless --output /tmp/hammerr-working-region-check --render
python3 -B scripts/validate_fog_preview.py --binary target/debug/hammerr-headless --gameinfo "/path/to/cstrike/gameinfo.txt" --output /tmp/hammerr-fog-check
python3 -B scripts/validate_particles.py --binary target/debug/hammerr-headless --output /tmp/hammerr-particles-check
```

Working-region checks cover whole-owner bounds, authored hiding, model/terrain
bounds, shared query exclusion, stale guards and unchanged saved source/history
and synchronous/background BSPs. `--render` compares persistent-renderer output
before filtering and after Clear. Fog checks compare known-depth pixels with the
linear-space ramp, disabled and unsupported states, temporary overrides and
shared settings. Particle checks cover active manifests, definition metadata,
owned choices, changed-file rejection, reload and failed-load retry.

Software UI checks run on the private testing backend and save captures when
`HAMMERR_UI_SNAPSHOTS` names an existing directory:

```sh
cargo test -p editor --bin editor working_region_controls_apply_inspected_bounds_reject_invalid_input_and_clear_empty_view
cargo test -p editor --bin editor fog_preview_controls_choose_apply_cancel_and_reject_stale_or_unsupported_settings
cargo test -p editor --bin editor particle_browser_shares_search_review_apply_cancel_and_software_screenshot
cargo test -p editor --bin editor partition_texture_controls_share_preview_inheritance_keep_cancel_and_undo
```

The existing `repeat_array`, `partition_textures`, `overlay_authoring` and
`material_reload` scripts also cover per-copy brush scaling, Fit/Justify,
explicit overlay rebinding and cached model material-path changes. Model checks
retain geometry and bounds through path failure/repair and account for held
asset references within the cache budget.

### Requested static-prop cutout shadows

```sh
cargo test -p compiler --test prop_shadows
cargo test -p compiler --lib lighting::props
cargo build -p compiler --example prop_shadow_fixture
python3 -B scripts/validate_prop_shadows.py --binary target/debug/hammerr-headless --output /tmp/hammerr-prop-shadow-check
python3 -B scripts/validate_prop_shadows_native.py --binary target/debug/hammerr-headless --output /tmp/hammerr-prop-cutout-check --runtime
```

The native fixture copies one mounted fence model and its companions into private
content, changes the copy's texture-shadow flag and material, and compares opaque
and alpha-tested builds from unchanged source. Assertions require both newly lit
and still blocked floor luxels; optional runtime captures use the existing private
display with `-insecure` and `sv_lan 1`. No installed game asset is modified.

### History inspection and draft interaction

```sh
cargo test -p map-core --lib document::history_tests
cargo test -p editor-core --features mcp --lib history
cargo test -p editor --bin editor history_window_inspects_cancels_navigates_guards_and_reopens_with_software_rendering
python3 -B scripts/validate_history.py --binary target/debug/hammerr-headless --output /tmp/hammerr-history-check
```

Checks cover bounded pages and object summaries, original actors, exact undo/redo
states, branch replacement, stale snapshots, save/reopen and navigation limits.
The combined draft regression preserves armed and picked entity placement and
measurement tokens, including different owners, until explicit cancellation.
`HAMMERR_UI_SNAPSHOTS` captures `history.png` through the software backend.

### Contextual point-entity creation

```sh
cargo test -p editor-core --features mcp --lib point_entity_
cargo test -p editor --bin editor point_entity_placement_controls_pick_review_apply_cancel_and_reject_replacement
python3 -B scripts/validate_entity_placement.py --binary target/debug/hammerr-headless --output /tmp/hammerr-entity-placement-check
```

Uses private maps and shared operations for brush/terrain/model contact, grid
snapping, source orientation, FGD defaults, reviewed ownership, stale recovery,
Cancel, save/build guards, Undo/Redo and saved identities. The script renders the
shared draft offscreen. Software callbacks capture `entity-placement-controls.png`
when `HAMMERR_UI_SNAPSHOTS` points to an existing directory.

### Persistent measurement guides

```sh
cargo test -p map-core --lib measurements
cargo test -p editor-core --features mcp --lib measurement
cargo test -p compiler --test measurement_metadata
cargo test -p editor --bin editor measurement_window
cargo test -p editor --bin editor measurement_surface_controls_
python3 -B scripts/validate_measurement_picking.py --binary target/debug/hammerr-headless --output /tmp/hammerr-measurement-picking-check --render
mkdir -p /tmp/hammerr-measurement-renders
HAMMERR_MEASUREMENT_SNAPSHOTS=/tmp/hammerr-measurement-renders cargo test -p editor-core --features mcp --lib measurement_offscreen_mcp_renders_preview_saved_hidden_and_undo_in_all_projections -- --ignored --test-threads=1
```

Checks exact endpoint metadata, malformed-import preservation, draft ownership,
edit/delete/Undo, framing and visibility. Compiler checks require identical BSP
bytes with and without rulers. The explicit graphics check compares real MCP
pixels before preview, after Apply, hidden and after Undo in all four projections.
Software callbacks capture `measurements.png`, `measurements-preview.png` and
`measurements-reviewed.png` through `HAMMERR_UI_SNAPSHOTS`. Endpoint checks add
precise double-coordinate rays, pixel-center cameras in all projections, actual
terrain/model hits, hidden/helper-only misses, coincident-endpoint rejection,
stale map/scene/camera/viewport guards, token replacement and owned stale cleanup.
The stdio fixture launches a private Session; `--render` captures the shared guide
and `--skip-models` omits the installed-model case when assets are unavailable.
The software callback captures `measurements-surface-picked.png` and checks that
numeric edits retain their precision after surface picking.

### Static rope previews

```sh
cargo test -p render --lib ropes
cargo test -p editor-core --features mcp --lib rope_preview
cargo test -p editor --bin editor rope_preview_control_uses_shared_view_and_reports_bad_links_without_map_edits
python3 -B scripts/validate_rope_preview.py --binary target/debug/hammerr-headless --output /tmp/hammerr-rope-preview-check
```

Checks bounded curves, link/parameter diagnostics, oversized UTF-8 identifiers,
shared and temporary view options, actual offscreen pixels and pixel picks,
depth occlusion, hidden/region-filtered endpoints and source/history preservation.
These editor checks launch no game and send no desktop input. Use fresh private
output directories for scripts.

### Grid defaults, selection outlines and material sources

```sh
cargo test -p source-formats -p render -p editor-core --features editor-core/mcp --lib -- map_size source_reads_ material_source grid_preferences grid_defaults selection_outlines --test-threads=1
cargo test -p source-formats --test fgd map_size
cargo test -p editor --bin editor grid_defaults_window_saves_applies_retries_and_resets_through_shared_session_callbacks
cargo test -p editor --bin editor selection_outlines_control_shares_default_and_agent_view_without_map_changes
cargo test -p editor --bin editor material_source_ui::tests
cargo test -p editor --bin editor entity_picker_shows_shared_map_size_provenance_and_clears_on_reload
python3 -B scripts/validate_grid_defaults.py --binary target/debug/hammerr-headless --output /tmp/hammerr-grid-defaults-check
python3 -B scripts/validate_selection_outlines.py --binary target/debug/hammerr-headless --output /tmp/hammerr-selection-outlines-check
python3 -B scripts/validate_material_source.py --binary target/debug/hammerr-headless --output /tmp/hammerr-material-source-check
```

Grid checks use isolated state directories and process restarts, exact stored
bytes, outside replacements/corruption, retry/reset, failed writes and unchanged
map/history. Outline checks preserve default line bytes/order and compare actual
MCP pixels, picks and retained diagnostic/tool guides; `--model` adds a mounted
model case after verifying it displays. Source checks exercise real directory/VPK
priority, dangling winners, changed indexes, bounded reads/Patch includes and
unchanged active previews. FGD checks cover include precedence, authored/sorted
map-size provenance, failed reload retention and independent coordinate limits.
Set `HAMMERR_UI_SNAPSHOTS` to an existing directory for `grid-defaults.png`,
`grid-defaults-error.png`, `selection-outlines.png`, `material-source.png` and
`entity-catalog-mapsize.png`. Scripts require fresh output directories and never
launch a game or send desktop input.

### FGD size and color helpers

```sh
cargo test -p map-core --lib entity_helpers
cargo test -p editor-core --features mcp --lib fgd_helpers
cargo test -p editor --bin editor entity_panel_shows_shared_fgd_box_status_and_invalid_definition_diagnostics
python3 -B scripts/validate_fgd_size_helpers.py --binary target/debug/hammerr-headless --output /tmp/hammerr-fgd-size-helpers-check
```

Checks include inherited asymmetric dimensions and class colors, shared rendered
bounds/picks/framing/area-volume/working-region operations, reload and invalid-size
diagnostics, and model/light precedence. The private stdio script compares real
offscreen pixels (`class-color-size-box.png`, selected/hidden/invalid controls),
retaining saved source/history/camera/selection. It needs a graphics adapter but
launches no game or desktop input. Software UI callbacks capture
`fgd-size-helpers.png` through `HAMMERR_UI_SNAPSHOTS`.

### Terrain detail scattering and displayed model bounds

```sh
cargo test -p compiler --test detail_props --test material_details
cargo test -p compiler --lib detail_props
python3 -B scripts/validate_material_details.py --binary target/debug/hammerr-headless --output /tmp/hammerr-terrain-details-check --terrain --runtime
python3 -B scripts/validate_material_details.py --binary target/debug/hammerr-headless --output /tmp/hammerr-skewed-terrain-details-check --skewed-terrain
python3 -B scripts/validate_material_details.py --binary target/debug/hammerr-headless --output /tmp/hammerr-skewed-terrain-details-native --skewed-terrain --runtime
```

Checks final LOD triangles, displaced heights/normals, barycentric alpha groups,
empty and selected unsupported groups, parallelogram and convex planar skewed
bases, projected native base-triangle area distribution, start-corner rotation,
whole-region terrain, shared sample/work caps and unchanged source grids. Skewed
regressions include full/half-edge mixed-power LOD, sloped bases, invalid charts,
model bounds and cancellation. `--skewed-terrain` selects the private tapered-base
fixture and implies terrain mode; the legacy flat/parallelogram sampling stream
remains unchanged. Displayed model geometry must
fit the compile bounds even when its MDL header understates the mesh. The private
native terrain mode compares detail ON/OFF foliage in the exact MCP-built BSP;
existing flat artifacts can be compared byte-for-byte after compiler changes.
Runtime isolation, preservation and cleanup use the same policy as other private
CS:S validators, including `-insecure`, `-default` and pre/post-map `sv_lan 1`.

### Explicit detail sprites

```sh
cargo test -p compiler --test detail_sprites explicit_sprite
python3 -B scripts/validate_explicit_detail_sprites.py --binary target/debug/hammerr-headless --output /tmp/hammerr-explicit-detail-sprites-check
python3 -B scripts/validate_explicit_detail_sprites.py --binary target/debug/hammerr-headless --output /tmp/hammerr-explicit-detail-sprites-regions --regions
python3 -B scripts/validate_explicit_detail_sprites.py --binary target/debug/hammerr-headless --output /tmp/hammerr-explicit-detail-sprites-native --runtime
python3 -B scripts/validate_explicit_detail_sprites.py --binary target/debug/hammerr-headless --output /tmp/hammerr-explicit-sprite-controls-check --controls
python3 -B scripts/validate_explicit_detail_sprites.py --binary target/debug/hammerr-headless --output /tmp/hammerr-explicit-sprite-controls-native --controls --runtime
```

`validate_explicit_detail_sprites` authors 24 `prop_detail_sprite` entities through
the existing property calls. It checks exact SDK rectangle/UV fields, scale-one
type-one records, authored positions, dictionary deduplication, open visible
leaves, runtime-entity removal, save/reopen, unchanged unrelated geometry and
exact foreground/background output. Rejected fields must preserve previous
output, source and history. `--runtime` adds a private installed-engine detail
ON/OFF observation of the same BSP. Compiler regressions cover all three
orientations, mixed model/sprite dictionaries and invalid geometry/overrides.
`--controls` adds authored positive scale, fixed cross/tri geometry, tri fan/size
bytes and sway at all plain orientations. It checks native fields, unchanged
atlas dictionaries, scale/sway-only differences, save/reopen, deterministic builds
and source/history/previous-output preservation. Rejections include native
half-scale underflow/overflow, fast-path intermediate overflow, invalid shape
parameters/orientations and nondefault `modelscale`. With `--runtime`, private
captures compare scaled silhouettes and frame-to-frame sway on the same MCP-built
outputs; combine `--regions` to run the separate region-selection checks too.
`--regions` retains fifteen of the 24 authored cards, omits an invalid outside
sprite before card validation, and checks atomic inside-field and exact-boundary
open-leaf failures through both build paths. It also checks that undoing the
region restores full-map bytes. Compiler regressions cover implicit-zero and inclusive origin
selection, whole crossing cards, hidden omission and retained bounds/lighting
checks.
Use a fresh output directory; `--game-dir`, `--fgd` and `--port` override the
installed paths and private native port. Combining `--regions --runtime` observes
the region BSP instead of repeating the full-map capture. Native launches retain `-insecure`,
pre/post-map `sv_lan 1` and the shared preservation/cleanup policy.

### Detail sprites, shapes and light styles

```sh
cargo test -p compiler --test detail_sprites --test detail_lighting
cargo test -p compiler --test detail_sprites generated_shape_sway
cargo test -p source-formats detail_props
python3 -B scripts/validate_detail_sprites.py --binary target/debug/hammerr-headless --output /tmp/hammerr-detail-sprites-check
python3 -B scripts/validate_detail_sprites.py --binary target/debug/hammerr-headless --output /tmp/hammerr-detail-plain-sway-check --sway
python3 -B scripts/validate_detail_sprites.py --binary target/debug/hammerr-headless --output /tmp/hammerr-detail-plain-sway-native --sway --runtime
python3 -B scripts/validate_detail_sprites.py --binary target/debug/hammerr-headless --output /tmp/hammerr-detail-orientations-native --runtime --orientation-views
python3 -B scripts/validate_detail_shapes.py --binary target/debug/hammerr-headless --output /tmp/hammerr-detail-shapes-check
python3 -B scripts/validate_detail_shapes.py --binary target/debug/hammerr-headless --output /tmp/hammerr-detail-shapes-native --runtime
python3 -B scripts/validate_detail_shapes.py --binary target/debug/hammerr-headless --output /tmp/hammerr-detail-sway-check --sway
python3 -B scripts/validate_detail_shapes.py --binary target/debug/hammerr-headless --output /tmp/hammerr-detail-sway-native --sway --runtime
python3 -B scripts/validate_detail_lighting.py --binary target/debug/hammerr-headless --output /tmp/hammerr-detail-lighting-check
python3 -B scripts/validate_detail_lighting.py --binary target/debug/hammerr-headless --output /tmp/hammerr-detail-lighting-native --runtime
python3 -B scripts/validate_detail_lighting.py --binary target/debug/hammerr-headless --output /tmp/hammerr-detail-tint-check --tint
python3 -B scripts/validate_detail_lighting.py --binary target/debug/hammerr-headless --output /tmp/hammerr-detail-tint-native --tint --runtime --probe-library target/debug/hammerr_hdr_probe.so
python3 -B scripts/validate_detail_light_styles.py --binary target/debug/hammerr-headless --output /tmp/hammerr-detail-light-styles-check
python3 -B scripts/validate_detail_light_styles.py --binary target/debug/hammerr-headless --output /tmp/hammerr-detail-light-styles-native --runtime
```

`validate_detail_sprites` checks native atlas UVs and anchors, deterministic
placement, custom atlas validation, bounds/budgets, rejected selected groups,
source/history preservation and exact foreground/background output. Plain
orientations `0` (stored angles), `1` (full billboard) and `2` (vertical
billboard) share absolute Gaussian scale, preserving the raw emitted floats for
SDK lighting. Tests cover raw scaled bounds and the native half-path minimum of
2⁻²⁴ for fixed/full billboards and shapes. `--orientation-views` requires
`--runtime` and compares scaled detail ON/OFF silhouettes for orientations 0 and
1 from level and elevated cameras in four private Cage/VScript observations,
plus two elevated controls at scale one.

`validate_detail_shapes` checks fixed cross, tri and fanned-tri records against
the plain-sprite control, including encoded angle/size bytes, actual quad
bounds, deterministic placement, unchanged sprite dictionary, source/history
preservation, nonunit Gaussian scales and rejected shape/sway combinations.
`--runtime` compares detail ON/OFF captures, tri-parameter silhouettes and
unit/scaled cross and tri silhouettes.

The generated-shape sway regression checks SDK bytes 0/127/255, default-zero
output identity, unchanged sampling/lighting/unrelated lumps, deterministic
repeated and reopened builds, and atomic rejection of malformed, out-of-range
or unsupported sway. Bounds cover rest geometry; client sway and avoidance
can move vertices beyond it.

`validate_detail_shapes --sway` uses cross-zero, cross-half and tri-full fixtures
with encoded bytes 0, 127 and 255. With `--runtime`, each variant runs in one
private process with amplitude 8 temporal frames, amplitude 0 temporal frames,
and a detail-disabled background. Frames are separated by 1.5 seconds. The
checks require positive-byte motion, zero-byte stability, zero-amplitude
stability and unchanged background pixels within the measured tolerances.
The worker sets `cl_detail_max_sway` explicitly; the game's default 0 would
suppress this motion. This mode retains the usual foreground/background,
source/history and output preservation checks.

`validate_detail_sprites --sway` adds generated plain orientations 0, 1 and 2
with byte 127, checking unchanged raw scales, placement, dictionary, SDK lighting
and unrelated lumps, deterministic output and default-reader rejection without
the sway opt-in. `--runtime` applies the same amplitude 8/0 temporal controls to
each orientation through the shared phase worker. An encoded nonzero sway byte
requires the native half-scale minimum 2⁻²⁴ even for unstyled vertical billboards;
encoded zero retains their prior fast behavior unless styles select the slow path.

`validate_detail_lighting` compares bright, dim and colored static lights for
explicit models and generated sprites, without changing placement. It checks
Fullbright's white base colors, Direct LDR's baked colors, the final HDR shared
base under Direct LDR + HDR, and exact foreground/background builds.
`--runtime` additionally checks separate model and sprite foreground color masks
against detail-disabled backgrounds.

`validate_detail_lighting --tint` reuses that room, model assets, sprite atlas and
native observer for explicit `_detail_lighting_tint`. It checks omitted/white
BSP identity in Fullbright, LDR and dual modes, exact Fullbright tint bytes,
linear attenuation within RGBExp32 quantization, actual fixed/vertical/tri
records, black style membership, unchanged unrelated lumps, save/reopen and
atomic foreground/background malformed-channel failures. `--runtime` compares
white and asymmetric tint using separate model and sprite ON-minus-OFF masks;
detail-disabled ROI backgrounds must remain exactly identical. It covers fast
vertical and slow fixed sprites, actual named-light TurnOn/TurnOff on the same
white/tinted BSPs, plus dual-channel style colors with measured HDR hardware.
The supplied existing `hdr_runtime_probe.c` observer must be built first; no new
native observer is introduced. Gamma conversion means screen pixels are not
expected to equal the authored linear bytes.

`validate_detail_light_styles` checks named-light extras, unchanged placement,
sorted matching LDR/HDR style layouts, authored channel colors, Fullbright
without style arrays, strict reader opt-ins and source/history preservation.
`--runtime` observes one unchanged BSP with its named light on, off and restored;
separate model and sprite masks subtract detail-disabled backgrounds. These
controls check native response, without assuming SDK half-color extras restore
static-base intensity. Compiler and source-format tests cover record caps,
missing-channel zero fill, malformed ranges/layouts and atomic rejection.
Detail shadows and sway on model entries remain unsupported.

All four validators need fresh output directories and installed CS:S/FGD assets.
Native modes retain the shared private-launch, source/configuration preservation
and cleanup policy, including `-insecure` and pre/post-map `sv_lan 1`.
The existing detail observer accepts optional `--hdr-level 0|2` with
`--probe-library` for verified hardware selection; level 2 requires the existing
HDR observer. Without these options it retains the original LDR controls.
It also accepts `--sprite-orientation 0|1|2` and
`--view level|elevated` for prepared private sprite fixtures. `--allow-shapes`
admits fixed cross/tri records; `--allow-detail-styles` and
`--detail-style-state on|off` admit and control styled fixtures. Default readers
remain strict about white, unstyled plain-sprite/model records unless the
corresponding baked-color, shape and style options are selected.
