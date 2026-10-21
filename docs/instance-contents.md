[← Manual](README.md)

# Instance contents

Inspect the world and detail geometry referenced by saved `func_instance`
helpers, including power 2–4 terrain and nested references. The flat-color
reference layer displays actual displaced terrain surfaces and composes the
helpers' Source transforms from the innermost reference outward. Selection,
content bounds and camera framing use the displayed geometry. Child files and
parent map objects remain unchanged; no terrain switch is required.

1. Save the parent VMF. Use **Instance reference…** to create an explicit relative
   `.vmf` reference inside its directory tree; see [Instance authoring](instance-authoring.md).
2. Under **VIEWPORT**, enable **Instance contents**, then open **Instance contents…**.
3. Click **Refresh instance contents**. The inspector reports loading, ready,
   empty or failed state, owner references, transforms, world/detail counts,
   content bounds and checked dependency identities. **Cancel refresh** cancels
   the current request; **Clear** frees the resolved layer.
4. Click a displayed reference surface to select its root instance helper.
   Terrain picking follows the displaced triangles. Overlapping object cycling
   uses that same owner. **Frame instance contents** frames selected root
   instance owners, or all owners when nothing is selected. Ordinary
   frame-selection and camera focus include displayed content bounds.
5. After changing any child or descendant file, helper transform or file
   reference, click **Refresh instance contents** again. Stale contents disappear
   from rendering, picking and framing; a failed refresh keeps its error and
   shows no old layer.

![Reference inspector with owner and dependency details](images/instance-contents-terrain.png)

![Nested world and detail terrain reference surfaces](images/instance-terrain-reference.png)

Limits:

Turning the checkbox on or off displays an existing current snapshot; it never
loads child files. Refresh resolves one parent snapshot, including its unsaved
edits. All visible references and their descendants must resolve successfully
together. A missing
or unsupported descendant fails the whole layer, including repeated occurrences
of that file. Changing a grandchild makes the whole resolved layer stale.

Cancellation requests cleanup; a new refresh waits until that worker and its
result have retired. Normal exit cancels outstanding instance work and waits
for its cleanup before the final recovery snapshot.

The layer is available in ordinary **Objects** mode. Faces, components,
placement, paint, clipping, ruler endpoint picking and active geometry drafts suppress it, with a status
reason. Child geometry never becomes an editable root face or placement target.
Selection dimensions, transform pivots, measurement attachments, volume tools
and other authoring bounds continue to use real parent objects. A working region
includes a whole reference owner when its content bounds or marker intersects.

Reference surfaces use a distinct flat color, with a selected-owner tint and
outline. Root geometry occludes them by depth. **Wireframe** draws their edges
without filling the faces. Terrain draws the displaced sheet and its triangle
edges, with its brush cage faces omitted. Invalid terrain fails the whole
refresh; there is no cage fallback. The layer does not load child materials or
preview lighting, normal maps, sky, water, transparency, models or runtime
entities.
**Tool textures** does not classify this flat reference layer. Child editing,
parameter replacements and automatic live refresh remain unsupported.
[Instance authoring](instance-authoring.md) edits the root reference.

Terrain uses the same supported neighbor and density rules as compilation.
Ambiguous or mismatched shared edges remain unlinked and produce a diagnostic.
The preview cannot replace ordinary parent terrain: if expanded child contents
would change a parent patch's displaced vertices or triangle indices compared
with the parent without its instance contents, **Refresh instance contents**
fails with “Instance contents would require replacing ordinary parent terrain
LOD; compilation remains available”. A coarse child next to a finer parent
patch can require this change. Existing density transitions between parent
patches and unrelated child terrain remain allowed. Compilation can still
expand the complete scene under its own limits.

References allow four helper levels, counting each root helper as level one.
Every nested filename is relative to the saved root parent's referring directory,
not the containing child's directory. Limits include 256 helper occurrences and
32 distinct normalized requested file paths across the entire expansion, 4 MiB
per file and 32 MiB total source bytes, with bounded parsing and geometry work.
The [child terrain limits](instances.md) also apply: power 2–4 world/detail
terrain, 65,536 source-grid vertices, 262,144 expanded grid vertices and
2,097,152 grid-work units. The preview's complete terrain context, including
ordinary parent patches, is capped at 262,144 grid vertices. Preparing and
comparing the expanded and ordinary parent terrain contexts must fit a combined
64 MiB CPU budget and bounded geometry work.
Derived geometry is capped at 262,144 vertices, 786,432 indices, 524,288 edge
vertices and 262,144 pick triangles; retained CPU data is capped at 64 MiB and
GPU geometry payload at 32 MiB. Exceeding a preview limit rejects the entire
layer; ordinary parent rendering remains. Compilation can have a different
outcome under its own limits.

MCP inspection keeps `owners[].file` as the normalized root reference.
`brushes[].file` and `face_provenance[].file` identify each true leaf alongside
its original solid/side IDs. Terrain provenance rows additionally report
`terrain: true`, `power`, `vertices` and the displayed `triangles` count.
`geometry` counts describe the actual reference mesh, including any terrain
density adjustments. Repeated placements may share those leaf identities;
picks and framing still use the root helper.

MCP uses `inspect_instance_contents` and `set_instance_contents` with
`refresh`, `cancel`, `clear` or `frame`. Inspectors paginate at most 64 owners per
call. Shared `set_view.instance_contents` and temporary
`render_view.instance_contents` control display; the matching optional
`pick_at_pixel.instance_contents` selects what the temporary image shows.
Rendered metadata retains the report for the exact uploaded snapshot.
