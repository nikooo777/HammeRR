[← Manual](README.md)

# Selection

Choose objects, faces or bounded groups of visible geometry.

## Pick objects or faces

1. Choose **Selection → Objects** or **Faces · materials & textures**.
2. Click the target.
3. Use Ctrl+click to toggle membership.

Use **Pick object types** to allow **World brushes**, **Brush entities** and **Point entities & models**.
Alt+click cycles overlapping object hits. **Bypass groups** selects individual members.

## Save selection defaults

1. Choose the types under **Pick object types**.
2. Open **Selection defaults…**.
3. Compare **Current session** with **Startup defaults**.
4. Click **Save current as defaults** to use these filters for future documents.
5. Click **Apply saved defaults** to restore the saved filters in this document.

**Reload defaults** reads changes from disk without applying them.
**Reset saved defaults** saves all three types enabled; click **Apply saved defaults** to use them now.
If the dialog reports a storage error, repair the file and click **Reload defaults**,
or explicitly replace it with **Reset saved defaults**.

## Area selection

1. Choose **Intersects** or **Fully inside** under **Area**.
2. Choose **Replace**, **Add**, **Remove** or **Toggle**.
3. Drag a selection rectangle from empty viewport space.
4. Review the highlighted result while dragging.
5. Release to select the matches.

Escape cancels the drag. Changing the area controls discards any pending preview.

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
4. Release to apply the selection.

Escape cancels an active stroke.

## Temporary editor visibility

Use **Hide selected in editor** to remove targets from editor picking and display.
Use **Show all editor-hidden** to restore them.
This is separate from saved map visibility and [working regions](working-region.md).

## Limits

- Hidden targets are excluded. Type filters limit object-selection seeds; eligible hits still select complete visible groups. **Fully inside** requires every visible group member inside.
- Faces, components, geometric queries and explicit object-ID selection bypass type filters. Changing filters retains existing selection and ends pending object-selection gestures.
- Only the three object types are saved. Saving, reloading or resetting defaults leaves current filters unchanged; applying affects only this document. **New** and **Open** retain current filters in the same document session.
- Other running editors may require **Reload defaults** after saved defaults change. Storage errors disable Save and Apply until Reload or an explicit Reset succeeds.
- Loaded models use transformed bounds for area and volume matching; point picking uses mesh triangles.
- Terrain selection follows the displaced sheet, not its former parent volume.
- Overlap cycling retains up to 64 ordered hits.
- Asset reload or source changes can end active selection gestures; start a fresh review.
