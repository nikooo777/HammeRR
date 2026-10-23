[← Manual](README.md)

# Selection

Choose objects, faces or bounded groups of visible geometry.

## Pick objects or faces

1. Choose **Selection → Objects** or **Faces · materials & textures**.
2. Click the target.
3. Use Ctrl+click to toggle membership.

Use **Pick object types** to allow **World brushes**, **Brush entities** and **Point entities & models**.
Alt+click cycles overlapping object hits. **Bypass groups** selects individual members.

## Area selection

1. Drag a selection rectangle in a viewport.
2. Choose **Intersects** or **Fully inside**.
3. Choose **Replace**, **Add**, **Remove** or **Toggle**.
4. Review the highlighted result.

## Volume selection

1. Open **Select a volume…**.
2. Set **Minimum**, **Maximum**, **Match** and **Combine**.
3. Click **Preview matches**, then **Frame box**.
4. Inspect the geometry inside the box.
5. Click **Select matches**, or **Cancel**.

![A selection volume highlights geometry in part of the courtyard](images/selection-volume.png)

For terrain samples, retain the volume and use **Terrain triangles… → Select in volume**.

## Paint selection

1. Enable **Paint selection**.
2. Set **Radius**.
3. Paint over visible targets.
4. Click **Apply selection**, or cancel the draft.

## Temporary editor visibility

Use **Hide selected in editor** to remove targets from editor picking and display.
Use **Show all editor-hidden** to restore them.
This is separate from saved map visibility and [working regions](working-region.md).

## Limits

- Hidden and filtered targets are excluded.
- Loaded models use transformed bounds for area and volume matching; point picking uses mesh triangles.
- Terrain selection follows the displaced sheet, not its former parent volume.
- Overlap cycling retains up to 64 ordered hits.
- Asset reload or source changes can end active selection gestures; start a fresh review.
