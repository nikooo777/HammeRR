[← Manual](README.md)

# Baked lightmaps

Inspect actual face-lighting samples from the last successful owned build.

1. Compile with **Direct LDR** or **Direct LDR + HDR**.
2. Open **Build details… → Inspect baked lightmaps…**.
3. Choose **Channel** and **Compiled face**.
4. Click **Inspect face**.
5. Choose **Style index** and **Map index**.
6. Inspect sample values and source contributors.

Use **Inspect selected authored face** to find compiled matches for the selected source face.
Use page controls for long match and contributor lists.

## Limits

- Reads the owned BSP; it does not estimate lighting from the editor view.
- Compiled face numbers belong to one build and can change after rebuilding.
- Stale intact output remains historical. Missing, replaced or corrupt output is unavailable.
- Channel, style and directional-map availability depend on the build.
- Inspection does not change source, selection, camera or history.
