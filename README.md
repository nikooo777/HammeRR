# HammeRR

**A modern, AI-first Source map editor for Linux.**

HammeRR is written in Rust for Counter-Strike: Source. It reads and writes
Hammer's VMF format, renders the game's own materials, and gives humans and
AI agents the same editing operations through a shared session and MCP.

The aim is a new editor with integrated tooling and a modern interface.
Development is active; see [current limitations](#status) before compiling
maps for play.

![Current editor showing an MCP-built courtyard and the light inspector](docs/images/editor.png)

*A courtyard built through MCP, with textured brushes, detailed light helpers,
and the shared entity inspector and activity panel.*

[Features](#features) · [Quick start](#quick-start) · [Editing](#editing) ·
[MCP](#mcp) · [Compile and play](#compile-and-play) · [Status](#status)

## Features

| Area | Available now |
| --- | --- |
| Maps and content | VMF editing and saving; byte-for-byte round trips for unmodified files; game materials from Steam libraries, search paths and VPK archives. |
| Viewport and brushes | Textured perspective and planar views, grids, fractional snapping, brush drawing, move/rotate/scale handles, exact transforms, and texture locking. |
| Saved views | Named perspective and planar cameras stored in the VMF, with restore, rename, replace, delete and undo. |
| Selection | Object and face marquee or 3D volume selection, intersecting or fully contained matches, and replace/add/remove/toggle rules. |
| Brush clipping | Planar cut gestures or exact plane inputs, keep either side or both, cap materials, cancellable previews and one-step undo. |
| Brush shapes | Parameterized wedges, cylinders, cones, spheres and arches, with material previews, editable dimensions and cancellable geometry previews. |
| Recovery | Periodic and manual snapshots, previous-save backups, safe recovery copies, persistent settings and matching MCP controls. |
| Map checks | Geometry, IDs, textures, entity logic, asset paths and spawn checks; issue navigation and reviewed, undoable repairs. |
| Design context | Map briefs, object notes and editor-only regions; shared drafts, search, relevant-context retrieval, bounded storage and undo. |
| Face materials | Individual face selection, sampled material/projection painting, edge wrapping, world/face alignment, UV controls, per-face/grouped fitting, view projection and placement, with cancellable previews and grouped material thumbnails. |
| Material replacement | Exact map/object/face searches, explicit hidden-face handling, matching-face selection, guarded previews and atomic replacement. |
| Face lighting | Batch lightmap scales and smoothing groups, mixed-value controls, cancellable previews, and density/group viewport diagnostics. |
| Entities | FGD class search, typed properties, choices, spawn flags, lights and player spawns, plus brush entity conversion. |
| Entity logic | Connection editing, target/input suggestions and firing rules; reference inspection and safe renaming with previews, Cancel and atomic undo. |
| Map management | World settings, mixed-value property batches and default resets; object reports, search, selection, framing and visibility restore. |
| Humans and agents | Shared map, selection, camera, previews and undo history. MCP edits appear live in the editor. |
| Build and play | Native BSP compilation, shared stage/status reporting and cancellation, leak paths, CS:S launch controls, and game logs. |

## Quick start

Requires **Rust 1.92+**. The graphical editor and rendered views need a
**Vulkan-compatible graphics backend**; headless editing and compilation can
run without graphics initialization. Install Counter-Strike:
Source to use its materials; HammeRR discovers the game through Steam.

```sh
cargo build --release -p editor -j 8
./target/release/editor path/to/map.vmf
```

Omit the filename to start an empty map. The editor also discovers the
installed CS:S SDK entity definitions.

<details>
<summary>Launch options</summary>

| Option | Purpose |
| --- | --- |
| `--game <gameinfo.txt>` | Use a specific game content installation. |
| `--fgd <file.fgd>` | Load another entity definition file and its includes. |
| `--mcp-port <port>` | Change the HTTP MCP port; the default is `7377`. |
| `--no-mcp` | Run the GUI without the MCP server. |
| `--headless` | Serve MCP over stdio; initialize graphics only when a rendered view is requested. |

```sh
cargo test --workspace -j 8
```

</details>

## Editing

### Navigation and selection

| Action | Control |
| --- | --- |
| Fly in perspective | Hold right mouse; move with WASD and Q/E. |
| Change projection | Choose **Perspective**, **Top · XY**, **Front · XZ** or **Side · YZ**. |
| Pan / zoom in planar views | Right-drag / mouse wheel; zoom follows the cursor. |
| Frame selection or map | **Frame · F**; frames the map when nothing is selected. |
| Save / return to a camera | **Saved views…** beside Frame. |
| Select / toggle selection | Click / Ctrl+click. |
| Select an area | Left-drag away from a handle; choose **Intersects** or **Fully inside** and a selection rule. Ctrl+drag toggles. |
| Transform with handles | Choose **Move handles**, **Rotate handles** or **Scale handles**; drag a handle and release to apply. Escape cancels. |
| Nudge selection | Arrows and PgUp/PgDn; Shift uses at most one unit. |
| Duplicate / delete | Ctrl+D / Del. |
| Undo / redo | Ctrl+Z / Ctrl+Y. |

**Grid** accepts spacing from 0.125 to 4096 units. **Snap** and **Show grid**
are independent. In planar views, arrows move within the displayed plane;
PgUp/PgDn move along its depth axis.

### Saved camera views

Open **Saved views…**, name the current camera and choose **Save current as new**.
Select a view and **Go to view** to restore its projection, position, direction,
field of view and planar zoom. **Rename** changes its name; **Replace with current**
updates its camera. **Delete view** removes it. These edits use map undo and
persist when you save the VMF; moving to a saved view leaves the map unchanged.

A map can hold 64 views with names up to 80 characters. **Cancel / close**
discards the name field; applied changes remain undoable. Existing Hammer
camera records are preserved separately.

![Named camera views and shared restore/edit controls](docs/images/saved-views.png)

### Brushes and transforms

In a planar view, click **Draw brush**, set **Start Z/Y/X** and **Depth Z/Y/X**,
then drag a footprint. The preview shows snapped dimensions; releasing
places the brush in one undo step. Escape cancels, and **Done** returns to
selection. **Add brush** places a default block in perspective.

Choose a handle tool above the viewport for movement arrows and plane handles,
rotation rings, or axis and uniform scale handles. **World axes** and **Local
axes** control the frame; local axes follow the first selected object's brush
orientation or entity angles. **Snap** uses the grid for relative movement and
scaled dimensions, and **Angle snap** sets rotation increments. Releasing a
drag applies one undo step; Escape or **Cancel** discards the draft.

Enable **Transform selection** in the inspector for exact XYZ inputs. Choose
the selection center, world origin or custom coordinates as the pivot. These
settings also control handles. **Keep textures attached** preserves surface
UVs. Numeric transforms remain exact, independent of grid snapping.

Area selection works in perspective, Top, Front and Side, for objects or
faces. It includes geometry through the view's depth and skips hidden brushes
and hidden tool materials. A brush entity is selected as a whole. **Fully
inside** requires all its visible brushes to fit; **Intersects** accepts any
intersection. Selection previews highlight candidates without changing the
map or adding undo history.

Open **Select a volume…** to select inside exact XYZ bounds. Start from selection
or map bounds, choose objects or faces and a selection rule, then **Preview
matches**. The blue box and highlights survive camera changes; **Frame box**
shows the region. **Select matches** applies it; **Cancel** leaves selection alone.

Imported terrain uses its supporting brush faces for region queries. Paint
selection is still pending.

| Volume controls | Shared region preview |
| --- | --- |
| ![Volume bounds, match rules and cancellation](docs/images/volume-selection.png) | ![Blue volume guide and matching brush](docs/images/volume-preview.png) |

### Brush clipping

Select brushes or a brush entity, then click **Clip selected brushes**.
Draw a cut in Top, Front or Side, or enter a plane normal and distance.
Choose **Arrow side**, **Opposite side** or **Both sides**, and optionally
browse for the cut-face material. **Apply cut** commits the preview in one
undo step; **Cancel cut** or Escape discards it.

Existing faces keep their texture projections. Brushes the plane does not
cross stay unchanged. Intersected displacements cannot be clipped yet.

### Brush shapes

Open **Brush shapes…**, choose a shape and enter its world bounds. Cylinders,
cones and spheres use segment counts; arches also have an inset, start angle
and sweep. Choose the height/depth axis and browse for the face material.
**Preview shape** shows the geometry without changing the map. **Focus preview**
frames it; **Add shape** creates and selects every brush in one undo step.
**Cancel**, Escape or closing the window discards the preview.

Curves are polygonal and inscribed in the supplied bounds. Arch bounds describe
the full ellipse; a 180° sweep builds half of it, and 360° builds a ring. The
preview reports the actual size and face count. Arches consist of separate
convex brushes; all shapes remain ordinary editable VMF geometry.

| Shape controls | Shapes created through MCP |
| --- | --- |
| ![Brush shape dimensions, material preview and cancellation controls](docs/images/brush-shapes.png) | ![Wedge, cylinder, cone, sphere and segmented arch](docs/images/brush-primitives.png) |

### Check a map

Open **Check map…** and choose **Run checks**. Filter errors and warnings,
select an issue to read its explanation, then use **Select issue** or
**Focus issue** to locate it. Reports show when edits or changed entity
definitions make them stale.

**Review repair…** shows the exact before/after values for a supported repair.
**Apply repair** records one undo step; **Cancel repair** changes nothing.
Initial repairs reset invalid texture projections and zero lightmap scales.
Other issues link you to the affected content for manual editing.

![Map issues, face navigation and an explicit repair preview](docs/images/map-checks.png)

Checks include hidden content and inspect applied edits. Custom properties are
preserved. Material/model checks verify paths; full asset dependencies, spawn
clearance and engine-specific resource limits need further checks. Compile and
test in CS:S to validate leaks and playability.

### Design context

Open **Design context…** to add a **Map brief**, a **Note on selection**, or a
**New region**. Keep intent, constraints and useful observations close to the
geometry they describe. Region bounds and guide colors are editable; guides can
be hidden or focused. Search all notes or find relevant context for the selected
object, a point or an area.

**Save note** records one undo step. **Cancel edit** and **Cancel / close** discard
the shared draft. Notes show their author and whether the source changed since
they were saved. Copied objects retain notes with observations marked for review;
deleting objects removes their notes, and undo restores both.

![Design notes, region bounds and shared editing controls](docs/images/design-context.png)

Notes are editor metadata, excluded from compiled BSPs. Limits are 200 summary
characters, 4 KiB of text per note and 1 MiB shared across the map. MCP retrieves
relevant context in bounded pages and exposes summaries during object inspection.
Notes are advisory; current instructions take precedence. Region change tracking
currently covers the whole map. Other editors' handling of this metadata has not
yet been verified.

### Autosave and recovery

**Recovery…** lists periodic snapshots, manual checkpoints and previous saved
versions. Autosave captures changed maps every two minutes by default, including
unnamed maps. It leaves the original VMF, dirty state and undo history alone.
You can change the interval and retention, or create a snapshot immediately.

Select a snapshot and choose **Open recovered copy**. It opens as an unsaved
map; choose a save path to keep it. Replacing unsaved edits and deleting a
snapshot require explicit confirmation. **Cancel / close** discards unapplied
settings fields. Settings and recovery files persist across editor restarts.

![Autosave settings, retained snapshots and safe recovery controls](docs/images/recovery.png)

Recovery uses `$XDG_STATE_HOME/hammerr/recovery`, falling back to
`~/.local/state/hammerr/recovery`. Set `HAMMERR_RECOVERY_DIR` to an absolute path
for a separate store. Turning autosave off leaves manual snapshots and backups
available. Failed snapshots appear in Recovery and the activity log.

### Face textures and materials

Choose **Faces · materials & textures**, then select surfaces with click
and Ctrl+click. **Apply** assigns the chosen material to those faces.
**Edit face textures…** controls offsets, scales, rotation, one-tile
fitting, edge/center placement and world/face alignment. **Preview textures** shows a draft; **Apply textures** commits it
in one undo step. **Cancel**, Escape or closing the dialog discards it.

To reuse a texture, select one source face and choose **Sample face texture**.
Select targets, open **Edit face textures…**, then choose material painting,
projection copying or wrapping across a shared edge. Wrapping keeps the seam
continuous; each target must touch the source. Samples retain their captured
values until sampled again or another map is opened.

**Browse materials…** offers thumbnail pages, a larger preview, search,
folder/category groups, and map-used, recent and favorite materials.
Choosing a thumbnail selects a candidate; **Use material** accepts it.
**Cancel** keeps the previous choice.

| Material browser | Face texture controls |
| --- | --- |
| ![Material thumbnails, categories and favorites](docs/images/material-browser.png) | ![Face material, offset, scale, rotation and preview controls](docs/images/face-textures.png) |

Blank texture fields preserve existing values when selections are mixed.
Fitting uses the material's real dimensions. Categories are inferred from
material paths; favorites and recent applications last for the session.

### Map textures across faces

Select faces and open **Map textures together…**. **Fit faces together** uses
one face's texture direction and fits across the combined bounds of coplanar
faces. **Project along current view** uses the camera's right and down directions
for a shared parallel projection. Faces edge-on to that view are rejected.

Choose a shared material and set the horizontal/vertical tile counts. **1 × 1**
fits one tile across the whole group; **2 × 1** repeats it twice horizontally.
Fractions enlarge the texture. **Preview mapping**, **Apply mapping** and
**Cancel** share the face-preview workflow and one-step undo. Geometry and
face lighting remain unchanged. View projection uses the camera at preview time
and does not add perspective distortion.

| Grouped texture controls | One texture fitted across adjoining faces |
| --- | --- |
| ![Shared material, reference direction and tile counts](docs/images/grouped-textures.png) | ![Continuous grouped texture mapping created through MCP](docs/images/grouped-textures-viewport.png) |

### Replace a material

Open **Replace materials…** and choose the source material. Search the whole
map, selected objects, or selected faces. **Find faces** reports matches and
how many hidden faces were skipped; enable **Include hidden faces** to include
them. Matching uses the exact material path and ignores case.

**Select matches** selects every result. Choose a replacement with its material
preview, then **Preview replacement** and **Replace faces**. **Cancel**, Escape
or closing the dialog discards its draft. Replacement preserves UV placement,
lighting settings and hidden flags, and undoes in one step. Changed maps or
search selections require a fresh search before applying.

![Material replacement scope, match counts, previews and cancellation](docs/images/material-replacement.png)

### Face lighting

Select faces and open **Face lighting…**. Set a lightmap scale in world units
per lighting pixel, or use the 8/16/32/64 presets. Smaller values mean finer
authored lighting detail. Blank fields preserve each face's current scale.

Toggle groups individually to preserve other memberships across a mixed
selection. Groups **1–24** allow smoothing; shared **25–32** groups prevent it.
**Clear all groups** removes memberships, while **Reset group changes** restores
the current selection's values. **Preview settings**, **Apply settings** and
**Cancel** share the same draft and undo behavior as face texture editing.

Choose **Lightmap density** to inspect the authored grid, or **Group membership**
to highlight one group. These views work in perspective; planar views retain
wireframes. The native compiler preserves the settings but does not yet bake
lighting or compute smoothed lighting normals.

| Face lighting controls | Authored lightmap density |
| --- | --- |
| ![Mixed smoothing groups, scale presets and preview controls](docs/images/face-lighting.png) | ![Lightmap density grids on brushes with different face scales](docs/images/face-lighting-density.png) |

### Entities and lights

Click **Entity…** to search point classes and place lights, spawns or other
entities. The initial position uses the camera or planar workplane and
shared snapping settings. Select an entity to edit its typed properties,
choose enumerated values and toggle spawn flags. Invalid values leave the
map unchanged.

The inspector puts common settings first: position and rotation components,
color pickers, brightness sliders, numeric inputs and material previews.
**?** shows descriptions; **Advanced properties** exposes engine keys and
custom fields. Entity, color, material and texture dialogs all have Cancel
buttons and Escape dismissal.

Point lights use a bundled [CC0 bulb model by Josh Dean / Poly Haven](https://polyhaven.com/a/lightbulb_led).
Spotlights and sunlight have direction helpers; selected spotlights show
cone guides. These are editor markers, visible in both the viewport and MCP
renders. They add no models or geometry to the exported map. See
[asset provenance](crates/render/assets/lightbulb/SOURCE.txt).

Select world brushes and use **Tie to entity…** to create a brush entity.
**Move to world** converts it back. Both operations share undo history with
agent edits. **Load FGD…** in the class picker loads other definitions;
custom classes and unknown keys remain editable.

### Entity connections

Select a point or brush entity and open **Connections…**. **Outgoing** lists
its outputs; **Incoming** shows map connections targeting it and lets you
edit their source. Click **Add connection** or **Edit connection**, then
choose an output, target and input. Suggestions use the map's named entities
and inherited FGD definitions; custom names and I/O are also supported.

Set the parameter, delay in seconds, and **Unlimited**, **Once** or a
specific firing count. **Apply connection** creates one undo step. **Cancel**,
Escape or closing the dialog discards unapplied fields. Deletion is undoable.
MCP can inspect and change the same draft while the dialog is open.

Missing targets, undeclared I/O and parameter type mismatches show warnings.
Invalid encoding, negative delays and invalid counts prevent Apply. Map edits
invalidate an open draft, so it cannot overwrite a shifted connection index.
Incoming links resolve named targets, prefix wildcards and `!self`; other
procedural targets resolve in game.

New connections use CS:S's comma-separated format, so their fields cannot
contain commas, quotes or control characters. Imported escape-separated
connections retain their delimiter; other imported records stay untouched
until explicitly edited or deleted.

![Entity connection form with readable fields, suggestions and Cancel](docs/images/entity-connections.png)

### References and safe renaming

Select an entity and open **References and rename…**, or edit its **Name**.
The preview lists incoming connections, FGD entity-reference properties,
parent links and reference parameters such as `SetParent`. Enter a new name
to see which fields will change. **Apply rename** updates the name and exact
references in one undo step. **Cancel**, Escape or closing the window discards
the draft.

Names shared by several entities require **Rename all … entities**; the preview
lists every member, including hidden entities. An occupied destination name
blocks Apply. Wildcards, procedural targets, ambiguous class references and
untyped text stay unchanged. References whose meaning may change are listed
under **Needs your attention**, with an explanation of each effect. Review them
before choosing **I’ve reviewed these effects; leave these fields unchanged**.
Changing the name or group resets that acknowledgment. **What will change**
lists the link updates separately. **Show engine details** reveals raw keys,
class names and IDs when needed.

Attachment suffixes, duplicate outputs, connection delimiters and imported
numeric formatting are preserved. Edits, undo/redo and FGD changes invalidate
an open preview. MCP shares the same draft, preview and guards. Raw MCP property
edits and explicit Advanced-property removal remain available; use this workflow
when renaming should propagate references. Embedded scripts, runtime-created
links and arbitrary text expressions require manual inspection.

![Reference rename preview with exact updates and unchanged references to review](docs/images/entity-rename.png)

### World settings and property batches

**World settings…** edits map-wide values such as the sky. Select several
entities and open **Edit selected entity properties…** to work on their
common properties. Mixed values and flags are identified; color and material
pickers are available. Changing one flag or color component preserves the
other values on each entity.

Edits are staged until **Apply properties**, then saved as one undo step.
**Cancel** discards the draft. **Use default** explicitly writes the FGD
default; **Remove override** deletes the stored key. The inspector distinguishes
stored values from defaults. Names use **References and rename…**.

### Finding and organizing objects

**Object report…** searches IDs, names, classes and materials, with type and
visibility filters and optional XYZ region bounds. Select or frame an
individual result or all matches,
including results beyond the current page. Selection can replace, add, remove
or toggle matching objects.

**Hide selected**, **Isolate selected**, **Show selected** and **Show all**
change saved visibility in one undo step. **Restore visibility** restores
the previous state, including hidden brushes inside entities. Newly created
objects keep their current visibility; deleted objects stay deleted.
**Hidden objects are omitted from compilation**, including imported hidden
objects. Show them before building if they should appear in the game.

| Property batches | Object report |
| --- | --- |
| ![Shared property draft with mixed values, brightness, defaults and Cancel](docs/images/batch-properties.png) | ![Object report with filters, selection modes and visibility actions](docs/images/object-report.png) |

## MCP

The GUI serves MCP at **`http://127.0.0.1:7377/mcp`**. Connect an agent with:

```json
{
  "mcpServers": {
    "hammerr": {
      "url": "http://127.0.0.1:7377/mcp"
    }
  }
}
```

For an isolated session without a window:

```sh
./target/release/editor path/to/map.vmf --headless
```

Human controls and MCP calls use the same Session operations. Agents can
build geometry, place lights, edit individual faces, preview changes,
compile maps and launch the game. `render_view` returns a PNG from any
viewpoint; `pick_at_pixel` resolves a rendered pixel to an object and face.

For example, place a warm light:

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

| Courtyard built through MCP | Face selection and fitted crate textures |
| --- | --- |
| ![MCP render of textured walls, stairs, crates and light helpers](docs/images/agent-render.png) | ![MCP close-up of stacked crates with one face selected](docs/images/agent-block.png) |

<details>
<summary>Tool reference by task</summary>

| Task | Tools |
| --- | --- |
| Files and inspection | `new_map`, `open_map`, `save_map`, `get_map_info`, `find_objects`, `get_objects` |
| Selection and history | `select`, `get_selection`, `set_selection_mode`, `undo`, `redo` |
| Object edits | `move_objects`, `duplicate_objects`, `delete_objects` |
| Spatial authoring | `set_spatial_settings`, `draw_block`, `cancel_block_preview`, `create_block`, `transform_objects` |
| Transform handles and previews | `set_transform_tool`, `get_transform_handles`, `begin_transform_drag`, `update_transform_drag`, `commit_transform_preview`, `cancel_transform_preview` |
| Area selection | `set_area_selection_options`, `select_area`, `get_area_selection`, `commit_area_selection`, `cancel_area_selection` |
| Brush clipping | `set_clip_tool`, `clip_brushes`, `get_clip_preview`, `commit_clip_preview`, `cancel_clip_preview` |
| Brush shapes | `create_primitive`, `get_primitive_preview`, `frame_primitive_preview`, `commit_primitive_preview`, `cancel_primitive_preview` |
| Entity authoring | `create_entity`, `set_entity_properties`, `tie_to_entity`, `move_to_world` |
| Entity connections | `get_entity_connections`, `get_connection_choices`, `begin_connection_edit`, `update_connection_draft`, `commit_connection_edit`, `cancel_connection_edit`, `delete_entity_connection` |
| References and renaming | `get_entity_references`, `begin_entity_rename`, `update_entity_rename`, `commit_entity_rename`, `cancel_entity_rename` |
| Map reports and visibility | `get_object_report`, `select_object_report`, `set_object_visibility` |
| World settings and property batches | `get_property_inspector`, `begin_property_edit`, `update_property_edit`, `get_property_edit`, `commit_property_edit`, `cancel_property_edit` |
| Entity definitions and controls | `search_entity_classes`, `get_entity_class`, `load_entity_definitions`, `get_entity_inspector`, `set_entity_color`, `set_entity_component` |
| Map checks | `run_map_checks`, `get_map_checks`, `select_map_issue`, `preview_map_fixes`, `apply_map_fixes`, `cancel_map_fixes` |
| Design context | `get_context_state`, `list_context_notes`, `get_context_note`, `get_context`, `begin_context_edit`, `update_context_edit`, `apply_context_edit`, `cancel_context_edit`, `delete_context_note`, `set_context_view`, `focus_context` |
| Recovery | `get_recovery`, `set_recovery_settings`, `create_recovery_snapshot`, `restore_recovery_snapshot`, `delete_recovery_snapshot` |
| Volume selection | `select_volume`, `get_volume_selection`, `commit_volume_selection`, `cancel_volume_selection`, `frame_volume_selection` |
| Face textures | `select_faces`, `get_face_selection`, `set_material`, `edit_faces`, `sample_face_texture`, `get_surface_sample`, `paint_face_texture`, `project_face_group`, `commit_face_preview`, `cancel_face_preview` |
| Material replacement | `find_material_faces`, `get_material_search`, `select_material_faces`, `replace_materials`, `commit_face_preview`, `cancel_face_preview` |
| Face lighting and diagnostics | `edit_faces`, `get_face_selection`, `get_objects`, `commit_face_preview`, `cancel_face_preview`, `set_view`, `render_view` |
| Material browsing | `search_materials`, `browse_materials`, `preview_material`, `set_material_favorite` |
| Saved cameras | `get_camera_views`, `save_camera_view`, `restore_camera_view`, `rename_camera_view`, `delete_camera_view` |
| Camera and visual checks | `get_camera`, `set_camera`, `frame_selection`, `set_view`, `render_view`, `pick_at_pixel`, `raycast` |
| Compile and play | `start_compile`, `get_compile_status`, `cancel_compile`, `compile_map`, `run_map`, `launch_game`, `game_log`, `stop_game` |

`edit_faces` supports material, UV offsets/scales, rotation, fitting, lightmap
scale, smoothing groups and non-destructive previews. Commit or cancel the
shared preview from either front end. Material browsing and inspector tools
return the same catalog, labels and control types used by the GUI.

For example, preview denser lightmaps and add group 3 to the selected faces:

```json
{
  "name": "edit_faces",
  "arguments": { "lightmap_scale": 8, "smoothing_add": [3], "preview": true }
}
```

Pass the returned `state.preview_token` as `token` to `commit_face_preview`
or `cancel_face_preview` to guard against another client replacing the draft.
`smoothing_groups` replaces memberships; `[]` clears them. `smoothing_add` and
`smoothing_remove` preserve unrelated groups. `set_view` shares `surface` values
`textured`, `lightmap_density` and `smoothing_groups`, with `smoothing_group`
from 1 to 32. The same options on `render_view` inspect a view without changing
the user's settings.

The repeatable workflow checks run through MCP and save fixtures, rendered
views where relevant, and a JSON report:

```sh
python -B scripts/validate_face_lighting.py --output /tmp/hammerr-lighting-check
python -B scripts/validate_material_replacement.py --output /tmp/hammerr-material-check
python -B scripts/validate_surface_group.py --output /tmp/hammerr-group-check
python -B scripts/validate_volume_selection.py --output /tmp/hammerr-volume-check
python -B scripts/validate_compile_jobs.py --output /tmp/hammerr-compile-check
python -B scripts/validate_saved_views.py --output /tmp/hammerr-views-check
```

Use a new output directory for each run. Add `--url http://127.0.0.1:7377/mcp`
to check an idle GUI session with a clean map and empty undo/redo history.
Each script temporarily opens its fixture, then restores the original file,
camera, selection and view.

`clip_brushes` accepts a plane normal/distance, three points, or a snapped
planar line. Use `preview: true` to inspect the shared cut before committing
or cancelling it. `get_clip_preview` reports the resulting brushes and faces;
`render_view` and picking use the same preview geometry as the viewport.

`transform_objects` accepts `space: "world"` or `"local"` and `preview: true`.
Handle tools support numeric amounts and viewport pointers, with updates
measured from the gesture's start. `get_transform_handles` returns the rendered
paths in world coordinates and pixels, and can hit-test a pixel. Shared drafts
can be inspected, applied or cancelled from the editor or MCP.

`select_area` takes pixel corners, viewport dimensions and an optional camera
override, plus `contained`/`intersecting` and selection rules. Use `preview:
true` to inspect candidates before applying. Camera overrides leave the user's
view unchanged; face mode returns stable solid/side IDs.

`select_volume` takes world-space `mins`/`maxs`, `selection_mode` (`objects`
or `faces`), a match rule and a combination mode. Use `preview: true`, inspect
the paginated matches, then pass the returned preview token to commit, cancel
or frame. Camera changes preserve the box; map, selection and visibility
changes invalidate its candidates.

`save_camera_view` stores the current shared camera under `name`; `replace_id`
updates an existing view. Pass both `generation` and `revision` from
`get_camera_views` to guard saved-view actions against map changes. Restore
moves the camera without changing selection, map data or edit history.

`begin_connection_edit` starts an addition or edits an outgoing `index`.
`update_connection_draft` accepts readable fields; `delay` and `times` are
strings so unfinished values can be shared with the UI. Inspect feedback,
then commit or cancel. Pass the draft's `token` to guard against another
operation replacing the form. Starting an edit or deleting supports optional
`generation` and `revision` guards from `get_entity_connections`.

`get_entity_references` includes proven links and untyped text candidates.
Begin a rename, update `new_name` and optionally `rename_group`, then inspect
the returned preview. If `requires_review` is true, acknowledge it with
`accept_review: true` in a subsequent update. Commit or cancel with the draft's
`token`; starting a rename accepts generation/revision guards from inspection.
Preview rows include the same readable `display` labels and explanations used
by the UI. `show_details` controls the shared engine-details toggle.

`begin_property_edit` takes `world: true` or entity `ids` (selection by
default). Update the returned `token` with a `changes` list; each operation
uses `action: "set"`, `"remove"`, `"default"`, `"flag"` or `"component"`.
Pass inspection `generation`/`revision` guards when starting from an earlier
report. Inspect the preview, then commit or cancel. Updates replace the
pending list;
failed validation keeps the previous draft. `get_property_inspector` reports
stored/default values per target and mixed flags.

`get_object_report` accepts a `filter` with `query`, `classname`, `targetname`,
`material`, `kind`, `hidden` and `within`. `select_object_report` uses the same
filter across all pages, with optional `frame: true`. `set_object_visibility`
accepts `hide`, `show`, `isolate`, `show_all` or `restore`. Its result includes
hidden counts and the compiler's visibility policy.

</details>

## Compile and play

| Control | Behavior |
| --- | --- |
| **Compile** | Build the current snapshot, including unsaved edits, into a BSP beside the VMF. Unsaved maps prompt for an output filename. |
| **Run Map** | Compile and launch the snapshot in the installed Linux CS:S client with `-insecure`. |
| **Build details…** | Inspect the current stage, elapsed time, snapshot freshness and compile diagnostics. |
| **Cancel build** | Stop the active build while preserving previous output. |
| **Game Log** | Read console and launcher output. |
| **Stop Map** | Stop the game process started by this editor. |
| **Leak path** | Toggle the red leak path drawn through geometry after a failed compile. |

Compilation reports stages as it runs in the background. **Cancel build** waits
for the worker to stop; cancelled builds keep the previous BSP and pointfile.
Output is published atomically, and a build cannot be cancelled after publication.
Closing Build details leaves the build running.

Through MCP, `start_compile` returns immediately with a job token. Poll
`get_compile_status`; pass that token to `cancel_compile`. The existing
`compile_map` and `run_map` calls wait for completion and expose the same status.
A map edit marks the build snapshot stale and prevents launching it.

![Build status with stage, output path and structured diagnostics](docs/images/build-status.png)

Maps need sealed world geometry and
at least one point entity in open space. A leak produces a `.lin` pointfile
and preserves any existing BSP. Editing hides the previous leak path until
the next compilation.

Each game run uses a uniquely named BSP copy and its own logs. The copy is
removed when the game exits; logs remain until the next run or editor
shutdown. Closing the editor stops its managed game process. Existing maps
and unrelated game processes remain untouched.

<details>
<summary>Standalone compiler and room example</summary>

```sh
cargo run --release -p compiler --bin hammerr-compile -- \
  map.vmf --game /path/to/cstrike/gameinfo.txt
```

Use `-o /path/to/map.bsp` to choose the output. To generate a sealed room
with both player spawns:

```sh
cargo run -p compiler --example room -- room.vmf
```

After compiling, copy the BSP to `cstrike/maps/` and run `map room` in the
CS:S console. The room has been compiled and loaded in Linux CS:S with a
player spawned inside and fullbright rendering enabled by the engine.

</details>

## Status

**The compiler is experimental.** It produces fullbright BSP v20 maps with
a single visibility cluster. Lights are saved in VMF and carried into BSP
entities; baked lighting is still pending.

VVIS, VRAD, physics collision, water volumes, compiled displacements,
static props, areaportals, overlays, cubemaps and origin brushes remain
unfinished. Unsupported geometry is reported in diagnostics. The BSP
reader/writer is verified against stock CS:S maps.

Editor work ahead includes broader map checks, vertex and edge editing,
displacement tools, model rendering, paint strokes and chained texture seams.

## Project layout

| Crate | Responsibility |
| --- | --- |
| `source-formats` | Valve file formats and the game filesystem. |
| `map-core` | Geometry, displacements, undoable documents, edits and picking. |
| `render` | wgpu renderer, materials, camera and offscreen snapshots. |
| `editor` | Slint UI, shared Session and MCP server. |
| `compiler` | Native VMF-to-BSP compilation and the `hammerr-compile` CLI. |

Counter-Strike: Source and its assets are not included.
