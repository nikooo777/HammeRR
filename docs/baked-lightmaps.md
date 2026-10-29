[← Manual](README.md)

# Baked lightmaps

Inspect face-lighting samples from the last successful build.

1. Compile with **Direct LDR** or **Direct LDR + HDR**.
2. Open **Build details… → Inspect baked lightmaps…**.
3. Choose **Channel** and **Compiled face**.
4. Click **Inspect face**.
5. Choose **Style index** and **Map index**.
6. Inspect sample values and source contributors.
7. Click **Frame face** to fit the compiled world/detail, terrain or initial-pose brush-model face in the viewport.

Use **Inspect selected authored face** to find compiled matches for the selected source face.
Use page controls for long match and contributor lists.
Set **Rows per page** from 1 to 32 for face, match and contributor pages.
If contributor routes exceed the copy budget, lower it and click **Inspect face** again.
Click an external contributor to find matches for its exact captured occurrence.
Its details show each containing helper ID and relative source file, from root
to leaf. Repeated uses of the same leaf face have distinct routes.

The preview follows the native rectangular lightmap grid. Its grid coordinates
are not actual bake sampling points. Planar boundary samples can use covered
cell centroids before origin clearance; the BSP does not store those positions.
See [lightmap boundary sampling](compile-and-play.md#lightmap-boundary-sampling).

## Limits

- Physical paths that cannot be reported show stale provenance and omitted path fields.
- Reads that build's BSP; it does not estimate lighting from the editor view.
- Compiled face numbers belong to one build and can change after rebuilding.
- Stale intact output remains historical. Missing, replaced or corrupt output is unavailable.
- Captured routes stay with that build. They do not identify current expanded objects or open source files.
- Face-source storage caps at 8 MiB; contributor route copies cap at 512 KiB per page. A smaller contributor page can admit a refused route page.
- Channel, style and directional-map availability depend on the build.
- **Frame face** requires the current build. It uses the compiled face bounds and keeps the current projection.
- **Frame face** refuses invalid geometry and active viewport gestures. Finish or cancel the gesture, then try again.
- Multiple source contributors do not prevent framing a valid compiled face.
- Inspection keeps the camera unchanged. Framing keeps source, selection, drafts and history unchanged.
