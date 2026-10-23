[← Manual](README.md)

# Merge brushes

Join adjacent or overlapping brushes into one convex solid.

1. Select the source brushes.
2. Open **Merge brushes…**.
3. Check the brushes under **Source brushes**.
4. Click **Preview merge**.
5. Inspect the outer shape and face count.
6. Click **Apply merge**.

**Discard preview** retains the source choices.
**Cancel** closes without applying. One Undo restores all sources.

## Limits

- 2–16 brushes, with at most 64 faces per source and result.
- Sources must share their world or brush-entity owner.
- The complete filled volume must be convex. Gaps, cavities and concave unions reject.
- Terrain is unsupported.
- Outer faces retain their source settings. Coincident planes use the lowest brush ID, then face ID.
- The lowest source brush ID survives. Internal faces disappear.
