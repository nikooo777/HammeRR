[← Manual](README.md)

# Merge brushes

Merge joins adjacent or overlapping brushes into one ordinary convex brush.
The complete combined volume must already be convex: gaps and concave outlines
are rejected before the map changes.

1. Select two or more brushes, or select a brush entity.
2. Click **Merge brushes…** in the inspector.
3. Under **Source brushes**, check the brushes to join. At least two must remain checked.
4. Click **Preview merge**. The viewport shows the single result, and the summary lists its brush ID and outer face count.
5. Click **Apply merge** to replace the source brushes. One **Undo** restores them all.
6. Use **Discard preview** to keep choosing the same source candidates, including captured child subsets of large brush entities, or **Cancel** to close without applying. Escape also cancels.

![Merge brush sources, preview summary and Apply controls](images/brush-merging.png)

The lowest source brush ID is retained, together with its brush and editor
metadata. Each outer face keeps its source face ID, material, world texture
projection, lighting, smoothing and custom fields. Where several source faces
share one plane, the lowest brush ID wins, then the lowest face ID. Internal
faces disappear; patterns on those internal faces are not retained.

## Limits

- Choose 2–16 distinct convex brushes with at most 64 faces per brush. The result must also have at most 64 faces.
- World brushes merge together. Children of one brush entity merge while retaining their owner and unrelated siblings. Brushes with different owners cannot merge.
- Brushes containing terrain/displacements are unsupported.
- A nonconvex shape, disconnected brushes, edge/corner contact without filled volume, or an interior cavity rejects. Overlapping and fully contained brushes are supported when their union is convex.
- Geometry follows the editor's component precision and coordinate limits. Coverage checks use the same plane tolerance as carving and stop at 256 intermediate fragments; overly complex or thin cases may reject.
- Changing selection, editing sources or starting another geometry preview closes the current draft. Camera movement keeps it.
