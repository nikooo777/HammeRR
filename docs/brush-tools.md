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

1. Select brushes.
2. Click **Clip selected brushes**.
3. Draw a line in a planar view, enter a plane, or choose **Pick three points**.
4. Choose **Arrow side**, **Opposite side** or **Both sides**.
5. Inspect the preview.
6. Click **Apply cut**.

![A diagonal cut creates a sloping brush](images/clipped-brush.png)

Use **Move plane** or **Rotate plane** to adjust the cut with handles.
**Cancel cut** or Escape discards the preview.

### Cut terrain on its existing grid

1. Select world terrain brushes with power 3 or 4.
2. Click **Clip selected brushes**.
3. Set a plane through the grid's complete middle row or column.
4. Choose **Arrow side**, **Opposite side** or **Both sides**.
5. Inspect the native terrain patches, then click **Apply cut**.

Each retained half has two patches. **Both sides** produces four patches.
Their power decreases by one; grid spacing, paint samples and triangle flags
stay the same. The cut follows the displaced surface, not its backing brush.
It does not create a wall beneath the cut.
Matching neighbor borders are checked before Apply. Neighbor grids stay unchanged.

![A painted terrain cut preserves its joined neighboring patches](images/terrain-grid-cut.png)

## Limits

- Hollow: 16 brushes, 64 faces per brush, 256 walls; thickness 0.125–4096 units.
- Carve: 1–8 cutters, 16 targets, 256 pieces, 64 faces per piece.
- Carving away every child brush of an entity rejects.
- Terrain cannot be hollowed or carved. Terrain clipping requires a power-3/4
  world patch and an existing complete middle grid line. Matching full/half
  borders must retain their sample spacing and LOD state.
- Arbitrary cuts, power-2 splitting, quarter-edge links, LOD changes, overlapping
  or unmatched borders and unresolved map instances reject.
- Terrain cuts accept 16 sources and 64 output patches. Unsupported bindings,
  notes, masks and opaque payloads reject before the map changes.
- A connected cut checks at most 128 neighboring faces.
- Invalid, collapsed or excessively thin results reject before the map changes.

<!-- page-nav -->

---

[← Brushes and transforms](brushes.md) · [Manual](README.md) · [Polygon brushes →](polygon-brushes.md)
