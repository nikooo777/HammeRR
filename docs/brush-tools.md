[← Manual](README.md)

# Hollow, carve and clip

Turn solid geometry into rooms, openings and cut shapes.

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
5. Click **Preview carve**.
6. Inspect removed and unchanged targets.
7. Click **Apply carve**.

![A doorway cut with a box and cylindrical cutter](images/carved-doorway.png)

Cutters remain in the map. Move or delete them afterward.
New cut faces inherit the cutter's material and texture placement.
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

## Limits

- Hollow: 16 brushes, 64 faces per brush, 256 walls; thickness 0.125–4096 units.
- Carve: 1–8 cutters, 16 targets, 256 pieces, 64 faces per piece.
- Carving away every child brush of an entity rejects.
- Terrain cannot be hollowed or carved. A clip through terrain rejects.
- Invalid, collapsed or excessively thin results reject before the map changes.
