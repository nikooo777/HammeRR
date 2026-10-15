# Repeated placement

Create a line, ring or progressively scaled array of existing brushes and
entities while reviewing every copy before changing the map. Originals stay in
place. Applying the complete array records one Undo step.

1. Select 1–16 objects and choose **Repeat array…** in the selection controls.
2. Set **Additional copies**. This count excludes the original objects.
3. Choose **World** or **Source local** under **Step space**, then enter XYZ
   **Move step · units** and **Rotate step · °** in that frame. Copy 1 uses one
   step, copy 2 uses two steps, and so on. Each copy starts from the originals.
4. Set XYZ **Scale factor per copy**. **1** keeps size, **1.1** grows an axis by
   10% per copy, and **0.9** shrinks it by 10%. Copy 2 uses the factor squared:
   a factor of **1.1** gives **1.21** times the original size. Use positive
   factors; scaling requires brushes or brush entities.
5. Set **Fixed world pivot**. It initially uses the current transform pivot, or
   the selection center. Each copy scales, then rotates about this same point
   before its translation in the chosen frame is added. For a ring, use a zero
   move step and a Z rotation.
6. Choose **Keep textures attached**, then **Preview array**. Attached textures
   stretch with scaled geometry; clearing the option keeps their original world
   projection. The viewport shows the copies; the dialog lists their generated
   names and internal-link counts.
7. Review the geometry and names, then choose **Apply array**. All new top-level
   objects become selected. **Undo** removes the complete array; **Redo** restores
   it. **Cancel**, Escape or closing the dialog discards its unapplied preview.

![Repeat array controls for world or source-local translation, rotation and scaling](images/repeat-array.png)

**Source local** uses the same frame as local object transforms, taken from the
first source in stable object-ID order: world brushes before entities, then the
lowest numeric ID within each kind. Brushes supply their authored face frame;
point entities supply their orientation, and usable models supply their model
transform frame.
The preview identifies this source. That original frame stays fixed for every
copy, including mixed selections; later copies do not follow earlier copies'
rotated axes. **Fixed world pivot** always stays in world coordinates.
Scale uses those same captured axes. Copy n scales the originals by each axis
factor raised to n, then rotates, then moves; copies never build on earlier
copies' geometry.

Names become unique `name_copyN` values. An occupied name gets a numeric suffix.
Certain exact internal FGD properties, parent links and I/O targets or reference
parameters follow the corresponding copy. Parent attachment suffixes and raw
connection formatting remain. External links and procedural targets stay as
authored. Original and external entities are never renamed or rewritten.

Shared source names, duplicate name fields, uncertain internal text matches,
internal wildcards and generated names that would acquire existing references
reject the whole preview. Rename or resolve those fields first. Named source
sets with scripts or dynamic I/O (`AddOutput`, `RunScriptCode`, `RunScriptFile`)
are excluded because their links cannot be safely remapped. Overlay entities,
node IDs and FGD-declared numeric node/side references are also excluded.
Other custom metadata and embedded text remain verbatim; their reference
semantics are outside the supported remapping policy.

Limits are 64 additional copies, 256 new objects including owned entity brushes,
8192 new sides, names up to 128 UTF-8 bytes, and geometry within ±65536 units.
Object and owned-brush design notes copy with their existing authorship and
observations, subject to the full note and map budgets. An overflow rejects the
entire operation. Saved face partitions stay attached to the originals.

Invalid settings retain the last valid preview. Replacing the map, editing the
source, changing selection or reloading entity definitions prevents stale Apply.
Apply the array before saving or compiling; these operations use committed
geometry. World and source-local translation, rotation and scale steps support
ordinary brushes, brush entities and checked displaced terrain. Nonidentity
scaling rejects point entities, including models, because
their size cannot be represented by this transform. Zero, negative, nonfinite,
overflowing or collapsed scales reject the entire preview. Surface-sprinkle
arrays are not available.

MCP uses the same Session operations: inspect `get_repeat_array`, pass its latest
`snapshot` to `preview_repeat_array` with `space: "world"` (default) or
`space: "local"`, then pass the resulting `preview.token` to
`apply_repeat_array` or `cancel_repeat_array`. `render_view` shows the shared
preview; `get_objects` continues to inspect committed data.
The preview exposes `space`, the fixed frame's column vectors in `basis`, and
`frame_source` for local mode. Invalid space or frame requests keep the last valid
preview.
Pass optional `scale: [x, y, z]` for per-copy factors, default `[1, 1, 1]`.
`preview.scale` reports these factors, and each `preview.copies` entry reports
its resulting `scale` for that copy.
