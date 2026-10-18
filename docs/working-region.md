[← Manual](README.md)

# Working region visibility

A working region limits the editor view to whole objects touching a box. It helps
focus on one part of a map without changing saved visibility, geometry or build
scope. Desktop and MCP renders and spatial selection use the same filter.

1. Click **Working region…** beside the viewport controls.
2. Select objects or faces and click **Use selection bounds**, or enter the
   **Minimum** and **Maximum** X/Y/Z corners. Selection bounds load the fields;
   they do not apply the filter yet.
3. Check **Enabled** and click **Apply**. The dialog reports excluded entries and
   selected objects/faces excluded by the region, temporary editor hiding or authored hiding. Existing
   selections are retained. MCP inspection also identifies tool-texture filters.
4. Click **Cancel** to close the dialog and discard unapplied field changes.
   Applied settings stay active; the **Working region active · editor view only**
   bar remains visible above the viewport.
5. Click **Clear working region** in that bar or dialog to restore all otherwise
   visible objects. Clear stays reachable even when the region excludes the
   entire map. Unchecking **Enabled** and applying retains the bounds for reuse.

![Working-region bounds and shared visibility controls](images/working-region.png)

Objects touching the box boundary remain whole. A brush entity stays together
when its visible-child bounds intersect the box; a hidden child does not pull an
outside owner into view. Models use their transformed asset/body bounds. Missing
or unsupported models retain helper bounds, and MCP inspection identifies that
fallback. Displacements use their displaced-sheet bounds.

The filter also affects object cycling, face/component/terrain picking,
marquee/volume/paint selection and picked placement/cut surfaces. Selection
handles and object-specific guides disappear for excluded units. Explicit
ID-based reports and editing remain available. Invalid bounds or stale requests
preserve the current filter. Source, selection, camera, history and owned geometry
or overlay drafts remain intact; changing the region ends spatial gestures.

## Limits

- One axis-aligned box per editor Session; it resets when opening or creating a
  map. It does not follow the camera, clip objects or save a visibility preset.
- During document shutdown, **Apply**, **Use selection bounds** and **Clear
  working region** reject without changing the current filter. **Cancel** still
  closes the dialog.
- [Temporary editor hiding](selection.md#temporary-editor-visibility), saved hide/show and tool-texture filters remain independent. Clearing the region does not clear temporary exclusions. **Compile region…**
  controls builds separately; working-region filtering never changes BSP output.
- Unknown bounds remain visible with an `unresolved_bounds` inspection reason.
  Loaded models and helper fallbacks can have different bounds.
- Global context, compile-region, selection-volume and leak-path guides retain
  their own visibility controls. Light-target lookup and terrain-neighbor shading
  still use the authored visible map, so excluded neighbors do not change the
  appearance of retained objects.
- This slice does not add draw-distance filtering, arbitrary class/material
  filters or a new spatial index for scene construction.
