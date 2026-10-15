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

`--headless` runs the editor without a window and serves MCP over stdin/stdout, giving the agent a private session. Rendering still works; graphics start on the first render.

```json
{
  "mcpServers": {
    "hammerr": {
      "command": "/path/to/hammerr/target/release/editor",
      "args": ["/path/to/map.vmf", "--headless"]
    }
  }
}
```

## What agents know

When an agent connects, the server tells it how HammeRR works, so you don't need to explain:

- **Units and axes**: Hammer units with Z up. A player is 32 × 32 × 72 units. Brushes stay within ±16,384.
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
| Selection and history | `select`, `get_selection`, `set_selection_mode`, `undo`, `redo` |
| Object edits | `move_objects`, `duplicate_objects`, `delete_objects` |
| Brush drawing and transforms | `set_spatial_settings`, `draw_block`, `cancel_block_preview`, `create_block`, `transform_objects` |
| Transform handles | `set_transform_tool`, `get_transform_handles`, `begin_transform_drag`, `update_transform_drag`, `commit_transform_preview`, `cancel_transform_preview` |
| Area selection | `set_area_selection_options`, `select_area`, `get_area_selection`, `commit_area_selection`, `cancel_area_selection` |
| Volume selection | `select_volume`, `get_volume_selection`, `commit_volume_selection`, `cancel_volume_selection`, `frame_volume_selection` |
| Paint selection | `set_paint_tool`, `begin_paint_selection`, `update_paint_selection`, `get_paint_selection`, `commit_paint_selection`, `cancel_paint_selection` |
| Brush shapes | `create_primitive`, `get_primitive_preview`, `frame_primitive_preview`, `commit_primitive_preview`, `cancel_primitive_preview` |
| Hollow | `preview_hollow`, `get_hollow_preview`, `look_inside_hollow`, `commit_hollow`, `cancel_hollow` |
| Carve | `get_carve_preview`, `preview_carve`, `preview_carve_many`, `commit_carve`, `cancel_carve` |
| Clip | `set_clip_tool`, `clip_brushes`, `get_clip_preview`, `set_clip_points`, `pick_clip_point`, `get_clip_handles`, `set_clip_handle_options`, `begin_clip_drag`, `update_clip_drag`, `finish_clip_drag`, `cancel_clip_drag`, `commit_clip_preview`, `cancel_clip_preview` |
| Vertices, edges and faces | `get_component_sources`, `update_component_scope`, `get_brush_components`, `set_component_picking`, `pick_brush_components`, `select_brush_components`, `preview_component_move`, `commit_component_move`, `cancel_component_move` |
| Component move handles | `get_component_handles`, `set_component_move_options`, `begin_component_drag`, `update_component_drag`, `commit_component_drag`, `cancel_component_drag` |
| Scale, rotate, weld, split, ridge | `preview_component_scale`, `preview_component_rotate`, `preview_component_weld`, `preview_component_split`, `preview_component_crease`, `commit_component_edit`, `cancel_component_edit` |
| Flat partitions | `preview_component_partition`, `update_component_partition_cut`, `update_component_partition_material`, `update_component_partition_texture`, `raise_component_partition`, `preview_partition_extrusion`, `select_component_partitions`, `frame_component_partitions`, `pick_component_partitions`, `commit_component_edit`, `cancel_component_edit` |
| Saved flat pieces | `get_saved_component_partitions`, `store_component_partition`, `resume_component_partitions`, `remove_saved_component_partitions` |
| Face textures | `select_faces`, `get_face_selection`, `set_material`, `edit_faces`, `sample_face_texture`, `get_surface_sample`, `paint_face_texture`, `propagate_face_texture`, `project_face_group`, `commit_face_preview`, `cancel_face_preview` |
| Texture paint strokes | `begin_texture_stroke`, `update_texture_stroke`, `get_texture_stroke`, `commit_texture_stroke`, `cancel_texture_stroke` |
| Terrain vertices | `get_terrain_vertices`, `highlight_terrain_vertices`, `pick_terrain_vertex`, `frame_terrain_vertices`, `get_terrain_volume_vertices`, `select_terrain_vertices_in_volume`, `get_terrain_vertex_mask`, `preview_terrain_vertex_move` |
| Terrain brush strokes | `begin_terrain_stroke`, `update_terrain_stroke`, `get_terrain_stroke`, `break_terrain_stroke`, `finish_terrain_stroke`, `cancel_terrain_stroke` |
| Material replacement | `find_material_faces`, `get_material_search`, `select_material_faces`, `replace_materials` |
| Material browsing | `search_materials`, `browse_materials`, `preview_material`, `set_material_favorite` |
| Entities | `create_entity`, `set_entity_properties`, `tie_to_entity`, `move_to_world` |
| Entity definitions and inspector | `search_entity_classes`, `get_entity_class`, `load_entity_definitions`, `get_entity_inspector`, `set_entity_color`, `set_entity_component` |
| Connections | `get_entity_connections`, `get_connection_choices`, `begin_connection_edit`, `update_connection_draft`, `commit_connection_edit`, `cancel_connection_edit`, `delete_entity_connection` |
| Safe renaming | `get_entity_references`, `begin_entity_rename`, `update_entity_rename`, `commit_entity_rename`, `cancel_entity_rename` |
| World settings and property batches | `get_property_inspector`, `begin_property_edit`, `update_property_edit`, `get_property_edit`, `commit_property_edit`, `cancel_property_edit` |
| Object report and visibility | `get_object_report`, `select_object_report`, `set_object_visibility` |
| Map checks | `run_map_checks`, `get_map_checks`, `select_map_issue`, `preview_map_fixes`, `apply_map_fixes`, `cancel_map_fixes` |
| Design notes | `get_context_state`, `list_context_notes`, `get_context_note`, `get_context`, `begin_context_edit`, `update_context_edit`, `apply_context_edit`, `cancel_context_edit`, `delete_context_note`, `set_context_view`, `focus_context` |
| Recovery | `get_recovery`, `set_recovery_settings`, `create_recovery_snapshot`, `restore_recovery_snapshot`, `delete_recovery_snapshot` |
| Camera and visual checks | `get_camera`, `set_camera`, `frame_selection`, `set_view`, `render_view`, `pick_at_pixel`, `raycast` |
| Saved camera views | `get_camera_views`, `save_camera_view`, `restore_camera_view`, `rename_camera_view`, `delete_camera_view` |
| Pointfiles | `load_pointfile`, `get_pointfile`, `navigate_pointfile`, `frame_pointfile`, `clear_pointfile` |
| Compile and play | `get_compile_settings`, `set_compile_settings`, `start_compile`, `get_compile_status`, `cancel_compile`, `compile_map`, `run_map`, `launch_game`, `game_log`, `stop_game` |

## Good to know

These details aren't obvious from the tool list.

**Transforms**

- `transform_objects` does move, rotation, positive scale, reflection and single-axis shear, with an optional `pivot` and `space: "world"` or `"local"`. Combined operations apply scale and reflection first, then shear, then rotation, then translation.
- `texture_lock` controls textures during movement and rotation. `texture_scale_lock` controls stretching, mirroring and shear. Leaving out `texture_scale_lock` makes it follow `texture_lock`.
- `get_transform_handles.state.preview.display` holds the same summary the inspector shows.

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
- `get_compile_settings.visibility_choices` exposes **Fast (conservative)** and **Detailed (portal clipping)**, shown under **Visibility quality** in Build settings. `visibility` freezes in each job's captured settings and changing it stales prior output. Detailed refines PVS through bounded source/pass portal apertures; PAS includes the refined visible rows. Statistics expose `visibility_mode`, `visibility_work`, `visibility_removed_pairs` (directed PVS entries removed from the broad result) and `visibility_fallbacks`, plus `visibility_workers` for the largest actual worker count across the base and flow stages. Larger Fast and Detailed jobs parallelize independent base-facing portal rows on at most eight available CPUs; Detailed also parallelizes separator flow. Source portals for each cluster retain their order, and workers share one exact global work budget. Fallbacks retain conservative geometry; exceeded work/stack budgets preserve the previous output.
- `get_compile_settings` returns the current choices, including `sky_choices` with their labels and direction counts (32, 128 and 512). These tools work before the first build, like the toolbar's **Build settings…**. The UI calls `sky_quality` **Sampling quality**; it affects Direct LDR sky ambient, soft sun, model ambient and bounce rays.
- `get_compile_settings.bounce_choices` exposes **Direct only** (`bounces: 0`, default) and **One diffuse bounce**, **Two diffuse bounces**, **Three diffuse bounces** or **Four diffuse bounces** (`bounces: 1`–`4`), shown under **Indirect light**. It gathers reflected light per source style from immutable previous-pass world/detail/terrain lightmaps into world, terrain and brush-model receivers using 32/128/512 cosine-weighted rays per receiver per pass. Each pass rebuilds receivers from the original direct maps. Model rays check their own geometry before accepting a world hit; unrelated models stay separate. Source albedo must be between zero and one; sky/backfaces do not supply bounce energy. Stats expose actual `bounces`, `bounce_rays`, `bounce_samples` (final-pass receivers with positive indirect light) and `bounce_work`, within the shared lighting budget. Fullbright retains the setting and reports zero bounce work.
- `edit_faces` (including `preview: true`), `commit_face_preview` and `cancel_face_preview` drive the same authored brush smoothing as **Face lighting…**. Shared groups 1–24 blend; shared groups 25–32 keep hard edges and take priority. Direct LDR applies checked world/detail and brush-model corner normals, including eligible corners inside neighboring source edges. Original fan centers and source fans remain stable across lightmap subdivision. Stats expose `smoothed_faces`, `smoothed_corners` and `normal_work` (construction work; receiver interpolation also counts toward `lighting_work`). Unassigned faces stay flat at angle zero; terrain, world coordinates and each model's local coordinates remain separate. Source/corner limits and the shared lighting budget preserve prior output on rejection.
- `smoothing_angle` defaults to `0` (disabled). Positive values blend only two unassigned source faces within one world/model scope whose normal angle meets the threshold. Explicit smooth/hard masks retain precedence; terrain and individual model/world scopes stay separate. `get_compile_settings.smoothing_angle_range` reports bounds/unit. Each job captures the angle; changing it stales previous output. Actual stats expose `smoothing_angle` and the same `smoothed_faces`, `smoothed_corners` and `normal_work`. Fullbright retains the setting but reports zero actual angle/work; no VMF/history edits occur.
- `edit_faces` and object material edits select VMT `$surfaceprop` values for world brush and terrain collision. With `physics: engine`, native triangle mappings use a shared table of at most 127 names including default; missing properties retain default. `compile_map` and background results expose `physics_surface_properties`, shown as **Surface properties** in **Build details…**. Names use 1–127 ASCII letters, digits, underscores, dots or dashes; custom definitions must be available to the game. Invalid mappings/names/capacity reject before publication. Brush-model surfaces, material density and compiler-derived model masses remain unfinished.
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
- `reload_model_assets` clears the bounded CPU model cache and refreshes geometry/picks after model-file changes, without editing source/history. It refreshes MDL/VVD/VTX data and material search paths; existing VMT/VTF texture content remains cached.
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
