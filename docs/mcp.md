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

**Compiling**

- `set_compile_settings` takes `lighting` (`fullbright` or `direct_ldr`), `physics` (`none` or `engine`) and `sky_quality` (`fast`, `balanced` or `detailed`). It changes only the fields you pass, and invalid combinations leave every setting unchanged.
- `get_compile_settings` returns the current choices, including `sky_choices` with their labels and direction counts (32, 128 and 512). These tools work before the first build, like the toolbar's **Build settings…**. The UI calls `sky_quality` **Sampling quality**; it affects Direct LDR sky ambient, soft sun and model ambient.
- Settings cannot change while a build runs. Each job reports its captured `settings`; changing settings after a build makes that output out of date for launching.
- Create and edit baked lights with `create_entity` and `set_entity_properties`. For `light_environment`, `_light` is sunlight and `_ambient` is sky ambient; color values use `R G B brightness`. Set ambient brightness to zero with a value such as `255 255 255 0`. Set `SunSpreadAngle` from 0 to 90 degrees: zero keeps hard shadows, positive values soften them using the selected sky quality.
- `get_compile_status.launch` says whether the last build can be played, or why not.
- Build statistics include `ambient_samples` and `ambient_bytes`, shown as **Model ambient** in Build details. Direct LDR writes one safe probe per open world leaf from world-face averages and sky radiance; skipped leaves are reported in diagnostics.
- `run_map` and `launch_game` always launch CS:S with `-insecure`.
