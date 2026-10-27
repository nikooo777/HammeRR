[← Manual](README.md)

# Compile regions

Build a sealed part of a map without deleting other source objects.

1. Open **Compile region…**.
2. Enter **Minimum** and **Maximum** coordinates.
3. Or click **Use selection / map bounds**.
4. Enable **Enable region for builds**.
5. Click **Preview region**, then **Frame region**.
6. Inspect the box and inclusion summary.
7. Click **Apply region**.
8. Compile and inspect **Compile scope at build start** in **Build details…**.

**Cancel** discards the preview. Region bounds are separate from the editor's [working region](working-region.md).

## Return to full-map builds

Clear **Enable region for builds**, preview and apply the change.
Use **Preview clear** to remove stored bounds entirely.

## Include terrain and liquids

Keep complete terrain parents and structural-world water or slime volumes inside the region.
Include ordinary geometry that seals the resulting playable area.
Clipped or incomplete required surfaces fail explicitly.

## Include overlays and visibility helpers

Retain complete overlay projections, all required target surfaces and supported visibility helpers.
A marker position alone does not establish that the helper is included safely.

## Limits

- Builds capture the committed region at job start.
- Drafts, source changes and incompatible bindings require another review.
- Region builds retain source data outside the box.
- Missing retained dependencies, unsupported intersections or leaks stop publication.
