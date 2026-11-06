[← Manual](README.md)

# Hollow, carve and clip

Turn solid geometry into rooms, openings and cut shapes.

Select brushes to show **Brush tools** in the Inspector. The same commands are in the **Brush** menu.

![Brush tools in the Inspector](images/inspector-brush.png)

## Hollow

1. Select the source brushes.
2. Open **Hollow brushes…**.
3. Set wall thickness.
4. Choose **Inside the original size** or **Outside the original size**.
5. Check the faces to remove under **Openings**.
6. Click **Preview walls**.
7. Inspect the cavity with **Look inside**.
8. Click **Create walls**.

![A hollow room with its roof and front removed](images/hollow-room.png)

Inside keeps the outer dimensions. Outside keeps the original interior dimensions.
Each source needs at least one wall. Open shells need other geometry to seal a playable map.

## Carve

1. Select cutters and targets.
2. Open **Carve brushes…**.
3. Choose **Cutter · shape to subtract** and **Target brushes**.
4. Enable **Use multiple cutter brushes** for more than one cutter.
5. Under **New cut faces**, choose cutter settings, corresponding target settings, or a named material.
6. For a named material, enter its name or click **Browse materials…**.
7. Click **Preview carve** and inspect the targets.
8. Click **Apply carve**.

![A doorway cut with a box and cylindrical cutter](images/carved-doorway.png)

Cutters remain in the map. Move or delete them afterward.
The default copies cutter surface settings. Target mode requires a unique
original target face parallel to each new face. Named material mode keeps the
cutter's texture placement and lighting settings. Unsupported matches reject
the complete preview; use cutter or named material mode for oblique cuts.
**Discard preview** keeps the source choices. **Cancel** closes the tool.

## Clip

1. Select brushes. Use **Select → Object selection → Solids** to choose a child
   brush inside an entity.
2. Click **Clip selected brushes**.
3. For terrain, choose **Existing grid** or **Retile flat terrain** under **Terrain cut**.
4. Draw a line in a planar view, enter a plane, or choose **Pick three points**.
5. Choose **Arrow side**, **Opposite side** or **Both sides**.
6. Inspect the preview.
7. Click **Apply cut**.

![A diagonal cut creates a sloping brush](images/clipped-brush.png)

Use **Move plane** or **Rotate plane** to adjust the cut with handles.
**Cancel cut** or <kbd>Escape</kbd> discards the preview.
Changing **Terrain cut** discards the current preview, picked points and plane
positioning without changing the map or Undo history.
A child-brush cut keeps its entity and unselected siblings. Apply selects the
resulting child brushes. Selecting a whole entity clips all crossing children.

### Cut terrain on its existing grid

1. Select power-3/4 world or `func_detail` terrain brushes.
2. Click **Clip selected brushes**.
3. Under **Terrain cut**, choose **Existing grid**.
4. Set a plane through a complete middle row or column. Power 4 also permits
   quarter-grid lines: row or column 4 or 12 of its 16 cells.
5. Choose **Arrow side**, **Opposite side** or **Both sides**.
6. Inspect the native terrain patches, then click **Apply cut**.

A middle cut produces two patches per retained half, at one lower power.
A quarter cut produces four power-2 patches on the short side and six power-2/3
patches on the long side. **Both sides** produces ten patches.
Grid spacing, paint samples and triangle flags stay the same.
The cut follows the displaced surface, not its backing brush.
It does not create a wall beneath the cut.
Matching neighbor borders are checked before Apply. Neighbor grids stay unchanged.
The preview also checks retained points at native game precision. A cut that
cannot keep those points and their side of the plane rejects.
Visible map instances use freshly resolved child terrain for these checks. Save
a new parent first. Child maps stay unchanged; prepare another preview after
child edits or a parent save.

![A painted terrain cut preserves its joined neighboring patches](images/terrain-grid-cut.png)

### Retile flat terrain

Use this mode to cut an eligible planar surface across its existing grid.

1. Select planar power-2/3/4 world or `func_detail` terrain. Its base cage
   must lie on a world-axis plane; the visible surface can be sloped.
   Select all patches that the cut must split.
2. Click **Clip selected brushes**.
3. Under **Terrain cut**, choose **Retile flat terrain**.
4. Draw a line, enter a plane, or choose **Pick three points**.
5. Choose **Arrow side**, **Opposite side** or **Both sides**.
6. Inspect the new native patches, then click **Apply cut**.

Each retained three-to-five-corner region becomes power-2 quadrilateral patches.
Four-corner regions stay one patch when their joins permit. Select the results
to cut them again. Compatible joined patches are checked as one component.
Each surface must remain on one exact plane. Height can vary linearly along
the base normal. Blend alpha and native physical triangle flags must be uniform.
Retiling keeps
that field, material, texture placement and smoothing. It does not flatten a
curved or variably painted surface, and **Existing grid** never switches modes after a
failed cut.

![A diagonal cut retiles flat terrain into native quad patches](images/terrain-flat-cut.png)

![Repeated cuts preserve a sloped terrain sheet](images/terrain-sloped-cuts.png)

## Limits

- Hollow: 16 brushes, 64 faces per brush, 256 walls; thickness 0.125–4096 units.
- Carve: 1–8 cutters, 16 targets, 256 pieces, 64 faces per piece.
- Carving away every child brush of an entity rejects.
- Terrain cannot be hollowed or carved. **Existing grid** requires a power-3/4
  world or `func_detail` patch and a complete middle grid line, or a power-4
  quarter line.
  Matching full/half borders must retain their sample spacing and LOD state.
  Coarsened source and output patches must retain their native masks, positions,
  collision triangles and maximal render triangles. Unselected neighbors must
  keep their complete native state.
- **Existing grid** rejects other cut lines, power-2 splitting, quarter-edge
  links, changed LOD geometry and overlapping or unmatched borders.
  Missing or unsupported child sources stop the preview. Saved-hidden references
  are omitted; display filters do not remove compiler neighbors.
- **Retile flat terrain** requires a common base plane, surface plane and
  orientation across its affected contact component. Curved neighbors, overlaps,
  unsupported partial joins and affected LOD coarsening reject. Source paint and
  physical tags must be uniform; restricted masks and opaque metadata reject.
  Native positions, shared samples and texture fields must remain exact.
  Normal inputs must pass the native arithmetic and conditioning checks.
  Normal bits and final pixels can differ after retiling.
- Retiling requires a direct static `LightmappedGeneric` material with an
  untransformed `$basetexture` and an ordinary single-frame VTF with positive
  power-of-two dimensions no larger than 32768. Material and texture references
  use case-insensitive lookup.
  Use relative names without extensions or a `materials/` prefix.
  An optional static `$detail` layer supports blend modes 0–3 and a factor from
  zero to one. Its scale must be an exact native float, written without an
  exponent in at most 32 characters. Omitted scale uses the native value 4.
  Each scaled UV sample must stay exact; some otherwise valid scales reject.
  Patch materials, blend shaders, base transforms, animated or runtime textures
  and unsupported resources reject. Changed source images or materials require
  another preview before Apply.
- Terrain cuts accept 16 sources and 64 output patches. Unsupported bindings,
  notes, masks and opaque payloads reject before the map changes.
- Numerically uncertain cuts reject. They do not move retained points to fit
  the new patch cages.
- Select one child in **Solids** mode, or the whole `func_detail` entity.
  Only targeted crossing brushes change. Runtime brush-model terrain rejects.
- A connected cut checks at most 128 neighboring faces. Terrain and resolved
  instance context share a 64 MiB snapshot budget.
- Invalid, collapsed or excessively thin results reject before the map changes.

<!-- page-nav -->

---

[← Brushes and transforms](brushes.md) · [Manual](README.md) · [Polygon brushes →](polygon-brushes.md)
