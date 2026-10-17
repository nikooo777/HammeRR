# Instance contents

Inspect the world and detail geometry referenced by saved `func_instance`
helpers. The flat reference layer uses the helpers' complete Source transforms,
with owner selection, depth and camera framing. It leaves child files and parent
map objects unchanged.

1. Save the parent VMF. Give each visible instance helper an explicit relative
   `.vmf` **file** reference inside the saved parent's directory tree.
2. Under **VIEWPORT**, enable **Instance contents**, then open **Instance contents…**.
3. Click **Refresh instance contents**. The inspector reports loading, ready,
   empty or failed state, owner references, transforms, world/detail counts,
   content bounds and checked dependency identities. **Cancel refresh** cancels
   the current request; **Clear** frees the resolved layer.
4. Click a displayed reference surface to select its root instance helper.
   Overlapping object cycling uses that same owner. **Frame instance contents**
   frames selected root instance owners, or all owners when nothing is selected.
   Ordinary frame-selection and camera focus include displayed content bounds.
5. After changing a child file, helper transform or file reference, click
   **Refresh instance contents** again. Stale contents disappear from rendering,
   picking and framing; a failed refresh keeps its error and shows no old layer.

Turning the checkbox on or off displays an existing current snapshot; it never
loads child files. Refresh resolves one immutable parent snapshot outside the
Session lock. Unsaved edits in that parent remain authoritative. All visible
references must resolve successfully together. A missing or unsupported child
fails the whole layer, including repeated occurrences of that child.

The layer is available in ordinary **Objects** mode. Faces, components,
placement, paint, clipping and active geometry drafts suppress it, with a status
reason. Child geometry never becomes an editable root face or placement target.
Selection dimensions, transform pivots, measurement attachments, volume tools
and other authoring bounds continue to use real parent objects. A working region
includes a whole reference owner when its content bounds or marker intersects.

Reference surfaces use a distinct flat color, with a selected-owner tint and
outline. Root geometry occludes them by depth. **Wireframe** draws their edges
without filling the faces. The layer does not load child materials or evaluate
lighting, normals, sky, water, transparency, models or runtime entities.
**Tool textures** does not classify this flat reference layer. Nested instances,
child editing, file browsing/replacement authoring and live transformed child
previews remain unsupported.

Limits include 256 occurrences, 32 unique files, 4 MiB per file and 32 MiB total
source bytes, with bounded parsing and geometry work. Derived geometry is capped
at 262,144 vertices, 786,432 indices, 524,288 edge vertices and 262,144 pick
triangles; retained CPU data is capped at 64 MiB and GPU geometry payload at
32 MiB. Exceeding a preview limit rejects the entire layer; ordinary parent
rendering remains. Compilation can have a different outcome under its own limits.

MCP uses `inspect_instance_contents` and `set_instance_contents` with
`refresh`, `cancel`, `clear` or `frame`. Inspectors paginate at most 64 owners per
call. Shared `set_view.instance_contents` and temporary
`render_view.instance_contents` control display; the matching optional
`pick_at_pixel.instance_contents` selects what the temporary image shows.
Rendered metadata retains the report for the exact uploaded snapshot.
