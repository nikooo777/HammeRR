[← Manual](README.md)

# Atlas-region texturing

Fit flat faces into a rectangular part of a material image.

1. Select faces under **Selection → Faces · materials & textures**.
2. Open **Edit face textures…**, then **Fit atlas region…**.
3. Enter **Material**, then click **Inspect material**.
4. Enter **X**, **Y**, **Width** and **Height** in texture pixels.
5. Click **Preview atlas fit**.
6. Inspect the faces, then click **Apply atlas fit**.

The top-left image edge is `0, 0`. Each face fits independently and keeps its texture direction and mirroring.
**Cancel** discards the preview. Changed fields require another preview.

## Limits

- Flat world, detail and brush-entity faces only; terrain and collapsed projections reject.
- One static base image with a supported `LightmappedGeneric` or `UnlitGeneric` declaration.
- No tool overrides, proxies, extra layers, packing, rectangle dragging or image editing.
- The rectangle must have positive size and stay inside the original image.
- Up to 256 faces. Choose source-image gutters to prevent filtering across adjacent regions.
