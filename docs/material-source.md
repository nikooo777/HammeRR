[← Manual](README.md)

# Material source

Read the material declaration and find its mounted files.

1. Open **Browse materials…**.
2. Select a material.
3. Click **Source…**.
4. Read **Resolved shader declaration** and the source files.
5. Inspect texture parameters and their mounted locations.
6. Click **Close** to return to the browser.

Patch materials include their resolved declaration and bounded include chain.
Water materials also report their capture dependencies and unsupported settings.
Use **Refresh source** to reread declarations. Use **Reload material assets** to refresh cached display images.

## Inspect image layers and alpha

1. Select a material in **Browse materials…**.
2. Choose **Layer 1 · Editor display** or **Layer 2 · $basetexture2** above the preview. The thumbnails use the same chosen layer.
3. Inspect the image, original dimensions, texture path and declaration identity.
4. Click **Alpha** below the preview to read the source alpha range.
5. Click **Color** to restore the RGBA preview.

Black is alpha 0; white is alpha 255. The alpha view is opaque.
It shows decoded image alpha, not shader opacity, animation or native blending.

Layer 2 uses the actual second image of a `WorldVertexTransition` material, including resolved Patch declarations. It has its own dimensions and sampled mip; it does not reuse the first image's dimensions. A missing declaration, unreadable image or unsupported material reports an explicit error and clears the old preview. The first image is never substituted.

Cached first/second images retain the texture paths and material declaration identity captured during the same load. **Source…** inspects current declarations independently; it does not relabel old pixels. Use **Reload material assets** after editing a VMT, Patch include or VTF.

## Limits

- This is a source inspector, not a complete shader preview.
- Image-layer inspection does not apply blend weights, texture transforms, proxies or shader-opacity effects.
- Content mount order decides which file wins. Identical virtual paths can refer to different files in different mounts.
- Missing, malformed or over-budget source shows an error; partial text is marked as truncated.
- Inspection does not rewrite materials or reload a running game's assets.
- Restart the editor after replacing a mounted VPK index; display reload does not remount archives.
