[← Manual](README.md)

# Atlas-region texturing

Fit selected flat faces into a rectangular part of a material's base image.
Each face keeps its texture direction, rotation and mirroring.

1. Set **Selection** to **Faces · materials & textures** and select the faces.
2. Open **Edit face textures…**, then **Fit atlas region…**.
3. Enter **Material**, or use **Browse materials…**, then click
   **Inspect material**. Check the original image dimensions and thumbnail.
4. Enter **X**, **Y**, **Width** and **Height** in texture pixels. The top-left
   image edge is `0, 0`; X grows right and Y grows down. The outlined rectangle
   must have positive dimensions and stay inside the original image.
5. Click **Preview atlas fit** and inspect the faces in the viewport. Each
   face's projected bounds fit independently into the same rectangle.
6. Click **Apply atlas fit** to keep that reviewed result in one Undo step.
   Changing a field requires another preview. **Cancel**, Escape or closing
   the window discards the owned draft.

![Atlas image, pixel rectangle and reviewed fit controls](images/atlas-face-fit.png)

Limits:

- During document shutdown, new inspection, field edits, previews, Apply and
  material browsing reject. **Cancel** still closes the window without clearing
  another tool’s preview.
- Flat world/detail and brush-entity faces are supported. Terrain, collapsed
  projections and edge-on texture axes reject the entire request. Polygon
  corners can leave parts of the rectangle unused. Geometry, lightmap scale
  and smoothing groups remain unchanged.
- Materials must use a static, single base image with `LightmappedGeneric`
  or `UnlitGeneric` and an identity base transform. Tool/sky materials,
  proxies, fallback blocks, extra/detail layers, editor-image substitutions
  and animated/volume/cubemap/procedural/render-target textures are unsupported.
- The image is cached until **Reload material assets**. A changed material
  declaration requires reload before inspection; a reload requires another
  review before Apply. Unrelated preview image uploads do not invalidate a fit.
- Images are limited to 8192 pixels per axis, VTF reads to 32 MiB and VMT/Patch
  reads to 1 MiB each. Thumbnails are at most 256 pixels per axis and source
  decoding at most 16 megapixels. A fit accepts at most 256 distinct faces.
- Rectangle dragging, named hotspots, atlas packing, grouped continuity,
  padding and source-image editing are absent. Texture filtering can sample
  neighboring atlas regions; choose suitable gutters in the source image.
