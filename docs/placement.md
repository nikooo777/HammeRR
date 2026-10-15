[← Manual](README.md)

# Surface placement

Place selected objects against a floor, ceiling, brush face, terrain triangle or supported model mesh. Review the shared preview before changing the map.

1. Select one or more objects, then click **Place on surface…** in the selection controls.
2. Choose **Target**: **Floor** casts down from the selection; **Ceiling** casts up; **Pick surface** lets you choose a visible surface in the viewport.
3. Enter **Offset** in map units. Positive values leave space away from the surface; negative values embed the selection.
4. Enable **Align local up to surface** to rotate the selection so its first object's local up direction follows the hit surface normal. Leave it off to retain the current orientation.
5. Choose **Keep textures attached** for brushes, then click **Preview placement**. With **Pick surface**, click **Pick surface**, then click a target in the viewport. A miss keeps picking active; **Escape** or **Cancel picking** abandons it.
6. Review the preview and its movement/offset readout. The yellow mark shows the target and its normal. **Frame preview** moves the camera to the selected preview.
7. After changing settings, preview again. Click **Apply placement** to change the map in one Undo step. **Cancel**, **Escape** or closing the window discards this placement.

![Surface placement controls with a reviewed wall alignment](images/surface-placement-controls.png)

![A model aligned against a wall with an offset](images/surface-placement.png)

## Limits

- Selections move as one rigid group. This is a geometric placement operation; it does not settle each object separately, simulate physics or resolve overlaps with nearby surfaces.
- Floor and ceiling placement use one vertical ray through the selection's bounds center and retain its horizontal location. Picked placement centers the group over the hit point. A curved or small target can therefore intersect another part of the selection.
- Brush vertices, actual terrain samples and supported model-body mesh vertices determine contact. Unsupported or blank model choices and ordinary point entities use their editor helper bounds.
- Targets are visible brushes, actual terrain triangles and supported model meshes. Selected objects and their owned brushes are excluded. Hidden objects and point helper boxes are not targets.
- Alignment uses the first selected object's local up direction for the entire group. Terrain and model normals are the hit triangle's geometric normal, rather than an interpolated lighting normal.
- Finish another geometry preview before starting. Changing the source, selection or scene invalidates an existing placement; preview it again. An old window's Cancel preserves replacement previews.
- Placement supports at most one million finite support points and offsets within ±65536 units. Ordinary transform validation still applies.
