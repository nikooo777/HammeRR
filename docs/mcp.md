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

Editor exit stops new HTTP connections and closes active sessions. Network
shutdown has a two-second grace period; a timeout remains a reported exit error.
Owned blocking work finishes before the service is released. `close_document`
closes the current document and quits its owning editor process. Multiple
documents remain unavailable.

`get_map_info.document_close` inspects the same state used by the editor's close
confirmation. It reports an opaque `review_token`, `document_id`,
`unsaved_changes`, pending `drafts`, `discard_required`, owned `work` and
`closing`. The review itself does not discard drafts, stop playback or cancel
jobs. It belongs to that document and expires when its reviewed state changes; it is
separate from `replacement_review`.

### Close the document and editor

1. Call `get_map_info` and inspect `document_close`, including unsaved changes,
   drafts and owned work.
2. Pass its exact `review_token` to `close_document`. Leave `discard_changes`
   false unless you intend to discard those reviewed unsaved changes and drafts.
3. If the review is stale, consent is missing or a request is busy, the document
   stays open. Inspect the current state before retrying.
4. Accepted Close is terminal. It stops owned game/audio, drains document work
   and recovery, and retires the desktop UI. It continues if the caller cancels
   or disconnects; do not send further editing calls.

The completed result reports `document_id`, a decimal-string `attempt`,
`terminal: true`, `closed`, `ui` (`headless`, `retired` or `failed`) and a nullable
bounded `error`. `closed` describes document/UI cleanup. A failed cleanup leaves
admission closed and reports an error; it does not reopen the document.

The host verifies output of this exact result before quitting. Private stdio
exits with stdin still open. HTTP additionally requires completion of the
original POST body and its connection; a resumed GET cannot replace it. Output
failure or timeout remains a process-exit error after document cleanup. The
response does not attest remote consumption or later host/transport success.

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

Use an MCP client's process pipes, or redirect both standard streams to pipes or
regular files. Direct terminal, socket and device descriptors are unsupported;
other platforms without Unix pipe readiness reject process stdio explicitly.
Pipe reads and writes are cancellable, including an unread full output pipe.
Failure to deliver the accepted result remains a process error.

The server accepts `--game` and `--fgd` as described in [Getting started](getting-started.md). `editor --headless` remains available with the same stdio tools. The standalone server opens no editor window and serves no HTTP endpoint. Engine-physics builds need `hammerr-physics`, and generated reflections need `hammerr-native-runtime`, beside the executable. Build these helpers with `cargo build --release -p compiler -p native-runtime`.

## What agents know

When an agent connects, the server tells it how HammeRR works, so you don't need to explain:

- **Units and axes**: Hammer units with Z up. A player is 32 × 32 × 72 units. Each tool reports its coordinate limits.
- **IDs**: `solid:<id>` for world brushes and `entity:<id>` for entities. Tools act on the current selection when IDs are left out, and creation tools select what they create.
- **Materials**: ordinary material names omit `materials/` and the extension, such as `tools/toolsnodraw`. `search_materials` finds more. Sprite Name choices use full virtual `materials/<name>.vmt` references.
- **Previews**: most edit tools accept `preview: true` and return a `token`. Pass it to the matching commit or cancel tool, so a preview someone else replaced can't be applied by mistake.
- **Stale-state guards**: inspection tools return a `snapshot`, `stamp`, or `generation` and `revision`. Passing them back makes a call fail safely if the map changed in between.
- **Checking work**: `render_view` returns a PNG from any camera without moving yours, `pick_at_pixel` identifies the object and face at a pixel, and `raycast` finds the first surface along a ray.

Each tool's description lists its parameters.

### Review a map before replacing it

1. Call `get_map_info` and retain its opaque `replacement_review`.
2. Pass it as `expected_review` to `new_map`, `open_map`, `open_recent_map` or
   `restore_recovery_snapshot`. Use `discard_changes: true` only when you intend
   to discard the reviewed edits.
3. If the review expires, inspect the current map before trying again. The
   operation checks the review inside replacement admission, so an intervening
   edit, save, buffer replacement or partition-preview change cannot be discarded
   using the old review.

These optional guards preserve existing immediate calls when omitted. Reviews
belong to one Session; they do not transfer between processes or documents.
Other uncommitted draft types are outside this replacement review.

New/Open/Recent/Restore and both Save forms reject once document shutdown has
begun, even with an earlier matching review or generation. These file guards do
not provide multiple-document routing or general lifecycle leases.

For a delayed Save or Compile request, retain `get_map_info.generation` and pass
it as `expected_generation` to `save_map`, `start_compile`, `compile_map` or
`run_map`. Replacing the buffer rejects the request before writing or starting a
job. Edits within the same buffer remain allowed. Native Save and Compile file
choosers use this same guard.

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

Managed builds share one compiler worker. Treat `queued`, `running` and
`cancelling` as active job states; terminal states are `completed`, `cancelled`
and `failed`. `job.queue` exposes `position`, `admitted`, `limit` and
`waiting_ms`.
The queue admits at most four jobs in total and one per document, reserving
capacity before replacing its current job. Multiple public documents remain
unavailable.

Queued cancellation retires that captured request without waiting for the active
build. Cancelling a running build waits for its normal cleanup. Dropping a
waiting `compile_map` or `run_map` call detaches the reply; use `cancel_compile`
to request cancellation. The same frozen map/settings and output reservations
serve background and awaited builds.

Standalone launch captures its exact Session, map generation and build before
worker scheduling. Queued work cannot choose a replacement map later. Abandoned
or cancelled pending work retires only its own launch, after request/process
cleanup. A game already accepted by the Session remains owned by it; use
`stop_game` to stop that game.

Game launch completion belongs to its originating buffer and launch ticket.
Replacing the map during launch rejects that completion and cleans only its
managed client. Foreign or repeated completions cannot clear a newer launch.

Managed launches and reflection captures share one process-wide native
activity. `get_compile_status.native_activity`,
`get_native_capture_runtime.native_activity` and the `native_activity` field in
launch/game-log results expose availability and the current owner's activity
ID, document ID, phase, frozen targets and cleanup error. Its bounded `message`
is also displayed in capture details and Game Log. Runtime `ready` still
describes configuration preflight; polling it reserves nothing.

A queued reflective build reserves native capacity only at dispatch. Busy
dispatch fails before compiler work. Ordinary compilation needs no native
permit. Optional Play uses its installation captured with the build, and a
busy or failed Play leaves the successful BSP available. Paired capture and
capture-to-Play retain one activity ID; the compiler reservation ends before
optional Play startup. Native cleanup failure stays occupied and makes document
shutdown fail. `stop_game` controls only this Session's owned client; use the
exact build token to cancel capture. There is no peer stop, native retry or new
manual-game discovery. Lower capture preflight retains its existing
same-installation running-client rejection.

A saved parent can compile bounded single-level world/detail brush instances;
see [Referenced brush assemblies](instances.md). `create_entity` uses
`classname: "func_instance"`, an origin and string properties `file` (relative
`.vmf`) and optional `angles` (Source pitch/yaw/roll). The current in-memory parent
is authoritative. Unsaved untitled parents, unsupported contents and visible
`func_instance_parms` reject; hidden helpers are excluded.

`compile_map`, `start_compile` and `run_map` use the same automatic expansion
before region selection. Optional `stats.instances` contains occurrence/file/
world/detail counts and dependency paths, bytes and SHA256. Completed instance
jobs report `instance_dependencies.current/error`; `stale`, launch readiness
and baked inspection include the retained build's dependencies. Launch verifies
exact contents in its worker. Source/settings changes, dependency failures and
cancellation preserve previous BSP/pointfile bytes. **Generate reflections**
with instances rejects; nested/runtime-name expansion remains unavailable. The
explicitly refreshed flat reference layer is separate from compilation. `run_map_checks` performs only pure helper-field/path-state
admission, leaving child reads and complete validation to compilation.

## Extract a brush assembly

`get_instance_extraction {}` inspects the shared owned review, token, readiness,
source/selection, relative child filename, world/detail/side counts, child bytes
and SHA-256, validation policy and explicit errors. `worker_active` reports
physical worker/result cleanup independently of the review status. It initializes
no graphics.

`set_instance_extraction` accepts these actions:

1. `prepare` requires a new relative `file` ending in `.vmf`. Select supported
   visible world-brush suffixes and whole neutral `func_detail` suffix owners
   first. Preparation freezes the in-memory parent and validates a private child
   through serialized parsing and the real compiler resolver, outside the lock.
2. `extract` requires the owned `token`. It rechecks exact source/stage bytes
   off-lock, then publishes without overwriting after fallible map edits/cleanup.
   Successful publication and the parent replacement complete as one Undo step.
3. `review` transfers a ready/failed review to a fresh token. `cancel` requires
   the current owned token and removes only owned private staging. Late Cancel
   after successful publication reports completion and retains the child.

`prepare` and `extract` optionally accept `background: true`; inspect until ready,
completed or failed. Their default waits for bounded off-lock work. No automatic
Refresh/display change occurs. Undo restores parent objects but retains the child
file; Redo changes only the parent. Linux, world-origin pivot, exact ordering,
path, metadata and resource restrictions are in [Extract brush assemblies](instance-extraction.md).

## Instance reference tools

`get_instance_reference` inspects the owned authoring form without loading child
contents. `set_instance_reference` shares **Instance reference…** and **Edit
instance reference…**: use `action:"begin"` with an optional root helper `id`,
then `update` with changed `file`, `origin` and `angles` strings. Subsequent
`check`, `file`, `review`, `apply` and `cancel` actions require the current token.
`file` converts an explicitly supplied picker `selected_path` through the same saved-parent
path admission; it does not copy or search. `review` transfers the form with a new
token. Create/Edit applies one Undo step and preserves untouched imported data.
Source/save/revision/definition changes and competing drafts reject stale work.
`contents_validation:"not_checked"` means path/presence only; explicit Refresh or
Compile performs child-content admission. See [Instance authoring](instance-authoring.md).

`inspect_instance_contents` reads the default-off reference layer's state,
source/dependency identities, transforms, bounds, counts and diagnostics without
loading child bytes or initializing graphics. `offset` and `limit` page at most
64 owners. `set_instance_contents` accepts `action:"refresh"` and optional
`background:true`; poll inspection for completion. Refresh resolves the current
immutable parent outside the Session lock and evicts any earlier effective layer.
One missing or unsupported visible child fails the complete layer.
Inspection's `worker_active` remains true until the owned request/result retires;
`loading` retains its existing pending-request meaning. Cancellation and buffer
replacement do not permit a second worker while physical cleanup is active.

`cancel`, `clear` and `frame` require the current contents token. Frame optionally
takes root `ids` and a positive `aspect`; omitted IDs use selection, and empty
selection frames all. These operations do not toggle the checkbox. Shared
`set_view.instance_contents` and temporary `render_view.instance_contents` control
display; `pick_at_pixel.instance_contents` matches temporary visibility. Picks
return root helper ownership with null root solid/side, never editable child
surfaces. Face/component modes, ruler endpoint picks and incompatible authoring
tools suppress the layer. Camera framing can include child bounds, while editing
bounds and pivots remain rooted in parent objects.

Stale metadata is checked before UI and MCP render-cache reuse. Render metadata
retains the exact uploaded layer report, separately from newer Session state.
Reference toggles, Refresh/Clear and suppression use a derived render revision;
they do not invalidate active root editing tools. Refresh/Clear never edits map
source/history or loads child materials. See [Instance contents](instance-contents.md)
for source, geometry and memory limits.

## Tools by task

The shared desktop and private stdio server register 418 tools. `tools/list`
returns their current schemas and descriptions.

| Task | Tools |
| --- | --- |
| Files and inspection | `close_document`, `new_map`, `open_map`, `save_map`, `get_map_info`, `get_map_statistics`, `find_objects`, `get_objects` |
| Recent maps | `get_recent_maps`, `open_recent_map`, `remove_recent_map`, `clear_recent_maps`, `reload_recent_maps` |
| Grid defaults | `get_grid_defaults`, `set_grid_defaults` |
| Fly speed and navigation defaults | `get_navigation_settings`, `set_navigation_settings`, `set_navigation_defaults` |
| Preview defaults | `get_preview_defaults`, `set_preview_defaults` |
| Keyboard shortcuts and viewport commands | `get_keyboard_shortcuts`, `set_keyboard_shortcuts`, `execute_editor_command` |
| Entity definitions | `get_entity_definitions`, `load_entity_definitions`, `search_entity_classes`, `get_entity_class` |
| Material source | `get_material_source` |
| Sky asset inspection | `inspect_sky_assets` |
| Baked lightmap inspection | `inspect_baked_lightmaps` |
| Authored lighting preview | `get_lighting_preview`; shared `set_view` and temporary `render_view` settings |
| Selection and history | `select`, `get_selection`, `set_selection_mode`, `undo`, `redo`, `get_history`, `navigate_history` |
| Measurement guides | `get_measurements`, `set_measurement` |
| Temporary editor visibility | `get_editor_visibility`, `set_editor_visibility` |
| Object-selection filters | `get_selection_filters`, `set_selection_filters` |
| Overlapping objects | `get_object_hit_candidates`, `cycle_object_hit` |
| Instance reference authoring | `get_instance_reference`, `set_instance_reference` |
| Extract brush assemblies | `get_instance_extraction`, `set_instance_extraction` |
| Instance contents preview | `inspect_instance_contents`, `set_instance_contents` |
| Prefab library | `get_prefabs`, `inspect_prefab_thumbnail`, `render_prefab_thumbnail`, `render_prefab_gallery`, `save_prefab`, `preview_prefab`, `rename_prefab`, `delete_prefab`; placement uses `apply_paste` / `cancel_paste` |
| Layers | `get_layers`, `begin_layer_edit`, `update_layer_edit`, `apply_layer_edit`, `cancel_layer_edit`, `select_layer_members`, `set_layer_visibility` |
| Clipboard and transfer | `get_clipboard`, `copy_objects`, `cut_objects`, `import_clipboard`, `export_clipboard`, `preview_paste`, `apply_paste`, `cancel_paste` |
| Object edits | `move_objects`, `duplicate_objects`, `delete_objects` |
| Repeated placement | `get_repeat_array`, `preview_repeat_array`, `apply_repeat_array`, `cancel_repeat_array` |
| Brush drawing and transforms | `set_spatial_settings`, `draw_block`, `cancel_block_preview`, `create_block`, `transform_objects` |
| Transform handles | `set_transform_tool`, `get_transform_handles`, `begin_transform_drag`, `update_transform_drag`, `commit_transform_preview`, `cancel_transform_preview` |
| Surface-picked transform pivot | `transform_pivot_pick` |
| Absolute grid placement | `preview_snap_to_grid`; inspect and apply/cancel through the transform tools |
| Overlay authoring | `get_overlay_state`, `browse_overlay_targets`, `preview_overlay`, `preview_overlay_refit`, `preview_overlay_rebind`, `preview_overlay_corners`, `overlay_corner_handles`, `review_overlay`, `apply_overlay`, `cancel_overlay` |
| Surface placement | `get_surface_placement_sources`, `preview_surface_placement`, `begin_surface_placement_pick`, `pick_surface_placement`, `get_surface_placement`, `apply_surface_placement`, `cancel_surface_placement` |
| New entities on surfaces | `get_entity_placement`, `begin_entity_placement`, `pick_entity_placement`, `review_entity_placement`, `apply_entity_placement`, `cancel_entity_placement` |
| Rope preview | `get_rope_preview`, `set_view`; `render_view` and `pick_at_pixel` accept `rope_preview` overrides |
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
| Face textures | `select_faces`, `get_face_selection`, `set_material`, `edit_faces`, `vary_face_uv`, `sample_face_texture`, `get_surface_sample`, `paint_face_texture`, `propagate_face_texture`, `project_face_group`, `commit_face_preview`, `cancel_face_preview` |
| Atlas-region face texturing | `inspect_material_atlas`, `preview_atlas_fit`; finish through `commit_face_preview` / `cancel_face_preview` |
| Texture paint strokes | `begin_texture_stroke`, `update_texture_stroke`, `get_texture_stroke`, `commit_texture_stroke`, `cancel_texture_stroke` |
| Terrain vertices | `get_terrain_vertices`, `highlight_terrain_vertices`, `pick_terrain_vertex`, `frame_terrain_vertices`, `get_terrain_volume_vertices`, `select_terrain_vertices_in_volume`, `get_terrain_vertex_mask`, `preview_terrain_vertex_move` |
| Terrain brush strokes | `begin_terrain_stroke`, `update_terrain_stroke`, `get_terrain_stroke`, `break_terrain_stroke`, `finish_terrain_stroke`, `cancel_terrain_stroke` |
| Material replacement | `find_material_faces`, `get_material_search`, `select_material_faces`, `replace_materials` |
| Material browsing | `search_materials`, `browse_materials`, `preview_material`, `set_material_favorite`, `reload_material_favorites`, `reload_material_assets` |
| Entities | `create_entity`, `set_entity_properties`, `tie_to_entity`, `move_to_world` |
| Linked paths | `get_linked_path`, `set_linked_path` |
| Entity definitions and inspector | `search_entity_classes`, `get_entity_class`, `load_entity_definitions`, `get_entity_inspector`, `set_entity_color`, `set_entity_component` |
| Particle definitions and choices | `browse_particles`, `inspect_particle`, `begin_particle_choice`, `choose_particle`, `get_particle_choice`, `apply_particle_choice`, `cancel_particle_choice`, `reload_particle_assets` |
| Connections | `get_entity_connections`, `get_connection_choices`, `begin_connection_edit`, `update_connection_draft`, `commit_connection_edit`, `cancel_connection_edit`, `delete_entity_connection` |
| Safe renaming | `get_entity_references`, `begin_entity_rename`, `update_entity_rename`, `commit_entity_rename`, `cancel_entity_rename` |
| World settings and property batches | `get_property_inspector`, `begin_property_edit`, `update_property_edit`, `get_property_edit`, `commit_property_edit`, `cancel_property_edit` |
| Object report and visibility | `get_object_report`, `select_object_report`, `set_object_visibility` |
| Map checks | `run_map_checks`, `get_map_checks`, `select_map_issue`, `preview_map_fixes`, `apply_map_fixes`, `cancel_map_fixes` |
| Design notes | `get_context_state`, `list_context_notes`, `get_context_note`, `get_context`, `begin_context_edit`, `update_context_edit`, `apply_context_edit`, `cancel_context_edit`, `delete_context_note`, `set_context_view`, `focus_context` |
| Recovery | `get_recovery`, `set_recovery_settings`, `reload_recovery_settings`, `create_recovery_snapshot`, `restore_recovery_snapshot`, `delete_recovery_snapshot` |
| Camera and visual checks | `get_camera`, `set_camera`, `frame_selection`, `set_view`, `render_view`, `pick_at_pixel`, `raycast` |
| Working region | `get_working_region`, `set_working_region` |
| Fog preview | `get_fog_preview`, `set_fog_preview`; temporary overrides through `render_view` |
| Normal-map preview | `get_normal_map_preview`, `set_view`; temporary `render_view.normal_maps` override |
| Saved camera views | `get_camera_views`, `save_camera_view`, `restore_camera_view`, `rename_camera_view`, `delete_camera_view` |
| Pointfiles | `load_pointfile`, `get_pointfile`, `navigate_pointfile`, `frame_pointfile`, `clear_pointfile` |
| Compile and play | `get_compile_settings`, `set_compile_settings`, `start_compile`, `get_compile_status`, `cancel_compile`, `compile_map`, `run_map`, `launch_game`, `game_log`, `stop_game` |
| Build recipes | `get_build_recipes`, `save_build_recipe`, `rename_build_recipe`, `delete_build_recipe`, `apply_build_recipe`, `reload_build_recipes` |

## Good to know

These details aren't obvious from the tool list.

**Extended entity definitions**

`load_entity_definitions` resolves includes and inheritance, then applies ordered `@ExtendClass` declarations to existing classes. `get_entity_class`, inspector controls and connection suggestions share the resulting catalog. Existing stored values, custom keys, connections and map history remain intact; a failed load retains the previous definitions. See [Entity definitions](entities.md#load-extended-entity-definitions) for replacement rules and unsupported directives.

Loading uses the same owned Session request as **Load FGD…**, outside the UI/
Session lock. One load is admitted at a time. Closing cancels it and waits for
result cleanup; superseded results cannot replace newer definitions. Root and
included sources must be regular files (16 MiB each, 64 MiB total, 256 sources),
with bounded parsing and inheritance work. Valid canonical aliases are supported.

**Temporary editor visibility**

`get_editor_visibility` accepts `offset` and `limit` (1–200, default50), returning
`snapshot`, `hidden_total`, sorted root `hidden` IDs, `selected_hidden`, paging,
`can_hide_selection` and policy. `set_editor_visibility` uses tagged actions:
`{"action":"hide","snapshot":"…","ids":["solid:3"]}` or
`{"action":"show_all","snapshot":"…"}`. Omit `ids` to hide the selection;
Show all accepts no IDs. Missing/child IDs reject the entire Hide batch. Duplicate
roots coalesce; no extra group expansion occurs.

These Session-local exclusions remove complete owners from editor display and
spatial hits while retaining selection and explicit-ID reports/edits. Builds,
VMF hidden flags, source and Undo stay unchanged. Apply/cancel displayed geometry
or material previews before Hide; Show all remains available during previews.
Actual changes cancel active spatial gestures; no-op requests preserve them.
New/Open clears the set. Committed edits/Undo/Redo prune departed roots, so Undo
can restore an object without restoring its old temporary exclusion.

Show all clears only this set, preserving authored hiding, tool-texture filters
and the working region. Object-type filters remain independent selection seeds.
Working-region and helper/model/instance inspection distinguish `editor_hidden`,
`owner_editor_hidden` and line `endpoint_editor_hidden` from region exclusion.
Human controls: **Selection → Hide selected in editor / Show all editor-hidden**.
See [Selecting](selection.md#temporary-editor-visibility).

**Object-selection filters**

`get_selection_filters` returns `filters`, `revision`, `scope` and `groups`.
`set_selection_filters` requires the complete three-boolean object:
`{"world_brushes":true,"brush_entities":true,"point_entities":true}`.
Missing/extra fields reject. Point entities include models, helpers, ropes and
root instance owners; terrain follows its authored brush owner. All-disabled
is valid. Existing selection and object visibility stay unchanged.

Object clicks, overlap candidates/cycling, marquee, volume and selection paint
use enabled hit types as seeds. An enabled hit selects its whole visible group,
including disabled-type peers; Fully inside still requires every visible peer
inside. Group bypass retains individual selection. Explicit IDs, report results,
explicit group/layer selection, faces/components and geometric tools bypass.
`raycast` and `pick_at_pixel` remain raw geometric queries.

A change cancels unfinished object-selection gestures and object area/volume/
paint previews, while preserving face, texture and geometry workflows. No-op
changes leave drafts and revisions unchanged. Values survive New/Open in the
same Session; they are not saved preferences or map data. Object-hit and area/
volume/paint inspection report the active `selection_filters`. Human controls:
**Selection → Objects → Pick object types**. See [Selecting](selection.md).

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
- `vary_face_uv` takes a `seed` from 0 to 4294967295, finite nonnegative
  `max_shift_u` and `max_shift_v` in texels, optional `faces`, and `preview`
  (default false). A stable face-ID/seed digest adds independent signed shifts to
  committed offsets. Face order and repeated IDs do not change the result;
  reapplying after a commit adds another shift. Axes, skew, scales, materials,
  lighting, geometry and terrain metadata are preserved. Use `preview:true`,
  review `faces_changed`, then pass the returned `preview_token` to
  `commit_face_preview` or `cancel_face_preview`. Both maxima at zero preserve
  existing drafts and return `preview_created:false`, `preview_token:null`;
  any preview in `state` then belongs to an earlier operation. Invalid batches
  preserve the prior draft. UI: **Edit face textures… → Vary texture offsets →
  Preview variation**. See [Texture variation](textures.md#vary-texture-offsets).
- `edit_faces` also accepts `u_axis` and `v_axis` as three world-space numbers or null components, such as `"u_axis": [2, null, 0.125]`. An omitted axis or null component preserves each target face's committed value independently, including mixed selections. Vectors retain their authored magnitude and may skew; UV coordinates remain `dot(world_position, axis) / scale + offset`. Explicit scale/offset, Fit and justification work after the direct basis; rotation metadata, geometry, lighting and other face data remain unchanged unless separately requested.
- Direct vectors cannot combine with `alignment` or `rotation`; zero, nonfinite, nearly singular or edge-on projections reject the whole batch without replacing a valid preview. `get_face_selection` and returned `state.preview_faces` expose numeric `u_axis` / `v_axis` alongside legacy `uaxis` / `vaxis` strings. Use `preview: true`, inspect or render the shared draft, then pass `state.preview_token` to `commit_face_preview` or `cancel_face_preview`. UI **Advanced U/V world vectors** shows current/mixed values per component and requires a preview of matching fields before **Apply textures**. See [Textures](textures.md#set-uv-vectors-and-skew).
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
- `preview_partition_extrusion` takes the current flat partition `token`, signed `height`, optional `wall_material` and `texture_lock`. Magnitudes are 0.125–4096: positive adds outward brushes; negative subtracts highlighted convex prisms from each selected source. Cuts clip to the actual source, allowing pockets and through openings. Floors inherit each piece's material/texture plan; `texture_lock: true` attaches their UVs. UI uses positive **Extrusion depth** and **Outward · add brushes / Inward · cut source**.
- Inspect `get_brush_components.preview.extrusion`: signed `height`, `direction: inward|outward`, `pieces` and `fragments`. Inward `pieces[].solid` is null because these are virtual cutters; `fragments[]` reports real `source_solid`, `solid`, `owner`, `bounds` and `faces`. Face records include `side`, `source_side`, nullable `partition`, `surface: source|top|base|boundary|divider` and material/projection/lighting fields. `top` denotes an inward pocket floor; `source` denotes inherited exterior. `texture_mode: floor_attached` identifies attached inward floor UVs.
- Extrusion limits retain 16 sources, 32 highlighted pieces and 256 cutter planes; inward output adds `limits.extrusion_fragments: 256` with at most 64 sides per fragment. Source and virtual-cutter corners must fit ±65,536. Entire-source removal, displacements, stale/nonplanar loops, invalid fragments and malformed saved metadata reject atomically. The first surviving fragment retains its source solid ID; unrelated sources and owner siblings remain. Saved partition records on every changed source brush are consumed in the same transaction.
- Apply the matching `token` with `commit_component_edit`; `cancel_component_edit` discards it. **Return to flat** drops the extrusion and retains flat piece plans. Changed highlights or direction/depth require another preview before Apply. Failed updates retain the previous valid draft; Apply selects output fragment faces and adds one Undo step. See [Partitions](partitions.md).
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
- `load_pointfile` accepts regular files up to 2 MiB and 65,536 points. Unix
  reads use nonblocking opened-target checks and preserve valid file/directory
  aliases; changed sources, special files and closing reject without replacing
  the previous overlay. Non-Unix targets report unsupported file admission.

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

- `preview_terrain_blend` takes `mode: paint|fill|invert`, optional `faces` and `value` 0–255 (default 128). Paint requires world-space `center` and uses `radius`, `direction` and `strength` 0–1 (default 1); fill/invert affect complete grids. Geometry and face metadata stay fixed. Read row-major alpha with `get_terrain_grids(preview: true)`, then use the same `commit_face_preview` / `cancel_face_preview` token. A two-texture material is needed to see blending. With engine physics, each emitted collision triangle uses `$surfaceprop2` when declared and its three `f32` alpha values sum strictly above 382.5; equality or a missing secondary property keeps `$surfaceprop`. UI: **Edit terrain… → Operation**, with brush sliders, exact inputs, **Preview**, **Apply** and **Cancel**.

- `preview_terrain_sew` takes optional `faces` and `blend` (default false). Matching full/whole-to-half base edges or Source-float base corners at any supported powers share averaged original common positions; finer border vertices interpolate the sewn coarse edge; `blend: true` also averages/interpolates alpha. Vertex groups are formed first so multi-patch corners average once. It returns `faces_changed`, `neighbor_pairs` (border-only), `corner_joins` (additional corner unions after border groups), `sewn_vertices`, `interpolated_vertices`, a shared `summary` and `state.preview_token`. Interior vertices and face metadata stay fixed; ambiguous edges, collapsed float corners and unsupported caches reject without replacing the last valid draft. Commit or cancel through the shared face-preview tools. UI: **Edit terrain… → Sew borders & corners → Also sew blend values → Preview → Apply / Cancel**.

- `preview_terrain_smoothing` takes optional `faces` and `angle`. Omitted/null restores **Automatic**; `0` keeps **Hard borders**; positive finite values through `180` set an **Angle limit**. Both surfaces must allow each pairwise local-normal blend; accepted junction groups share a normal. Geometry, alpha, flags, tags and topology stay fixed. The viewport and both lit modes use the same normal rules on their respective source/compiled geometry. Inspection returns a null angle for Automatic and an error string for malformed/duplicate imported `smooth` fields; a valid preview can repair them. Use token-guarded commit/cancel and Undo. UI: **Edit terrain… → Border smoothing → Maximum angle · degrees → Preview → Apply / Cancel**. Native stats expose `displacement_smoothing_overrides`, labeled **Border smoothing** in **Build details…**; the standard VMF `smooth` and native angle fields round-trip. This is separate from global brush `smoothing_angle`.

- `preview_terrain_collision` takes optional positive `physics_collision`, `player_collision` and `ray_collision` booleans for selected or explicit faces. Omitted fields preserve each face. It modifies only native bits 2/4/8, retains other data and returns the shared preview token. UI: **Edit terrain… → Collision**, with **Keep current / Enabled / Disabled** and the committed state. Unsupported imported bits and individual triangle removal remain separate.

- `remove_terrain` removes displacement data from 1–16 selected or explicit faces, rejecting missing, ambiguous or ordinary faces atomically. The response gives `surfaces_removed` and shared face state. Geometry, IDs, material, mapping and lighting stay fixed; undo restores all imported grid rows and extras. A parent regains its brush volume once no terrain faces remain. Imported entity terrain can also be removed. UI: **Remove terrain**.

- `raycast` and `pick_at_pixel` intersect rendered terrain triangles, retaining owner/side identity and visibility filters. `frame_selection` uses displaced bounds and active previews; `get_objects` reports committed visible bounds. Invisible parent faces are excluded from terrain hits. Pass the returned `point` into `preview_terrain_sculpt.center` or `preview_terrain_blend.center` to choose the same brush center as UI **Pick center in viewport…**. Imported entity terrain supports grid inspection and cleanup with Undo, while native runtime brush-model terrain remains unsupported; consumed `func_detail` terrain compiles through the world pipeline.

- `tie_to_entity` with `classname: func_detail` moves existing terrain parents into a consumed helper. `compile_map`, `start_compile` and `run_map` emit world displacement bindings with `SOLID | DETAIL` contents, saved grids/tags, lighting, neighbors and collision; hidden helpers are omitted. `render_view` includes compatible helper border normals. Source/history remain unchanged by building. Use `move_to_world` / UI **Move to world** to reverse the grouping. Direct creation inside helper entities and runtime brush-model terrain remain separate.

- Native terrain results include `displacement_neighbor_pairs` for compatible full/half edges and `displacement_corner_neighbor_pairs` for reciprocal corner-only contacts, excluding edge-connected pairs. Corner-only contact can use different powers when base and displaced corners match. UI **Build details…** labels these **Neighbor pairs** and **Corner pairs**. `displacement_mixed_power_pairs`, `displacement_half_edge_pairs`, `displacement_lod_disabled_vertices` and `displacement_lod_snapped_vertices` expose full-edge LOD constraints and actual geometry changes. UI **Build details…** labels these **Mixed-power pairs**, **Half-edge pairs**, **LOD disabled vertices** and **Snapped vertices**. Imported grids with the VMF `subdiv` marker compile their explicit rows without generating subdivision; source editing rejects actual unsupported caches, rather than the marker alone. Both compile paths automatically disable dependent vertices and snap unsupported detail without editing the source grid; physics and lighting use the resulting geometry. Whole-to-half connections support either or both halves, with spacing-aware masks and overlap rejection. Other fractional spans remain unfinished.

**Compiling**

- `set_compile_settings` takes `lighting` (`fullbright`, `direct_ldr` or `direct_ldr_hdr`), `physics` (`none` or `engine`), `visibility` (`fast` or `detailed`), `sky_quality` (`fast`, `balanced` or `detailed`), `bounces` (whole counts `0`–`4`), `smoothing_angle` (whole degrees `0`–`90`), `pack_sky_assets` and `generate_reflections` (booleans, both default `false`). Packing works with every lighting mode; reflection generation requires `direct_ldr` or `direct_ldr_hdr`. Omitted/null fields retain their values, unknown arguments reject, and invalid combinations leave every setting unchanged.
- `get_compile_settings.visibility_choices` exposes **Fast (conservative)** and **Detailed (portal clipping)**, shown under **Visibility quality** in Build settings. `visibility` freezes in each job's captured settings and changing it stales prior output. Detailed clips source/pass portal apertures; PAS includes the refined visible rows. Statistics expose `visibility_mode`, `visibility_work`, `visibility_removed_pairs` (directed PVS entries removed from the broad result) and `visibility_workers` for the largest actual worker count across the base and flow stages. Larger jobs parallelize base-facing rows on at most eight available CPUs; Detailed also parallelizes separator flow. Source portals retain their order, and workers share one exact global work budget. Invalid apertures and exceeded work/allocation budgets fail without publishing output. The former `visibility_fallbacks` statistic is removed because clipping is no longer abandoned.

Authored `func_viscluster` uses `create_block` then `tie_to_entity` with that
classname. Ordinary compile paths group dry open leaves whose exact convex
intersection exceeds 10% of leaf volume, subject to bounded numerical admission.
Up to 16 single-convex-brush helpers are supported; ambiguous overlapping claims,
liquid/portal/area crossings and active compile regions reject. Geometry is not
merged. The compiler coarsens already solved source/destination PVS groups and
recomputes PAS, preserving baseline visibility and individual ambient leaves.
Stats distinguish `visibility_solved_clusters` from emitted `clusters`, plus
`viscluster_helpers`, `viscluster_groups` and `viscluster_grouped_leaves`. Removed-pair
statistics refer to the pre-grouping solve. There is no claim of reduced VIS
work, compressed size or native float parity. See [Visibility clusters](visclusters.md).

- `get_compile_settings` returns the current choices, including `sky_choices` with their labels and direction counts (32, 128 and 512). These tools work before the first build, like the toolbar's **Build settings…**. The UI calls `sky_quality` **Sampling quality**; it affects sky ambient, soft sun, model ambient and bounce rays in either lit mode.
- `get_compile_settings.bounce_choices` exposes **Direct only** (`bounces: 0`, default) and **One diffuse bounce**, **Two diffuse bounces**, **Three diffuse bounces** or **Four diffuse bounces** (`bounces: 1`–`4`), shown under **Indirect light**. It gathers reflected light per source style from immutable previous-pass world/detail/terrain lightmaps into world, terrain and brush-model receivers using 32/128/512 cosine-weighted rays per receiver per pass. Each pass rebuilds receivers from the original direct maps. Model rays check their own geometry before accepting a world hit; unrelated models stay separate. Source albedo must be between zero and one; sky/backfaces do not supply bounce energy. Stats expose actual `bounces`, `bounce_rays`, `bounce_samples` (final-pass receivers with positive indirect light) and `bounce_work`, within the shared lighting budget. Fullbright retains the setting and reports zero bounce work.
- `edit_faces` (including `preview: true`), `commit_face_preview` and `cancel_face_preview` drive the same authored brush smoothing as **Face lighting…**. Shared groups 1–24 blend; shared groups 25–32 keep hard edges and take priority. Both lit modes apply checked world/detail and brush-model corner normals, including eligible corners inside neighboring source edges. Original fan centers and source fans remain stable across lightmap subdivision. Stats expose `smoothed_faces`, `smoothed_corners` and `normal_work` (construction work; receiver interpolation also counts toward `lighting_work`). Unassigned faces stay flat at angle zero; terrain, world coordinates and each model's local coordinates remain separate. Source/corner limits and the shared lighting budget preserve prior output on rejection.
- `smoothing_angle` defaults to `0` (disabled). Positive values blend only two unassigned source faces within one world/model scope whose normal angle meets the threshold. Explicit smooth/hard masks retain precedence; terrain and individual model/world scopes stay separate. `get_compile_settings.smoothing_angle_range` reports bounds/unit. Each job captures the angle; changing it stales previous output. Actual stats expose `smoothing_angle` and the same `smoothed_faces`, `smoothed_corners` and `normal_work`. Fullbright retains the setting but reports zero actual angle/work; no VMF/history edits occur.
- `edit_faces` and object material edits select VMT `$surfaceprop` values for world, terrain and brush-model collision. Terrain chooses `$surfaceprop2` when the three emitted collision-triangle `f32` alpha values sum strictly above `382.5`; equality or a missing secondary property keeps the primary. Existing terrain blend tools supply these weights. With `physics: engine`, native triangle mappings use a shared table of at most 127 names including default; a material without `$surfaceprop` uses default. `compile_map` and background results expose `physics_surface_properties`, shown as **Surface properties** in **Build details…**; it counts distinct shared names. Names use 1–127 ASCII letters, digits, underscores, dots or dashes. Uniform models additionally record the body property; mixed models keep per-face contact properties. Brush-model base mass uses the property with the greatest emitted face area: solid materials use collision volume and density, while shells use emitted area, thickness and density, capped at 50,000 kg. Surface definitions load through the existing game mounts; invalid or missing definitions, native property indices or mappings fail before publication. Entity diagnostics report the base mass and selected property. Existing `set_entity_properties` calls retain runtime `func_physbox` mass-scale/override and `phys_convert` mass-override fields. Custom density controls and broader moving-body behavior remain unfinished. See [Physics and brush-model mass](physics.md).
- `create_entity` and `set_entity_properties` author point/spot `style` (Appearance `0`–`31`), `targetname`, `pattern` (up to 64 lowercase letters), and `spawnflags` bit `1` (Initially dark). Named groups get compiler-only IDs `32`–`63`; authored nonzero appearances become `defaultstyle`, and matching names share an initial pattern/state. `begin_connection_edit`, `update_connection_draft` and `commit_connection_edit` author relay outputs targeting `TurnOn`, `TurnOff`, `Toggle` or `SetPattern`. Both compile paths emit up to four face channels including Normal, with actual `switchable_light_groups`, `light_styles`, `styled_faces` and `light_style_maps` stats. Compilation keeps VMF/history fixed. Invalid fields, conflicting groups and exceeded limits preserve previous output. With `bounces: 1`–`4`, reflected light follows the same styles and can add a channel beyond direct reach; the four-style face limit still applies. Switching model-ambient cubes remain unfinished.
- `tie_to_entity` and `set_entity_properties` author rendered brush models and pivots. Both lit modes compile model-local faces with world-space lighting positions, isolated self shadows, and the same point/spot style channels and sky sampling. Stats expose `lit_brush_models`, `model_lighting_faces`, `model_bounce_faces` (processed receiver faces) and `model_bounce_samples` (samples with positive reflected energy). Local geometry, UVs and collision stay fixed; direct and bounced lightmaps remain attached in game. Models do not cast permanent world shadows or supply reflected light. They can receive world bounce; dynamic relighting and independently rotated initial poses remain unfinished. Both compile paths share the checked emitted model-to-entity bindings and common budgets.
- `edit_faces` material/projection edits feed automatic directional lightmaps for lit VMTs declaring `$bumpmap`. Both lit modes emit base plus three direction maps per style on world/detail/model/terrain receivers, including the optional world-source bounce. Stats expose `bump_faces` and `bump_lightmaps` (auxiliary directional maps); `light_style_maps` includes base and directional maps. `lightmap_samples` counts receiver positions, while bytes/work include every map. Base averages, style IDs, geometric shadow bias and source/history remain shared. Material-only second normal maps, transparent shadows and baked prop maps remain unfinished; viewport previews stay diffuse.
- Settings cannot change while a build runs. Each job reports its captured `settings`; changing settings after a build makes that output out of date for launching.
- Both lit modes automatically cull point/spot candidates using each source's PVS and exact falloff endpoints, across world, terrain and pivot-aware model samples. Unknown/boundary clusters and cache overflow retain candidates; sky lighting stays eligible. Results expose `light_evaluations`, `light_pvs_skipped`, `light_range_skipped` and `light_culling_work` (row/cache/cluster lookup work within `lighting_work`). Both compile paths preserve emitter order and share the same bounded implementation; no extra setting is required.
- Create and edit baked lights with `create_entity` and `set_entity_properties`. For `light_environment`, `_light` is sunlight and `_ambient` is sky ambient; color values use `R G B brightness`. Set ambient brightness to zero with a value such as `255 255 255 0`. Set `SunSpreadAngle` from 0 to 90 degrees: zero keeps hard shadows, positive values soften them using the selected sky quality.
- `get_compile_status.launch` says whether the last build can be played, or why not.
- Build statistics include `ambient_samples` and `ambient_bytes`, shown as **Model ambient** in Build details. Each lit channel writes up to 1/4/8 safe, separated probes per open world leaf at Fast/Balanced/Detailed quality from filtered local world/terrain lightmaps and sky radiance; skipped leaves are reported in diagnostics. `sky_quality` controls probe density and the ray-cone footprint used to filter local detail. Probes remain more than 128 units apart; the bake caps probes at 65,536 per channel and retains the shared work limit.
- Terrain normal sewing follows per-surface Automatic/hard/angle choices for matching full/half edges in both compile paths and `render_view`; hidden patches do not contribute. `displacement_neighbor_pairs` counts all verified native full/half-edge links. Compilation also shares normals at common vertices of full/half edges after LOD snapping. Viewport rendering and surface picking use the same derived geometry; raw grid inspection and editing retain authored samples. Other fractional edge spans remain unfinished.
- Create areaportals with `create_block` and `tie_to_entity` (`classname: "func_areaportal"`). `targetname` is the human **Name**, and `StartOpen` chooses the **Initial State**. Entity connections accept **Open**, **Close** and **Toggle**. Compilation assigns native keys without editing the VMF. Shared build statistics include `areas`, `areaportals` and `areaportal_vertices`; invalid boundaries preserve the previous output.
- `func_areaportalwindow` uses the same `create_block`, `tie_to_entity` and `set_entity_properties` operations. `target` is **Rendered Window**, `BackgroundBModel` is the optional **Foreground bmodel**, and `FadeStartDist`, `FadeDist`, `TranslucencyLimit` control fading. Render targets must be unique visible brush models with enclosing bounds; foreground and rendered models share a pivot. Connections can send `SetFadeStartDistance` and `SetFadeEndDistance`. `areaportal_windows` counts windows within the total `areaportals` count.
- `run_map` and `launch_game` always launch CS:S with `-insecure`.
- Hint helpers use `create_block` with `tools/toolsskip`, then `select_faces` and `edit_faces` to paint `tools/toolshint` onto the intended split faces. Keep them as world brushes. Both compile paths protect finite hint polygons during initial structural plane selection, then partition remaining filled open leaves. Helpers add no rendering/collision. Results expose `hint_surfaces`, `hint_splits`, `hint_priority_splits` (retained added initial planes, included in `hint_splits`) and cumulative `hint_work`. Skip sides bound the helper; mixed materials and unsupported ownership fail atomically. Hidden/coplanar/outside helpers need no new split. Initial selection runs in `world_tree`; remaining refinement runs in cancellable `hint_planes`.
- Imported `info_overlay` entities compile through `compile_map`, `start_compile` and `run_map`. `overlays` and `overlay_face_refs` count regular projections, imported water transitions and their compiled surface bindings. Edit regular overlay material, U/V ranges, render order and fades with `set_entity_properties`. Imported basis/corner fields remain read-only in general property editing. Supported projection changes use the explicit overlay authoring operations below. Compilation preserves VMF properties and history, resolves source sides to final world/detail/terrain faces and creates named accessors for regular overlays. Nested `info_overlay_transition` blocks compile as imported explicit projections, are consumed without runtime accessors and never inherit top-surface bindings onto generated water undersides. They allow 1,024 records and 256 compiled references each, sharing the 65,536 total binding/reference budgets with regular overlays; regular fade indexing remains separate. Transition authoring, fades and accessor fields are unsupported. Invalid targets or exceeded budgets preserve the previous BSP. See [Overlays](overlays.md) for limits.

### Linked paths

`get_linked_path` returns the current `snapshot`, `pick_snapshot`, owned `preview`, supported
`classes` and `limits`. `set_linked_path` uses strict tagged actions:
`preview` takes `classname`, `prefix`, ordered `points`, optional `closed` and
the current `snapshot`; `review`, `apply` and `cancel` require `token`.
Preview and Review preserve source/history. Review issues a fresh token; only
the current owner can Apply or Cancel. Apply creates and selects the planned
nodes in one Undo step. Occupied names receive bounded numeric suffixes; names
that would acquire existing references reject. Open endpoints omit `target`;
closed paths link the last node to the first. See [Linked paths](path-authoring.md)
for controls and limits.

`inspect` accepts one existing node `id` and returns its complete simple chain
and snapshot. `edit` takes that snapshot and ordered `nodes: [{id, origin}]`;
it reuses Review/Apply/Cancel while retaining node IDs, names, custom values,
flags, connections and outside references. Its preview reports `existing: true`.
To insert or remove nodes, pass the inspected `seed` with `action: "edit"`,
ordered `nodes` and the same snapshot. Retained rows use `id`; new rows omit it.
Optional `prefix` (default `path`) names only new nodes. Keep at least one
original node and 2–64 distinct finite points within ±65536. The preview reports
`seed`, each row's `source_id` (null for inserted nodes) and exact `removed`
IDs/names. Removed names are not recycled. A removed node with I/O connections
or references beyond the chain's ordinary `target` links rejects; retained nodes
keep other values and incoming references, while inserted nodes receive FGD
defaults. Apply revalidates the complete plan and makes one Undo step. Omitting
`seed` retains the earlier requirement to provide every original node exactly
once. Renaming, branching and open/closed conversion are unsupported.

After creating a preview, `begin_pick` takes its owned `token`, a zero-based
existing `row`, `snapshot` set to the latest `pick_snapshot`, `width`/`height`
from 1–8192 and an optional temporary `camera` matching `render_view`. It arms
that row and returns
a fresh token without changing any position. `pick_ray` takes this token plus
finite world `origin` and nonzero finite `direction`. `pick_pixel` takes the
token, continuous viewport `x`/`y` and the captured `width`/`height`; the center
of pixel `(i,j)` is `(i+0.5,j+0.5)`. The captured camera supplies the ray without
moving the shared viewport.

Both use the first visible brush, terrain triangle or supported model mesh.
Current path nodes, helper boxes, hidden surfaces and empty space supply no
substitute plane. A successful hit changes only the chosen row, retaining IDs,
names, links and other rows. It finishes the gesture with a fresh preview token
and `last_pick: {row,target,solid,side,point,normal}`; model hits have null brush
IDs. `preview.picking` is null or `{row,width,height,valid}`. Misses, duplicate
positions, invalid rays/pixels and stale source/scene/camera/viewport guards
preserve the current draft. Apply rejects while picking.

`cancel_pick` accepts the matching owned token, including stale cleanup while
the draft remains present, and returns a fresh token with rows unchanged.
Replaced drafts reject old tokens. Numeric edits remain exact and unsnapped;
picked points do not follow subsequent surface changes. Existing **Pick
position…** / **Cancel position pick**, **Review preview**, **Apply** and **Cancel**
use the same Session operations. Apply makes the complete path edit in one Undo
step; no pick commits map data. Free-space workplanes, offsets and object
attachment remain unavailable.

### Surface-picked transform pivot

`transform_pivot_pick` shares **Pick pivot…** and **Cancel pivot pick** with the
transform inspector. Each call takes an `action`:

| Action | Fields and result |
| --- | --- |
| `inspect` | No other fields. Returns `snapshot`, nullable `current_pivot`, nullable `picking`, `last_pick`, validity/provenance flags and `begin_error`. |
| `begin` | Latest `snapshot`, `width` / `height` (1–8192), and optional temporary `camera` matching `render_view`. Returns `picking.token`, a numeric ownership token. |
| `ray` | Owned `token`, finite `ray_origin` and nonzero finite `ray_direction`. |
| `pixel` | Owned `token`, continuous `x` / `y` and the captured `width` / `height`. Coordinates have no implicit half-pixel offset. |
| `cancel` | Owned `token`; stale cleanup is allowed and leaves the current pivot exactly unchanged. |

Begin requires a nonempty whole-object selection and no active preview,
viewport tool, build or launch. The first visible brush, terrain sheet or
supported model triangle supplies an exact unsnapped world point, including
selected objects. A hit sets only the shared custom transform pivot and disarms
picking. `last_pick` reports `target`, nullable `solid` / `side`, `point`,
`normal`, `generation` and `revision`; `last_pick_document_current` and
`last_pick_matches_pivot` distinguish retained provenance from a current choice.
Misses leave picking armed. Helpers and empty space supply no fallback plane.

Foreign/replaced tokens, invalid rays/pixels and changed source, definitions,
selection, tool, scene, view, camera or viewport reject without replacing the
pending pick or current pivot. Only a fresh User-owned context captures desktop
input. No map/history/selection/camera/framing change, attachment, offset or
persistence is added. The point stays fixed in world space. Numeric UI
transforms and handles use it; pass returned `current_pivot` explicitly to
`transform_objects`, whose omitted-pivot selection-center behavior is unchanged.
See [Brushes and transforms](brushes.md#pick-a-custom-pivot-on-a-surface).

### History, measurements and surface creation

`get_history` returns bounded transaction pages, authors, actual object/section
changes, the current state and navigation blockers. `navigate_history` requires
the returned `snapshot` and a retained `target` state ID; `"0"` selects the initial
state. Jumps use ordinary Undo/Redo, retain the redo chain and never save the file.
At most 256 transactions can be traversed in one request. See [History](history.md).

`get_measurements` reports saved rulers, the owned draft, limits, errors and a
snapshot. `set_measurement` supports `preview`, `review`, `apply`, `cancel`,
`delete`, `frame`, `view`, `attach_origin`, `attach_local`, `detach_origin`, `world_endpoint`,
`begin_pick`, `pick_ray`, `pick_pixel` and `cancel_pick`.
Rulers use editor-only metadata and the shared guide mesh, with no BSP output.

Preview first. `begin_pick` requires its owned `token`, `endpoint: "start"|"end"`,
`width`, `height` (1–8192), and the latest `pick_snapshot`. Optional `camera`
matches `render_view` without moving the desktop camera. Use the new token for
`pick_ray` with finite `ray_origin` / nonzero finite `ray_direction`, or
`pick_pixel` with integer `x` / `y` and the captured `width` / `height`. Pixel
picks use the pixel center. The first visible brush, terrain triangle or supported
model mesh supplies the exact world endpoint; hidden targets, entity helpers and
empty space provide no substitute plane. Misses, coincident endpoints and stale
map/scene/camera/pixel dimensions preserve the draft. Successful picks return a
new token and `last_pick: {endpoint, target, solid, side, point, normal}`; brush
IDs are null for model hits. `cancel_pick` stops only the owned pending pick,
including stale cleanup, and returns a new token with both endpoints retained.
Apply rejects while picking. Numeric edits and surface-picked endpoints remain
exact unsnapped world points.

To attach an owned preview endpoint to a point entity, call `set_measurement`
with `action:"attach_origin"`, its `token`, `endpoint:"start"|"end"` and
`object:"entity:<id>"`. The endpoint references that entity's authored origin
by stable ID. Use `action:"attach_local"` with the same fields to capture its
current resolved world endpoint in that entity's rigid unit-axis frame. The
result follows authored Source orientation and translation, preserving its
local offset; model scale, parenting and mesh vertices are not evaluated.
`detach_origin` takes `token` and `endpoint` and freezes either resolvable entity
attachment into a world point. `world_endpoint` takes those fields plus exact
`point:[x,y,z]`, explicitly replacing even an unavailable reference. Each
successful action returns a new token without changing the map/history;
Apply resolves again and saves one Undo step. Numeric Preview preserves
attachments and rejects implicit coordinate changes; detach or use
`world_endpoint` before numeric edits or surface picking.

Local frames use Source pitch/yaw/roll, with `angles` taking precedence over
scalar `angle`, including vertical -1/-2 and authored light pitch conventions.
Absent orientation means zero; invalid stored orientation makes only local
attachments unavailable. Origins/resolved points stay within ±65536 units;
local offset components need only be finite. Local Apply upgrades measurement
metadata to version2; Cancel preserves the original block and Undo restores it
exactly. Removing local modifiers retains version2. Older builds preserve that
block with measurement editing disabled; retained origin-ID keys protect orphan
references from unrelated ID reuse.

Measurement rows report resolved `start`/`end`, `distance`, `components`,
`available`, `error`, original `stored_start`/`stored_end` and per-endpoint
`attachments` with kind, object ID and availability/error. Local attachments
report `kind:"entity_local"` and finite `local_offset:[x,y,z]`. Inspection resolves
against `coordinate_source:"committed_document"`; rendered guides follow the
displayed transform draft. Missing/deleted entities, duplicate IDs/origins,
invalid origins, invalid/duplicate active local orientation, brush entities and
coincident resolved endpoints remain
unavailable, draw no guide and reject Frame/Apply. References never silently
freeze, detach or retarget; duplication keeps the original target. Explicit
world replacement can repair an unavailable endpoint. Target visibility does not
hide an otherwise available ruler. Brush/surface/local-offset attachments and
3D workplanes are unavailable.
See [Measurement guides](measurements.md).

`get_entity_placement` reports the current snapshot and owned draft.
`begin_entity_placement` takes that snapshot, a point `classname`, optional
`properties`, `offset`, `align` and `snap`. `pick_entity_placement` takes the
returned `token` and `ray_origin` / `ray_direction`. Inspect the plan's target,
contact, bounds and final
properties before Apply. Review replaces the ownership token; Apply creates one
entity and one Undo step. Cancel preserves source/history. Armed drafts block
save/build/play until applied or cancelled. See [Entities](entities.md).

### Rope preview

`set_view` accepts `rope_preview`; `get_rope_preview` reports checked static
spans, diagnostics and limits. `get_map_info` exposes the same state. Temporary
`render_view` and `pick_at_pixel` overrides preserve the shared setting; use the
same override when picking a rendered image. Spans belong to their source
entity and obey depth, visibility and working-region filtering. This preview
does not simulate runtime physics or render rope materials. See [Ropes](ropes.md).

### Grid defaults and selection outlines

`get_grid_defaults` reports the current and saved grid triple, factory defaults,
storage path, readiness/errors and an action snapshot. `set_grid_defaults` takes
`snapshot` and `action`: `save` persists the current grid, `apply` loads saved
values into this session, `reload` rereads storage, and `reset` explicitly saves
factory values without applying them. Only grid size, snapping and visibility
persist; ordinary `set_spatial_settings` calls stay temporary. Stale storage or
write failures preserve the current grid and map/history. A completed write with
failed verification reports `write_committed: true` and an explicit error; inspect
readiness before continuing. Save/Reset compare the retained source under a
stable nonblocking Linux lock; Apply checks its original directory. A busy lock
preserves current values and disk bytes. Changed directories require Reload;
nonparticipating writers remain outside that guarantee. See [Grid defaults](basics.md#grid-and-snapping).

`set_view` and `render_view` accept `selection_outlines` (factory default true).
`get_map_info.view` and render metadata report the effective value. Disabling
outlines retains selection tints, geometry, picking, diagnostic lines and tool
guides. Temporary render overrides do not change the shared view. See
[Selection outlines](basics.md#selection-outlines).

`set_view` and `render_view` also accept optional booleans `entity_images` and
`entity_model_guides`, both true under factory defaults. Shared
`get_map_info.view` and render metadata expose the effective flags. The human controls are **Entity images**
and **Entity model guides**. They independently control literal FGD
`iconsprite(material)`/static Sprite resource images and `studio`/`studioprop`
editor meshes respectively. Disabling either retains the ordinary editable FGD box, including bounds, picking,
contact, selection and framing. Authored model entities, specialized lights,
ropes and other guides retain their existing behavior.

```json
{"entity_images": false, "entity_model_guides": true}
```

A temporary `render_view` override returns its effective flags without changing
the shared view, camera, source or history. Pure changes to these two shared
flags preserve pending model-choice, placement and review tokens. Both onscreen
and offscreen scene cache keys include the flags; resource visibility changes
the view revision without invalidating the source scene revision.

Otherwise eligible disabled helpers report `suppression_reason:"view_disabled"`
and defer asset reads and cache admission. Icon status is `suppressed`, with no
source/texture provenance or dimensions and an explicit deferred-admission
message. Model state is `not_loaded`, with `declaration_supported:true`, null
visual bounds/pose/skin/body and no asset error. Structural, hidden and
working-region suppression retains precedence; unsupported declarations still
report their pure declaration errors. Disabled reports do not establish asset
readiness. Valid Sprite inputs report `state:"suppressed"` and `image:null`
while disabled; unavailable scale/dispatch still reports its input error.
Toggling visibility never clears an existing successful or failed asset cache
entry. After external repairs, use `reload_material_assets` for icons/Sprite
resources or `reload_model_assets` for model guides.

### Fly speed and navigation defaults

`get_navigation_settings {}` reports current/saved/factory `fly_speed`, bounds,
Shift/wheel policy, storage readiness/errors and the latest action `snapshot`.
`get_map_info.navigation` includes the same report. `set_navigation_settings`
takes `fly_speed`, a finite 25–20000 units per second, matching **Fly speed**
and perspective right-button wheel changes. Factory speed is 600 and Shift
moves 4× faster. These temporary edits never save automatically.

`set_navigation_defaults` takes the latest `snapshot` and `action`:
`save` writes current speed; `apply` restores saved speed; `reload` rereads
without applying; `reset` explicitly writes factory speed without applying.
Only Save/Reset write. Malformed/unavailable storage, outside changes and stale
speed snapshots reject with current valid state preserved; inspect readiness
and `write_committed` after errors. Files are bounded to 4096 bytes, with
symlink/nonregular rejection and retained directory/source guards. Save/Reset
coordinate participating Linux writers through a stable nonblocking lock;
contention preserves saved bytes and current speed. Changed directories require
explicit Reload; programs ignoring the lock remain outside this guarantee.

No action moves the camera, edits source/history/selection or invalidates owned
map reviews/picks. Camera positions and other sensitivities remain separate.
See [Fly speed and navigation defaults](navigation-defaults.md).

### Keyboard shortcuts and viewport commands

`get_keyboard_shortcuts` returns `snapshot`, `current`, `factory_defaults`,
command rows/labels, `ready`, `stored`, storage `path`, `error`, `limits` and
reserved scope. It shares **Viewport → Keyboard shortcuts…**. Bindings apply
only with viewport focus; text fields retain native text editing.

`set_keyboard_shortcuts` requires the latest `snapshot` and tagged `action`:

| Action | Behavior |
| --- | --- |
| `save` | Validate, atomically save and immediately activate a complete `bindings` object. |
| `reload` | Load and activate valid storage, preserving active bindings on failure. |
| `reset` | Explicitly write and activate factory bindings, including malformed-file recovery. |

`bindings` must contain `undo`, `redo`, `frame_selection`, `delete_selection`
and `duplicate_selection`. Optional `perspective`, `top`, `front`, `side`,
`objects`, `faces`, `show_grid`, `snap_to_grid`, `tool_textures` and
`selection_outlines` slots default to unbound when omitted, including existing
version-1 files. Loading preserves the existing file bytes until an explicit Save.
Each value is exactly two nullable slots; each
nonnull chord is `{"key":"F","control":false,"shift":false}`. Keys are
logical A–Z, F1–F12, Delete and Backspace, with case normalization and exact
Ctrl/Shift modifiers. Duplicate/reserved chords and unknown fields reject.
Alt/Meta, scan codes, sequences and other commands are unavailable. File,
clipboard, camera, navigation and Escape controls remain reserved.

The versioned `shortcuts.json` file lives beside grid/preview preferences and
caps at 16 KiB. Save is blocked after storage errors until Reload succeeds or
explicit Reset. Writes compare inspected identity/bytes; nonregular targets
and final-file symlinks reject. `write_committed:true` reports a completed write
even if verification failed. Save/Reset compare the retained source under a stable,
nonblocking Linux lock. Changed directories require explicit Reload. Programs
that ignore the lock remain outside this guarantee. Configuration
changes preserve map/history/selection/camera and active drafts.

`execute_editor_command` accepts `command` using the shared command identifiers.
Undo/Redo perform one step and return a nullable label; Frame accepts optional
positive finite `aspect` (default 1). Delete uses the current object selection;
Duplicate copies it by `[grid,grid,0]` and selects the copies. Object edits reject
empty selection and component-editing scope. This shared dispatcher does not
synthesize keyboard input. `perspective`, `top`, `front` and `side` use the same
per-projection camera memory as the viewport controls and return the resulting
projection with `camera_changed:true`. They affect only the originating Session,
leaving the map, selection and history unchanged. The four projection factory bindings are
unbound. `objects` and `faces` call the same selection-mode operation as
`set_selection_mode` and return `mode` plus `mode_changed` for the actual boolean
transition. Both default unbound. Either command exits component editing;
a changed mode clears selection and obsolete drafts/gestures, and entering Faces
stops clipping/brush drawing. Repeating the current mode otherwise preserves
ordinary selections and drafts. Object-type filters remain configured across
mode changes. Camera, source and Undo remain unchanged. Existing
explicit-ID/offset object tools remain
available. See [Keyboard shortcuts](keyboard-shortcuts.md).

`show_grid`, `snap_to_grid`, `tool_textures` and `selection_outlines` invert the
originating Session's live setting and return its actual `grid_visible`, `snap`,
`tool_textures` or `selection_outlines` boolean respectively. Their factory
bindings are unbound. They use the same `set_spatial_settings`/`set_view`
operations as the checkboxes, including existing spatial/drag cleanup; tool
texture visibility also retires area/volume/paint gestures. They do not write
map data, Undo history or saved grid/preview defaults. `aspect` is unused.

### Saved preview defaults

`get_preview_defaults` reports `current`, `saved`, effective `defaults`,
`factory_defaults`, `stored`, `ready`, storage `path`, errors and an action
`snapshot`. It reads no guide assets and starts no renderer. The five stored
booleans are `selection_outlines`, `entity_images`, `entity_model_guides`,
`rope_preview` and `sky_preview`; the first three factory defaults are true,
and rope/sky are false.

`set_preview_defaults` accepts only `snapshot` and `action`:

| Action | Behavior |
| --- | --- |
| `save` | Explicitly write the current five booleans as startup defaults. |
| `apply` | Restore saved defaults into this session without writing. |
| `reload` | Reread storage without changing the view. |
| `reset` | Explicitly write factory defaults without applying them. |

The UI is **Preview defaults… → Save current as defaults / Apply saved defaults /
Reload defaults / Reset saved defaults**. Ordinary viewport toggles, `set_view`
and temporary `render_view` overrides never save automatically. Normal-map
preview, sky channel and exposure, fog, rendering mode, tool filters, camera,
grids and active tools stay outside this format. See [Preview defaults](preview-defaults.md).

Save/Apply/Reset recheck inspected file identity and bytes. Invalid, unsupported
or oversized files remain untouched and block Save/Apply until Reload succeeds
or Reset is explicitly requested; files cap at 4096 bytes. Nonregular targets
and final-file symlinks reject even on Reset. Only Save/Reset write. A completed
write followed by failed verification returns `write_committed:true` with an error;
inspect readiness before continuing. Save/Reset compare the retained source
under a stable, nonblocking Linux lock. Apply checks the original directory;
changed directories require explicit Reload. Nonparticipating writers remain
outside the locking guarantee. Coordinated publication requires Linux; other
platforms reject writes.
Apply preserves unrelated view settings, source/history/selection/camera and
rejects active geometry/measurement previews, viewport tools, builds or launches.
Save/Reload/Reset leave the view and pending viewport contexts unchanged.

### Normal-map preview

`set_view` accepts optional boolean `normal_maps` (default false), matching
**Viewport → Normal map preview**. `render_view.normal_maps` overrides only
that render; shared settings and the desktop camera stay unchanged. Effective
flags appear in `get_map_info.view` and render metadata. This flag is outside
saved Preview defaults and preserves pending geometry reviews.

`get_normal_map_preview {}` returns `enabled`, `summary`, `report` and `limits`
through the shared CPU scene without initializing graphics. Reports identify
admitted materials, mounted VMT/VTF provenance, applied/rejected flat brush
polygons, transforms, bounded errors and truncation. Render metadata includes
`normal_maps: {report, limits}` for the requested view. Disabled and non-Textured
views perform no normal admission.

Only ordinary flat LightmappedGeneric polygons with one static supported RGB
`$bumpmap`, identity base transform and positive axis-scaled/translated bump
transform are covered. Terrain, models, helpers, tool surfaces, proxies,
fallbacks, secondary/detail normals, signed/swizzled/HDR/SSBump data and special
sampling flags reject. Unsupported faces retain their base image. Limits are
1024 pixels per texture axis, 16 MiB per VTF, 64 KiB per VMT/Patch file, 128 cached
admissions and 64 MiB decoded pixels. `reload_material_assets` clears successes
and failures; external VMT changes require reload before combining new normal
settings with a cached base. Ordinary shading uses the fixed editor light;
Detail authored lighting can compose with normals. Native appearance parity
remains separate. See [Normal-map preview](normal-maps.md).

### Authored lighting preview

`set_view` and temporary `render_view` accept `lighting_preview` (default false),
`lighting_quality` (`fast` or `detail`, default Detail) and
`lighting_exposure_ev` (finite −8..8, default 0). Fast interpolates vertex
contributions; Detail evaluates fragments and supports `normal_maps`. Active
Fast plus normal maps rejects atomically. These resource settings preserve
camera, selection, source/history and owned editing reviews.

`get_lighting_preview {}` uses the shared CPU scene without graphics startup.
It returns admitted/excluded emitters, flat receiver polygons and separate
`report.terrain` surface/drawn-triangle counts and `report.models` admitted/excluded
instance, selected-part and drawn-triangle counts, bounded diagnostics, limits, summary and the captured document/render/region/material
source stamp. `render_view.lighting_preview.report` belongs to the scene that
was rendered; temporary overrides leave shared settings unchanged.

Only visible constant unnamed style-0 `light`/`light_spot` entities and opaque
flat LightmappedGeneric brush receivers or supported power-2/3/4 world/detail
terrain are admitted. Terrain uses checked sewn LOD normals, no declared
bump/blend/detail effects and the same cached opaque base/material identity;
outline-only or suppressed tool/sky sheets count zero drawn triangles. Models
require whole-selected-instance opaque single-base VertexLitGeneric admission,
Normal render mode, receiver positions within ±16384 and safe transformed world
normals. Selected bodygroups/skins, supported scale and RGB tint use the same evaluator. Unsupported
selected model material declarations exclude the entire instance with an entity/
material reason. Hidden meshes and helper guides do not enter model counts; tool-
suppressed batches contribute zero drawn triangles. The same cached material and
base image determine admission until `reload_material_assets`; no fresh VMT is
paired with old pixels. See [Models](models.md#preview-authored-lighting).
The 32-light or summed
radiance limit disables the whole preview with a reason. Zero admitted lights
make supported receivers black. Excluded surfaces retain ordinary display.
There are no shadows, sun, ambient, bounce, animation or native brightness
claims. Display applies linear diffuse contribution, exposure and per-channel
Reinhard before sRGB output. Non-Textured modes perform no light admission.
See [Authored lighting preview](lighting-preview.md).

### Atlas-region face texturing

1. Call `inspect_material_atlas` with `material`. It returns a CPU PNG thumbnail
   no larger than 256 pixels per side, original `width` / `height`, `snapshot`,
   texture identity, cache policy and limits without initializing graphics.
2. Call `preview_atlas_fit` with `material`, that `snapshot`, and
   `rectangle:{x,y,width,height}`. Coordinates are integer texture edges with
   top-left origin; width/height must be positive and the rectangle must fit the
   original image. Optional `faces` contains `{solid,side}` pairs; otherwise the
   selected faces are used. Batches admit 1–256 distinct flat faces.
3. Inspect `atlas.preview` or `get_face_selection.atlas`. The preview reports
   string `token`, `owner`, `material`, `snapshot`, `rectangle`, `dimensions`,
   normalized `uv_bounds` and face identities. To revise an owned atlas draft,
   pass its current string `token` to `preview_atlas_fit`.
4. Pass the exact token to `commit_face_preview` for one Undo step, or
   `cancel_face_preview` to discard it. Foreign owners and stale tokens reject;
   stale assets block Apply while the owner can still Cancel.

Each world/brush-entity face fits independently, preserving axes, skew,
rotation, mirroring, geometry and lighting metadata. Terrain and invalid or
collapsed projections reject atomically without replacing a valid preview.
Supported atlases are static single-layer LightmappedGeneric/UnlitGeneric base
images with identity base transforms; a `%tooltexture` must match the actual
`$basetexture`. Proxies/fallbacks, extra layers and animated/volume/cubemap/
procedural/render-target images reject. Texture axes cap at 8192 pixels; new
reads cap at 1 MiB per VMT/Patch and 32 MiB per VTF, with a 16-megapixel thumbnail
decode budget. Cached base pixels supply both thumbnail and UV dimensions until
`reload_material_assets`; a fresh/cached VMT identity mismatch requires reload.
Unrelated normal-image admission does not invalidate an unchanged atlas review.
No texture-file writes, atlas packing, named hotspots, grouped continuity,
filter gutters or native rendering parity is added. UI: **Face Textures →
Fit atlas region… → Inspect material → Preview atlas fit → Apply atlas fit /
Cancel**. See [Atlas-region texturing](atlas-texturing.md).

### Material source and FGD preferences

`get_material_source {"material":"path/to/material"}` returns raw VMT text,
the winning directory/VPK origin, bounded Patch includes and resolved shader/
texture declarations. Read/parse/decode failures appear in `error`; invalid input
paths reject. The report exposes limits and truncation flags. It never substitutes
a lower-priority source behind a broken winner and changes no cached texture,
map, selection, preview or history. See [Material source](material-source.md).

`preview_material` accepts optional `channel: "color" | "alpha"`, matching
the material browser's **Color / Alpha** control. Omitted/null selects Color
and preserves the existing PNG. Alpha outputs opaque `[a,a,a,255]` from that
same decoded preview image. Metadata includes `channel`, original decoded
`alpha_range`, texture reference, cached material identity, `cached`, mip,
frame and face. Missing assets stay unavailable; errors return no replacement
image. The current-source inspector is separate from cached preview provenance.

`get_entity_definitions` reports loaded classes, sources, load error and optional
`map_size: {authored:[first,second], bounds:[minimum,maximum], source, line}`.
Load/search responses and `get_map_info.definitions` include the same metadata.
FGD `@mapsize` is informational: reversed endpoints sort, equal endpoints leave
the previous preference, and the last effective declaration wins in existing
include order. An absent declaration reports null; coordinate limits do not
change. `get_entity_inspector.helpers` also reports resolved FGD visual helpers:
`status`, `bounds_source`, `invalid_size`, `local_bounds`, `world_bounds`,
`class_color`, `errors`, `unsupported` declaration names and `definition_sources`.
Inherited/extended `size(width height depth)` and `size(mins, maxs)` declarations
supply world-aligned generic point boxes; `color(r g b)` sets their unselected
class color. Native integer arguments, positive volume and local coordinates
within ±65536 are required. Loaded model meshes and specialized lights retain
precedence. Invalid/ambiguous size declarations expose an amber marker and
`invalid_fgd_size`; working-region bounds remain unresolved with an explicit
error. Picking, framing, area/volume selection and placement use the same valid
bounds. Helper forms outside the box/color, property-sphere, literal-line,
literal-icon, static Sprite resource, literal-model and admitted frustum scopes
remain listed and unevaluated. Catalog source
paths are available; individual inherited helpers lack file/line provenance.
FGD reload changes no source values or Undo history.

`get_entity_inspector.helpers.spheres` resolves `sphere(property)` and
zero-argument `sphere()` declarations
for point entities, including brushless FGD `@FilterClass` definitions such as
`filter_enemy`, through the same Session operations as the property panel and
`render_view`. Each entry reports `property`, `raw`, `implicit_property`, `value_source` (`authored`,
`fgd_default` or `missing`), `property_type`, `radius`, `center`, potential
`guide_bounds`, intrinsic `state`, `error`, `displayed` and `suppression_reason`.
`raw_truncated` and `property_truncated` identify bounded diagnostic previews;
`sphere_limits` states the scope and caps. Use ordinary `set_entity_properties`
with `set` or `remove` to author or clear a radius override. No new tool or
viewport setting is required.

`sphere()` binds the declared `radius` property, matched case-insensitively,
only when its FGD kind is `float`, `integer` or `string`. The reported property
uses the declaration's spelling. `implicit_property` is always serialized:
true for a zero-argument attempt, including an unsupported missing/wrong-type
declaration; false for named helpers and the synthetic limit diagnostic. It
describes the binding independently of `value_source`. Missing/incompatible
declarations report `state:"unsupported"`, no radius/bounds and no ring.
Equivalent effective keys such as `sphere()` and `sphere(radius)` deduplicate
case-insensitively in resolved declaration order, retaining the first binding
flag. Declared properties without a usable value retain the existing
missing/invalid diagnostics; zero or negative values remain nonpositive with
no fabricated fallback radius or infinite guide.

Positive finite `float`, `integer` and numeric `string` values draw three
world-axis rings for selected visible generic point entities. Missing authored
values use the FGD default; authored zero/negative values suppress the guide
without falling back. Invalid/missing/duplicate/unsupported values retain source
text and report errors without fabricated radii. Loaded models, model/studio
classes (including missing assets), brush entities, specialized lights, hidden
objects and working-region exclusions suppress
display with explicit reasons. Intrinsic state remains independent of display
eligibility. At most sixteen distinct effective properties per entity, 128 guides per frame
and 32 segments per ring are supported. Scalar values must be finite and within
±65536; native conversion must retain finite, noncollapsed geometry. Diagnostic
previews cap raw values at 256 bytes and property names at 128 bytes; oversized
input is rejected before clipped text can be parsed. Rings remain visible when
`selection_outlines:false` hides ordinary selection edges.

`helpers.world_bounds`, picking, area/volume matching, working-region inclusion,
placement and framing retain the ordinary object bounds; `guide_bounds` never
becomes the selection target. Rings describe editor distances and do not certify
engine effect ranges or nonpositive runtime behavior. Inspection and rendering
do not edit source, camera or history. Sphere centers follow displayed geometry
previews; ordinary helper bounds retain their committed-object reporting policy.
See [Entity definitions](entities.md).

`get_entity_inspector.helpers.lines` reports the shared selected FGD line guides.
`line(R G B, targetname, property)` connects the source origin to the point named
by that property; adding a second lookup/property pair connects the two
referenced points. A lookup can also be `nodeid` under the authored-ID rules
below; the two-reference form can mix lookup kinds. RGB must be literal integers 0–255 and stays independent of
`rendercolor`. References use authored values before labeled FGD defaults;
endpoint origins must be authored once. Targetname lookup also requires one
authored targetname; only unique exact-case literal names resolve.
Case-insensitive collisions remain ambiguous even when
a duplicate is hidden or outside the working region; a case-only match reports
`unsupported_case_match`.

Each line entry contains `color`, two `endpoints`, intrinsic `state`, `error`,
`displayed` and `suppression_reason`. Each endpoint reports `property` (null for
self), `lookup_property` (`targetname`, `nodeid` or null for self/unsupported),
`raw`, `raw_truncated`, `value_source` (`self_origin`, `authored`,
`fgd_default` or `missing`), numeric `entity`, world `position`, candidate
`matches`, `state` and `error`. Missing/duplicate properties, duplicate
targetnames, invalid origins, unsupported references, ambiguity, coincident
positions and lookup limits remain explicit states. `helpers.line_limits`
describes the supported forms and caps.

Use existing `set_entity_properties` with `set`/`remove` to edit references and
`load_entity_definitions` to reload helper declarations. `render_view` uses the
same Session report and displayed transform-preview origins, including model
or light point endpoints. Sources must be selected visible generic points;
brush, model/light/rope sources and model/studio helper classes remain
specialized. Hidden or working-region-excluded endpoints suppress display
without changing intrinsic resolution. Wildcards, `!tokens` and brush
endpoints are unsupported. Finite origins must fit ±65536 units and remain
distinct at native float precision. At most sixteen declarations per entity,
128 displayed lines and 65,536 lookup entities or raw targetname plus eligible
nodeid values are supported;
property names cap at 128 bytes and raw references at 256 bytes, with explicit
truncation rather than resolving clipped text. Exceeding the lookup cap reports
`entity_limit` without a partial search. Guide lines stay visible when
`selection_outlines:false` hides selection edges. They do not change object
bounds, picking, framing, placement, selection, working regions, source,
history or camera. No new tool or setting is added; these are editor links,
without runtime beam, parenting, attachment or I/O simulation.

For `line(R G B, nodeid, property)`, the source reference property must be
declared `node_dest`. Destinations must have a loaded FGD point class with a
declared readonly integer `nodeid` and exactly one authored ID. Only canonical
decimal `0`–`2147483647` resolves; zero is valid and the exact reference `-1`
reports `no_association` without geometry. Endpoint IDs never come from FGD
defaults. References retain independent `authored`/`fgd_default` provenance.
Imported IDs remain read-only through ordinary property operations; this
feature adds no allocator, assignment or renumbering operation.

The endpoint's numeric `entity` is the resolved VMF entity ID; it is distinct
from the reference's authored node ID. VMF IDs, entity order and runtime node
indices are never inferred as node associations. Missing IDs report `missing`;
multiple matching entities report `ambiguous`, including hidden or
working-region-excluded numeric aliases. Bounded raw aliases such as `042`,
`+42` and padded `42` participate in ambiguity checks but never render.
A sole alias reports `invalid_nodeid`, duplicate endpoint keys report
`duplicate_nodeid`, and noncanonical references report `unsupported_reference`.
Oversized or unparsable candidate IDs do not resolve. The shared 65,536-value
budget counts raw targetnames and eligible nodeid values before parsing or
filtering, with `entity_limit` and no partial search on overflow. Existing
128-byte property, 256-byte raw-value, origin, precision and visibility guards
remain unchanged. Model/light points may be endpoints; their source helpers
retain precedence. These guides show stored authored associations without
native AI graph connectivity, runtime node-index mapping or ground-node dropping.

`get_entity_inspector.helpers.source_projector` reports the independent Source
projector guide for `env_projectedtexture` with a loaded FGD class; other classes
or missing definitions return null. Existing `set_entity_properties`,
`set_entity_color` and `render_view` use the same Session operations as the
property panel and viewport. No new tool or setting is added. The guide uses
authored Source Pitch Yaw Roll: zero angles face +X, right is −Y, up is +Z and
positive pitch points down. Horizontal and vertical FOV are equal; near/far
distances measure along the forward axis. Displayed transform previews update
the same eight corners and twelve edges.

The report contains `orientation_policy`, `inputs`, `fov_degrees`, `near_z`,
`far_z`, `color`, optional fourth-component `intensity`, `origin`, `angles`,
`corners`, potential `guide_bounds`, intrinsic `state`, `error`, `displayed` and
`suppression_reason`. Each input reports `property`, `property_type`, `raw`,
`raw_truncated`, `value_source` (`authored`, `fgd_default` or `missing`) and
`error`. Inputs cover `origin`, `angles`, `lightfov`, `nearz`, `farz`,
`lightcolor`, `target`, `parentname` and `cameraspace`. Origin must be authored;
other values use authored fields before compatible declared FGD defaults.
The human summary identifies defaults and warns that engine defaults may
differ. RGB colors the guide; intensity is reported without opacity or exposure
conversion, and RGB edits preserve an omitted fourth component.

Within `helpers.source_projector`, `helper_pitch_scale_evaluated` remains false.
`helper_pitch_scales` retains up to
sixteen FGD `frustum()` declarations, with `argument_count`, `declaration_state`
(`declared_unevaluated` for five arguments, otherwise `unsupported_helper_form`),
the fifth argument's `raw`/`raw_truncated`, and `evaluated:false`.
`omitted_helper_declarations` counts additional declarations. Changing this
unevaluated argument changes inspection metadata without changing Source guide
geometry. The independent Source orientation does not consume FGD pitch scale;
`helpers.frustums` evaluates admitted FGD declarations separately.

`helpers.projector_limits` describes the admission and display caps: at most
128 displayed projectors in entity-ID order, 256-byte values and helper
argument previews, 0 < FOV < 180 and 0 < nearz < farz ≤ 65536. All eight corners
must remain finite and pairwise distinct at native precision, with nonzero
near/far plane areas and coordinate offsets within ±65536 of the origin.
Malformed, duplicate, missing or oversized values are not clamped or replaced.
Nonempty `target`/`parentname`, nonzero `cameraspace` and legacy `angle` are
unsupported. Display requires a selected visible point source inside the
working region; loaded models, model helpers and brush sources retain
precedence. Intrinsic state remains separate from suppression. Guide extents
never change object bounds, picking, selection, placement, framing or working
regions, and read/render operations preserve source, history and camera. The
guide does not preview projected textures, lighting, shadows or runtime state.

`get_entity_inspector.helpers.frustums` reports evaluated Hammer2009 FGD
`frustum()` authoring guides through the same Session operations as the property
panel, viewport and `render_view`. Existing `load_entity_definitions` and
`set_entity_properties` reload declarations or edit bindings; no new tool or
setting is added. Zero to five arguments are supported. Omitted first-four
bindings are `_fov`, `_NearPlane`, `_FarPlane` and `_light`; omitted fifth
`pitch_scale` is `1`. First-three bindings require declared `float`/`integer`
properties, fourth `color255`. Origin is authored-only and angles require an
`angle` declaration. Authored values precede labeled FGD defaults; absent
values never acquire native constructor substitutes. Numeric helper bindings
allow surrounding whitespace without changing reported `raw` text or its byte
limit.

Each row contains `declaration_index`, bounded `arguments`,
`arguments_truncated`, `inputs`, `orientation_policy`, `pitch_scale_evaluated`,
`pitch_scale`, `pitch_scale_source` (`helper_argument` or `helper_default`),
`fov_degrees`, `near_z`, `far_z`, `color`, `intensity`, `origin`, raw `angles`,
`effective_angles`, eight `corners`, potential `guide_bounds`, intrinsic `state`
(`ready`, `invalid` or `unsupported`), `error`, `displayed`, `presentation` and
`suppression_reason`. Inputs retain `property`, `property_type`, `raw`,
`raw_truncated`, `value_source` (`authored`, `fgd_default` or `missing`) and
per-input `error`. Parent `omitted_frustum_declarations` counts declarations
beyond sixteen; `frustum_limits` gives the scope and resource limits.

The proven authoring transform is
`T(origin) · Rz(yaw) · Ry(-pitch_scale × raw_pitch) · Rx(roll)`.
Scale multiplies raw pitch before angle reduction, including pitches beyond
360°. Zero angles face +X. This is the installed Hammer2009 helper contract;
it does not establish universal Hammer++ or native projection behavior.
Fourth-component color intensity remains metadata, independently of RGB guide
color and opacity.

`presentation:"separate"` draws an eligible generic guide. An actually
displayed Source projector with exactly equal corners/RGB yields
`shared_with_source_projector`, reusing its one presentation. Ready FGD
geometry on `env_projectedtexture` that cannot share reports
`presentation:"suppressed"` and `source_projector_precedence`, including
unavailable Source branches. The Source report and its orientation remain
independent. Models, specialized lights/ropes/sprites, hidden/unselected
sources and working-region exclusions retain explicit suppression reasons.

Binding names require 1–79 UTF-8 bytes, must be distinct case-insensitively and
cannot alias `origin` or `angles`. Literal scalar/RGB helper extensions and
legacy `angle` are unsupported. Values/argument previews cap at 256 bytes;
scale and its raw-pitch product must remain finite at native precision.
Nominal FOV is 0.02–178 degrees inclusive, with 0 < near < far ≤ 65536.
Corners must remain pairwise distinct and finite with nonzero native plane
areas and coordinate offsets within ±65536 of the origin. Invalid inputs are
never clamped or replaced. At most sixteen declarations per entity and 128
displayed guides in entity-ID/declaration order are supported. Display requires
a selected visible FGD point class. Displayed
previews update geometry without changing ordinary object bounds, picking,
selection, placement/contact, snapping, framing or working-region membership.
Read/render operations preserve source, camera and history. The guides do not
preview projected textures, lighting, shadows, parenting or runtime state.

`get_entity_inspector.helpers.icon_sprite` reports a loaded class's literal FGD
`iconsprite(material)` through the same Session inspection and scene operations
as the property panel, viewport and `render_view`. A class without this helper
returns null. Exactly one declaration with one literal material argument is
supported; `helpers.icon_limits` describes admission and display limits.
`entity_images` controls their visibility; `load_entity_definitions` loads or
reloads declarations. No new tool is added.

The report contains `status`, `material`, optional material `source`,
`texture`, `texture_source`, pixel `width`/`height`, `world_size`, `error`,
`displayed` and `suppression_reason`. Status is `ready`, `unavailable`,
`unsupported`, `invalid` or `suppressed`. Directory sources contain
`{kind:"directory",mount,file}`; VPK sources contain `{kind:"vpk",index,files}`.
`material` is canonical after admission starts; deferred or invalid references
retain a bounded preview. Errors are bounded. `orientation_policy` is
`live_camera_right_up`, `rendering_policy` is `unlit_rgba_editor_image` and
`size_policy` is `32_source_unit_long_axis`: the image's long axis is 32 Source
units, preserving texture aspect ratio. It faces the camera passed to each
render in Top, Front, Side and perspective views, independently of entity
angles, selection outlines, illumination and fog. The editor class image
retains texture RGBA alpha.

Admission requires a direct UTF-8 `Sprite` material with `$basetexture`, explicit
`$spriteorientation vp_parallel` and centered `$spriteorigin "[0.5 0.5]"`
(an optional third zero is accepted). Optional `$no_fullbright` must be `1`;
the editor image remains unlit. Other parameters, duplicates, nested blocks,
Patch materials and legacy `.spr` aliases are unsupported. The VTF must decode
all mips as a static single-frame, single-face, depth-one LDR image, with axes
1–512. References cap at 256 bytes, VMTs at 64 KiB and VTFs at 4 MiB. A material
generation caches at most 64 successful or failed entries and 32 MiB of encoded
assets; `reload_material_assets` clears both successes and failures after
external changes. The human equivalent is **Browse materials… → Reload material
assets**. A failed winning mount does not fall through to a lower-priority
asset.

Admission failures retain the ordinary helper and an explicit error, without a
replacement image.

Origins must be finite and within ±65536 units. At most 256 visible icons display
in entity-ID order. Hidden sources,
working-region exclusions, invalid origins, nonpoint/brush sources, loaded
models, model or sprite helpers, specialized lights and ropes suppress the
image with explicit reasons. Suppression defers asset admission, so the report
does not claim readiness or provenance for an unread asset. Eligible icons
display selected or unselected and follow displayed transform previews.
Ordinary FGD helper boxes retain bounds, picking, area/volume selection,
placement, framing and working-region inclusion; image extents do not become
selection targets. Read/render operations preserve source, history and camera.
Animated icons, runtime `env_sprite` behavior and native Sprite rendering modes
remain outside this icon policy. Static resource guides use the separate policy
below.

### Static Sprite resource guides

`get_entity_inspector.helpers.sprite_resource` reports one zero-argument
`sprite()` declaration on an eligible point class with `model(sprite)`.
`model` and `scale` inputs retain bounded raw values, truncation flags and
`value_source` (`authored`, `fgd_default` or `missing`). The report includes
`source_kind`, `declaration_supported`, `resolved_vmt`, native `scale_value`,
physical `world_size`, `state`, `displayed`, suppression/error information and
an `image` report containing winning VMT/VTF origins and pixel dimensions.
`helpers.sprite_resource_limits` exposes the complete policy. Sprite-valued
model fields are classified before MDL loading; `inspect_entity_model` directs
these sources to the Sprite report.

Resource identity follows these explicit dispatch rules:

| Authored Sprite Name | Resolved VMT |
| --- | --- |
| `sprites/glow01.spr` | `materials/sprites/glow01.vmt` |
| `glow01.vmt` | `materials/sprites/glow01.vmt` |
| `materials/glow01.vmt` | `materials/glow01.vmt` |
| `materials/materials/foo.vmt` | `materials/materials/foo.vmt` |

Extensionless model values, legacy `.spr` references outside
`sprites/<basename>.spr` and dots in filename stems are unsupported. Dots in
relative directory names are allowed; traversal is rejected. Inspection and
opening/cancelling **Sprite Name → Browse…** preserve the exact stored reference.
An explicit catalog choice authors `materials/<catalog-relative-name>.vmt`
through the existing `set_entity_properties` operation. Thus catalog name
`materials/foo` produces `materials/materials/foo.vmt`; no prefix is repeatedly
removed. Sprite Name uses the material control and **FX Color** keeps its FGD
label.

The guide's width/height are admitted texture dimensions multiplied by one
positive finite authored or labeled FGD Scale. Missing, blank, zero, negative,
malformed, nonfinite or native-unrepresentable sizing stays unavailable, without
an inferred default. Native dimensions/vertices must remain finite with relative
corner extents within ±65536. Literal icons keep their independent 32-unit policy.
Both use the strict direct centered `Sprite`/static LDR admission above and share
the 64 success/failure entries, 32 MiB encoded budget and 256 visible images in
entity-ID order. `reload_material_assets` clears both successes and failures;
a malformed winning mount never selects a lower substitute.

`entity_images` gates admission and drawing, including temporary `render_view`
overrides. Ready images replace only marker fill, face the actual draw camera
and use `unlit_rgba_editor_image_no_fog` with `texture_pixels_times_positive_scale`.
Ordinary FGD boxes retain bounds, selection outlines, picks, placement, framing
and working-region membership. Failures retain editable diagnostic markers.
Oriented/parented sprites, nonzero angles/legacy angle, brush sources, loaded
MDLs, studio/model helpers, specialized lights and ropes are excluded.
Raw guides keep `runtime_fields_evaluated:false`; appearance provenance lives
in `blend_preview`. Color/amount/FX, HDRColorScale, glow visibility, animation,
enable state and I/O remain unevaluated. This independent editor image does not
establish native sprite parity or extend native reflection-capture eligibility.
See [Sprite resource guides](sprite-guides.md).

`set_view.additive_sprite_guides` and temporary
`render_view.additive_sprite_guides` opt into authored mode-5 LDR guides;
`entity_images` remains the parent control. One unique explicitly authored
Render Mode 5 is required, with white 255 RGB, amount 255 and FX 0 supplied by
unique authored values or individually labeled FGD defaults. Unsupported,
missing, duplicate or malformed appearance values retain the raw guide with an
explicit reason. HDRColorScale remains unevaluated and imposes no LDR constraint.

`helpers.sprite_resource.blend_preview` reports requested/supported/applied
states, effective policy, raw values and authored/FGD provenance, the evaluated
guide mode and diagnostics. `render_view.sprite_guides` uses the effective
temporary flags, up to 256 rows with complete totals/truncation. Shared cached
images stay immutable. RGB uses SRC_ALPHA/ONE, destination alpha is preserved
as an editor policy, depth writes remain disabled and mixed guides keep stable
camera-depth order. Literal icons retain their ordinary alpha policy.

### Literal FGD model guides

`get_entity_inspector.helpers.model_guide` reports one literal
`studio("models/*.mdl")` or `studioprop("models/*.mdl")` declaration from the
loaded FGD catalog. A class without either declaration returns null. Existing
`load_entity_definitions`, inspector and `render_view` calls share the property
panel and viewport's Session operations. `entity_model_guides` controls their
visibility; no new tool is added.

The report contains `source:"fgd_literal"`, `helper`, exact `literal`,
`literal_truncated`, canonical optional `path`, `declaration_supported`, `state`,
`displayed`, `suppression_reason`, `error`, separate `visual_bounds`, optional
`pose` (`static` or `reference`), `skin`, `body` and
`material_policy:"flat_editor"`. State is `unsupported`, `not_loaded`, `ready`
or `unavailable`. A valid declaration suppressed before lookup remains
`not_loaded` without reading its asset; the scene geometry budget can reject a
loaded asset as `unavailable`. `helpers.definition_sources` lists catalog files.
This declaration provenance does not identify the winning MDL/VVD/VTX mount,
and the report exposes no authored transform fields or per-helper file/line
locations. `helpers.model_guide_limits` gives the supported scope and budgets.

Only one exact literal path is admitted, at most 256 bytes, with no surrounding
whitespace, traversal, controls or wildcards. Zero-argument, property-driven,
extra-argument and multiple declarations remain unsupported. The shared model
loader uses LOD0 static/reference geometry, existing pose/skin/body validation
and unit-modelscale guards. Origins and complete visual bounds must remain
finite within ±65536 units. At most 64 eligible source evaluations, including
failed lookups, run in entity-ID order per scene, with 65,536 vertices and
393,216 indices. The existing model cache holds at most 128 entries/256 MiB;
MDL, VVD and DX90 VTX files cap at 64 MiB each. Report errors cap at 512 bytes.

Guides display selected or unselected and follow displayed previews. Authored
model fields, including an empty value, declared model properties, specialized
lights/ropes, sprite/decal/overlay helpers and brush/nonpoint sources retain
precedence. Invalid FGD size, hidden or working-region-excluded sources, invalid
origins and scene budgets suppress display with explicit reasons. Missing,
malformed, blank-body or oversized models retain the ordinary editable box and
an error. Visual mesh triangles never become pick or placement surfaces;
ordinary boxes retain bounds, contact, selection, snapping, framing and
working-region inclusion.

`flat_editor` uses the existing geometry renderer and editor material, including
entity `rendercolor` tint, without guide VMT/VTF reads or shader fidelity.
Guide-only cache entries defer material-name resolution and material-reload
prefetch; an ordinary model preview may later resolve bindings on the same
geometry. The guide stays flat. **Reload models** beneath the helper summary
calls the same operation as `reload_model_assets`, refreshing cached successes
and failures. Definition reload re-evaluates declarations; material reload does
not reread model geometry. Inspection and rendering preserve source, Undo,
camera and unrelated model-choice drafts, and add no stored `model` property.
**Model details…** and `inspect_entity_model` continue to target authored model
entities. Animated poses and engine collision fidelity remain unavailable.

### Portable sky assets

`set_compile_settings {"pack_sky_assets":true}` enables the same **Pack sky assets** checkbox as **Build settings…** through the shared Session operation. It is default-off, independent of baked lighting and usable with `fullbright`; enabling starts no process or asset lookup. `get_compile_settings` returns the boolean and `sky_packing_description`. Active jobs freeze settings, and `get_compile_status.job.settings.pack_sky_assets` retains the captured value. Setting updates preserve VMF, history, camera, selection and drafts. Older recipes/recent-map records default this field to false; built-in recipes keep it disabled.

Packing embeds the exact authored six-face family, original VMTs, bounded Patch dependencies and every declared runtime LDR/HDR texture, including overridden references, with original file/mip bytes and relative names. Six static `UnlitGeneric` LDR faces or six `Sky` faces with complete LDR and RGBA16F/RGBA16 HDR bases are supported. Any declared HDR channel must be complete even with an LDR or Fullbright bake. The strict raw-material subset rejects ambiguous, conditional, compressed-HDR, animated, nested or unsupported declarations; no family repair or fallback occurs. Editor-only `%tooltexture` images and unrelated map dependencies are excluded. See [Pack portable sky assets](compile-and-play.md#pack-portable-sky-assets) for exact syntax and budget limits.

Successful output includes `stats.sky_assets:{family,channels,files,asset_bytes}`. `channels` is `ldr` or `ldr_hdr`; file and byte counts exclude the manifest. The optional object is omitted when packing is off. These describe generated output; publication status still comes from `success` / `published` and the job's `status`. **Build details…** displays **Packed sky assets** only for a completed published result, retaining that record when later setting changes make the build stale. A prepared-but-rejected result may retain statistics without having replaced the previous BSP.

The reserved PAK entry `hammerr/packed-sky.json` uses version 1 with exact `family`, `channels` and sorted `assets:[{path,sha256}]`. Paths are canonical and hashes are lowercase SHA-256; no machine-specific mount paths are stored. Offline verification reconstructs the complete raw closure from BSP bytes, checks every hash, supported VTF layout and world `skyname`, and rejects missing, extra, ambiguous or inconsistent records without installed-content fallback. The manifest is at most 64 KiB and 128 asset records; VMTs are at most 1 MiB, runtime entries at most 16 MiB each, full names at most 259 bytes, and the combined encoded/decoded archive is bounded to 64 MiB each, including existing entries and reflections. The archive supports at most 4096 entries and the BSP at most 512 MiB.

Packing requires complete UTF-8 VMT syntax with one root and unambiguous scalar declarations. Directives, duplicate keys, C-style block comments, bracket-wrapped scalars and unsupported/nested parameters reject. Patch replacement requires an existing key; insertion cannot change an existing value. Every declared runtime texture remains in the closure even if a later Patch overrides it. These are HammeRR admission rules; scalar rendering behavior and HDR radiance are not evaluated.

World verification requires exactly one first `worldspawn`, one canonical lowercase `classname` key for it and one case-insensitive `skyname` key whose value matches the manifest verbatim. Every block requires lowercase spelling for `classname` keys; the `worldspawn` class value remains case-insensitive. Entity text is bounded to 4 MiB, 65,536 flat blocks and 262,144 total key/value pairs, with every key and value at most 2047 UTF-8 bytes. Keys that become `classname` or `skyname` after removing trailing ASCII spaces reject rather than act as alternative identities.

The flat reader admits ASCII unquoted tokens without parentheses, single quotes or unsupported control separators. An unquoted token immediately followed by a double quote rejects (`key"value"`); quoted-first adjacency remains parsed normally. Quoted UTF-8, punctuation and non-NUL controls remain literal. Packed-world verification additionally rejects every key/value equal to `{` or beginning with `}`, including quoted tokens. Empty quoted keys/values, quoted `{prefix` and embedded braces remain admitted. These conservative parser and identity rules describe HammeRR's subset, not general native-engine ceilings.

Both synchronous and background builds guard original source identities, winning-mount resolution, destination leases and the current map/path/settings/content snapshot before publication. Packing-only builds need no native capture runtime. Failure, staleness or cancellation preserves previous output. After publication, packing-only Run can verify and stage the sky without the original custom mount, including after material reload. Run verifies before and after generated-cubemap rebasing and preserves the sky manifest and registered bytes literally. Combined reflection builds retain their existing native/runtime/content-freshness requirements. The compiler CLI exposes the same default-off choice as `--pack-sky-assets`.

### Generated reflections

`set_compile_settings {"lighting":"direct_ldr","generate_reflections":true}` enables the same **Generate reflections** control as **Build settings…**. Use `lighting:"direct_ldr_hdr"` to request paired LDR and HDR captures. Enabling starts no process. Fullbright with generation rejects atomically; turn generation off before choosing Fullbright, or supply both changes in one update. `pack_sky_assets` is a separate setting; both features share the final PAK budget and guarded publication.

| Tool | Arguments and behavior |
| --- | --- |
| `get_native_capture_runtime` | Returns resolved `engine_root` and `steam_root`, `configured_engine_root` and `configured_steam_root` overrides, `steam_root_source`, `ready`, `reason` and `editable`, plus `supported_channels:["ldr","hdr"]`, selected `channel` (`"ldr"` or `"ldr_hdr"`), `default_sample_size:128` and `maximum_sample_size:128`. Readiness checks the environment; it neither launches a client nor proves the map has eligible HDR arrays. |
| `set_native_capture_runtime` | Replaces both machine-local overrides with `engine_root` and `steam_root`; omitted or null values clear that override to its checked default. Changed pairs validate before mutation. Invalid updates preserve the previous pair; active jobs freeze changes. |

The default engine installation is the mounted game's base directory only when it verifies as supported CS:S. Custom source assets can use an explicit installation override. The Steam default is the unique running main client's root, checked against client configuration and the installation's library relationship. An explicit Steam override must match that client. These roots stay outside recipes, recent maps and VMF; startup `--engine-root` and `--steam-root` set the same machine context.

Existing `compile_map`, `start_compile` and `run_map` perform the shared compile/capture/verify/publication job. `get_compile_status.job.settings.generate_reflections` and `job.native_runtime` retain its start configuration; `job.reflections.channel` identifies `ldr` or `ldr_hdr`; `job.reflections.samples_complete` and `job.reflections.samples_total` report capture progress. For dual builds, progress totals include both channel passes (`2 ×` sample positions). `stats.reflections.channel` is `ldr` or `ldr_hdr`; `verified_samples` counts logical positions, while `capture_artifacts` lists both receipts for a dual result and the existing `artifacts` path remains the first receipt. Cancellation waits for owned-process cleanup and the preservation audit. Failed, cancelled or stale capture preserves the prior published BSP and launch state without adding map Undo.

Generation supports one capture bounce per channel, 1–32 distinct integer sample positions and cube edges up to 128 pixels; size `0` uses 128 pixels. Direct LDR uses `mat_hdr_level 0`; dual lighting captures LDR first and HDR with `mat_hdr_level 2`, retaining both lighting channels. HDR capture requires nonempty native HDR lighting/world-light/ambient arrays and rejects known HDR fallback warnings. Both matching sample sets and preservation/source guards must pass before one publication. Additional bounces and editor reflection previews are unavailable. The native helper, private compositor dependencies, supported assets and running Steam client must pass preflight. See [Generated reflections](reflections.md) for controls and capture limits, and [Cubemap faces](cubemap-faces.md) for binding authoring.

Generated packing keeps the existing version-1 `channel:"ldr"` manifest for
LDR-only builds. Dual builds use version 2 with `channel:"ldr_hdr"`, LDR `samples`
and matching `hdr_samples`: helper IDs, integer origins, sizes and edges must
pair exactly, with independent bootstrap and packed-asset hashes. HDR textures use
`materials/maps/<namespace>/cX_Y_Z.hdr.vtf`; material aliases remain unsuffixed.
`default_texture` remains LDR; optional `hdr_default_texture` records a captured
HDR default. An extra LDR default from the HDR pass is validated and diagnosed,
with raw evidence retained; it never replaces the LDR pass's published default.

HDR samples require VTF 7.4 RGBA16F with seven native faces and complete mips.
Packing bit-preserves finite nonnegative RGB on the six image faces at every mip,
sets unused alpha to half-float one and zeros unused sphere/header data. Negative
zero is permitted; negative RGB or nonfinite values reject. Reflectivity and
bump metadata must also be finite and nonnegative; unknown flag bits reject. The raw HDR receipt
remains available separately from canonical packed bytes. Both channels share
64 MiB generated assets, 1,024 material aliases and 4 MiB per VTF. Run staging
rebases all registered channel/default names with their data hashes retained.
No new MCP tool or authoring setting is required.

### Explicit model detail props

Use `create_entity` with `classname: "prop_detail"` (or imported alias `detail_prop`), an `origin` and `properties.model` naming a mounted static-prop `models/…mdl` asset whose materials use `UnlitGeneric`. Vertex-lit models reject because the engine substitutes an error model for them. Existing `set_entity_properties`, model choices and transforms edit the same source entities as the inspector. `compile_map`, `start_compile` and `run_map` consume visible detail entities into checked native detail records; hidden entities are omitted. Results expose `stats.detail_props`, `stats.detail_prop_models` and `stats.detail_prop_sprites`, also shown as **Detail props**, **Model assets** and **Sprite entries** in **Build details…**.

These explicit models are non-colliding. Only default skin/body, scale 1, normal orientation and default rendering/fade settings are supported; unsupported rendering overrides, connections, missing assets and solid-leaf origins reject before replacing previous output. Compile regions include models whose authored origins lie inside the inclusive bounds, treating an omitted origin as zero; outside and hidden models are omitted. Region filtering preserves retained source records and source/history; model geometry may cross the box. Normal asset, transform, Source-bounds, open-leaf and lighting-receiver checks still apply; sealing-boundary points are not moved inward. Visible malformed or out-of-range origins reject before selection. See [Model detail props](model-detail-props.md).

Explicit `prop_detail`/`detail_prop` models may set `lightingorigin` through existing `create_entity` or `set_entity_properties` calls. Create one visible `info_lighting` with `properties.targetname` equal to that exact case-sensitive name and a finite origin within ±16384 units. The shared static-prop resolver rejects missing, hidden-only or ambiguous helpers, including duplicates at the same point. An absent, empty or whitespace-only detail field keeps the rotated validated model hull midpoint. The resolved helper point uses native float precision and replaces only the baked sampling point; model position, angles, render leaf and stored local-up normal stay unchanged. `fullbright` validates references and coordinates without requiring the helper's point to occupy an open leaf; lit modes require an open visible receiver in the compiled world. Region-exterior helpers remain referenced but may fail that receiver check, preserving prior output; enlarge or disable the region. These edits use the existing human **Advanced properties → Property key / Value** fields and shared Session operations, with no new tool or setting.

### Explicit detail sprites

Use existing `create_entity` and `set_entity_properties` calls with `classname: "prop_detail_sprite"`, an `origin`, and required string properties `position_ul`, `position_lr`, `tex_ul`, `tex_size` and `tex_total_size`. For example, `"-20 45"`, `"20 0"`, `"0 0"`, `"128 128"` and `"512"` respectively make a 40-by-45-unit card from the first logical 128-by-128 atlas rectangle. `position_ul`/`position_lr` are local world-unit corners; `tex_ul`/`tex_size` are logical atlas coordinates and `tex_total_size` supplies the positive scalar atlas size with a half-texel inset. The rectangle must fit that atlas and span at least one logical texel per axis. All fields require finite values.

`detailOrientation` accepts integers 0 (default fixed), 1 (full billboard) or 2 (vertical billboard). `angles` uses the existing Source pitch/yaw/roll path. Optional `scale` defaults to 1 and must remain positive and finite as a native float. `sprite_shape` may be `cross` or `tri`, both requiring orientation 0; tri `shape_angle` is an integer 0–255 and `shape_size` is finite 0–1, defaulting to zero. Cross parameters must be omitted or zero. Plain sprites accept all three orientations. `sway` is finite 0–1 (default zero), stored through SDK float conversion and truncated byte encoding; 0.5 becomes 127. Shape size uses the same byte quantization. Sprites share world `detailmaterial` and the existing checked atlas. No model asset is needed. Origins and complete card bounds must fit ±16384 units; origins must resolve to open visible leaves. Lit sampling also needs an open visible receiver. Named lighting-origin overrides, I/O, parenting, scripts, collision, nonzero `spriterandomscale`, nondefault `modelscale` and nondefault rendering/fading are unsupported. Orphan nonzero shape parameters reject. Fixed/full sprites, cross/tri shapes and vertical sprites with encoded nonzero sway or styles require a native half-path scale from 2⁻²⁴ to 65504; unstyled vertical sprites with encoded sway zero keep the positive-float fast path. Native intermediates and complete rest bounds are checked before publication. Runtime motion still requires a nonzero client `cl_detail_max_sway`; animated bounds are not generated. Compile regions select visible sprites by inclusive authored origin, defaulting to world zero when omitted. Outside and hidden sprites are omitted before card/atlas validation; visible malformed or out-of-range origins still reject. Included cards keep their geometry and pass normal bounds, open-leaf and lighting checks, even when they cross the box. A selected boundary point is never moved inward and can fail the normal open-leaf check.

The shared compiler used by `compile_map`, `start_compile` and `run_map` removes these source entities from runtime output, adds native detail records and reports the existing `detail_props` and `detail_prop_sprites` statistics. Bounds, instance/dictionary/asset budgets, lighting modes and style extras are shared with generated sprites. The human workflow uses the custom-class field in **Entity…**, **Place entity** and **Advanced properties → Property key / Value**, with no new tool or setting. See [Explicit detail sprites](detail-sprites.md).

### Baked detail lighting tint

Use existing `create_entity` or `set_entity_properties` with
`properties._detail_lighting_tint` / `set._detail_lighting_tint`, for example
`"255 128 64"`, on explicit `prop_detail`, `detail_prop` and `prop_detail_sprite`
sources, including fixed cross/tri shapes. The human inspector uses **Advanced
properties → Property key / Value** for the same key, described as **Baked
lighting tint · linear RGB**. Exactly three integer channels from 0 to 255 are
required. Omission and `"255 255 255"` preserve existing BSP bytes; remove the
property to restore identity. `rendercolor` remains unsupported.

Channels attenuate linear baked lighting by `channel/255`, without an sRGB
transfer curve. They are not target screen colors. Each channel's completed base
and every style contribution are multiplied once before native encoding;
SDK half-strength extras, untinted style membership and the shared LDR/HDR style
layout remain intact. Black tint therefore retains style slots, resource charges
and the sprite half-precision path. Dual builds store the tinted final HDR shared
base with separate tinted LDR/HDR extras. `fullbright` stores the authored tint
from white without styles. The client still applies its normal gamma conversion;
dynamic light added afterward and the game's `mat_fullbright` override remain
outside this baked modifier. There is no runtime tint/opacity control or per-entry
dictionary tint. Existing bounds, asset, leaf, lighting-origin, region,
atomic-publication and source/history rules remain in force.

For generated details, author world `_generated_detail_lighting_tint` through
`begin_property_edit {"world":true}`, then `update_property_edit` with its token
and `changes:[{"action":"set","key":"_generated_detail_lighting_tint","value":"255 128 64"}]`,
then `commit_property_edit {"token":...}`. Use `action:"remove"` to delete the
override. The human path is **World settings… → Custom property key / Value →
Stage value → Apply properties**; the stored field is labeled **Generated detail
lighting tint · linear RGB** and stays a text input for the exact three integer
0–255 factors. Each channel multiplies linear baked lighting by `channel/255`;
absent or white preserves BSP bytes. The map-wide value applies once to every
generated dictionary model, plain card and cross/tri shape on flat surfaces or
terrain, independently of explicit entity tint. It retains the same base/style,
black-membership, half-precision, HDR and dynamic-light/`mat_fullbright` rules
above. Invalid or repeated world values reject even when no details are emitted.
The compiler consumes this key from runtime worldspawn. It does not recolor the
viewport, rewrite dictionaries or add per-entry tint. See [Material detail
lighting](material-detail-scattering.md).

### Material detail scattering

For [material-driven model and sprite scattering](material-detail-scattering.md), assign a mounted VMT declaring `%detailtype` through `edit_faces` or the material tools, and set world `detailvbsp` through the shared property draft (**World settings… → Detail.vbsp file**). Compilation samples exposed world/detail polygons and valid convex planar terrain quads, including skewed bases, with deterministic samples on clipped flat regions and wholly retained terrain. Skewed bases use projected final native base-triangle areas to distribute samples; final terrain LOD triangles supply displaced positions, geometric normals and barycentric alpha; adjacent alpha groups are selected probabilistically. Flat faces use the highest group. Selected empty groups emit nothing without substitution; unsupported unused groups do not reject a different selection. Generated and explicit records share statistics, bounds and resource caps.

Plain sprite dictionary entries use `sprite` for logical atlas coordinates and `spritesize` for anchors and world dimensions. `detailOrientation` accepts exact integers: `0` (default) keeps stored angles, `1` faces the camera including its height, and `2` faces the camera while remaining vertical. All three accept `spriterandomscale` from 0–1 as Gaussian standard deviation around one followed by absolute value; omitted or zero deviation keeps scale one. World `detailmaterial` (**Detail material file**) selects the atlas, defaulting to `detail/detailsprites`. Custom atlases require checked `UnlitGeneric` material flags and a static square texture; see the manual for exact asset limits.

Fixed shapes use `sprite_shape "cross"` or `sprite_shape "tri"` in the same sprite dictionary entry. Both require `detailOrientation 0` (the default) and accept the same Gaussian random scale. Cross ignores horizontal anchoring and requires `shape_angle` and `shape_size` omitted or zero. Tri accepts integer `shape_angle` 0–255 (default 0) and finite `shape_size` 0–1 (default 0); size is quantized to one byte, so 0.5 becomes 127/255. Bounds follow the raw scaled, rotated cards at rest. These shapes share the existing instance, sprite-dictionary, atlas and lighting limits and statistics.

Generated plain sprites at orientations 0/1/2 and cross/tri shapes at orientation 0 may use finite dictionary `sway` from 0–1, default 0. SDK encoding converts to float, multiplies by 255 and truncates to a byte: 0/0.5/1 become 0/127/255. Dictionary and explicit model details reject nonzero sway; explicit sprites accept it through their properties. The CS:S client global `cl_detail_max_sway` defaults to 0, so authored sway needs an explicit runtime opt-in such as `cl_detail_max_sway 8` in the game console; set it back to 0 to disable motion. Runtime sway and player avoidance can move vertices outside the checked rest bounds. No new map property, compiler setting, MCP tool, animated bounds or collision is added.

Existing `set_compile_settings` lighting controls apply to explicit/generated models, plain sprites and fixed shapes. `lighting: "fullbright"` retains white unbaked base colors, or the respective explicit/world-generated baked tint, without style extras. `direct_ldr` bakes static direct light plus spherical ambient from filtered world/terrain lightmaps and sky, including configured diffuse bounces through those maps. `direct_ldr_hdr` leaves the HDR pass's final color in the native format's single shared detail base field; detail records do not hold separate LDR/HDR base colors. Placement and source/history stay unchanged by the lighting mode, and failed builds preserve the previous output.

Styles 1–63 contribute through separate native extras alongside style 0's base. Author named groups, appearances and initial state through the existing light properties (`targetname`, `style`, `pattern`, `spawnflags`) and control them through connections. Following the SDK, each extra encodes half its direct-plus-filtered-ambient color; the client applies the current style value without restoring a factor of two. Its final intensity can differ from an equivalent static-base light. Each receiver permits 63 extras. LDR stores its array in `dplt`; dual-channel builds add `dplh` with matching per-receiver sorted style unions and zero color for a style absent from one channel. Both arrays together permit 65,536 records, so dual builds allow at most 32,768 union entries per channel. Static builds without extras emit no new style lumps. Contribution counts appear in build diagnostics.

Invalid dictionaries, unsupported shapes or sway forms, folded/collapsed/nonplanar terrain bases, crossing terrain regions, runtime models, missing or vertex-lit assets, invalid style layouts and exceeded budgets reject before publication. Fixed/full billboards, cross/tri shapes and vertical billboards with styles or encoded nonzero sway require raw scale from 2⁻²⁴ to 65504 for the native half-precision path; positive half conversion truncates downward. Emitted float scales remain unchanged for SDK lighting, while raw scaled bounds conservatively cover the smaller static native geometry at rest. Unstyled vertical billboards with encoded sway zero retain their existing fast path and positive-float scale rules, including authored values that quantize to byte zero. Detail objects do not cast new baked shadows. Generated models, sprites and shapes keep their existing lighting points; the world tint applies map-wide, while per-entry dictionary tint remains unavailable. Use the existing `compile_map`, `start_compile` and `run_map` calls; no additional tool or setting is required.

### HDR build channels

Choose `set_compile_settings` with `lighting: "direct_ldr_hdr"` for **Direct LDR + HDR**. `fullbright` remains the default; `direct_ldr` retains the existing single-channel output. Sampling, bounces and smoothing apply to both lit modes through the same Session operations. Jobs capture the complete settings; active builds freeze changes, and differing later settings stale previous output. Custom recipes and compatible recent-map contexts retain the new mode; built-in **Fast preview** and **Final lighting** keep their original settings.

For point/spot/sun lights, HDR selects `_lightHDR`, then `_light`; eight-number values use the second color/brightness tuple for HDR and the first for LDR. `_lightscaleHDR` scales selected HDR light intensity. Sky ambient selects `_ambientHDR`, then `_ambient`, then half the selected/scaled sun; `_AmbientScaleHDR` applies afterward. Negative RGB sentinels take the fallback path; explicit zero remains zero. Missing scales default to 1. Malformed selected inputs, negative/nonfinite scales and nonrepresentable effective intensity fail before output publication. These properties use existing `create_entity` / `set_entity_properties` calls.

`compile_map` and `get_compile_status.job.result` expose the same `stats`:

| Scope | Statistics |
| --- | --- |
| LDR | Existing flat world-light, style, face/model, sample/byte/work, ambient, bounce and directional-map counters remain LDR. `lighting_work` includes shared preparation plus LDR work. |
| HDR | `hdr` is null outside the dual mode. After a successful dual build it mirrors those channel counters, including `world_lights`, `light_styles`, `styled_faces`, `light_style_maps`, `lighting_faces`, `lit_brush_models`, `model_lighting_faces`, `lightmap_samples`, `lighting_bytes`, `lighting_work`, `ambient_samples`, `ambient_bytes`, `bounces`, `bounce_rays`, `bounce_samples`, `bounce_work`, `model_bounce_faces`, `model_bounce_samples`, `bump_faces` and `bump_lightmaps`. HDR work is incremental. |
| Shared | `lighting_total_work` includes preparation and both passes. Authored geometry, prop-shadow preparation and smoothing counts remain counted once. |

`hdr.map_data_present` reports nonempty HDR light samples, world lights and ambient samples (native lumps 53/54/55). **Build details…** shows **HDR data ready** or **HDR unavailable: incomplete map data**. Legitimately empty output preserves a valid dual/LDR BSP and names the missing arrays in diagnostics; it receives no dummy emitters or probes. Input, solve, budget and cancellation failures preserve prior output. This flag describes map-data eligibility, not the game's selected channel. CS:S selects the baked LDR or HDR channel according to its HDR setting and hardware support; select that setting before a fresh map load.

HammeRR limits are 16 MiB of samples, 8,192 world lights and 65,536 probes per channel, 32 MiB total samples and one cancellable 100-million-work budget covering preparation and both passes. A shared 512 MiB live channel-allocation budget also applies; source assets retain their separate reader limits. Authored HDR sky families retain the native material path; `pack_sky_assets` admits and embeds supported complete material/VTF dependencies. Native sky shader/exposure parity and baked prop lightmaps remain unavailable; explicit editor HDR sky preview uses the separate preview policy below. Paired cubemap capture uses **Generate reflections** and the guarded workflow above. See [Compile and play](compile-and-play.md#hdr-channels).

### Baked lightmap inspection

`inspect_baked_lightmaps` reads this Session's last successful owned BSP without
starting graphics or a game. The human entry is **Build status → Inspect baked
lightmaps…**. Optional arguments are `channel:"ldr"|"hdr"` (default `ldr`),
zero-based compiled `face`, `style_index` (slot 0–3, default 0), `map_index`
(0 base or 1–3 directional maps, default 0), `offset` (default 0) and `limit`
(1–32, default 32). An omitted face selects the first valid lit face in the
requested channel. There is no arbitrary path argument.

Instead of `face`, pass `source:{"kind":"root","solid":123,"side":456}`,
`source:{"kind":"instance","instance":789,"file":"child.vmf","solid":12,"side":34}`,
or `selected_authored_face:true`. These selectors are mutually exclusive. Root
IDs and child-scoped origins are the exact retained compiler contributors;
source matches are paginated with `offset`/`limit`. `contributor_offset` pages
contributors of the inspected compiled face. Choosing inspection rows leaves
editor selection/camera and owned drafts unchanged. Instance filenames only
match retained origins and authorize no file lookup.

The JSON report contains build provenance/status, both channel availabilities,
paginated compiled-face summaries, selected dimensions/styles/sample byte ranges,
linear RGBExp32 minimum/mean/maximum, the stored base-map average and diagnostics.
An available selection also returns a PNG content block at most 128 pixels per
side. For display, each channel uses `(v/(1+v))^(1/2.2)` with
`v = linear intensity/255` and nearest sample reduction; statistics retain the
original linear intensities. This display does not establish native tone mapping,
exposure or actual game LDR/HDR channel selection.

Build status distinguishes `current`, `stale` and `unavailable`. Changed source,
map revision, settings or content make the retained build explicitly stale but
readable. Missing/replaced/unverifiable BSP bytes produce no image. Reads and
identity checks cap each source/BSP at 128 MiB. Missing lighting, Fullbright,
unsupported/corrupt face ranges and absent channels return diagnostics without
an invented image. Inspection preserves source/history/selection/camera; it adds
no viewport baked-lighting mode, prop relighting or arbitrary BSP browsing.
`mapping` reports retained contributors, source matches and their page totals.
It preserves many-to-many relationships, explicit generated/unknown labels and
original child solid/side IDs scoped by root helper and registered file. The
mapping is capped at 65,536 faces, 262,144 references and 8 MiB; explicit mapping
failure preserves ordinary compiled-face inspection. Geometry fingerprints reject
mismatched channel/face layouts. Explicit inspection checks exact root/output and
instance-child bytes. `build.render_context` separately marks displayed drafts as
not baked; stale numeric IDs cannot become current merely by existing again.
See [Baked lightmaps](baked-lightmaps.md).

### Water material binding

Use `create_block`, `select_faces` and `edit_faces` to author horizontal structural-world water whose horizontal top and exposed vertical water sides use VMTs declaring their own `$bottommaterial`. Existing `compile_map`, `start_compile` and `run_map` calls resolve material patches and bind each resolved bottom material to the corresponding clipped inward faces. Each generated inward face keeps its authored face's texture/lightmap vectors and surface flags, belongs to submerged leaves and receives no overlays attached to its authored face. The top material remains the water-fog reference; connected lowest-depth metadata and fluid physics retain their existing behavior. `$bottommaterial` names the surface viewed from below, rather than the pool-floor material; `$underwateroverlay` remains a runtime setting of the top material.

A missing `$bottommaterial` leaves a one-sided surface with a warning. An explicitly named missing or malformed target fails without replacing prior output. No new tool or setting is required. Depth-specific material packing, sloped/detail/model liquids, currents and broader native rendering validation remain unfinished. See [Water](water.md).

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

- `preview_terrain_subdivision` takes optional `faces` (defaults to selected faces), with 1–16 quad cages. Matching full edges and either or both whole-to-half neighbors share a conforming seed cage with one or two compatible grid densities per connected component; mixed powers and start corners are supported. It generates rounded geometry from base faces using four Catmull–Clark rounds and fixed base boundary edges, sampled at each existing power. Existing sculpting is replaced; alpha, IDs, start corners, materials, projections, flags and tags remain. Results report `faces_changed`, `shared_edges`, `vertices` and the owned `state.preview_token`.
- Inspect `get_terrain_grids` with `preview: true`, then `commit_face_preview` or `cancel_face_preview`. Apply is one Undo step; repeated generation from the same base cage changes nothing. `get_terrain_grids` now reports each face's saved `subdivided` marker.
- Supported generated/imported marked grids remain editable through existing sculpt, stroke, vertex-move, blend, sew and resolution operations. Actual opaque blocks and unsupported geometry caches retain their guards. Inconsistent density cycles, nested half-edge chains requiring a third density, ambiguous/fractional contacts, corner-only/non-manifold or inconsistently oriented cage connections reject before replacing a valid preview. Existing full-edge-only inputs retain their output.
- UI: **Edit terrain… → Operation → Subdivide base cage → Preview → Apply / Cancel**. Compilation uses the saved explicit rows and its existing mixed-resolution LOD rules; there is no extra compiler setting.

### Source model previews

Literal FGD model guides use the separate `helpers.model_guide` report and keep
ordinary helper-box spatial behavior.

- `inspect_model` takes a `path` under `models/` and optional zero-based `skin` and native `body` number. It reads MDL44/48, VVD4 and DX90 VTX7 LOD0 data on the CPU, returning geometry counts, selected-body material paths, skin count, local/hull bounds and resource limits. `pose` is `static` or `reference`; `reference_bones` counts decoded pose bones, and `static_prop` reports asset eligibility. It preserves map/history and does not initialize graphics.
- `inspect_entity_model` takes optional `id` (defaults to the selected entity). It reports the shared asset, authored skin, transformed bounds, displayed/hidden state and helper fallback reason from the rendered draft. Static meshes, checked reference poses and selected bodygroups at LOD 0 are supported, with positive uniform entity scale for eligible classes; see [Scale a dynamic model](models.md#scale-a-dynamic-model). Unsupported/missing/malformed assets remain helpers and inspection errors explain unavailable data.
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

`get_entity_inspector` exposes dynamic-model `rendercolor` as **Tint**. Use
`set_entity_color` with `id`, `key:"rendercolor"` and byte `color:[r,g,b]`;
omit `intensity`. RGB edits preserve an authored fourth component and commit
one Undo step. That fourth component does not set opacity.

#### Texture opacity

Use existing `set_entity_properties` for `rendermode`, `renderamt` and `renderfx`:

```json
{"id":"entity:12","set":{"rendermode":"2","renderamt":"128","renderfx":"0"}}
```

The inspector's **Render mode** choices are **Normal** (`0`) and **Texture ·
opacity** (`2`); **Opacity** is an integer slider from 0 to 255. In Textured views,
Texture multiplies material alpha by `renderamt/255`, even for opaque VMTs.
Normal keeps ordinary material rendering and ignores the amount. Render FX must
be None (`0`); missing mode/amount/FX defaults are 0/255/0. Texture batches depth-test
without depth writes and sort back-to-front by each transformed batch center
under the actual render camera. Zero opacity keeps ordinary bounds, triangle
picking, selection and outlines. Flat/diagnostic/wireframe views keep their
ordinary presentation; asset-only browser images keep original materials.

`inspect_entity_model.appearance` returns `supported`, `state`
(`normal`, `texture` or `unsupported`), `render_mode`, `render_amount`, effective
`opacity`, raw authored mode/amount/FX, `error` and limits. `opacity_preview`
identifies an instance using the Texture policy; `tint` retains RGB inspection.
Unsupported mode/FX or malformed appearance values keep a diagnostic helper.
Static props require white tint and Normal mode with no FX; unsupported values
also reject compilation without replacing prior output. This editor preview
does not evaluate native game appearance, animation, runtime fades, material
proxies, intersecting/cyclic transparency or other modes.

#### Uniform model scale

`set_entity_properties` writes the string `modelscale`, matching **Model scale**.
Absent means 1. Generic eligible MDL point entities share positive uniform
origin-relative scaling, triangle picks, bounds, framing, region/volume tests and
reference-pose attachments. `inspect_entity_model.model_scale` reports the raw
value, effective native-float value, admission/applied status, errors and limits.
Ordinary attachment axes scale; world-aligned attachments retain their axes and
scale their position. Browser asset previews and cached meshes stay at unit scale.

The editor accepts native-float values from 1/64 through 64, with at most 64 UTF-8
bytes and transformed mesh bounds within ±65536. Unsupported imported values
remain visible with a helper diagnostic, never clamped. Static props, native
detail records and literal FGD guides remain unit-only. Dynamic BSP entities retain
the raw property; native class behavior, collision scaling, animation and timed
scale inputs are not established by the editor preview. See [Models](models.md#scale-a-dynamic-model).

### Surface placement

1. Call `preview_surface_placement` with optional `ids` (selection by default), `method` (`floor`, `ceiling`, `ray` or `brush_corner`), `align`, `offset` and `texture_lock`. Floor is the default; ray and corner placement require `ray_origin` and `ray_direction`. It creates the same preview as **Place on surface…** without changing the source or history.
2. To share interactive picking, call `begin_surface_placement_pick` with `ids`, optional `method` (`ray`, the default, or `brush_corner`), placement options and the corner identity when required. It returns an owned `token`. Call `pick_surface_placement` with that token, `origin` and `direction`, or let a person click a viewport surface. Misses and unsupported hits preserve the pending pick.
3. `get_surface_placement` reports the active or stale token, `valid`, `picking`, `method`, selected IDs, options and any error. Its `plan` includes method, target ID/solid/side, hit point, geometric normal, pivot, translation, rotation, support-point count and achieved contact offset. An optional token checks ownership.
4. Inspect with `render_view` or frame with `frame_selection`. Call `apply_surface_placement` with the token for one Undo step, or `cancel_surface_placement` to discard only that placement. No-op placements create no Undo entry. Foreign tokens fail without clearing replacements.

Source, scene or selection changes invalidate the draft. Other geometry previews must be finished first. Opening the human window resumes a valid shared placement; replaced tokens require reopening for review. The whole selection moves rigidly, with actual brush/terrain/model vertices and helper-bound fallbacks. This operation does not simulate physics or resolve other contacts. See [Surface placement](placement.md) for target, alignment and resource limits.

For `method: "brush_corner"`, call `get_surface_placement_sources` with optional
whole-root `ids` (selection by default) for the human chooser's exact same
committed visible choices. It returns canonical `ids`, `sources` with owner ID,
numeric `solid`, complete incident `sides` and unrounded world `position`,
`provenance: "committed_geometry"` and `limits.source_brushes: 256` /
`limits.source_corners: 4096`. Invalid roots or exceeded limits return an error
without a partial list or changes to selection, map, history or drafts. The
report stays committed while a preview is displayed. Pass one returned
`source_corner: {solid, sides}`; `get_brush_components` can also inspect a vertex's
side-ID set, with no raw position field. `ids` identifies all moving whole world
brushes/entities; one selected brush owner must contain that source corner.
Ordinary point/model entities can accompany the brush owners. Every moving root
receives one identical offset, and all of them and their child brushes/models
are excluded as targets. The source vertex meets the nearest real corner of the first
visible flat brush face, prepared terrain triangle or supported model mesh triangle
hit by the ray. Terrain
targets use only that triangle's three vertices, including displayed sewn
mixed-power seam positions. Equal-distance ties use incident-side identity for
brushes, row-major vertex index for terrain and global decoded vertex index
for models. No grid rounding, rotation or surface offset is applied; `align: true`
or nonzero `offset` rejects. Other methods do not accept `source_corner`.

Read captured `source_corner` / `source_position` even while picking, then
`plan.corner.source`, `plan.corner.target`, `plan.corner.position`
and `plan.translation` before Apply. Brush targets retain `solid`/`sides` and
add `kind: "brush_corner"`. Terrain targets have `kind: "terrain_vertex"`,
`solid`, `side`, `triangle`, `index`, `grid_size`, zero-based `row`/`column`, and
`provenance: "prepared_surface"`; the exact world point is
`plan.corner.position`. No brush corner identity is fabricated for terrain.
Authored terrain rows stay unchanged. Model targets have `kind: "model_vertex"`,
`entity` (`entity:<id>`), cached canonical `model` path and `checksum`, `body`,
`skin`, uniform `scale`, `origin`, XYZW quaternion `rotation`, global `part`,
zero-based part `triangle`, global decoded `vertex`, and
`provenance: "cached_decoded_model_mesh"`. The world point is
`plan.corner.position`; it is one of the first hit triangle's three transformed
vertices. Blank/unsupported body choices supply no model target. Model reload
invalidates captured plans; changed disk bytes are not silently adopted by the
cache. Brush/terrain fields are absent for model targets. `plan.contact_offset` is null for corner
snapping. Source and flat target brushes require checked 4–64-side topology; every translated
child, including hidden brushes, must remain within ±65,536 units. Hidden/tool/
working-region exclusions apply. Individual owned child brushes and parent/child
mixtures reject; select the whole brush entity. Point/model entities and
displacement brushes cannot supply source corners. Companion point/model bounds
and finite authored origins must fit the same world limit. An unsupported first hit is
not bypassed. Invalid requests retain a valid plan. Existing Apply/Cancel and
Undo rules apply; an already coincident snap creates no history. Human controls:
**Place on surface… → Snap selected corner → Source corner → Pick target corner
→ Apply placement**. The source list shows object labels and coordinates, with
IDs as detail, and requires an explicit choice unless one was already selected
or captured by the current draft. Lists are bounded to 256 source brushes and
4096 corners; an explicit MCP corner also supports larger moving selections
and can be resumed in the dialog.

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

### Sky asset inspection

`inspect_sky_assets {}` calls the same read-only Session operation as **World settings… → Inspect applied sky…**. It accepts no overrides or other arguments. `authored_name` identifies the applied world `skyname`; a staged property draft is left untouched and is not inspected. Valid names remain exact; overlong invalid values set `authored_name_truncated`, and `report_truncated` marks bounded text or omitted diagnostics. No name repair, `_hdr` suffix substitution, file writes, asset reload, map/history/camera/clipboard/draft mutation, compile or game launch occurs.

The report returns `readiness_scope`, independent `ldr_ready` / `hdr_ready`, six `faces` for a valid family, top-level `issues`, `assets_read`, `bytes_read` and `limits`. Face order is `rt`, `bk`, `lf`, `ft`, `up`, `dn`. Each face contains `suffix`, exact `material`, winning `source`, `material_sources` for the VMT/Patch dependency chain, resolved `shader` / `shader_truncated`, channel reports `ldr` / `hdr`, and `issues`. A channel contains `ready`, `textures` and `issues`; each texture contains its authored `parameter` and `reference` / `reference_truncated`, resolved `path`, `source`, parsed `size` / `format`, `bytes`, `ready` and `issues`. Unavailable metadata stays null. Directory sources use `{kind:"directory",mount,file}`; VPK sources use `{kind:"vpk",index,files}`. Material-source entries contain `path`, `source` and `bytes`.

Readiness describes supported mounted declarations and complete VTF layout, without establishing editor preview support, HDR pixel radiance, shader rendering or the game's active channel. Recognized shaders are `Sky` and `UnlitGeneric`; HDR additionally requires `Sky` and `$hdrbasetexture` in `Rgba16161616F` or `Rgba16161616`. Compressed HDR parameters, unsupported material effects/parameters, missing or malformed winning assets and unsupported animated/cubemap/volume textures remain explicit diagnostics. A declared texture transform can pass structural inspection while remaining unsupported by the preview. Channels remain independent when only one texture fails.

`limits` exposes `asset_lookups:128`, `total_bytes:67108864`, `material_bytes:1048576`, `texture_axis:4096`, `path_bytes:512`, `issue_characters:1024`, `issues_per_scope:16`, `shader_characters:128` and `hdr_base_formats`. Counts cover distinct successfully read assets and their bytes; repeated references share the inspection's read budget. Family names must be relative, at most 256 bytes and without surrounding whitespace or control characters. Inspection reads current mounted files on each call; it does not remount content, pack custom sky assets or add compiler admission checks. See [Inspect mounted sky assets](sky.md#inspect-mounted-sky-assets).

### 2D sky preview

`set_view` accepts `sky_preview:true`, optional `sky_channel:"ldr"|"hdr"` and
`sky_exposure_ev`. Defaults remain preview off, LDR and 0 EV. Exposure must be
finite within −8…8 and affects only HDR sky pixels. Omitted fields retain shared
values; invalid settings reject atomically. `render_view` accepts the same
temporary overrides without changing shared view, camera, source or history.
These controls use the same Session operations as
**Sky channel → HDR · editor preview** and **HDR sky exposure (EV)** in the editor.
Preview requires perspective Textured
mode, `tool_textures:true` and six supported faces for the requested channel.

`get_map_info.view` and render metadata's `view` include both settings.
`get_map_info.skybox` and render metadata's `skybox` report `name`,
`name_truncated`, `ready`, `summary`, `requested_channel`, configured
`exposure_ev`, `tonemap`, `color_policy`,
`eligible` and `suppression_reason`. `ready` describes all six assets; `eligible`
adds view/camera/filter eligibility and does not prove visible sky pixels.
Suppression is `disabled`, `unsupported_view`, `tool_textures_hidden` or
`unavailable`, otherwise null. Faces report `suffix`, `material`, `issue`, actual
selected-channel `size` and `format`; HDR also reports the canonical VTF
`texture` path, material `source` and `texture_source` with directory/VPK
provenance. Empty `skyname` returns `name:null` and `ready:false`.
`name_truncated:true` marks an overlong HDR family's bounded display prefix;
clipped text is never used for asset lookup or cache identity.

LDR retains its existing sRGB sampling and target encoding; exposure is inactive
and `tonemap` is `none`. Explicit HDR requires six direct static `Sky` materials
with `$hdrbasetexture` in VTF format 24 (`Rgba16161616F`) or 25
(`Rgba16161616`), one frame, face and depth layer. Complete bounded VMT syntax,
unique supported scalar parameters and a supported axis-aligned texture
transform are required. Patch, compressed HDR, proxies, unknown/nested
parameters and unavailable faces reject the whole HDR family, retaining tool
surfaces without LDR or another-family substitution.

HDR half-float RGB is linear, finite and nonnegative; every half component in
every mip must be finite. UNORM RGB becomes `channel / 65535 × 16`. Texture
alpha is ignored and sky output is opaque. The renderer applies `2^EV`,
per-channel Reinhard `x/(1+x)`, then one sRGB target encoding;
`tonemap:"reinhard_per_channel"` and `color_policy` state this editor policy.
It applies to sky only, without changing foreground surfaces, native hardware
mode, build lighting, fog, material tint or game exposure adaptation.

HDR source budgets are 128 lookups, 64 MiB aggregate, 1 MiB per VMT, 16 MiB per
VTF and 4096 pixels per axis. Family names cap at 256 bytes and dependency
references at 512 bytes; surrounding whitespace, traversal and control
characters reject. Overlong families, duplicate world `skyname` keys and
unavailable game content return before reads or cache writes, preserving the
previous valid family cache.
Unique textures are shared within one cached HDR
family. Encoded data and GPU texture bytes each cap at 64 MiB per set;
replacement can briefly retain old and new GPU sets. Temporary CPU source/parse
copies cap at 128 MiB plus one 16 MiB conversion buffer. Successful and failed
HDR loads are retained for the cached family; `reload_material_assets` clears
them. LDR retains its 4096-axis and 64-MiB-per-texture limits.
Axis-aligned scale/translation and unequal face sizes work in both channels.
HDR metadata's `limits` object exposes `asset_lookups`, `total_bytes`,
`material_bytes`, `texture_bytes`, `texture_axis`, `cached_families`,
`gpu_bytes_per_set` and the `exposure_ev` range; it is absent for LDR.

Preview controls leave VMF and Undo unchanged; author `skyname` through
`begin_property_edit` with `world:true`, then `update_property_edit` with
`changes:[{"action":"set","key":"skyname","value":"sky_dust_hdr"}]` and
`commit_property_edit` using its token. This is the same **World settings…**
edit. HDR resolves that exact authored family without appending `_hdr`.
LDR readiness alone does not validate a family's HDR assets or select the
game's channel. Compilation preserves the name; explicit `pack_sky_assets`
embeds supported original dependencies independently of editor preview settings.
See [Compiled HDR skies](sky.md#compiled-hdr-skies).

Sky surfaces retain depth, picking and visibility filtering. Planar, flat,
wireframe and diagnostic views retain their usual behavior. 3D sky-camera
transforms, sky fog, animation and native HDR rendering parity remain outside
this preview; see [2D sky preview](sky.md).

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

`get_recent_maps` returns up to 20 entries in most-recent-first order with exact `path`, file availability, retained game directory, eight-field `build_settings` (including `generate_reflections` and `pack_sky_assets`) and `context_compatible`. Missing legacy booleans default to false. It also reports persistence, storage errors and bounds. Pass a listed `path` to `open_recent_map`, optionally with `discard_changes: true` when discarding the current edits is intended. It uses normal map validation and restores retained build values only while idle and with matching mounted game content. Game content and custom FGD files do not switch automatically.

`remove_recent_map {path}` and `clear_recent_maps {}` change only the retained list. Successful opens/saves update recency; changing build settings updates an existing entry without reordering it. Missing files remain listed. Failed primary opens/saves preserve the list; an auxiliary history-write failure is reported without failing an otherwise successful map operation.

Preferences use `recent-maps.json` in recovery storage. `HAMMERR_RECENT_MAPS_DIR` selects an absolute directory; otherwise `HAMMERR_RECOVERY_DIR` also isolates this list. Paths are bounded to 4096 UTF-8 bytes and storage to 192 KiB. Exact-source checks reject stale list publication. `reload_recent_maps {}` reloads a valid saved list through the same operation as **Reload saved list**, clears its error and preserves the document, current build settings, history, selection, camera and owned drafts. Invalid Reload retains the last valid local list and blocks writes until repaired. Participating Linux writers coordinate through a stable advisory lock; contention is retryable. Older/manual writers that ignore the lock remain outside that guarantee. Post-publication verification failure reports that the list was written, adopts it locally and requires Reload. See [Recent maps](recent-maps.md).

### Build recipes

`get_build_recipes` returns built-in and custom recipes with all eight settings and readable descriptions, per-document action `revision`, current `active` identity, editability, persistence, path and errors. Built-in IDs `fast` and `final` are immutable. Fast preview uses Fullbright, no physics, Fast visibility/sampling, no bounces and 0° smoothing. Final lighting uses Direct LDR, engine physics, Detailed visibility/sampling, four bounces and 45° smoothing, retaining the compiler's existing requirements and limits. Both disable sky packing and reflection generation. Custom recipes can retain sky packing in every lighting mode and reflection generation in either lit mode. Missing legacy booleans default to false; false packing fields remain omitted from stored version-1 records for byte compatibility.

| Tool | Arguments and effect |
| --- | --- |
| `save_build_recipe` | `name`, optional `replace_id` and `revision`; captures all current settings as a new custom recipe or replaces the chosen custom recipe. |
| `rename_build_recipe` | `id`, `name`, optional `revision`; changes only a custom recipe's name. |
| `delete_build_recipe` | `id`, optional `revision`; removes a custom recipe while retaining current compiler values. |
| `apply_build_recipe` | `id`, optional `revision`; applies all settings through the shared settings operation without starting a build. |
| `reload_build_recipes` | Optional `revision`; adopts a complete valid saved registry without applying compiler settings. |

Pass the returned per-document action `revision` to reject stale edits. It covers both the shared saved list and local compiler options/applied identity, including changes back to an earlier value. Refresh the list and current settings before retrying. Attached Sessions share saved recipes, storage errors and Reload; current options and applied IDs remain local. Active or cancelling jobs block recipe mutation, Apply and Reload in the originating document; peers may edit the saved list. `get_compile_settings.recipe` identifies the same applied ID only while its saved values still match current options: a peer rename updates the label, deletion or changed values clear it. `get_compile_status.job.recipe` keeps the identity captured when that build started, including after peer rename/deletion. Names alone do not stale output. These operations leave VMF and map Undo unchanged.

Custom recipes use `build-recipes.json` in recovery storage; `HAMMERR_BUILD_RECIPES` overrides the file and `HAMMERR_RECOVERY_DIR` isolates the default. Limits are 32 customs, unique names ignoring case, 1–80 UTF-8 bytes per trimmed name without control characters and a 65,536-byte file. Invalid loads block custom edits while built-ins remain usable. Failed writes preserve preferences and can retry; detected disk changes require `reload_build_recipes`. Reload shares **Reload recipes** in **Build settings…** or **Build details…**. A failed configured-path Reload retains the valid list and active identity, reports the error and blocks custom edits until repaired storage reloads. Successful Reload retains the same applied recipe ID only when its loaded settings still match current options; rename updates the label, replacement/deletion clears it. Neither outcome applies options or changes map/history/camera/owned drafts or captured jobs. Unconfigured session-only storage rejects Reload without blocking local edits. Edits to an attached application registry are serialized; separate processes still have no shared write lock. Loading recipes does not apply a custom recipe automatically. See [Build recipes](build-recipes.md).

### Prop shadows and occluders

Existing `create_entity` and `set_entity_properties` calls author static-prop
transforms; `compile_map`, `start_compile` and `run_map` share the compiler path.
Results expose `static_props`, `static_prop_models` and `static_prop_leaf_refs`,
shown as **Static props**, **Model assets** and **Leaf references**. Rotated props
clip the transformed union of render/hull header bounds through the world BSP;
unrotated membership stays unchanged. Header boxes can retain extra leaves and
do not provide exact mesh/hull placement. Traversal and clipping share 16 million
work units and four million cumulative reserved polygon points per build. Invalid
assets/trees, exceeded limits, cancellation or no open leaf preserve prior output.
No new setting or tool is required. See [Static props](static-props.md).

Existing synchronous and background compile results expose `prop_shadow_props`,
`prop_shadow_models`, `prop_shadow_triangles`, `prop_shadow_disabled` and
`prop_shadow_work`, also shown in **Build details…**. Both lit modes share LOD0
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

Create `func_occluder` through the existing brush-entity operations, set `StartActive`, and author its Activate/Deactivate/Toggle connections through the existing entity tools. Compiles report `occluders`, `occluder_polygons` and `occluder_vertex_refs` with the same entity-linked diagnostics as the human build panel. Each helper supports 1–16 visible convex brushes in one unambiguous visibility area. Separated brushes, exact face contacts and overlapping volumes produce a bounded union boundary; gaps and concave outlines remain. Edge-only/point-only contacts, thin or ambiguous contacts and collapsed fragments reject with diagnostics. Combined inputs are bounded to 256 sides and 4096 polygon vertices; boundary output and total union work have separate limits. Native records, activation indices and Activate/Deactivate/Toggle control runtime culling. See [Occluders](occluders.md).

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
reports the storage folder, limits, `registry_revision` and a `snapshot` binding
the shared catalog to the current map/selection/clipboard state. Attached Sessions
share catalog changes and storage errors; selection reports, thumbnail resources
and Paste reviews stay local. Unchanged inspections keep the review valid.

| Tool | Arguments and behavior |
| --- | --- |
| `inspect_prefab_thumbnail` | Exact `name`, optional `style` (`flat` or `textured`, default Flat); detached CPU geometry/material/count/bounds diagnostics, cache state and limits. Initializes no graphics. |
| `render_prefab_thumbnail` | Exact `name`, optional `style` and `refresh` (default false); returns the same 320×240 PNG and report as choosing **Flat** or **Textured**, then **Render thumbnail** or **Refresh thumbnail**. |
| `render_prefab_gallery` | Required `snapshot` from `get_prefabs`; optional `query`, `offset`, `limit` (1–8, default 8) and `style` (`flat` or `textured`, default Flat). Renders the same reviewed page as **Render page**. The first text block lists ordered entries; each successful entry's `image_content_index` identifies its PNG in the result's content array. Other entries report their own error without an image. |
| `save_prefab` | Latest `snapshot`, new `name` and optional whole-object `ids` (selection by default). Captures before atomically creating a new file. Never overwrites; map/history/clipboard stay unchanged. |
| `preview_prefab` | Latest `snapshot`, `name`, optional world `position` (defaults to zero), XYZ `rotation` in degrees, positive XYZ `scale`, source-world `pivot` (bounds center when omitted) and `keep_external_links` (false). Scale then rotation precede placement; nonidentity scale requires brushes only. Privately validates before changing clipboard/preview state. Returns `paste.preview.token` for `apply_paste` / `cancel_paste`. |
| `rename_prefab` | Latest `snapshot`, original `name` and `new_name`. Atomically renames without overwriting an existing entry. Retains VMF bytes, clipboard, selection and map history. |
| `delete_prefab` | Latest `snapshot` and exact `name`. Removes only its library VMF, separately from map Undo. Placed copies and loaded detached paste drafts remain usable. |

Placement retains clipboard name/reference/ID checks, native group/layer remapping,
terrain and object notes. Cut-name restoration is always disabled. Source files,
geometry and map history stay unchanged until Apply; failed loads/placements
retain the prior clipboard and draft. Assets remain external references.

Limits: 128 VMFs, 8 MiB each, 64 MiB total, 512 scanned directory entries and
1–80 UTF-8-byte names using letters/numbers/spaces/dashes/underscores. The usual
clipboard geometry/reference limits apply. `HAMMERR_PREFABS_DIR` overrides the
normal recovery/state folder's `prefabs` child. External writers are uncoordinated;
changed file metadata or Session state invalidates stale requests. Renamed or
replaced sources invalidate loaded placement reviews; deletion alone retains the
detached draft. Direct replacement, private formats and referenced instances are
unavailable. Rename requires Linux atomic no-replace support.
The review binds the library's physical directory, including an initially absent
folder's existing ancestor. Replacement or parent-alias retarget requires a fresh
inspection. File operations remain relative to the retained directory handle.
Successful mutations include `write_committed: true`. A nullable `write_error`
reports a committed write whose subsequent verification failed; inspect storage
before repeating it. This does not coordinate independent writers or turn the
metadata review into a digest of all VMF contents.

Flat remains the default material-free brush/terrain/model thumbnail, including
ordinary point helpers and authored hidden objects. Textured admits opaque
static single-base LightmappedGeneric/UnlitGeneric/VertexLitGeneric color images,
with fixed editor shading and identity base transforms. Shader effects, alpha,
normal maps, authored lighting and native appearance parity are outside this
mode. Unsupported sources return an error and no image; Flat is a separate
explicit choice.

The isolated scene/camera preserve map, selection, clipboard, owned drafts and
history. Both styles share an eight-image cap, keyed by source/style/content/FGD
state. Retrieval rechecks exact prefab bytes/identity. Textured also verifies
bounded winning VMT/Patch/VTF bytes, mounted origins and backing identities,
including after rendering and before UI publication. Refresh regenerates only
the selected style. Model geometry follows explicit Reload models semantics.

Geometry caps remain 2 MiB text, 128 brushes, 1024 sides, 64 sides per brush, 65536
terrain vertices, eight model paths, sixteen instances, 64 material bindings,
200000 vertices and 600000 indices. Textured adds 2048 pixels per axis, 1 MiB per
VMT/Patch, 16 MiB per VTF, 64 MiB read bytes per admission/recheck pass, 64 MiB
decoded mip bytes, 256 asset paths and 1024 physical paths. Its initial shared
source-guard capture can read at most 80 MiB. Larger assemblies can still use
placement. Inspection rejects unresolved instances, invalid geometry and missing
or unsupported model geometry with explicit errors.
See [Prefab library](prefabs.md).

Gallery rendering reuses the same renderer and eight-image cache. A changed
catalog rejects the whole page; source and texture dependencies are checked again
before returning PNGs. The operation preserves the map, clipboard, camera,
selection, placement review and history. It renders no other library pages.

### Overlay authoring

| Tool | Inputs and result |
| --- | --- |
| `get_overlay_state` | Current `snapshot`, eligible selected source or `selection_error`, eligible `selected_overlay` or `refit_error`, and draft operation/entity, owner/token/validity, options, corners, basis and bounds. |
| `browse_overlay_targets` | Search existing overlays by `query` (name/material/ID, at most 256 bytes), `offset` and `limit` (default 20, maximum 50). Returns snapshot, exact total and `entries` with numeric `id`, `entity` identity, name/material, source/settings, `eligible` and rejection `error`. Does not change selection. |
| `preview_overlay` | Latest `snapshot`, explicit `source:{solid,side}` and `material`; optional `targetname`, `render_order` (0–3), `u`/`v` endpoint pairs and `fade_min`/`fade_max` world distances. Fits a new owned draft without editing source/history/selection. |
| `preview_overlay_refit` | Latest `snapshot` and optional overlay `id` (defaults to selection); previews refitting its one existing visible world/detail quad binding. Keeps identity, raw settings and metadata. |
| `preview_overlay_corners` | Latest `snapshot`, optional overlay `id` (single selected overlay by default) and exactly four local world-unit `corners: [[u,v],…]` in perimeter order. Previews updating its existing single-face projection. |
| `preview_overlay_rebind` | Latest `snapshot`, existing overlay `id: "entity:<id>"` and a different destination `source:{solid,side}`. Both bindings must be eligible visible world/detail quads. Returns `operation: "rebind"`, same entity and `previous_source`; preserves raw settings and metadata. |
| `overlay_corner_handles` | Tagged `inspect`, `begin`, `pick`, `move`, `finish` or `cancel` action for an owned corner draft; captured viewport motion returns fresh tokens without map/history edits. |
| `review_overlay` | A valid draft `token`; transfers ownership and issues a fresh token without editing the map. |
| `apply_overlay` | Current owned `token`; one Undo step. `operation: "create"` returns `created`; `refit`, `rebind` and `corners` return `updated` for the same entity. |
| `cancel_overlay` | Matching owned `token`; discards only that draft, including a stale owned draft. |

Creation supports one visible convex planar world or `func_detail` quad,
including tilted trapezoids. Defaults are order `0`, U/V `0–1`, fade start `-1`
and end `0` (disabled). Preview fits the actual source corners; it does not accept
derived basis fields. Numeric corner editing is the explicit operation below.
The compiler and authoring share projection checks.
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

`get_overlay_state.selected_corners` exposes the eligible entity/source,
`options`, world `corners`, `local_corners`, `basis_origin`, `basis_u` and `basis_v`;
`corners_error` explains an unavailable selection. The draft additionally exposes
`basis_normal`.
`preview_overlay_corners` returns `operation: "corners"`; it retains the same
entity, single visible world/detail quad binding and raw basis/origin/settings,
connections and metadata. Apply changes only `uv0`–`uv3` local XY values in one
Undo step. Corners must be finite, noncollapsed, convex and perimeter ordered,
with complete world extents within ±16384 on the current projection plane,
including native float precision. They may extend past the bound face; native rendering clips to that surface. Map/selection/token
changes reject; invalid input preserves the previous valid draft. The shared
Review/Apply/Cancel ownership rules still apply. Terrain/model/multiside corner
editing and new basis authoring remain unavailable.

For viewport dragging, first own a valid corners draft using
`preview_overlay_corners` or `review_overlay`. Call
`overlay_corner_handles {"action":"inspect"}` for `handle_snapshot`, then
`action:"begin"` with the corner `token`, that value as `snapshot`,
`width`/`height` (1–8192) and optional `camera` matching `render_view`.
`draft.handles` reports captured dimensions, projected `pixels`, active
zero-based corner, validity/rejection and an 11-pixel pick radius.

`action:"pick"` and `"move"` require the current `token`, finite `x`/`y`
pixel positions and the captured `width`/`height`. These are pointer positions
without an implicit half-pixel offset. Pick chooses the nearest projected corner;
Move applies relative unsnapped motion on the fixed projection plane. Every
successful gesture action returns a new token. Misses, crossed/collapsed corners,
native-invalid values and stale map/view/camera/selection/viewport preserve the last
valid draft. After a successful Pick, `finish` takes its token and returns to
ordinary Apply/Cancel review. Apply rejects active handles and remains one Undo. `cancel` restores the exact
pre-handle draft, including owned stale cleanup. Foreign/replaced tokens reject.
Review ends handles and transfers the latest valid corner draft to its reviewer.

UI: **Overlay… → Edit selected corners → Preview corners → Viewport handles**,
then drag a cyan square and use **Apply corners**. **Cancel handles** restores
the pre-handle draft. Escape/focus cleanup affects only a current interactive
User-owned gesture; foreign/stale handles leave ordinary desktop input available.
Single visible world/`func_detail` quad, no snapping or projected material preview.
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
point origins and resolved model pivots determine entity inclusion. Explicit
`prop_detail`/`detail_prop` models and `prop_detail_sprite` cards use inclusive authored origins, defaulting to
zero when absent. Outside and hidden models are omitted; retained records pass
the normal asset, transform, Source-bounds, open-leaf and lighting checks without
clipping or moving them inward. Visible invalid origins reject before selection.
Outside and hidden sprites are omitted before per-card and atlas validation.
References remain unchanged. Fully contained structural world water retains the normal
native water/physics path. World/detail terrain needs its complete cage and grid
contained; wholly disjoint combined bounds are omitted. Areaportals/windows and
occluders use complete visible brush bounds rather than editor origins: contained
helpers remain whole, disjoint helpers are omitted and crossings reject.
Imported regular overlays and water transitions retain their complete definitions
regardless of marker origin; every target must resolve to an emitted
world/detail/terrain surface. Missing window render targets and ordinary helper
validation errors retain prior output.
Crossing terrain/water, detail/model water, slime and visclusters
reject before publication.
Runtime-model terrain remains unsupported by the normal compiler too.
Sun and water LOD controllers retain original origins/settings even outside
the region. Their editor positions do not seed region-only exterior leaks.
Visible `info_lighting` helpers remain compiler-only reference data regardless
of marker position. Retained static props and explicit detail models keep their
authored lighting-origin references; hidden-only or ambiguous matches fail
ordinary reference validation. Lit detail helpers must still lie in open visible
compiled space, so an exterior reference can fail without being moved inward.
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
mounted content. It reloads cached display materials and LDR sky faces, clears
the HDR family cache, rescans the material catalog, re-resolves cached model
material paths and refreshes
desktop/offscreen textures and thumbnails. Decoded model geometry stays shared;
preferred paths appearing or disappearing are resolved from the existing mounts.
The icon admission cache, including failures, is cleared for the next inspection
or render.
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

### Recovery settings and errors

Retry a failed snapshot with a new `create_recovery_snapshot` call. Each call
admits a fresh request; internal repeated or concurrent execution of that same
request cannot write another snapshot, including after a failed write.

Manual capture freezes committed content, its source identity and retention
settings before worker scheduling, through the same operation as **Save snapshot
now**. Edits or settings changes while it waits affect later requests. Dropping
a queued call skips capture; once capture starts, its write and retention finish
even if the caller disappears. Abandoned work retires only its own request and
cannot clear a newer capture or another Session's activity.

`get_recovery.status` includes `settings_revision` and independent
`settings_error`, `storage_error`, `snapshot_error`, `retention_error` and
`drain_error` fields. `storage_error` describes the originating document's
snapshot-folder configuration, independently of shared saved settings.
Its combined `error` retains unresolved causes for existing clients. Saving
settings clears only settings errors; successful capture clears snapshot and
retention errors. Terminal drain failures remain visible. Snapshot-list warnings
are reported separately and do not replace these causes.

Attached Sessions share validated settings, `settings_revision` and settings
errors. Optional fields in `set_recovery_settings` merge with the latest shared
values under one lock. Document snapshot stores, scheduling, pending changes,
capture errors and workers remain independent. An admitted request retains its
captured settings policy after a peer update. These operations do not provide
multiple-document routing.

`reload_recovery_settings {expected_revision?: u64}` reads the saved complete
settings through the same Session operation as **Recovery… → Reload settings**.
Pass the current `settings_revision` to reject a stale request before disk access.
A successful Reload adopts validated settings, advances that revision and clears
only settings errors. A failed Reload retains current settings and their
revision, reports its settings error and preserves snapshot/retention/drain
errors, pending requests, map/history/camera and owned reviews. Human inputs are
replaced only after success. Reload neither captures nor prunes snapshots.

`settings.json` is limited to **32 KiB** and must resolve to a regular file;
valid regular-file symlinks remain supported. An absent file loads factory
settings. Startup load errors disable autosave with a visible settings error;
manual snapshots and previous-file backups remain available. Explicit validated
Apply can repair the file. Settings publication retains its existing atomic
write behavior; independent writers have no shared lock or automatic merge.
See [Autosave and recovery](map-tools.md#autosave-and-recovery).

### Recovery shutdown

Normal desktop exit and stdin EOF in either headless executable explicitly drain
recovery. Pending committed changes are captured once when autosave is enabled;
the original VMF is not saved. A failed exit snapshot or worker drain returns a
nonzero process status and reports its cause on stderr. If the primary UI or
transport also failed, the final error retains both causes. Shared manual and
automatic recovery completion records capture failure and clears busy state,
including a caught capture-worker panic.

Shutdown also cancels instance Contents/Extraction work and FGD loading, and waits
for their owned result/staging cleanup before final recovery. Instance cleanup diagnostics remain
in their instance reports; they do not become recovery failures.

The owning desktop and stdio servers reject new document tool calls once
shutdown begins. Already admitted calls and their queued workers retain their
original document until cleanup completes. Tool discovery remains available;
there is no new document-target parameter or multiple-document API.

### Source and build destinations

`open_map`, `save_map`, `start_compile`, `compile_map` and `run_map` use the same
physical-file reservations as the desktop. Source maps, BSPs and derived `.lin`
files cannot overlap through path aliases or hard links, including cross-role
aliases owned by the same document. Rebuilding the same output role is allowed.
Conflicts preserve the previous source, outputs and build status.

Omitting `save_map.path` retains the source observation captured at Open or the
last Save. Supplying a path explicitly chooses a fresh Save As destination,
even when it spells the current filename. Use that explicit choice after
repairing or intentionally retargeting a stale source alias. Saving preserves
valid source symlinks and target permissions; ordinary compiler output replaces
the final directory entry. Guarded instance/reflection builds retain their
stricter output-symlink checks.

Save/compiler publication requires Linux. A successful build can include a
`warning` when the committed file's path could not be verified;
the write has already happened. Manual writers and separate processes are not
covered by the application's reservations. Launch checks the retained output
binding and metadata, without a new post-build byte-rehash guarantee.

### Coordinated material favorites

`set_material_favorite {material, favorite}` retains canonical names, a 512-entry
limit and version-1 storage. Exact-source writes and a stable Linux advisory lock
reject stale sessions before replacing the set. Source conflicts require
`reload_material_favorites {}`, which returns `{favorites: Status}` and shares
**Reload favorites**. Status includes `reload_available`.

Reload replaces only a valid complete set and preserves the document, camera,
selection, history, renderer caches and owned reviews. A dedicated favorites
revision refreshes browser badges, counts and status without staling viewport
picks. Failed Reload retains the last valid set; repair storage and Reload again.
Lock contention can retry. A written-but-unverified result adopts the intended
set locally, reports its error and blocks persistent changes until Reload.
Unconfigured storage keeps editable session-only favorites and disables Reload.
No implicit merge, watcher or coordination with nonparticipating writers is
provided. See [Material favorites](textures.md).
