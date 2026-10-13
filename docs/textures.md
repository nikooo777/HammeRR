[← Manual](README.md)

# Materials and textures

Most texture tools work on faces. Set **Selection** to **Faces · materials & textures**, then click faces (Ctrl+click toggles). In **Objects** mode, the material controls apply to whole brushes.

## Apply a material

Type a material name in the inspector or click **Browse materials…**, then click **Apply**.

### Material browser

The browser shows thumbnails in pages with a larger preview, search, and groups by folder and category (inferred from the material's path). It also lists the materials **Used in this map**, **Recently applied** and **Favorites**.

Click a thumbnail to preview it, then **Use material**. **Cancel** keeps the previous choice.

- Favorites are saved across sessions and shared with agents, up to 512.
- Recently applied materials last for the current session.

![Material thumbnails, categories and favorites](images/material-browser.png)

## Edit face textures

**Edit face textures…** controls offsets, scale and rotation, fitting one tile to the face, justification (left/center/right, top/center/bottom), and alignment (**Keep current axes**, **Align to world**, **Align to face**). Click **Preview textures**, then **Apply textures**.

- When the selected faces have different values, blank fields leave each face's value alone.
- Fitting uses the material's real size.

![Face material, offset, scale, rotation and preview controls](images/face-textures.png)

## Sample and reuse a texture

1. Select one face and click **Sample face texture**.
2. Select the target faces and open **Edit face textures…**.
3. Choose what to do with the sample:
   - **Paint sampled material**: apply only the material.
   - **Copy sampled projection**: apply the material with its texture axes, offsets, scale and rotation.
   - **Wrap across a shared edge**: continue the texture seamlessly onto faces that touch the source.
   - **Wrap connected faces**: carry the texture through a chain of selected faces, folding it at every shared edge, for example around a column.
4. Preview, then Apply.

The sample stays available until you sample again or open another map.

**Wrap connected faces** handles up to 256 target faces and leaves the source face unchanged. If targets are disconnected, or a closed loop can't wrap consistently, it explains why. Terrain is excluded.

| Wrapping controls | Texture continued around a faceted column |
| --- | --- |
| ![Connected face wrapping with preview and cancel](images/connected-wrapping.png) | ![Sampled texture carried across five shared edges](images/connected-wrapping-viewport.png) |

## Paint textures

Brush a sampled texture across faces.

1. Sample a face, then choose **Paint texture** in the viewport toolbar.
2. Choose **Material only** to keep each face's texture placement, or **Copy projection** to copy the sample's placement too.
3. Set the **Radius** and drag across visible faces. The toolbar shows how many faces will change.
4. Release, or click **Apply textures**, to apply. Escape or **Cancel** discards the stroke.

- The selection doesn't change.
- Each face is painted once per stroke. Hidden faces and point helpers are skipped.
- A projection that would collapse on a target face is rejected.
- Sampling a new face cancels an open stroke.

![Sampled wood floor painted across five stair treads](images/texture-strokes.png)

## Map one texture across several faces

**Map textures together…** treats a group of faces as one surface:

- **Fit faces together** uses one face's texture direction and fits the texture across the combined area of coplanar faces.
- **Project along current view** projects the texture straight from the camera, with no perspective distortion. Faces seen edge-on are rejected.

Choose a material and the tile counts: **1 × 1** fits one tile across the whole group, **2 × 1** repeats it twice horizontally, and fractions enlarge it. Click **Preview mapping**, then **Apply mapping**. Geometry and lighting settings don't change.

| Grouped mapping controls | One texture fitted across adjoining faces |
| --- | --- |
| ![Shared material, reference direction and tile counts](images/grouped-textures.png) | ![Continuous grouped texture mapping](images/grouped-textures-viewport.png) |

## Replace a material

1. Open **Replace materials…** and choose the material to find.
2. Search the **Whole map**, **Selected objects** or **Selected faces**.
3. **Find faces** reports the matches and how many hidden faces were skipped. Check **Include hidden faces** to include them. Matching uses the exact material path and ignores case.
4. Optionally click **Select matches**.
5. Choose the replacement, click **Preview replacement**, then **Replace faces**.

Texture placement, lighting settings and hidden flags are preserved. If the map or selection changes, search again before replacing.

![Material replacement scope, match counts, preview and cancel](images/material-replacement.png)

## Face lighting

Each face carries two lighting settings that the compiler uses.

Select faces and open **Face lighting…**:

- **Lightmap scale**: world units per lighting pixel. Presets are 8, 16, 32 and 64. Smaller values give sharper lighting and larger lightmaps. A blank field keeps each face's value.
- **Smoothing groups**: toggle groups one at a time, so other memberships stay intact across a mixed selection. Groups 1–24 allow smoothing; groups 25–32 prevent it. **Clear all groups** removes every membership; **Reset group changes** restores the current values.

Click **Preview settings**, then **Apply settings**.

To see the settings, use the **Lightmap density** and **Group membership** views (also available as [viewport modes](basics.md#viewport-modes)).

Direct LDR builds use each face's lightmap scale. Smoothing groups are kept, but the compiler doesn't smooth lighting across brush faces yet. Terrain is already lit with smooth normals.

| Face lighting controls | Lightmap density on brushes with different scales |
| --- | --- |
| ![Smoothing groups, scale presets and preview controls](images/face-lighting.png) | ![Lightmap density grids on brushes with different scales](images/face-lighting-density.png) |
