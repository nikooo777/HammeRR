[← Manual](README.md)

# Terrain

Turn world quad faces into flat terrain surfaces at three grid resolutions. Sculpt heights and paint two-material blends through shared previews. Imported terrain also displays and compiles, including saved grids marked as already subdivided.

## Create a terrain surface

1. Create or select an ordinary world brush with **Add brush** or **Draw brush**. Keep solid world brushes around your room to seal the map.
2. Set **Selection → Faces · materials & textures** and select one quad face on the brush.
3. In the face inspector, choose **Terrain grid**: **5 × 5 · power 2**, **9 × 9 · power 3** or **17 × 17 · power 4**.
4. Choose **Start corner**. **Corner 1** is the default; the four choices show world coordinates for a single selected face. The grid rows follow the next corner and columns follow the previous one. For multiple faces, the same corner index applies to each face.
5. Click **Create terrain**. The selected face becomes a flat terrain surface; its IDs, material, texture mapping and lighting settings stay fixed. **Undo** restores the ordinary brush.
6. To change its material, click **Browse materials…**, choose **Use material**, then **Apply**. **Cancel** leaves the material unchanged.
7. Open **Build settings…** and choose **Direct LDR** for baked light and **CS:S engine physics** for contact with the terrain, then **Close** and **Compile** or **Run Map**.

![A newly created terrain grid](images/terrain-creation.png)

Limits: 1–16 world quad faces per action, one per parent brush. Nonquad faces, brush entities and parents with existing terrain are rejected together; no partial conversion occurs. Terrain is a surface: its parent brush does not seal the map or become a solid collision box. Creation adds one undo step and retains face selection. Subdivision and holes remain planned. Use a visible ordinary solid material for native compilation.

## Change grid resolution

1. Select existing terrain faces and click **Edit terrain…**, then choose **Operation → Grid resolution**.
2. Choose **Grid resolution**: **5 × 5 · power 2**, **9 × 9 · power 3** or **17 × 17 · power 4**. The dialog shows the current grid size, or **mixed grid sizes** for a batch.
3. Click **Preview** and inspect the grid in the viewport. Finer grids keep existing height and blend samples; coarser grids can lose detail. The map and Undo history stay unchanged during preview.
4. Click **Apply** to keep the preview in one undo step, or **Cancel** to discard it. Changing the grid choice discards this dialog's previous preview. Reopen the dialog if the map or face selection changes.

![Terrain grid resolution controls](images/terrain-resolution.png)

![A refined terrain grid retaining its heights](images/terrain-refinement.png)

Limits: 1–16 terrain faces per action. Start corners, material, mapping, lighting and IDs stay fixed. Uniform triangle tags, unrestricted vertex masks and zero offset-normal rows are resized or retained. Subdivision, mixed triangle tags, restricted masks, nonzero offset normals, opaque grid blocks and malformed/nonfinite rows reject together; these need topology remapping. Coarsening samples the old grid rather than preserving every old detail. A stale dialog cannot apply or cancel another actor's preview.

## Sculpt heights

1. Select terrain faces and click **Edit terrain…**.
2. Choose **Operation**: **Raise / lower**, **Flatten**, **Smooth** or **Noise**.
3. Set **Brush center · world coordinates**, or click **Pick center in viewport…** and choose a point on the selected terrain. The dialog returns with that point filled in. **Escape** or **Cancel picking** restores the dialog without changing its fields. Terrain selection and **Frame · F** follow the displaced surface, including previews. Choose **Brush direction**: **Up / down · world Z** or **Along the face normal**. Set **Radius · world units** with the slider or exact numeric input.
4. For **Raise / lower**, set **Height change · negative lowers**. For **Flatten** or **Smooth**, set **Strength** from 0 to 1. Flatten moves toward the plane through the brush center; with world-Z direction, **Z** sets its target height. Smooth uses the original neighboring samples. For **Noise**, set **Noise amount · world units** and **Noise seed**.
5. Click **Preview** and inspect the shape. Each brush fades smoothly toward its edge. Click **Apply** to keep it, or **Cancel** to discard it. **Undo** restores the complete source grid.

![Terrain sculpting controls with viewport center picking](images/terrain-center-picker.png)

Limits: 1–16 terrain faces. Radius is 1–8192 world units; height/noise change is within ±4096. Positions must remain within ±65536. The same source and noise seed repeat exactly. Subdivision, opaque terrain blocks, malformed rows and nonzero offset-normal caches reject together. Start corner, material, texture mapping, IDs, alpha and authored flags/tags remain. Brushes use a world-space center rather than a live viewport stroke; use **Sew edges** to join matching full/half borders across grid sizes; sewing is not automatic.

## Paint a material blend

1. Assign a two-texture material through **Browse materials…**, **Use material**, then **Apply**. A single-texture material cannot show the alpha blend.
2. Select terrain faces, click **Edit terrain…**, then choose **Paint blend**, **Fill blend** or **Invert blend** under **Operation**.
3. For **Paint blend**, set the brush center, direction, radius and strength. Choose **Blend value · 0 first texture, 255 second texture** with the slider or numeric input. Painting moves toward that value and fades toward the brush edge.
4. **Fill blend** applies the chosen value to every vertex in the selected grids. **Invert blend** swaps weights across the complete grids.
5. Click **Preview**, then **Apply** or **Cancel**. Geometry, material, mapping and lighting settings stay fixed; **Undo** restores the original blend values.

![Terrain blend controls](images/terrain-blend-controls.png)

![Painted grass and dirt on terrain](images/terrain-blending.png)

Limits: 1–16 terrain faces, alpha 0–255 and strength 0–1. A material can remap how the alpha affects its textures. Use **Pick center in viewport…** or exact coordinates to place the bounded brush. Live viewport strokes remain planned. **Sew edges** can also average shared alpha. Secondary blend-material physics remains unfinished; current physics uses the primary material.

## Sew neighboring terrain

1. Select 2–16 terrain faces with matching full base edges or exact whole-to-half borders. Grid resolutions may differ.
2. Click **Edit terrain…**, then choose **Operation → Sew edges**.
3. Enable **Also sew blend values** to join alpha along those shared borders as well as positions.
4. Click **Preview**. Common border points average their original positions once, including junctions. Finer border points follow the sewn coarse edge; the preview reports their count. Interior vertices stay fixed.
5. Click **Apply** or **Cancel**. **Undo** restores all original grids.

![Terrain sewing controls](images/terrain-sewing-controls.png)

![Different terrain grids joined across full and half borders](images/terrain-sewn.png)

Limits: full or exact whole-to-half base edges at powers 2–4. Missing matches, ambiguous overlaps and unsupported subdivision/cache data reject atomically. Unmatched borders stay fixed. Sewing can remove fine detail along a border to make it match the coarser grid. Other fractional spans and subdivision authoring remain planned. Start corners, materials, mapping, IDs, triangle tags and collision settings remain fixed.

## Set collision behavior

1. Select terrain faces and click **Edit terrain… → Operation → Collision**.
2. Set **Physics collision** for crates and other dynamic objects, **Player collision** for player hulls, and **Ray collision** for traces and shots. Each label shows the committed state, including **mixed** for a batch.
3. Choose **Keep current**, **Enabled** or **Disabled** for each setting. **Keep current** preserves every face's own value.
4. Click **Preview**, then **Apply** or **Cancel**. The visible terrain geometry remains; only the chosen collision behavior changes. Use **CS:S engine physics** when compiling physical contact.

![Terrain collision controls](images/terrain-collision.png)

Limits: 1–16 faces; other flags, geometry, alpha and tags stay fixed. Unsupported imported flags still need compiler support. These controls affect complete surfaces; individual triangle tags/removal remain planned. Editor picking continues to use visible geometry, even when game collision is disabled.

## Remove terrain

1. Select the terrain faces under **Selection → Faces · materials & textures**.
2. Click **Remove terrain**. Removing the last terrain face restores the parent's solid brush volume; another terrain face on that parent keeps it as terrain.
3. Use **Undo** to restore the complete grid, including imported heights, alpha, tags and extra data.

Limits: every target must contain terrain, with 1–16 faces per action. Invalid or mixed ordinary/terrain targets reject together. IDs, brush planes, material, mapping and lighting remain. Imported brush-entity terrain can be removed to repair it, while its native compilation remains unsupported.

## Compile different grid resolutions together

1. Create adjacent terrain faces whose complete base edges match. Set their powers independently with **Terrain grid** during creation or **Edit terrain… → Operation → Grid resolution** afterward.
2. Keep the heights at shared coarse vertices aligned. Refining an existing grid preserves those samples; independently sculpted borders may need repair.
3. Open **Build settings…**, choose **Direct LDR** and **CS:S engine physics**, then **Close** and **Compile**.
4. Open **Build details…** and inspect **Mixed-power pairs**, **LOD disabled vertices** and **Snapped vertices**. Read **Diagnostics** for unmatched edges or reduced fine detail.

![Compiled terrain with adjacent grid resolutions](images/terrain-mixed-resolution.png)

Limits: powers 2–4, complete base edges with matching common heights, and bounded topology work. Compilation removes unsupported fine vertices and snaps their geometry onto the compatible surface. The editable grid remains unchanged, so the compiled surface can be less detailed near a coarse neighbor. Lighting and physics use the compiled geometry. Exact whole-to-half connections also compile; other partial spans, subdivision and holes remain planned. **Sew edges** repairs these source borders through a preview.

## Compile split borders

1. In **Top**, use **Draw brush** with one **Start** and **Depth** to create a larger parent beside two smaller parents. Each smaller border must cover exactly half of the larger border.
2. Select the top faces under **Selection → Faces · materials & textures** and use **Create terrain**. Choose each **Terrain grid** independently; their powers may differ.
3. Keep shared coarse heights aligned, then **Compile** with the lighting and physics settings above.
4. Open **Build details…** and check **Half-edge pairs**. A large border can link both smaller neighbors, or either half by itself. Overlapping candidates stay unlinked and appear in **Diagnostics**.

![Compiled terrain across a split border](images/terrain-half-edge.png)

Limits: exact full-to-half base edges with matching common samples. Masks and geometry snapping follow sample spacing within each covered interval. Shared junction samples stay aligned; other fractional spans, subdivision generation and holes remain planned. Imported subdivided grids compile their saved rows; source-editing tools still require cache remapping. Source grids stay unchanged.

## Connected lighting

Matching equal-power full edges share lighting normals in the viewport and compiled map. See [Compile and play](compile-and-play.md#terrain-displacements) for the settings and limits.
