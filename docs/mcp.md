[← Manual](README.md)

# AI agents and MCP

HammeRR is built so that an AI agent can do anything a person can. Every editor feature has a matching [MCP](https://modelcontextprotocol.io) tool, and both run the same internal operations. So:

- the agent edits the map you have open, and you see each change live;
- agent edits go into the same undo history as yours;
- previews are shared: an agent can set one up for you to apply, or the other way around;
- the agent can render any view of the map to check its own work.

| Courtyard built by an agent | Agent close-up with one face selected |
| --- | --- |
| ![Agent render of textured walls, stairs, crates and light helpers](images/agent-render.png) | ![Close-up of stacked crates with one face selected](images/agent-block.png) |

## Connect

While the editor runs, it serves MCP over HTTP at `http://127.0.0.1:7377/mcp`. Add it to your MCP client:

```json
{
  "mcpServers": {
    "hammerr": {
      "url": "http://127.0.0.1:7377/mcp"
    }
  }
}
```

Use `--mcp-port <port>` to change the port, or `--no-mcp` to turn the server off.

### Without a window

`hammerr-headless` serves MCP over stdin/stdout in a private session. It uses the same tools and editing operations as the desktop editor. Rendering still works; graphics start on the first render.

1. Build the standalone server without compiling the desktop UI:

   ```sh
   cargo build --release -p editor-core --features mcp --bin hammerr-headless
   ```

2. Add the executable to your MCP client. Leave out the map argument to start an empty map.

   ```json
   {
     "mcpServers": {
       "hammerr": {
         "command": "/path/to/hammerr/target/release/hammerr-headless",
         "args": ["/path/to/map.vmf"]
       }
     }
   }
   ```

The server accepts `--game` and `--fgd` as described in [Getting started](getting-started.md). `editor --headless` remains available with the same stdio tools. The standalone server opens no editor window and serves no HTTP endpoint. Engine-physics builds need `hammerr-physics` beside the executable; build it with `cargo build --release -p compiler`.

## What agents know

When an agent connects, the server tells it how HammeRR works, so you don't need to explain:

- **Units and axes**: Hammer units with Z up. A player is 32 × 32 × 72 units. Each tool reports its coordinate limits.
- **IDs**: `solid:<id>` for world brushes and `entity:<id>` for entities. Tools act on the current selection when IDs are left out, and creation tools select what they create.
- **Materials**: paths under `materials/` without the extension, such as `tools/toolsnodraw`. `search_materials` finds more.
- **Previews**: most edit tools accept `preview: true` and return a `token`. Pass it to the matching commit or cancel tool, so a preview someone else replaced can't be applied by mistake.
- **Stale-state guards**: inspection tools return a `snapshot`, `stamp`, or `generation` and `revision`. Passing them back makes a call fail safely if the map changed in between.
- **Checking work**: `render_view` returns a PNG from any camera without moving yours, `pick_at_pixel` identifies the object and face at a pixel, and `raycast` finds the first surface along a ray.

Each tool's description lists its parameters.

## A first call

Place a warm light, then look at it:

```json
{
  "name": "create_entity",
  "arguments": {
    "classname": "light",
    "origin": [0, 0, 128],
    "properties": { "_light": "255 220 180 400" }
  }
}
```

```json
{ "name": "render_view", "arguments": {} }
```

To build the map, set the build options once, then compile:

```json
{ "name": "set_compile_settings", "arguments": { "lighting": "direct_ldr", "physics": "engine", "sky_quality": "balanced" } }
```

`start_compile` returns a job token right away; poll `get_compile_status` and stop with `cancel_compile`. `compile_map` and `run_map` wait for the build to finish. `launch_game` does the same as **Play built map**.

## Tools by task

| Task | Tools |
| --- | --- |
| Files and inspection | `new_map`, `open_map`, `save_map`, `get_map_info`, `get_map_statistics`, `find_objects`, `get_objects` |
| Recent maps | `get_recent_maps`, `open_recent_map`, `remove_recent_map`, `clear_recent_maps` |
| Selection and history | `select`, `get_selection`, `set_selection_mode`, `undo`, `redo` |
| Overlapping objects | `get_object_hit_candidates`, `cycle_object_hit` |
| Prefab library | `get_prefabs`, `save_prefab`, `preview_prefab`, `delete_prefab`; placement uses `apply_paste` / `cancel_paste` |
| Layers | `get_layers`, `begin_layer_edit`, `update_layer_edit`, `apply_layer_edit`, `cancel_layer_edit`, `select_layer_members`, `set_layer_visibility` |
| Clipboard and transfer | `get_clipboard`, `copy_objects`, `cut_objects`, `import_clipboard`, `export_clipboard`, `preview_paste`, `apply_paste`, `cancel_paste` |
| Object edits | `move_objects`, `duplicate_objects`, `delete_objects` |
| Repeated placement | `get_repeat_array`, `preview_repeat_array`, `apply_repeat_array`, `cancel_repeat_array` |
| Brush drawing and transforms | `set_spatial_settings`, `draw_block`, `cancel_block_preview`, `create_block`, `transform_objects` |
| Transform handles | `set_transform_tool`, `get_transform_handles`, `begin_transform_drag`, `update_transform_drag`, `commit_transform_preview`, `cancel_transform_preview` |
| Absolute grid placement | `preview_snap_to_grid`; inspect and apply/cancel through the transform tools |
| Overlay authoring | `get_overlay_state`, `browse_overlay_targets`, `preview_overlay`, `preview_overlay_refit`, `preview_overlay_rebind`, `review_overlay`, `apply_overlay`, `cancel_overlay` |
| Surface placement | `preview_surface_placement`, `begin_surface_placement_pick`, `pick_surface_placement`, `get_surface_placement`, `apply_surface_placement`, `cancel_surface_placement` |
| Area selection | `set_area_selection_options`, `select_area`, `get_area_selection`, `commit_area_selection`, `cancel_area_selection` |
| Volume selection | `select_volume`, `get_volume_selection`, `commit_volume_selection`, `cancel_volume_selection`, `frame_volume_selection` |
| Paint selection | `set_paint_tool`, `begin_paint_selection`, `update_paint_selection`, `get_paint_selection`, `commit_paint_selection`, `cancel_paint_selection` |
| Brush shapes | `create_primitive`, `create_polygon_brush`, `get_primitive_preview`, `frame_primitive_preview`, `commit_primitive_preview`, `cancel_primitive_preview` |
| Hollow | `preview_hollow`, `get_hollow_preview`, `look_inside_hollow`, `commit_hollow`, `cancel_hollow` |
| Carve | `get_carve_preview`, `preview_carve`, `preview_carve_many`, `commit_carve`, `cancel_carve` |
| Merge brushes | `get_merge_preview`, `preview_merge`, `commit_merge`, `cancel_merge` |
| Clip | `set_clip_tool`, `clip_brushes`, `get_clip_preview`, `set_clip_points`, `pick_clip_point`, `get_clip_handles`, `set_clip_handle_options`, `begin_clip_drag`, `update_clip_drag`, `finish_clip_drag`, `cancel_clip_drag`, `commit_clip_preview`, `cancel_clip_preview` |
| Vertices, edges and faces | `get_component_sources`, `update_component_scope`, `get_brush_components`, `set_component_picking`, `pick_brush_components`, `select_brush_components`, `preview_component_move`, `commit_component_move`, `cancel_component_move` |
| Component move handles | `get_component_handles`, `set_component_move_options`, `begin_component_drag`, `update_component_drag`, `commit_component_drag`, `cancel_component_drag` |
| Scale, rotate, weld, split, ridge | `preview_component_scale`, `preview_component_rotate`, `preview_component_weld`, `preview_component_split`, `preview_component_crease`, `commit_component_edit`, `cancel_component_edit` |
| Flat partitions | `preview_component_partition`, `update_component_partition_cut`, `update_component_partition_material`, `update_component_partition_texture`, `place_component_partition_texture`, `raise_component_partition`, `preview_partition_extrusion`, `select_component_partitions`, `frame_component_partitions`, `pick_component_partitions`, `commit_component_edit`, `cancel_component_edit` |
| Saved flat pieces | `get_saved_component_partitions`, `store_component_partition`, `resume_component_partitions`, `remove_saved_component_partitions` |
| Face textures | `select_faces`, `get_face_selection`, `set_material`, `edit_faces`, `sample_face_texture`, `get_surface_sample`, `paint_face_texture`, `propagate_face_texture`, `project_face_group`, `commit_face_preview`, `cancel_face_preview` |
| Texture paint strokes | `begin_texture_stroke`, `update_texture_stroke`, `get_texture_stroke`, `commit_texture_stroke`, `cancel_texture_stroke` |
| Terrain vertices | `get_terrain_vertices`, `highlight_terrain_vertices`, `pick_terrain_vertex`, `frame_terrain_vertices`, `get_terrain_volume_vertices`, `select_terrain_vertices_in_volume`, `get_terrain_vertex_mask`, `preview_terrain_vertex_move` |
| Terrain brush strokes | `begin_terrain_stroke`, `update_terrain_stroke`, `get_terrain_stroke`, `break_terrain_stroke`, `finish_terrain_stroke`, `cancel_terrain_stroke` |
| Material replacement | `find_material_faces`, `get_material_search`, `select_material_faces`, `replace_materials` |
| Material browsing | `search_materials`, `browse_materials`, `preview_material`, `set_material_favorite`, `reload_material_assets` |
| Entities | `create_entity`, `set_entity_properties`, `tie_to_entity`, `move_to_world` |
| Entity definitions and inspector | `search_entity_classes`, `get_entity_class`, `load_entity_definitions`, `get_entity_inspector`, `set_entity_color`, `set_entity_component` |
| Particle definitions and choices | `browse_particles`, `inspect_particle`, `begin_particle_choice`, `choose_particle`, `get_particle_choice`, `apply_particle_choice`, `cancel_particle_choice`, `reload_particle_assets` |
| Connections | `get_entity_connections`, `get_connection_choices`, `begin_connection_edit`, `update_connection_draft`, `commit_connection_edit`, `cancel_connection_edit`, `delete_entity_connection` |
| Safe renaming | `get_entity_references`, `begin_entity_rename`, `update_entity_rename`, `commit_entity_rename`, `cancel_entity_rename` |
| World settings and property batches | `get_property_inspector`, `begin_property_edit`, `update_property_edit`, `get_property_edit`, `commit_property_edit`, `cancel_property_edit` |
| Object report and visibility | `get_object_report`, `select_object_report`, `set_object_visibility` |
| Map checks | `run_map_checks`, `get_map_checks`, `select_map_issue`, `preview_map_fixes`, `apply_map_fixes`, `cancel_map_fixes` |
| Design notes | `get_context_state`, `list_context_notes`, `get_context_note`, `get_context`, `begin_context_edit`, `update_context_edit`, `apply_context_edit`, `cancel_context_edit`, `delete_context_note`, `set_context_view`, `focus_context` |
| Recovery | `get_recovery`, `set_recovery_settings`, `create_recovery_snapshot`, `restore_recovery_snapshot`, `delete_recovery_snapshot` |
| Camera and visual checks | `get_camera`, `set_camera`, `frame_selection`, `set_view`, `render_view`, `pick_at_pixel`, `raycast` |
| Working region | `get_working_region`, `set_working_region` |
| Fog preview | `get_fog_preview`, `set_fog_preview`; temporary overrides through `render_view` |
| Saved camera views | `get_camera_views`, `save_camera_view`, `restore_camera_view`, `rename_camera_view`, `delete_camera_view` |
| Pointfiles | `load_pointfile`, `get_pointfile`, `navigate_pointfile`, `frame_pointfile`, `clear_pointfile` |
| Compile and play | `get_compile_settings`, `set_compile_settings`, `start_compile`, `get_compile_status`, `cancel_compile`, `compile_map`, `run_map`, `launch_game`, `game_log`, `stop_game` |
| Build recipes | `get_build_recipes`, `save_build_recipe`, `rename_build_recipe`, `delete_build_recipe`, `apply_build_recipe` |

## Good to know

These details aren't obvious from the tool list.

**Overlapping objects**

- `get_object_hit_candidates` and `cycle_object_hit` accept world-space `origin` and nonzero `direction`; distances are in world units even for non-unit directions. Use Objects mode and finish conflicting tools or geometry previews first.
- Reports include actual hit object/solid/side/point, its expanded `selection_unit`, the exact `total`, `limit: 64`, `truncated` and zero-based `chosen` (null for inspection or a miss). A cycle's `message` matches the human status and activity log, including label and truncation. Brush owners and outermost groups are deduplicated before the cap. Group bypass and visibility follow ordinary selection.
- Cycling advances from an exact current selection match and wraps; other selections start with the nearest hit. A miss preserves selection. Inspection and cycling leave authored data, camera and Undo history unchanged.

**Transforms**

- `transform_objects` does move, rotation, positive scale, reflection and single-axis shear, with an optional `pivot` and `space: "world"` or `"local"`. Combined operations apply scale and reflection first, then shear, then rotation, then translation.
- `texture_lock` controls textures during movement and rotation. `texture_scale_lock` controls stretching, mirroring and shear. Leaving out `texture_scale_lock` makes it follow `texture_lock`.
- `get_transform_handles.state.preview.display` holds the same summary the inspector shows.
- `preview_snap_to_grid` accepts optional `ids`, otherwise the selection. It rigidly moves the combined world bounds minimum to the nearest shared grid coordinate, using the current grid even when automatic Snap is off. `grid_snap` reports the grid, original/nearest bounds minimum and delta. Relative geometry and the current texture-lock policy stay intact; use normal transform inspection, commit or cancellation. Already aligned coordinates, including floating-point roundoff, create no history. Reloading model assets cancels this derived preview; preview it again afterward.

**Faces and textures**

- `edit_faces` sets material, texture placement, fitting, `lightmap_scale` and smoothing groups. `smoothing_groups` replaces memberships (`[]` clears them); `smoothing_add` and `smoothing_remove` keep the others.
- `set_view` and `render_view` accept `surface`: `textured`, `flat`, `wireframe`, `lightmap_density` or `smoothing_groups` (with `smoothing_group` from 1 to 32). On `render_view`, these options don't change your view.

**Vertices, edges and faces**

- Use the latest `snapshot` to select components or start a preview, and the preview's `token` to apply or cancel it.
- One side ID identifies a face, two identify an edge, three or more a vertex.
- Picking uses pixel coordinates from the top-left; a pixel's center is `column + 0.5, row + 0.5`. The radius is 1–32 pixels (default 10), and up to 64 candidates are returned.
- `preview_component_weld`: leave out the target for each brush's own average, or pass `target: [x, y, z]` or `target_corner: {solid, sides}`.
- `preview_component_crease` raises a roof ridge. It takes `pairs: [{solid, edges: [side, side]}]` and `rise`; `get_brush_components.crease_choices` lists valid pairs.

**Flat partitions**

- `get_brush_components.partition_choices` lists the edge pairs for each face, up to 28.
- `raise_component_partition` with `rise: 0` returns to flat; otherwise use 0.125–4096.
- `update_component_partition_cut` takes `edge_parameters: [first, second]`, each strictly between 0 and 1.
- `store_component_partition` keeps the pieces. `resume_component_partitions` and `remove_saved_component_partitions` take `sources: [{solid, side}]`.
- `place_component_partition_texture` takes the current `token`, 1–32 distinct `partitions: [{solid, partition}]`, and `fit: true` and/or `justify_u` / `justify_v`: `start`, `center` or `end`. Fit maps one tile over each piece's projected bounds; justification places a tile edge or center without changing scale. Fit runs first when combined. Actual loop corners and each effective material's dimensions are used, preserving projection axes, rotation and signed mirroring.
- Read each loop's `effective_texture`, `texture_plan` and `texture_inherited` from the replacement preview, then use its new token for Keep, raise/extrude, Apply or Cancel. Placement requires a flat draft and available texture dimensions; empty, duplicate, stale or unsupported requests preserve the previous draft. Omitted justification axes retain their shifts unless Fit changes them. Saved plans and replay retain these settings; arbitrary projection-axis authoring is unavailable.

**Carve, hollow and clip**

- Carving and hollowing need the `stamp` from `get_carve_preview` or `get_hollow_preview`. `preview_hollow` takes `open_faces` for openings.
- `clip_brushes` takes a normal and distance, three points, or a planar line. `finish_clip_drag` only ends handle positioning; `commit_clip_preview` applies the cut.

**Selection**

- A paint stroke accepts up to 256 points, 32,768 new samples and 1,048,576 pixel visits per update, and 262,144 samples in total.
- Pointfile point numbers start at 0 over MCP, and at 1 in the editor.

**Entities**

- In connection drafts, `delay` and `times` are strings, so half-typed values can be shared with the open dialog.
- During a rename, if the preview reports `requires_review`, send `accept_review: true` in a following update.
- `begin_property_edit` takes `world: true` or entity `ids`. Each entry in `changes` uses `action`: `set`, `remove`, `default`, `flag` or `component`.
- `get_object_report` filters by `query`, `classname`, `targetname`, `material`, `kind`, `hidden` and `within`.

**Map checks**

- `run_map_checks` / `get_map_checks` include standing-hull intersections with supported visible world/func_detail terrain sheets. They use the authored 32×32×72 hull, 0.01-unit contact tolerance, known solid materials and positive player-collision state. Terrain parent volumes are excluded. No-physics/no-ray flags do not disable player clearance. Unknown materials, unsupported flags, malformed geometry, removed triangles and exhausted work produce explicit unchecked warnings. Runtime brush-model owners remain outside clearance scope and receive compiler warnings. The terrain pass has a one-million-work limit and native triangle cap. Pending previews, compiler LOD snapping, models and runtime spawn placement are excluded. Existing issue navigation retains source/token guards; malformed terrain smoothing and tag rows receive face issues.

**Terrain**

- `create_terrain` converts selected or explicit `faces: [{solid, side}]` into flat surfaces. `power` is `2` (default), `3` or `4`, producing 5×5, 9×9 or 17×17 grids. `start_corner` is `0`–`3` (default `0`) in the winding order reported by `get_terrain_creation`; rows follow the next corner and columns the previous one. It accepts 1–16 world quad faces, one per ordinary parent brush, and rejects existing terrain, entity parents, ambiguous/nonquad targets and invalid powers atomically. The response gives `surfaces_created`, `power`, `grid_size`, `start_corner` and shared face state. IDs, materials, mapping and lighting stay fixed; one undo step and face selection remain shared. `get_objects` reports each face's `displacement` power. UI equivalents are **Terrain grid**, **Start corner** and **Create terrain**. Parents do not seal maps or become collision boxes. Sculpting and blending use the shared terrain-preview tools.

- `get_terrain_creation` inspects selected or explicit eligible faces without editing. It returns `faces` with `solid`, `side` and four ordered world-space `corners`, plus `default_start_corner`. UI corner labels are numbered 1–4; MCP indices are 0–3. Invalid selections return the shared creation error.

- `get_terrain_grids` returns 1–16 selected or explicit terrain faces with `power`, `grid_size`, `flags`, positive `physics_collision`, `player_collision` and `ray_collision`, `border_smoothing_angle`/`border_smoothing_error`, row-major world-space `positions` and `alphas` (0–255). `preview: true` inspects the rendered draft; omitted or false reads committed terrain.

- `preview_terrain_resolution` takes `power: 2|3|4` and optional `faces`. It samples the old triangles into a new grid while preserving shared heights/blends on refinement; coarsening can lose detail. Source and Undo history stay fixed until `commit_face_preview` with `state.preview_token`; `cancel_face_preview` discards it. Unsupported topology data reject atomically and preserve the last valid draft. Start corners and face metadata stay fixed. UI: **Edit terrain… → Operation → Grid resolution → Preview → Apply / Cancel**.

- `preview_terrain_sculpt` takes `mode: raise|flatten|smooth|noise`, a world-space `center`, optional `faces`, `radius` (1–8192, default 128) and `direction: world_z|face_normal` (default `world_z`). Raise/noise use signed `amount` within ±4096 (default 16); flatten/smooth use `strength` 0–1 (default 0.5). Flatten approaches the center plane, smoothing reads immutable local neighbors, and noise uses `seed` (a whole 0–4294967295, default 1). Each brush has smooth radial falloff. Grid coordinates must fit ±65536. Preview stays separate from source/history until token-guarded commit/cancel; invalid updates retain the previous draft.

- `preview_terrain_blend` takes `mode: paint|fill|invert`, optional `faces` and `value` 0–255 (default 128). Paint requires world-space `center` and uses `radius`, `direction` and `strength` 0–1 (default 1); fill/invert affect complete grids. Geometry and face metadata stay fixed. Read row-major alpha with `get_terrain_grids(preview: true)`, then use the same `commit_face_preview` / `cancel_face_preview` token. A two-texture material is needed to see blending; primary material physics remains in use. UI: **Edit terrain… → Operation**, with brush sliders, exact inputs, **Preview**, **Apply** and **Cancel**.

- `preview_terrain_sew` takes optional `faces` and `blend` (default false). Matching full/whole-to-half base edges or Source-float base corners at any supported powers share averaged original common positions; finer border vertices interpolate the sewn coarse edge; `blend: true` also averages/interpolates alpha. Vertex groups are formed first so multi-patch corners average once. It returns `faces_changed`, `neighbor_pairs` (border-only), `corner_joins` (additional corner unions after border groups), `sewn_vertices`, `interpolated_vertices`, a shared `summary` and `state.preview_token`. Interior vertices and face metadata stay fixed; ambiguous edges, collapsed float corners and unsupported caches reject without replacing the last valid draft. Commit or cancel through the shared face-preview tools. UI: **Edit terrain… → Sew borders & corners → Also sew blend values → Preview → Apply / Cancel**.

- `preview_terrain_smoothing` takes optional `faces` and `angle`. Omitted/null restores **Automatic**; `0` keeps **Hard borders**; positive finite values through `180` set an **Angle limit**. Both surfaces must allow each pairwise local-normal blend; accepted junction groups share a normal. Geometry, alpha, flags, tags and topology stay fixed. The viewport and Direct LDR use the same normal rules on their respective source/compiled geometry. Inspection returns a null angle for Automatic and an error string for malformed/duplicate imported `smooth` fields; a valid preview can repair them. Use token-guarded commit/cancel and Undo. UI: **Edit terrain… → Border smoothing → Maximum angle · degrees → Preview → Apply / Cancel**. Native stats expose `displacement_smoothing_overrides`, labeled **Border smoothing** in **Build details…**; the standard VMF `smooth` and native angle fields round-trip. This is separate from global brush `smoothing_angle`.

- `preview_terrain_collision` takes optional positive `physics_collision`, `player_collision` and `ray_collision` booleans for selected or explicit faces. Omitted fields preserve each face. It modifies only native bits 2/4/8, retains other data and returns the shared preview token. UI: **Edit terrain… → Collision**, with **Keep current / Enabled / Disabled** and the committed state. Unsupported imported bits and individual triangle removal remain separate.

- `remove_terrain` removes displacement data from 1–16 selected or explicit faces, rejecting missing, ambiguous or ordinary faces atomically. The response gives `surfaces_removed` and shared face state. Geometry, IDs, material, mapping and lighting stay fixed; undo restores all imported grid rows and extras. A parent regains its brush volume once no terrain faces remain. Imported entity terrain can also be removed. UI: **Remove terrain**.

- `raycast` and `pick_at_pixel` intersect rendered terrain triangles, retaining owner/side identity and visibility filters. `frame_selection` uses displaced bounds and active previews; `get_objects` reports committed visible bounds. Invisible parent faces are excluded from terrain hits. Pass the returned `point` into `preview_terrain_sculpt.center` or `preview_terrain_blend.center` to choose the same brush center as UI **Pick center in viewport…**. Imported entity terrain supports grid inspection and cleanup with Undo, while native runtime brush-model terrain remains unsupported; consumed `func_detail` terrain compiles through the world pipeline.

- `tie_to_entity` with `classname: func_detail` moves existing terrain parents into a consumed helper. `compile_map`, `start_compile` and `run_map` emit world displacement bindings with `SOLID | DETAIL` contents, saved grids/tags, lighting, neighbors and collision; hidden helpers are omitted. `render_view` includes compatible helper border normals. Source/history remain unchanged by building. Use `move_to_world` / UI **Move to world** to reverse the grouping. Direct creation inside helper entities and runtime brush-model terrain remain separate.

- Native terrain results include `displacement_neighbor_pairs` for compatible full/half edges and `displacement_corner_neighbor_pairs` for reciprocal corner-only contacts, excluding edge-connected pairs. Corner-only contact can use different powers when base and displaced corners match. UI **Build details…** labels these **Neighbor pairs** and **Corner pairs**. `displacement_mixed_power_pairs`, `displacement_half_edge_pairs`, `displacement_lod_disabled_vertices` and `displacement_lod_snapped_vertices` expose full-edge LOD constraints and actual geometry changes. UI **Build details…** labels these **Mixed-power pairs**, **Half-edge pairs**, **LOD disabled vertices** and **Snapped vertices**. Imported grids with the VMF `subdiv` marker compile their explicit rows without generating subdivision; source editing rejects actual unsupported caches, rather than the marker alone. Both compile paths automatically disable dependent vertices and snap unsupported detail without editing the source grid; physics and lighting use the resulting geometry. Whole-to-half connections support either or both halves, with spacing-aware masks and overlap rejection. Other fractional spans remain unfinished.

**Compiling**

- `set_compile_settings` takes `lighting` (`fullbright` or `direct_ldr`), `physics` (`none` or `engine`), `visibility` (`fast` or `detailed`), `sky_quality` (`fast`, `balanced` or `detailed`), `bounces` (whole counts `0`–`4`) and `smoothing_angle` (whole degrees `0`–`90`). It changes only the fields you pass, and invalid combinations leave every setting unchanged.
- `get_compile_settings.visibility_choices` exposes **Fast (conservative)** and **Detailed (portal clipping)**, shown under **Visibility quality** in Build settings. `visibility` freezes in each job's captured settings and changing it stales prior output. Detailed clips source/pass portal apertures; PAS includes the refined visible rows. Statistics expose `visibility_mode`, `visibility_work`, `visibility_removed_pairs` (directed PVS entries removed from the broad result) and `visibility_workers` for the largest actual worker count across the base and flow stages. Larger jobs parallelize base-facing rows on at most eight available CPUs; Detailed also parallelizes separator flow. Source portals retain their order, and workers share one exact global work budget. Invalid apertures and exceeded work/allocation budgets fail without publishing output. The former `visibility_fallbacks` statistic is removed because clipping is no longer abandoned.
- `get_compile_settings` returns the current choices, including `sky_choices` with their labels and direction counts (32, 128 and 512). These tools work before the first build, like the toolbar's **Build settings…**. The UI calls `sky_quality` **Sampling quality**; it affects Direct LDR sky ambient, soft sun, model ambient and bounce rays.
- `get_compile_settings.bounce_choices` exposes **Direct only** (`bounces: 0`, default) and **One diffuse bounce**, **Two diffuse bounces**, **Three diffuse bounces** or **Four diffuse bounces** (`bounces: 1`–`4`), shown under **Indirect light**. It gathers reflected light per source style from immutable previous-pass world/detail/terrain lightmaps into world, terrain and brush-model receivers using 32/128/512 cosine-weighted rays per receiver per pass. Each pass rebuilds receivers from the original direct maps. Model rays check their own geometry before accepting a world hit; unrelated models stay separate. Source albedo must be between zero and one; sky/backfaces do not supply bounce energy. Stats expose actual `bounces`, `bounce_rays`, `bounce_samples` (final-pass receivers with positive indirect light) and `bounce_work`, within the shared lighting budget. Fullbright retains the setting and reports zero bounce work.
- `edit_faces` (including `preview: true`), `commit_face_preview` and `cancel_face_preview` drive the same authored brush smoothing as **Face lighting…**. Shared groups 1–24 blend; shared groups 25–32 keep hard edges and take priority. Direct LDR applies checked world/detail and brush-model corner normals, including eligible corners inside neighboring source edges. Original fan centers and source fans remain stable across lightmap subdivision. Stats expose `smoothed_faces`, `smoothed_corners` and `normal_work` (construction work; receiver interpolation also counts toward `lighting_work`). Unassigned faces stay flat at angle zero; terrain, world coordinates and each model's local coordinates remain separate. Source/corner limits and the shared lighting budget preserve prior output on rejection.
- `smoothing_angle` defaults to `0` (disabled). Positive values blend only two unassigned source faces within one world/model scope whose normal angle meets the threshold. Explicit smooth/hard masks retain precedence; terrain and individual model/world scopes stay separate. `get_compile_settings.smoothing_angle_range` reports bounds/unit. Each job captures the angle; changing it stales previous output. Actual stats expose `smoothing_angle` and the same `smoothed_faces`, `smoothed_corners` and `normal_work`. Fullbright retains the setting but reports zero actual angle/work; no VMF/history edits occur.
- `edit_faces` and object material edits select VMT `$surfaceprop` values for world brush and terrain collision. With `physics: engine`, native triangle mappings use a shared table of at most 127 names including default; missing properties retain default. `compile_map` and background results expose `physics_surface_properties`, shown as **Surface properties** in **Build details…**. Names use 1–127 ASCII letters, digits, underscores, dots or dashes; custom definitions must be available to the game. Invalid mappings/names/capacity reject before publication. Face/object material edits also map brush-model collision triangles through the same shared table, bounded to 127 names including default. Uniform models additionally record the body property; mixed models retain per-face contact rather than choosing one face for the whole body. All-default model collision bytes remain unchanged. Blended secondary properties, material density and compiler-derived model masses remain unfinished. `physics_surface_properties` counts the distinct shared names.
- `create_entity` and `set_entity_properties` author point/spot `style` (Appearance `0`–`31`), `targetname`, `pattern` (up to 64 lowercase letters), and `spawnflags` bit `1` (Initially dark). Named groups get compiler-only IDs `32`–`63`; authored nonzero appearances become `defaultstyle`, and matching names share an initial pattern/state. `begin_connection_edit`, `update_connection_draft` and `commit_connection_edit` author relay outputs targeting `TurnOn`, `TurnOff`, `Toggle` or `SetPattern`. Both compile paths emit up to four face channels including Normal, with actual `switchable_light_groups`, `light_styles`, `styled_faces` and `light_style_maps` stats. Compilation keeps VMF/history fixed. Invalid fields, conflicting groups and exceeded limits preserve previous output. With `bounces: 1`–`4`, reflected light follows the same styles and can add a channel beyond direct reach; the four-style face limit still applies. Switching model-ambient cubes remain unfinished.
- `tie_to_entity` and `set_entity_properties` author rendered brush models and pivots. Direct LDR compiles model-local faces with world-space lighting positions, isolated self shadows, and the same point/spot style channels and sky sampling. Stats expose `lit_brush_models`, `model_lighting_faces`, `model_bounce_faces` (processed receiver faces) and `model_bounce_samples` (samples with positive reflected energy). Local geometry, UVs and collision stay fixed; direct and bounced lightmaps remain attached in game. Models do not cast permanent world shadows or supply reflected light. They can receive world bounce; dynamic relighting and independently rotated initial poses remain unfinished. Both compile paths share the checked emitted model-to-entity bindings and common budgets.
- `edit_faces` material/projection edits feed automatic directional lightmaps for lit VMTs declaring `$bumpmap`. Direct LDR emits base plus three direction maps per style on world/detail/model/terrain receivers, including the optional world-source bounce. Stats expose `bump_faces` and `bump_lightmaps` (auxiliary directional maps); `light_style_maps` includes base and directional maps. `lightmap_samples` counts receiver positions, while bytes/work include every map. Base averages, style IDs, geometric shadow bias and source/history remain shared. Material-only second normal maps, HDR, transparent shadows and baked prop maps remain unfinished; viewport previews stay diffuse.
- Settings cannot change while a build runs. Each job reports its captured `settings`; changing settings after a build makes that output out of date for launching.
- Direct LDR automatically culls point/spot candidates using each source's PVS and exact falloff endpoints, across world, terrain and pivot-aware model samples. Unknown/boundary clusters and cache overflow retain candidates; sky lighting stays eligible. Results expose `light_evaluations`, `light_pvs_skipped`, `light_range_skipped` and `light_culling_work` (row/cache/cluster lookup work within `lighting_work`). Both compile paths preserve emitter order and share the same bounded implementation; no extra setting is required.
- Create and edit baked lights with `create_entity` and `set_entity_properties`. For `light_environment`, `_light` is sunlight and `_ambient` is sky ambient; color values use `R G B brightness`. Set ambient brightness to zero with a value such as `255 255 255 0`. Set `SunSpreadAngle` from 0 to 90 degrees: zero keeps hard shadows, positive values soften them using the selected sky quality.
- `get_compile_status.launch` says whether the last build can be played, or why not.
- Build statistics include `ambient_samples` and `ambient_bytes`, shown as **Model ambient** in Build details. Direct LDR writes up to 1/4/8 safe, separated probes per open world leaf at Fast/Balanced/Detailed quality from filtered local world/terrain lightmaps and sky radiance; skipped leaves are reported in diagnostics. `sky_quality` controls probe density and the ray-cone footprint used to filter local detail. Probes remain more than 128 units apart; the bake caps total probes at 65,536 and retains the shared work limit.
- Terrain normal sewing follows per-surface Automatic/hard/angle choices for matching full/half edges in both compile paths and `render_view`; hidden patches do not contribute. `displacement_neighbor_pairs` counts all verified native full/half-edge links. Compilation also shares normals at common vertices of full/half edges after LOD snapping; the viewport retains the authored grids. Other fractional edge spans remain unfinished.
- Create areaportals with `create_block` and `tie_to_entity` (`classname: "func_areaportal"`). `targetname` is the human **Name**, and `StartOpen` chooses the **Initial State**. Entity connections accept **Open**, **Close** and **Toggle**. Compilation assigns native keys without editing the VMF. Shared build statistics include `areas`, `areaportals` and `areaportal_vertices`; invalid boundaries preserve the previous output.
- `func_areaportalwindow` uses the same `create_block`, `tie_to_entity` and `set_entity_properties` operations. `target` is **Rendered Window**, `BackgroundBModel` is the optional **Foreground bmodel**, and `FadeStartDist`, `FadeDist`, `TranslucencyLimit` control fading. Render targets must be unique visible brush models with enclosing bounds; foreground and rendered models share a pivot. Connections can send `SetFadeStartDistance` and `SetFadeEndDistance`. `areaportal_windows` counts windows within the total `areaportals` count.
- `run_map` and `launch_game` always launch CS:S with `-insecure`.
- Hint helpers use `create_block` with `tools/toolsskip`, then `select_faces` and `edit_faces` to paint `tools/toolshint` onto the intended split faces. Keep them as world brushes. Both compile paths protect finite hint polygons during initial structural plane selection, then partition remaining filled open leaves. Helpers add no rendering/collision. Results expose `hint_surfaces`, `hint_splits`, `hint_priority_splits` (retained added initial planes, included in `hint_splits`) and cumulative `hint_work`. Skip sides bound the helper; mixed materials and unsupported ownership fail atomically. Hidden/coplanar/outside helpers need no new split. Initial selection runs in `world_tree`; remaining refinement runs in cancellable `hint_planes`.
- Imported `info_overlay` entities compile through `compile_map`, `start_compile` and `run_map`. `overlays` and `overlay_face_refs` count projections and their compiled surface bindings. Edit material, U/V ranges, render order and fades with `set_entity_properties`; imported basis/corner fields remain read-only. Compilation preserves VMF properties and history, resolves source sides to final world/detail/terrain faces and creates named runtime accessors. Invalid targets or exceeded budgets preserve the previous BSP. See [Overlays](overlays.md) for limits.

### Terrain triangle classifications

- `get_terrain_triangles` inspects selected or explicit `faces`, with optional `preview: true`. Each face returns ordered native triangle IDs, grid indices, world positions and centers, raw optional `source_tags`, effective `walkable`/`buildable`, optional forced overrides, `removed` and `unknown_tags`. UI: **Terrain triangles…**.
- `preview_terrain_triangle_tags` takes `targets: [{face: {solid, side}, triangles: [0, 7]}]`, one entry per face, with 1–16 terrain faces. `walkable` and `buildable` accept `automatic`, `enabled` or `disabled`; omission keeps the current setting. Automatic recomputes source slope classifications. Unknown argument names, malformed rows and invalid IDs reject without replacing a valid preview.
- Finish with `commit_face_preview` or `cancel_face_preview` and `state.preview_token`. Geometry, other tags and history remain unchanged until Apply. These classifications do not toggle physical collision. Native compilation converts VMF override bits into BSP classification bits; removed/unknown tags remain rejected.

- `pick_terrain_triangle` takes a world `origin` and `direction`, normalized internally, with optional terrain `faces` (otherwise the selection). It reads rendered draft geometry and returns the native ID, power, point and distance, or `hit: null`. Hidden owners, tool textures, front faces and nearest visible brush-face occlusion apply; entity helper icons do not block a terrain pick.
- `highlight_terrain_triangles` takes the same per-face `targets` shape as classification edits. It returns a guarded token and a shared transient set that draws in the viewport and `render_view`. `get_terrain_triangle_highlight` reports it; `clear_terrain_triangle_highlight` accepts its optional `token`. A replaced token cannot clear another actor's guides. VMF, history and face selection stay fixed.
- `frame_terrain_triangles` frames visible `targets` at an optional positive finite `aspect` (default16/9). Hidden/tool-filtered targets do not frame. A different-power draft rejects framing; finish the draft and query its committed IDs again. Highlights follow compatible geometry previews, hide on changed-power drafts, and become stale after source/map changes.
- UI **Terrain triangles…** checkboxes retain subsets across surfaces; **Select surface**, **Clear selection**, **Pick triangle in viewport…**, **Frame selected**, Preview/Apply/Cancel call these shared operations. Closing the dialog clears only its owned guides and preview.

- `get_terrain_vertex_mask` converts the current highlight into deduplicated vertex IDs within selected/explicit `faces`. Optional `token` guards the highlight; the result includes its token, total vertex count and per-face vertex/triangle counts. Empty, stale or replaced masks reject.
- `preview_terrain_sculpt` and `preview_terrain_blend` accept optional `mask_token` from that query. Only its vertex union changes; unmasked faces/vertices remain fixed. Radius/falloff still apply, and smooth reads immutable neighboring samples. Paint, fill and invert all support the same mask. Unknown argument keys reject.
- A committed masked brush refreshes its matching retained selection for reuse. Another actor's replacement is preserved; Undo/Redo or unrelated source edits stale the mask. Human **Use selection** retains the highlight, and **Affect only selected terrain vertices** captures its token when opening terrain editing.

- `get_terrain_volume_triangles` takes the current `volume_token` from `select_volume` with `preview: true`, plus optional terrain `faces`. It returns bounded per-face native triangle `targets` and a total count using that box's contained/intersecting rule and actual displaced geometry. Hidden owners and tool-filtered faces stay excluded.
- `select_terrain_triangles_in_volume` takes the same inputs and atomically replaces triangle highlights, returning both the targets and highlight state. It retains face selection, box preview, VMF and history. UI: **Select a volume… → Preview matches**, then **Terrain triangles… → Select in volume**.
- Existing `select_volume` and `select_area` now match actual terrain sheets for both object and face scopes. Containment checks interior vertices; intersection clips displaced triangles. Ordinary parent faces and volumes remain excluded, matching rendered terrain bounds and picking. Invalid sheets do not fall back to parent boxes.

- `begin_paint_selection`/`update_paint_selection` and texture-stroke tools use cached displaced triangles and actual sheet bounds in their picking index. Raised and laterally offset terrain is picked at its visible surface; ordinary parent faces/volumes are excluded. Existing nearest-hit, hidden-source, tool-brush, owner and stroke-token rules remain. No new setting or tool is required.

### Explicit terrain vertices

- `get_terrain_vertices` accepts selected/explicit `faces` and optional `preview`. Each face returns power, grid size and row-major vertex IDs, rows, columns, world/base positions and alpha. IDs match terrain grid arrays and triangle indices.
- `highlight_terrain_vertices` takes `targets: [{face: {solid, side}, vertices: [12]}]`, one entry per face, up to 16 faces. IDs deduplicate; invalid IDs, duplicate faces and unknown arguments preserve the previous selection. Cyan markers follow rendered compatible drafts. This replaces the shared retained triangle/vertex selection without editing the VMF, history or face selection.
- `get_terrain_triangle_highlight` now reports `kind: triangles|vertices`, `triangles`, `vertices`, token and stale state. `clear_terrain_triangle_highlight` clears either kind with the same optional token guard. `get_terrain_vertex_mask` accepts either kind; explicit subsets have `triangle_count: 0`. Use the returned token as `mask_token` on sculpt/blend. Apply refreshes the matching selection for reuse; replacement, topology or external source changes reject stale masks.
- `pick_terrain_vertex` accepts the same finite world ray and face scope as triangle picking. It chooses the nearest vertex of the visible hit triangle, with stable ID ties, and returns vertex, power, position, hit point and ray distance. Hidden/tool filtering, front faces and nearest brush occlusion remain shared. Picking is read-only.
- `frame_terrain_vertices` takes explicit vertex targets and optional positive finite aspect. It uses visible rendered positions, including compatible drafts; invalid/empty targets and changed-power drafts reject without moving the camera.
- `preview_terrain_vertex_move` takes required `offset: [x, y, z]` and `mask_token`, plus selected/explicit `faces`. Offsets are world units within ±4096 per axis. Explicit vertex subsets and triangle unions both work; only selected offset rows change. Zero preserves absent rows exactly. Inspect vertices/grids with `preview: true`, then use owned face-preview commit/cancel. Apply refreshes its matching selection and creates one Undo step. Invalid bounds/caches/stale or replaced masks preserve a valid draft. UI: **Edit terrain… → Move selected vertices → World offset · units**.
- `get_terrain_volume_vertices` and `select_terrain_vertices_in_volume` accept selected/explicit faces and a current `volume_token`. Both box rules use inclusive point containment on displaced samples. The query is read-only; selection atomically retains its targets and returns the shared highlight. Stale/canceled/replaced boxes preserve prior guides.
- UI: **Terrain triangles… → Selection type → Vertices · exact brush masks**, checked rows, **Pick vertex in viewport…**, **Frame selected**, **Select in volume**, **Use selection** and **Cancel**. Classification controls remain under Triangles. Closing or changing selection type clears only this dialog's owned draft/guides.

### Terrain brush strokes

1. Call `begin_terrain_stroke` with exactly one `sculpt` or `blend` object. These use the same arguments as the fixed-preview tools; blend must be `paint`. Faces and optional mask token are captured. An active geometry preview rejects without replacing it.
2. Call `update_terrain_stroke` with its `token` and either `points: [[x, y, z], ...]` (1–64 world positions) or a finite `origin` and `direction` ray. Rays hit visible selected terrain with shared front-face, nearest brush occlusion and tool/owner filtering. A miss breaks the path; call `break_terrain_stroke` to create an explicit gap with world-point input.
3. Inspect `get_terrain_stroke`, `get_terrain_grids(preview: true)` or `render_view`. Grouped updates carry sample spacing without compounding a replay. Repeated positions and motion along the captured sampling normal add no dabs. The shared white radius guide appears in both render paths.
4. Call `finish_terrain_stroke` to retain the existing face draft, then `commit_face_preview` or `cancel_face_preview` with `state.preview_token`. Apply creates one Undo step and refreshes a matching retained mask. `cancel_terrain_stroke` discards only its owned draft; replaced stroke tokens reject, and foreign face previews stay intact.

Spacing is radius/4 clamped to 1–64 units in the brush plane. Face-normal spacing/guide orientation follows the first selected base face; each surface applies its own brush direction. Flatten uses the original supplied center plane. Each sample uses the fixed brush math and immutable-neighbor smoothing; overlapping samples accumulate. Limits are 512 dabs, 4096 input positions, 64 per update and two million vertex/sample evaluations. Invalid/stale/replaced updates preserve the prior draft. UI: **Edit terrain… → Draw stroke in viewport…**, drag, release, **Apply / Cancel**; **Escape** or **Cancel stroke** discards drawing. Losing viewport focus while dragging cancels without opening or focusing a window.

### Terrain vertex move handles

1. Retain a vertex or triangle selection, then call `enable_terrain_move_handles` with `mask_token` and optional `faces` (defaults to selected faces). Existing geometry previews reject. The result captures the source, mask and shared grid snapping and returns `token`.
2. Inspect `get_terrain_move_handles` for state and visible world/pixel segments. Optional `width`, `height`, `camera` and `point` inspect and hit-test without moving the editor. Call `begin_terrain_move_drag` with `token`, `handle` (`x/y/z`, `xy/xz/yz`, `view`) and optional `pointer: {position: [x,y], width, height}`.
3. Call `update_terrain_move_drag` with exactly one `pointer` or `offset: [a,b]`, measured from the gesture start. Axis handles use only `a`; planes use world axes, and `view` uses captured screen right/up. Grid snapping matches the UI. Invalid updates preserve the last valid draft and block finish until corrected.
4. Call `finish_terrain_move_drag`, then `commit_face_preview` or `cancel_face_preview` with `state.preview_token`. Apply creates one Undo step and retains the matching selection. `cancel_terrain_move_handles` can discard an armed, live or stale gesture and clears only its owned preview.

Source, mask, camera, viewport-size, visibility and grid changes guard the gesture. Numeric terrain movement bounds apply. UI: **Edit terrain… → Move selected vertices → Move with handles…**, drag, release, **Apply / Cancel**; **Escape** and **Cancel terrain move** discard. Focus loss while dragging cancels silently. Queries and `render_view` use the shared handle renderer.

### Terrain subdivision

- `preview_terrain_subdivision` takes optional `faces` (defaults to selected faces), with 1–16 quad cages. It generates rounded geometry from base faces using four Catmull–Clark rounds and fixed base boundary edges, sampled at each existing power. Existing sculpting is replaced; alpha, IDs, start corners, materials, projections, flags and tags remain. Results report `faces_changed`, `shared_edges`, `vertices` and the owned `state.preview_token`.
- Inspect `get_terrain_grids` with `preview: true`, then `commit_face_preview` or `cancel_face_preview`. Apply is one Undo step; repeated generation from the same base cage changes nothing. `get_terrain_grids` now reports each face's saved `subdivided` marker.
- Supported generated/imported marked grids remain editable through existing sculpt, stroke, vertex-move, blend, sew and resolution operations. Actual opaque blocks and unsupported geometry caches retain their guards. Half-edge, ambiguous, corner-only/non-manifold or inconsistently oriented cage connections reject before replacing a valid preview.
- UI: **Edit terrain… → Operation → Subdivide base cage → Preview → Apply / Cancel**. Compilation uses the saved explicit rows and its existing mixed-resolution LOD rules; there is no extra compiler setting.

### Source model previews

- `inspect_model` takes a `path` under `models/` and optional zero-based `skin` and native `body` number. It reads MDL44/48, VVD4 and DX90 VTX7 LOD0 data on the CPU, returning geometry counts, selected-body material paths, skin count, local/hull bounds and resource limits. `pose` is `static` or `reference`; `reference_bones` counts decoded pose bones, and `static_prop` reports asset eligibility. It preserves map/history and does not initialize graphics.
- `inspect_entity_model` takes optional `id` (defaults to the selected entity). It reports the shared asset, authored skin, transformed bounds, displayed/hidden state and helper fallback reason from the rendered draft. Static meshes, checked reference poses and selected bodygroups at scale 1 and LOD 0 are supported. Unsupported/missing/malformed assets remain helpers and inspection errors explain unavailable data.
- Existing `create_entity`, `set_entity_properties`, transforms, `pick_at_pixel`, `raycast`, camera focus and `render_view` author, inspect and view the same model instances as the editor. Loaded meshes replace helper-box picking; model bounds drive framing and shared selection/transform bounds. `select_area` and `select_volume` match transformed asset bounds for loaded models. Contained tests require the full box; intersecting tests include touching bounds and empty space inside them. Hidden models are excluded; unsupported assets retain helper bounds.
- `reload_model_assets` clears the bounded CPU model cache and refreshes geometry/picks after MDL/VVD/VTX changes, without editing source/history. Use `reload_material_assets` for changed VMT/VTF contents or newly available/deleted model material paths; that operation re-resolves cached model bindings while retaining decoded geometry. Model inspection reports `material_error` when resolution fails, and the mesh retains its bounds and picking with missing textures. Reloading model geometry cancels pending grid-snap previews and reports `snap_preview_cancelled`; other transform drafts remain intact.
- UI: model entity → **World Model**, **Position**, **Rotation**, **Skin**, **Model details… → Reload models / Cancel**. Files are limited to 64 MiB each; the cache holds at most 128 entries/256 MiB, and a scene has at most one million model vertices/six million indices. These are HammeRR resource caps.

Model selection uses the same Session draft as **World Model → Browse…**:

1. Call `browse_models` with optional `query`, `group` (`all`, `used` or `folder:models/...`), `offset` and `limit` (1–100). Listings do not decode meshes.
2. Call `begin_model_choice` with an entity `id`, then `choose_model` with its `token`, a model `path`, optional zero-based `skin` and `body` (both default 0). The draft validates the entity pose without changing source, selection or camera.
3. Call `render_model_preview` with `path`, `skin`, `body` and optional `width`/`height` (64–1024; defaults 640×400) for the browser’s isolated image. `get_model_choice` inspects the guarded draft. Rendering remains lazy and does not replace the active map render cache.
4. Call `apply_model_choice` with `token` to apply model/skin/body together in one Undo step, or `cancel_model_choice` to clear only that owned choice. Finish geometry previews before opening a choice. Map, selection, definitions and geometry-preview changes reject stale choices. Cancellation also accepts an owned stale choice.

The browser targets a single point entity’s `model`, `skin` and `body`; other model-reference fields and property drafts retain their existing text operations. `reload_model_assets` also refreshes the path catalog. Unsupported assets remain listed and return a clear error when chosen.

`set_entity_properties` accepts `id`, `set` and `remove`; unknown argument names are rejected. Use `set: {"skin": "5"}` to edit a property.

`get_objects`, `find_objects` and the shared object report use the same committed model bounds, retaining their existing response schemas and intersection filter semantics. Frame/map-projection navigation, entity context guides and local transform extents use model bounds; model local axes follow the same Source orientation as rendering, including legacy up/down angles. Face selection remains limited to brush/terrain faces.

Skeletal models use composed default bone transforms, inverse bind matrices and checked vertex weights for a reference pose. Animation sequences, IK/procedural poses and flexes are not evaluated. `choose_model` rejects a nonstatic asset for `prop_static`; choose a compatible entity such as `prop_dynamic`. Isolated images can inspect either asset kind. The compiler’s static-prop eligibility checks remain strict, while dynamic entity model/animation properties retain their normal native entity records.

`inspect_model.bodygroups` lists group indices/names/labels, variant indices/labels, active choices, blank flags and triangle counts. `choose_model_bodygroup` takes `token`, `group` and `variant` indices, preserves other group contributions, and uses the same operation as the picker’s named dropdowns. Blank selections return `blank: true` and null `local_bounds`; their previews show an editor helper. Entity inspection distinguishes `displayed` mesh state from `helper` state.

`get_model_choice` accepts an optional `token`. Omitting it reports the active token and `valid` state, including a stale token that can still be cancelled. With an explicit token it retains strict source/ownership checks. `begin_model_choice` with `resume: true` reuses a valid choice for the same entity, as opening the human picker does. An open picker mirrors agent changes to its token; an unseen change at Apply refreshes the preview for review before applying. Static props require body 0; use a compatible dynamic entity for bodygroup changes.

### Model attachment points

`inspect_model.attachments` lists the local MDL attachment records: native `index` starting at **1**, `name`, zero-based `bone`, raw `flags`, `world_aligned`, model-space `position` and X/Y/Z `axes`. `inspect_entity_model.attachments` gives the corresponding world positions/axes for a supported visible entity in the rendered draft. World-aligned attachments retain their transformed position and identity world axes. Hidden/unsupported entities have no world attachment list.

1. Inspect the entity and choose a named attachment's one-based index.
2. Call `show_model_attachment` with `id` and `attachment` to show the same axes as **Model details… → Attachment → Show attachment axes**. It returns an owned `token`; source, camera, selection and history remain unchanged. Finish geometry previews first.
3. Call `get_model_attachment_guide` to retrieve its current token, attachment data and `valid` state. Changed source, selection, assets or geometry previews invalidate the guide and hide it. Stale tokens remain available for cleanup.
4. Call `frame_model_attachment` with `id`, `attachment` and optional positive `aspect` (default 1) to frame the point. It changes camera only and does not require or replace a guide.
5. Call `clear_model_attachment` with the captured `token`. Foreign/replaced tokens fail without removing the replacement.

Attachment inspection uses the same checked default bone pose as model rendering. It does not evaluate animation, IK, procedural or included-model virtual attachments, and does not parent or move map entities. The decoder permits at most 1024 local attachments per model as a HammeRR resource cap.

### Sound exploration and audition

1. `browse_sounds` searches mounted WAV/MP3 paths and names from the active sound-script manifest. Optional `query`, `group` (`all`, `files`, `scripts`, `used`, `folder:…`, `script:…`), `offset` and `limit` (1–100) match the human browser. Results include groups, variant counts, ambiguity and catalog warnings.
2. `inspect_sound` takes `value` and optional zero-based `variant` (default 0). It returns native script metadata, source wave/path, codec/size and known source rate/channels/duration. It does not initialize audio or graphics.
3. `begin_sound_choice` takes entity `id`, a writable FGD sound `key` (for example `ambient_generic.message`) and optional `resume`. `choose_sound` takes its owned `token`, `value` and optional `variant`. `get_sound_choice` checks a token; omit it to discover the active or stale token. These operations preserve source/history/camera/selection and do not autoplay.
4. `apply_sound_choice` writes the captured field in one Undo step; `cancel_sound_choice` clears only its captured token, including stale choices. Script choices write the script name, while `variant` selects audition only. Opening the same human field resumes a valid agent-prepared choice; unseen changes require review at human Apply. Finish geometry previews before browsing.
5. `set_sound_preview_volume` sets shared next-preview volume from 0 to 1. It leaves game volume and current playback unchanged. `get_sound_preview` returns that setting and the current playback token/state.
6. `render_sound_preview` takes `value`, optional `variant` and optional `volume` (shared volume by default). It returns metadata and an MCP `audio/wav` content block containing the same bounded PCM audition as human **Play**. It does not use local speakers. `play_sound_preview` uses local audio and returns an owned token; `stop_sound_preview` stops only that token. Failed preparation and foreign stops preserve existing playback. Finished/failed playback retains its token for cleanup.
7. `reload_sound_assets` clears the catalog, invalidates pending choices and stops playback. The human equivalent is **Reload sounds**.

Audio helpers require FFmpeg/ffplay. Input files are limited to 16 MiB, output to 2 MiB/ten seconds/stereo 44.1 kHz, and decoding to five seconds. Paths stay relative to mounted sound content; script loading and traversal are bounded. Previews do not simulate native channel mixing, sound level, pitch/random ranges, DSP, distance, looping, sentences/voice or soundscapes. See [Sounds](sounds.md) for catalog limits and supported scope.

### Particle definitions and choices

`browse_particles` searches system names, PCF paths and materials with `query`,
`group` (`all`, `used`, `folder:particles/...` or `file:particles/...pcf`),
`offset` and `limit` (default 60, range 1–100). It returns paged `entries`, exact
`total`, groups/counts, active `manifest`, file count and limits. Each entry
identifies `name`, `source`, `material`, `precache` and `ambiguous` state.
`inspect_particle {name}` adds authored `max_particles`, function names, child
systems, encoded size, element count, fingerprint and encoding/version fields.
Search and inspection are CPU-only and preserve map/history/camera.

1. Call `begin_particle_choice` with an `info_particle_system` point entity `id`
   (selection by default); `resume: true` reuses its valid choice.
2. Call `choose_particle {token, name}`, then inspect `get_particle_choice`.
   The shared draft reports captured entity, field/value and particle metadata.
3. Call `apply_particle_choice {token}` for one undoable `effect_name` edit, or
   `cancel_particle_choice {token}`. A stale owned token remains cancellable;
   replaced tokens cannot clear another choice. The human equivalent is the
   entity field's **Browse… → Apply / Cancel**.

Map, selection, FGD, geometry-preview or particle-asset changes invalidate a
choice. Omit the token from `get_particle_choice` to discover its current token
and validity. Apply rereads the manifest/definition and rejects changed data.
`reload_particle_assets` clears the catalog and invalidates choices; reopen the
choice afterward. Failed choices retain prior drafts; failed catalog loads
remain retryable. Duplicate names are listed but cannot be chosen.

Discovery reads only mounted `particles/particles_manifest.txt` and supported
DMX binary 2 / PCF 1 files. Map manifests and unlisted PCFs are outside discovery;
includes, other encodings/versions and external element references reject.
Budgets are 128 files, 16 MiB per PCF,
64 MiB total, 16,384 systems and 16 MiB catalog metadata; names are at most
255 bytes and must be representable in VMF. Definition metadata is available;
particle simulation, animation and viewport rendering are not. See
[Particle effects](particles.md).

### Model tint

`get_entity_inspector` exposes dynamic-model `rendercolor` as the common **Tint** color control. Use the existing `set_entity_color` with `id`, `key: "rendercolor"` and byte `color: [r,g,b]`; omit `intensity`. This RGB edit preserves an authored fourth component and commits in one Undo step. `set_entity_component` and `set_entity_properties` retain their existing generic access. `render_view` shows the same modulation as the viewport, and `inspect_entity_model.tint` reports RGB bytes for supported model instances (`null` when unavailable). `opacity_preview: false` distinguishes the current RGB-only support. Asset-only browser images keep their untinted source materials.

Tint fields use three integer channels from 0 to 255 with an optional preserved fourth component. White/default static props are unchanged. The current static-prop writer has no color field, so nonwhite static tints return helper diagnostics and fail compilation without replacing an earlier BSP. Opacity/render modes and scaling remain separate gaps.

### Surface placement

1. Call `preview_surface_placement` with optional `ids` (selection by default), `method` (`floor`, `ceiling`, `ray` or `brush_corner`), `align`, `offset` and `texture_lock`. Floor is the default; ray and corner placement require `ray_origin` and `ray_direction`. It creates the same preview as **Place on surface…** without changing the source or history.
2. To share interactive picking, call `begin_surface_placement_pick` with `ids`, optional `method` (`ray`, the default, or `brush_corner`), placement options and the corner identity when required. It returns an owned `token`. Call `pick_surface_placement` with that token, `origin` and `direction`, or let a person click a viewport surface. Misses and unsupported hits preserve the pending pick.
3. `get_surface_placement` reports the active or stale token, `valid`, `picking`, `method`, selected IDs, options and any error. Its `plan` includes method, target ID/solid/side, hit point, geometric normal, pivot, translation, rotation, support-point count and achieved contact offset. An optional token checks ownership.
4. Inspect with `render_view` or frame with `frame_selection`. Call `apply_surface_placement` with the token for one Undo step, or `cancel_surface_placement` to discard only that placement. No-op placements create no Undo entry. Foreign tokens fail without clearing replacements.

Source, scene or selection changes invalidate the draft. Other geometry previews must be finished first. Opening the human window resumes a valid shared placement; replaced tokens require reopening for review. The whole selection moves rigidly, with actual brush/terrain/model vertices and helper-bound fallbacks. This operation does not simulate physics or resolve other contacts. See [Surface placement](placement.md) for target, alignment and resource limits.

For `method: "brush_corner"`, pass `source_corner: {solid, sides}` from a vertex
reported by `get_brush_components`: the numeric brush ID and complete incident
side-ID set, with no raw position field. `ids` must identify exactly one world
brush or brush-entity owner containing that corner. The mode translates the
whole owner so the source vertex meets the nearest real corner of the first
visible flat brush face hit by the ray. Equal-distance ties use incident-side
identity. No grid rounding, rotation or surface offset is applied; `align: true`
or nonzero `offset` rejects. Other methods do not accept `source_corner`.

Read captured `source_corner` / `source_position` even while picking, then
`plan.corner.source`, `plan.corner.target` (solid/sides), `plan.corner.position`
and `plan.translation` before Apply. `plan.contact_offset` is null for corner
snapping. Both brushes require checked 4–64-side topology; every translated
child, including hidden brushes, must remain within ±65,536 units. Hidden/tool/
working-region exclusions apply. Model/point sources, displacement source
brushes and terrain/model target vertices reject; an unsupported first hit is
not bypassed. Invalid requests retain a valid plan. Existing Apply/Cancel and
Undo rules apply; an already coincident snap creates no history. Human controls:
**Reshape brushes… → Vertices**, select one corner, then **Place on surface… →
Snap selected corner → Pick target corner → Apply placement**.

### Polygon brushes

Call `create_polygon_brush` with `outline: [[u,v],…]`, `base` and positive `depth`. Optional `axis` is `x` (YZ pairs), `y` (XZ) or `z` (XY, default), `material` defaults to nodraw and `snap` defaults to the current editor grid setting. The last corner connects to the first implicitly. Three to 64 distinct corners may describe a convex or simple concave boundary; concavity is decomposed into editable convex brushes without filling the omitted area.

Set `preview: true` to preserve source, selection and history. The existing `get_primitive_preview`, `frame_primitive_preview`, `commit_primitive_preview` and `cancel_primitive_preview` use its guarded string token. Preview state includes the snapped `spec.outline`, axial bounds and generated brush IDs. The human **Draw polygon…** window resumes these same corners. Apply is one Undo step; failed outlines preserve a previous valid preview. `create_primitive` also accepts `shape: "polygon"` with `outline` and matching `mins`/`maxs`; the dedicated tool calculates bounds automatically.

Crossings, touching edges, repeats, collinear corners, invalid dimensions and collapsed results reject atomically. The complete extrusion must fit ±65536 units with depth at least 0.125. Drawing planes are axis-aligned; holes are unsupported. See [Polygon brushes](polygon-brushes.md).

### Merge brushes

Call `get_merge_preview` for source candidates, owners, bounds, materials, the latest `stamp` and any current preview. Pass `sources: [numeric brush IDs]` and that `stamp` to `preview_merge`. Choose 2–16 distinct world brushes or children of one brush entity whose complete combined volume is convex. Gaps, concavities, cavities, mixed owners and displacement-bearing brushes reject without replacing a valid draft.

Inspect `preview.result` for the resulting brush and per-face inheritance; `render_view` and `get_brush_components` show the draft. Pass the exact string `preview.token` to `commit_merge` for one Undo step or `cancel_merge` to discard it. Changed sources, selection or competing geometry previews invalidate Apply; an old token cannot cancel a replacement.

The lowest brush ID supplies brush/editor metadata. Outer faces keep their original IDs, material, world texture projection, lighting and unknown fields; coplanar conflicts choose the lowest brush ID, then side ID. Internal faces disappear and unrelated entity children remain. Limits are 64 faces per input/output brush and 256 coverage-proof fragments. See [Merge brushes](merge.md).

### Repeated placement

Call `get_repeat_array`, then pass its `snapshot` to `preview_repeat_array` with
`count` and optional `ids`, `space`, `translation`, `rotation`, `scale`, `pivot`
and `texture_lock`. IDs default to selection, translation/rotation to zero,
scale to `[1,1,1]`, pivot to the source center and texture lock to true. `count`
is the number of additional copies; `space` is `world` (default) or `local`.
Copy n starts from the originals, scales each axis by its factor raised to n,
rotates by n times the XYZ degree step, then moves by n times the translation
step. Scale and rotation share the fixed world pivot; all steps use the chosen
frame. Local mode captures the first original source's axes once, in stable ID
order (world brushes before entities). `preview.space`, `basis` (column vectors),
`frame_source` and `scale` expose the plan; `preview.copies[].scale` gives each
copy's effective factors.

Scale factors must be finite and positive. Nonidentity scaling supports brushes
and brush entities, including checked terrain; point/model entities reject.
With `texture_lock: true`, textures stretch with scaled geometry. Overflow,
invalid geometry and malformed local source orientation retain the valid draft.

Inspect `preview.copies`, generated names, copied entity fields and bounds, or render the shared draft. Pass its numeric `preview.token` to `apply_repeat_array` or `cancel_repeat_array`. Apply rechecks the complete result, selects the copies and adds one Undo step. Invalid requests retain the previous valid draft; stale tokens cannot apply or cancel replacements. `get_map_info.repeat_array` exposes the same state; `get_objects`, saving and compiling use committed geometry.

Unique targetnames become `name_copyN` with collision suffixes. Supported exact internal FGD/I/O/parent references follow each copy; external links remain. Ambiguous names, uncertain internal references, internal wildcards, generated-name reference captures, overlays and numeric node/side references reject. Named source sets with scripts or dynamic I/O also reject. Design notes copy within their normal budgets; saved face partitions stay on originals. Limits are 16 sources, 64 additional copies, 256 generated objects including owned brushes, 8192 sides, coordinates within ±65,536 and 128 UTF-8 bytes per generated name. Surface sprinkle remains unavailable; see [Repeated placement](repeated-placement.md).

### Working region

`get_working_region` reports one temporary viewport box, its `snapshot`,
`enabled`, `mins` / `maxs`, and paged `objects` with `id`, optional `owner`,
`included`, `reasons`, `bounds`, `bounds_source` and `bounds_error`. Pagination
uses `offset`, `limit` (default 50, maximum 200) and `next_offset`; `total`,
`excluded` and `selected_excluded` count the complete report. Reasons distinguish
`authored_hidden`, `owner_hidden`, `tool_filter`, `outside_working_region` and
`unresolved_bounds`.

Pass the latest snapshot to `set_working_region` with one action:

- `set`: ordered finite `mins` and `maxs` within ±65,536, and optional `enabled`
  (default true).
- `selection_bounds`: capture current object/face selection bounds; accepts
  `enabled`, requires a nonempty selection and accepts no explicit bounds.
- `clear`: remove the box and disable filtering; accepts no bounds or enabled flag.

Whole world brushes and whole brush owners remain when their bounds touch or
intersect the box. Hidden children do not expand an owner's visible bounds.
Terrain uses displaced bounds; loaded models use transformed mesh bounds, with
explicit helper fallback when unavailable. Unresolved bounds remain visible
and are reported.
The existing tool-texture filter still applies. Scene rendering, spatial
picking and component/partition/paint queries share the filter; active spatial
gestures are cancelled when it changes. Authored source, saved hide/show,
selection, camera, Undo, compile scope and owned geometry/overlay drafts remain.
Map replacement clears the filter.

`get_map_info.working_region` and `render_view` metadata's `working_region` are
lightweight summaries with `counts_scope: "working_region_and_authored_visibility"`;
their counts are null when disabled. Full inspection uses
`counts_scope: "all_display_filters"`, including tool-filter reasons.
The human controls are **Working region…** and **Clear working region**.
This is a display filter, independent of compile regions; see
[Working region](working-region.md).

### 2D sky preview

`set_view` accepts `sky_preview: true` to show the world's `skyname` through authored sky surfaces. `render_view` accepts the same temporary override without changing the shared view. The default is false. Preview requires perspective Textured mode, `tool_textures: true` and six supported mounted LDR sky faces.

`get_map_info.skybox` and `render_view` image metadata report `name`, `ready`, `summary` and per-face `suffix`, `material`, `issue` and `size`. An empty `skyname` returns `name: null` and `ready: false`. Missing or unsupported faces leave the ordinary tool surfaces visible. Axis-aligned VMT scale/translation and unequal face dimensions are supported, within 4096 pixels per axis and 64 MiB per texture file.

Preview controls leave VMF and Undo unchanged; author `skyname` through the existing world-property draft. Sky surfaces retain depth, picking and visibility filtering. Planar, flat, wireframe and diagnostic views retain their usual behavior. 3D sky-camera transforms, HDR, fog, exposure and animated materials are outside this preview; see [2D sky preview](sky.md).

### Fog preview

`get_fog_preview` returns shared `enabled` and explicit `controller`, a review
`snapshot`, parsed `settings`, `ready`, `active`, `issue`, `summary`, `defaults`,
and all authored `controllers` with per-choice settings/errors. No controller is
chosen automatically; preview starts off. `set_fog_preview` takes `enabled`,
the latest snapshot and a `controller: "entity:<id>"` when enabling. Unsupported
controllers and stale snapshots preserve current settings. Disable with a fresh
snapshot. These are the same controls as **Fog preview… → Apply preview**;
**Cancel** leaves shared settings unchanged.

`render_view` accepts temporary `fog_preview` and `fog_controller` overrides.
Its `fog_preview` metadata identifies the effective settings/controller and
`applied` state; `view.fog` contains parsed parameters or null.
`get_map_info.fog_preview` reports shared state. Overrides leave the shared
choice unchanged. Authored edits and Undo refresh the selected controller;
invalid/deleted selections report an issue and draw no fog. Opening another map
clears the choice. Preview never changes source/history, selection or picking.

Only perspective Textured surfaces and model meshes receive this static
view-depth approximation. Sky/background, helpers, selection outlines, grid,
planar views and other surface modes retain their output. Explicit RGB and
finite `0 ≤ fogstart < fogend` with a finite inverse range are required;
`fogmaxdensity` is 0–1 and enabled flags are 0/1. Missing `fogenable` defaults
to 0 and missing density to 1. Invalid/duplicate keys, radial or
directional blending, positive `farz` and nonidentity HDR/zoom scales reject.
The capped linear range factor is squared for linear-space color blending;
fog volumes, water fog, runtime I/O, transitions and native parity are outside
this preview. See [Fog preview](fog.md).

### Recent maps

`get_recent_maps` returns up to 20 entries in most-recent-first order with exact `path`, file availability, retained game directory, six-field `build_settings` and `context_compatible`. It also reports persistence, storage errors and bounds. Pass a listed `path` to `open_recent_map`, optionally with `discard_changes: true` when discarding the current edits is intended. It uses normal map validation and restores retained build values only while idle and with matching mounted game content. Game content and custom FGD files do not switch automatically.

`remove_recent_map {path}` and `clear_recent_maps {}` change only the retained list. Successful opens/saves update recency; changing build settings updates an existing entry without reordering it. Missing files remain listed. Failed primary opens/saves preserve the list; an auxiliary history-write failure is reported without failing an otherwise successful map operation.

Preferences use `recent-maps.json` in recovery storage. `HAMMERR_RECENT_MAPS_DIR` selects an absolute directory; otherwise `HAMMERR_RECOVERY_DIR` also isolates this list. Paths are bounded to 4096 UTF-8 bytes and storage to 192 KiB. Invalid loads block preference changes until repaired and restarted; failed writes preserve the previous list and can retry. Separate editor processes do not coordinate writes. See [Recent maps](recent-maps.md).

### Build recipes

`get_build_recipes` returns built-in and custom recipes with all six settings and readable descriptions, registry `revision`, current `active` identity, editability, persistence, path and errors. Built-in IDs `fast` and `final` are immutable. Fast preview uses Fullbright, no physics, Fast visibility/sampling, no bounces and 0° smoothing. Final lighting uses Direct LDR, engine physics, Detailed visibility/sampling, four bounces and 45° smoothing, retaining the compiler's existing requirements and limits.

| Tool | Arguments and effect |
| --- | --- |
| `save_build_recipe` | `name`, optional `replace_id` and `revision`; captures all current settings as a new custom recipe or replaces the chosen custom recipe. |
| `rename_build_recipe` | `id`, `name`, optional `revision`; changes only a custom recipe's name. |
| `delete_build_recipe` | `id`, optional `revision`; removes a custom recipe while retaining current compiler values. |
| `apply_build_recipe` | `id`, optional `revision`; applies all settings through the shared settings operation without starting a build. |

Pass the returned registry revision to reject stale edits. Active or cancelling jobs block every recipe mutation and Apply. `get_compile_settings.recipe` identifies the current applied recipe or is null; a changed value clears it. `get_compile_status.job.recipe` keeps the identity captured when that build started, including after rename/deletion. Names alone do not stale output. These operations leave VMF and map Undo unchanged.

Custom recipes use `build-recipes.json` in recovery storage; `HAMMERR_BUILD_RECIPES` overrides the file and `HAMMERR_RECOVERY_DIR` isolates the default. Limits are 32 customs, unique names ignoring case, 1–80 UTF-8 bytes per trimmed name without control characters and a 65,536-byte file. Invalid loads block custom edits while built-ins remain usable. Failed writes preserve preferences and can retry; detected disk changes require restart. Simultaneous writes are not coordinated. Loading recipes does not apply a custom recipe automatically. See [Build recipes](build-recipes.md).

### Prop shadows and occluders

Existing synchronous and background compile results expose `prop_shadow_props`,
`prop_shadow_models`, `prop_shadow_triangles`, `prop_shadow_disabled` and
`prop_shadow_work`, also shown in **Build details…**. Direct LDR shares LOD0
prop blockers across direct, sky, bounce and ambient rays, honoring native
`disableshadows`. No new compile setting or tool is required.

Models compiled with `$casttextureshadows` request supported alpha-tested
materials through their native MDL flag; there is no editor toggle. Results expose
`prop_shadow_alpha_triangles`, `prop_shadow_alpha_materials`,
`prop_shadow_alpha_textures` and decoded `prop_shadow_alpha_bytes`. Cutouts use
barycentric mesh UVs, mip-0 nearest alpha, VTF wrap/clamp and an inclusive
`$alphatestreference` threshold. They remain two-sided blockers with no fractional
transmission. Unflagged models retain opaque mesh shadows and material warnings.

Requested cutouts support bounded VertexLitGeneric/UnlitGeneric materials,
axis-aligned scale/translation, and one-frame 2D textures up to 4096 pixels per
axis / 64 MiB total decoded alpha. Unsupported shaders, rotated transforms,
animation/conditional blocks, translucent/additive effects and missing/corrupt
required assets fail the build without replacing prior output. Prop vertex/
lightmap baking and animated props remain unavailable. See
[Static-prop shadows](prop-shadows.md).

Create `func_occluder` through the existing brush-entity operations, set `StartActive`, and author its Activate/Deactivate/Toggle connections through the existing entity tools. Compiles report `occluders`, `occluder_polygons` and `occluder_vertex_refs` with the same entity-linked diagnostics as the human build panel. Each helper supports 1–16 visible convex brushes in one unambiguous world area. Pairwise world bounds must be separated on an axis by more than 0.01 units; touching/overlapping bounds require separate entities until CSG support. Combined limits are 256 input sides/polygons and 4096 polygon vertices. Native records, activation indices and Activate/Deactivate/Toggle control runtime culling. See [Occluders](occluders.md).

### Cut, copy and paste

`get_clipboard` returns the current `snapshot`, detached `payload`, owned paste `preview`, `can_restore_cut_names`, format and bounds. Inspection never reads the system clipboard. Copy/import preserves geometry, camera, selection and map Undo. Payloads survive source edits, Undo and map replacement. Whole VMF text transfers only objects, required native groups/layers and attached object notes; world settings, map notes, regions and cameras stay local.

| Tool | Arguments and behavior |
| --- | --- |
| `copy_objects` | Latest `snapshot`, optional `ids` (selection by default), `destination: "session"` (default) or `"system"`. Fully captures before replacing the old payload. |
| `cut_objects` | Same arguments; validates capture and deletion, publishes the requested clipboard, then removes sources in one Undo step. Publication failure leaves objects and prior clipboard intact. |
| `export_clipboard` | Exact `payload.content_revision`; `destination: "response"` (default) returns portable VMF `text`, or `"system"` publishes it. |
| `import_clipboard` | Latest `snapshot`, plus `text` or `system: true`, exclusively. Invalid text preserves the old payload and valid preview. System reads are explicit. |
| `preview_paste` | Latest `snapshot`, optional XYZ `offset` (zero/in place), `texture_lock` (true), `keep_external_links` (false) and `restore_cut_names` (true only for an unchanged same-map Cut). |
| `apply_paste` / `cancel_paste` | Exact owned `token` from `preview_paste`. Apply checks the full regenerated VMF/result and records one Undo step. Old tokens cannot cancel replacement tools. |

`payload.external_links` reports outgoing references, source fields and current destination matches. External links need explicit keep-by-name acknowledgement. Unique generated names and certain exact internal FGD/I/O/parent links are remapped together. Generated-name acquisitions, ambiguity, numeric/overlay references and named script/dynamic-I/O dependencies reject. Unknown object metadata remains verbatim.

Original-name restoration requires the exact unchanged post-Cut revision, document generation, entity definitions and clipboard content. Opening another map, Undo or another map edit disables it; imported data cannot manufacture a receipt. Cancel affects only the paste preview, leaving a completed Cut in Undo.

Limits: 16 top-level sources, 256 objects including owned brushes, 8192 sides, 256 groups, 256 sides per brush, 8 MiB text, 64 nesting levels, 131072 KeyValues nodes and ±65536 authoring coordinates. Up to 256 required layers and their native memberships transfer with fresh independent IDs, reported in `visgroup_ids`; destination layers are never merged by name or number. Saved partitions and direct layer links on physical group records reject. Native clipboard support is UTF-8 VMF text through the available platform backend; private Hammer formats are unsupported. Use response export/import between independent headless sessions. See [Cut, copy and paste](clipboard.md).


### Native layers

`get_layers` inspects the native hierarchy, names, RGB colors, direct memberships
and whole-object selection counts. `query`, `offset` and `limit` page records
(default 64, maximum 128). `selected` plus `member_offset` pages 128 direct
members with raw `id`, whole-object `owner` and inherited hidden state.
`parent_choices` lists all bounded native parent choices. Inspection is read-only. The human window uses **Review pending edit** to read and adopt the same reported draft token/version; it never adopts a newer agent edit implicitly.

| Tool | Arguments and behavior |
| --- | --- |
| `begin_layer_edit` | Latest `snapshot`, optional layer `id`; omit it to create from current selection. `delete: true` stages deletion of an existing layer. |
| `update_layer_edit` | Exact draft `token` and `version`, complete `name`, RGB `color`, optional/null root `parent` and full direct `members`. Returns a new version and `valid`/`error`. Whole entity and owned-brush solid IDs are supported. |
| `apply_layer_edit` | Latest owned `token`/`version`; one atomic Undo step for record and membership changes. Map replacement or revision changes reject. |
| `cancel_layer_edit` | Exact `token`/`version`; older windows cannot erase replacement or newer agent drafts. |
| `select_layer_members` | Latest `snapshot`, layer `id`, `recursive` (true), `include_hidden` (false), `mode` replace/add/remove/toggle. Resolves owned-brush hits to whole entities without expanding physical groups. |
| `set_layer_visibility` | Latest `snapshot`, layer `id`, `recursive` (true) and `action` hide/show. Uses shared undoable map visibility, including compiler omission. Owned-brush hits affect whole entities. |

Deleting a layer promotes its children and preserves all objects and unrelated
memberships. IDs use an independent monotonic namespace; imported unsigned IDs,
including zero, remain addressable. Limits are 2,048 records, 64 nesting levels,
32,768 distinct memberships and 8,192 members in one edit. New names contain
1–128 characters without controls, quotes or surrounding whitespace. Unknown
layer metadata is retained within an 8 MiB / 131,072-node / 64-level budget.
Malformed graphs and direct layer links on physical group records reject.
Automatic categories and separate editor-only filters are unavailable.
See [Layers](layers.md).


### Moving brush contact

Create `func_movelinear` with `tie_to_entity` and set `movedir`, `movedistance`,
`speed` and `startposition` through shared entity properties. Connections expose
Open/Close, SetPosition/SetSpeed and OnFullyOpen/OnFullyClosed. Engine physics
supports vertical platform contact with `prop_physics`; rotating/blocking
contacts and broader moving-body behavior remain unfinished. See
[Moving platforms](compile-and-play.md#moving-platforms).

### Prefab library

`get_prefabs` lists named portable VMFs with `query`, `offset` and `limit`
(default/max 64); `selected` decodes one assembly's counts, authored bounds and
outgoing links. Inspection changes no map, clipboard or history. The response
reports the storage folder, limits and a `snapshot` binding library file metadata
to the current map/selection/clipboard state.

| Tool | Arguments and behavior |
| --- | --- |
| `save_prefab` | Latest `snapshot`, new `name` and optional whole-object `ids` (selection by default). Captures before atomically creating a new file. Never overwrites; map/history/clipboard stay unchanged. |
| `preview_prefab` | Latest `snapshot`, `name`, optional world `position` (bounds center; defaults to zero) and `keep_external_links` (false). Privately validates before changing clipboard/preview state. Returns `paste.preview.token` for existing `apply_paste` / `cancel_paste`; rendering uses that same draft. |
| `delete_prefab` | Latest `snapshot` and exact `name`. Removes only its library VMF, separately from map Undo. Placed copies and loaded detached paste drafts remain usable. |

Placement retains clipboard name/reference/ID checks, native group/layer remapping,
terrain and object notes. Cut-name restoration is always disabled. Source files,
geometry and map history stay unchanged until Apply; failed loads/placements
retain the prior clipboard and draft. Assets remain external references.

Limits: 128 VMFs, 8 MiB each, 64 MiB total, 512 scanned directory entries and
1–80 UTF-8-byte names using letters/numbers/spaces/dashes/underscores. The usual
clipboard geometry/reference limits apply. `HAMMERR_PREFABS_DIR` overrides the
normal recovery/state folder's `prefabs` child. External writers are uncoordinated;
changed file metadata or Session state invalidates stale requests. Direct library
replacement/renaming, private formats and referenced instances are unavailable.
See [Prefab library](prefabs.md).

### Overlay authoring

| Tool | Inputs and result |
| --- | --- |
| `get_overlay_state` | Current `snapshot`, eligible selected source or `selection_error`, eligible `selected_overlay` or `refit_error`, and draft operation/entity, owner/token/validity, options, corners, basis and bounds. |
| `browse_overlay_targets` | Search existing overlays by `query` (name/material/ID, at most 256 bytes), `offset` and `limit` (default 20, maximum 50). Returns snapshot, exact total and `entries` with numeric `id`, `entity` identity, name/material, source/settings, `eligible` and rejection `error`. Does not change selection. |
| `preview_overlay` | Latest `snapshot`, explicit `source:{solid,side}` and `material`; optional `targetname`, `render_order` (0–3), `u`/`v` endpoint pairs and `fade_min`/`fade_max` world distances. Fits a new owned draft without editing source/history/selection. |
| `preview_overlay_refit` | Latest `snapshot` and optional overlay `id` (defaults to selection); previews refitting its one existing visible world/detail quad binding. Keeps identity, raw settings and metadata. |
| `preview_overlay_rebind` | Latest `snapshot`, existing overlay `id: "entity:<id>"` and a different destination `source:{solid,side}`. Both bindings must be eligible visible world/detail quads. Returns `operation: "rebind"`, same entity and `previous_source`; preserves raw settings and metadata. |
| `review_overlay` | A valid draft `token`; transfers ownership and issues a fresh token without editing the map. |
| `apply_overlay` | Current owned `token`; one Undo step. `operation: "create"` returns `created`; `refit` and `rebind` return `updated` for the same entity. |
| `cancel_overlay` | Matching owned `token`; discards only that draft, including a stale owned draft. |

Creation supports one visible convex planar world or `func_detail` quad,
including tilted trapezoids. Defaults are order `0`, U/V `0–1`, fade start `-1`
and end `0` (disabled). Preview fits the actual source corners; it does not accept
derived basis/corner fields. The compiler and authoring share projection checks.
Source availability and final surface budgets are checked during compilation.

Map/selection changes stale Apply and Review. Invalid requests preserve valid
drafts; replaced tokens cannot cancel their successors. Explicit sources work
without changing face selection; humans can open **Overlay…** and review that
draft directly. `render_view` shows the cyan outline; `preview_material` exposes
the same material thumbnail. Refit and rebind change only projection/origin/side
fields, preserving raw material/name/UV/order/fade strings, connections and
metadata. Humans can select the destination face, open **Overlay…**, choose an
existing named overlay and use **Preview rebind → Apply rebind**. Missing,
hidden, multiside, terrain, model and nonquad existing bindings reject; rebind
cannot repair those bindings. Projected viewport materials, terrain/model
creation or refitting, and automatic rebinding remain unavailable.
See [Overlays](overlays.md).

### Compile regions

| Tool | Inputs and behavior |
| --- | --- |
| `get_compile_region` | Inspect committed bounds/enabled state, metadata errors, `snapshot`, suggested bounds, draft token and limits. |
| `preview_compile_region` | Latest `snapshot` plus `enabled`, `mins` and `maxs`; or `clear:true` alone with `snapshot` to remove metadata. Returns an owned draft token without editing source/history. |
| `apply_compile_region` | Current draft `token`; one undoable metadata edit. Stale map snapshots and replaced tokens reject. |
| `cancel_compile_region` | Matching `token` only; preserves committed metadata and any replacement draft. |
| `frame_compile_region` | Optional positive finite `aspect`, default 16:9; frames current draft or committed bounds without editing geometry/selection. |

`compile_map`, `start_compile` and Run Map share one preprocessing path and use
committed metadata only. The job's `compile_region` captures its start scope;
`result.stats.compile_region` reports actual bounds, shell thickness, `boundary_material`, generated
brush IDs and included/omitted source object IDs. It is null for full-map output.
Editing the region makes earlier output stale without changing a running job.
`get_map_info.compile_region` is the lightweight current effective build scope;
`get_compile_region` also includes the owned form, suggested bounds and shared
human-facing `help` text.
Suggested terrain bounds include its base cage as well as the displaced sheet;
invalid cage geometry returns `suggested_bounds.error`.

One finite axis-aligned box is supported, at least 16 units per axis and within
±16,368. Intersecting world/detail brushes and included brush models stay whole;
point origins and resolved model pivots determine entity inclusion. References
remain unchanged. Fully contained structural world water retains the normal
native water/physics path. World/detail terrain needs its complete cage and grid
contained; wholly disjoint combined bounds are omitted. Areaportals/windows and
occluders use complete visible brush bounds rather than editor origins: contained
helpers remain whole, disjoint helpers are omitted and crossings reject.
Imported overlays retain their complete definitions regardless of marker origin;
every target must resolve to an emitted world/detail/terrain surface. Missing
window render targets and ordinary helper validation errors retain prior output.
Crossing terrain/water, detail/model water, slime, overlay transitions, instances and visclusters
reject before publication.
Runtime-model terrain remains unsupported by the normal compiler too.
Sun and water LOD controllers retain original origins/settings even outside
the region. Their editor positions do not seed region-only exterior leaks.
Visible `info_lighting` helpers remain compiler-only reference data regardless
of marker position. Retained static props keep their authored lighting origins;
hidden or duplicate named helpers still fail ordinary reference validation.
The generated boundary requires mounted solid rendering `tools/toolsblack`;
missing or incompatible assets reject without substitution.
Disable regions for full-map leak checks. Globally disabled imported plural
metadata retains full-map behavior even with enabled children. Explicit Clear
is required before replacing imported plural/multiple records. See
[Compile regions](compile-regions.md).
For imports with one effective active box, `committed`, `build_scope`,
`frame_compile_region` and the viewport guide agree even when additional disabled
boxes or roots remain. Readback preserves all imported metadata. Malformed active
records and ambiguous disabled-only sets return inspection errors.

### Reload material assets

Call `reload_material_assets` without arguments after changing VMT/VTF files in
mounted content. It reloads cached display materials and sky faces, rescans the
material catalog, re-resolves cached model material paths and refreshes
desktop/offscreen textures and thumbnails. Decoded model geometry stays shared;
preferred paths appearing or disappearing are resolved from the existing mounts.
The human equivalent is **Browse materials… → Reload material assets**.

The response contains `generation` (the display-cache generation), `reloaded`
(cached material entries reread), `models_refreshed` (cached model material sets
resolved), `models_changed` (sets whose paths/error changed), `failed` (the full
unavailable-preview/read-failure count), `failures` (the first 100
`{material, error}` details) and `summary`. Catalog
and model-path errors also appear in these diagnostics; a failure's `material`
can identify the model path. `get_map_info.material_reload`
and `render_view` metadata's `material_reload` retain the last report, or `null`
before the first reload.

Unavailable previews are not a compiler-validity verdict: valid untextured/tool
materials can have no display texture. Broken display assets replace stale
images with the missing-texture fallback. Model resolution failures replace old
bindings visibly, retain geometry/bounds/picking, and appear as `material_error`
in model inspection. Repair files and reload again; rerun map checks or compile
to validate the map.

Reload preserves map data, selection, camera, Undo history and active draft
geometry. View-dependent snapshots may need renewed review. It uses existing
mounts only, with no file watcher, archive/content remount, model-geometry or
particle reload, or running-game cache refresh. Changed MDL/VVD/VTX data still
requires `reload_model_assets`. Valid model choices and attachment guides remain
usable; stale choices remain stale, and open material/model choices require
review of refreshed previews before use.

### Material asset errors

`preview_material` returns an error for malformed VMT/VTF data, broken patch
includes or a broken preferred editor texture. `run_map_checks` /
`get_map_checks` expose these as `invalid_material` issues with affected object/
face IDs, shared with **Check map…**. Neither operation edits source or history.
`compile_map` and `start_compile` reject failed material/primary-texture reads
required by compilation and preserve prior BSPs. Absent content is separately
reported; after external asset repairs, use `reload_material_assets` to refresh
display caches and rerun checks for current map diagnostics.

Directory lookup and scanning failures propagate through `search_materials`,
`browse_materials`, `browse_models` and `browse_sounds`. A failed catalog load
remains retryable after repairing the path. A genuine missing file may be
resolved from the next configured mount; an I/O failure cannot silently select
that mount instead. `run_map_checks` reports `asset_scan_failed` and skips
existence-based conclusions when discovery fails.

`render_view` includes `preview_issues` in its metadata, each with a `face`
(`solid`, `side`) and `message`. These describe the same terrain preparation
errors shown above the human viewport, including cached renders. Invalid
patches retain checked geometry as diagnostic wireframes, or base-face outlines
when their grids cannot be built, with no replacement shading.
`run_map_checks` / `get_map_checks` expose the underlying errors without a GPU;
shared-normal failures use `terrain_preview`. Checking and rendering preserve
imported data and history.
