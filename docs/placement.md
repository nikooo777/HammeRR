[← Manual](README.md)

# Surface placement

Align a selection with a floor, ceiling or picked surface.

1. Select the objects.
2. Click **Place on surface…**.
3. Choose **Floor**, **Ceiling** or **Pick surface** under **Target**.
4. Enter **Offset**.
5. Enable **Align local up to surface** if rotation is required.
6. Choose **Keep textures attached** for brushes.
7. Click **Preview placement**.
8. For picked placement, click **Pick surface**, then the target surface.
9. Inspect the yellow target mark and movement.
10. Click **Apply placement**.

Positive offsets leave space above the surface. Negative offsets embed the selection.
**Cancel** or Escape discards the preview.

## Snap a real corner

1. Select whole objects, including a flat brush.
2. Choose **Snap selected corner** under **Target**.
3. Choose **Source corner**.
4. Click **Pick target corner**.
5. Click a visible brush, terrain triangle or supported model mesh.
6. Inspect both corners and **Move**.
7. Click **Apply placement**.

The nearest vertex of the first hit face or triangle becomes the target.
The selection translates as one group, without grid rounding or rotation.

![A barrel follows the normal of a sloping surface](images/slope-placement.png)

## Limits

- Placement does not simulate physics or resolve other overlaps.
- Floor/Ceiling uses one vertical ray through the selection center.
- Hidden objects and selected sources cannot be targets.
- Corner snapping requires a real flat-brush source corner. Terrain and models can supply target vertices.
- Missing or unsupported meshes provide no vertex target.
- Geometry must remain finite and within ±65536 units.
- Changed sources or reloaded models require a new preview.
