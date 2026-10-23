[← Manual](README.md)

# Working region

Limit editor display and picking to a temporary box.

1. Open **Working region…**.
2. Set **Minimum** and **Maximum**, or click **Use selection bounds**.
3. Check **Enabled**, then click **Apply**.
4. Inspect the included objects and visibility reasons.
5. Click **Clear working region** to restore the full view.

## Limits

- The region filters complete objects; it does not cut geometry.
- Hidden objects remain hidden.
- Terrain and loaded models use their displayed geometry or checked bounds.
- This is not a compile scope. Use [Compile region…](compile-regions.md) for builds.
- The setting is temporary and does not delete map content.
