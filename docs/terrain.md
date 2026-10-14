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

Limits: 1–16 world quad faces per action, one per parent brush. Nonquad faces, brush entities and parents with existing terrain are rejected together; no partial conversion occurs. Terrain is a surface: its parent brush does not seal the map or become a solid collision box. Creation adds one undo step and retains face selection. Sculpting, blend painting, subdivision, holes and resolution changes on existing terrain remain planned. Use a visible ordinary solid material for native compilation.

## Remove terrain

1. Select the terrain faces under **Selection → Faces · materials & textures**.
2. Click **Remove terrain**. Removing the last terrain face restores the parent's solid brush volume; another terrain face on that parent keeps it as terrain.
3. Use **Undo** to restore the complete grid, including imported heights, alpha, tags and extra data.

Limits: every target must contain terrain, with 1–16 faces per action. Invalid or mixed ordinary/terrain targets reject together. IDs, brush planes, material, mapping and lighting remain. Imported brush-entity terrain can be removed to repair it, while its native compilation remains unsupported.

## Connected lighting

Matching equal-power full edges share lighting normals in the viewport and compiled map. See [Compile and play](compile-and-play.md#terrain-displacements) for the settings and limits.
