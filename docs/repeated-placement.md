# Repeated placement

Create a line, ring or stepped array of existing brushes and entities while
reviewing every copy before changing the map. Originals stay in place. Applying
the complete array records one Undo step.

1. Select 1–16 objects and choose **Repeat array…** in the selection controls.
2. Set **Additional copies**. This count excludes the original objects.
3. Enter the world XYZ **Move step · units** and **Rotate step · °**. Copy 1 uses
   one step, copy 2 uses two steps, and so on. Each copy starts from the originals.
4. Set **Fixed world pivot**. It initially uses the current transform pivot, or
   the selection center. Each copy rotates about this same point before its
   world translation is added. For a ring, use a zero move step and a Z rotation.
5. Choose **Keep textures attached**, then **Preview array**. The viewport shows
   the copies; the dialog lists their generated names and internal-link counts.
6. Review the geometry and names, then choose **Apply array**. All new top-level
   objects become selected. **Undo** removes the complete array; **Redo** restores
   it. **Cancel**, Escape or closing the dialog discards its unapplied preview.

![Repeat array controls](images/repeat-array.png)

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
geometry. This first workflow supports fixed world translation/rotation steps.
Local, scale and surface-sprinkle arrays are not available.

MCP uses the same Session operations: inspect `get_repeat_array`, pass its latest
`snapshot` to `preview_repeat_array`, then pass the resulting `preview.token` to
`apply_repeat_array` or `cancel_repeat_array`. `render_view` shows the shared
preview; `get_objects` continues to inspect committed data.
