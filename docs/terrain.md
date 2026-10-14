[← Manual](README.md)

# Terrain

Turn world quad faces into flat terrain surfaces at three grid resolutions. Imported terrain also displays and compiles; sculpting tools are still planned.

## Create a terrain surface

1. Create or select an ordinary world brush with **Add brush** or **Draw brush**. Keep solid world brushes around your room to seal the map.
2. Set **Selection → Faces · materials & textures** and select one quad face on the brush.
3. In the face inspector, choose **Terrain grid**: **5 × 5 · power 2**, **9 × 9 · power 3** or **17 × 17 · power 4**.
4. Choose **Start corner**. **Corner 1** is the default; the four choices show world coordinates for a single selected face. The grid rows follow the next corner and columns follow the previous one. For multiple faces, the same corner index applies to each face.
5. Click **Create terrain**. The selected face becomes a flat terrain surface; its IDs, material, texture mapping and lighting settings stay fixed. **Undo** restores the ordinary brush.
6. To change its material, click **Browse materials…**, choose **Use material**, then **Apply**. **Cancel** leaves the material unchanged.
7. Open **Build settings…** and choose **Direct LDR** for baked light and **CS:S engine physics** for contact with the terrain, then **Close** and **Compile** or **Run Map**.

![A newly created terrain grid](images/terrain-creation.png)

Limits: 1–16 world quad faces per action, one per parent brush. Nonquad faces, brush entities and parents with existing terrain are rejected together; no partial conversion occurs. Terrain is a surface: its parent brush does not seal the map or become a solid collision box. Creation adds one undo step and retains face selection. Sculpting, blend painting, subdivision and holes remain planned. Use a visible ordinary solid material for native compilation.

## Change grid resolution

1. Select existing terrain faces and click **Change resolution…**.
2. Choose **Grid resolution**: **5 × 5 · power 2**, **9 × 9 · power 3** or **17 × 17 · power 4**. The dialog shows the current grid size, or **mixed grid sizes** for a batch.
3. Click **Preview** and inspect the grid in the viewport. Finer grids keep existing height and blend samples; coarser grids can lose detail. The map and Undo history stay unchanged during preview.
4. Click **Apply** to keep the preview in one undo step, or **Cancel** to discard it. Changing the grid choice discards this dialog's previous preview. Reopen the dialog if the map or face selection changes.

![Terrain grid resolution controls](images/terrain-resolution.png)

![A refined terrain grid retaining its heights](images/terrain-refinement.png)

Limits: 1–16 terrain faces per action. Start corners, material, mapping, lighting and IDs stay fixed. Uniform triangle tags, unrestricted vertex masks and zero offset-normal rows are resized or retained. Subdivision, mixed triangle tags, restricted masks, nonzero offset normals, opaque grid blocks and malformed/nonfinite rows reject together; these need topology remapping. Coarsening samples the old grid rather than preserving every old detail. A stale dialog cannot apply or cancel another actor's preview.

## Remove terrain

1. Select the terrain faces under **Selection → Faces · materials & textures**.
2. Click **Remove terrain**. Removing the last terrain face restores the parent's solid brush volume; another terrain face on that parent keeps it as terrain.
3. Use **Undo** to restore the complete grid, including imported heights, alpha, tags and extra data.

Limits: every target must contain terrain, with 1–16 faces per action. Invalid or mixed ordinary/terrain targets reject together. IDs, brush planes, material, mapping and lighting remain. Imported brush-entity terrain can be removed to repair it, while its native compilation remains unsupported.

## Connected lighting

Matching equal-power full edges share lighting normals in the viewport and compiled map. See [Compile and play](compile-and-play.md#terrain-displacements) for the settings and limits.
