[← Manual](README.md)

# Instance references

Place or edit a reference to a saved child VMF.

1. Save the parent map.
2. Put the child VMF inside the parent's directory tree.
3. Click **Instance reference…**.
4. Enter **VMF reference**, or use **Browse…**.
5. Set **Position** and **Rotation**.
6. Optionally click **Check file**.
7. Click **Create**.

For an existing helper, use **Edit instance reference…**, then **Apply**.
Use **Instance contents… → Refresh instance contents** to resolve and inspect the child.
The file check confirms presence only; it does not validate child contents.

## Limits

- Paths must end in `.vmf` and stay inside the saved root parent's tree.
- Absolute paths, traversal, child symlinks and self-reference aliases reject.
- Nested references resolve from the saved root directory, not each child's directory.
- Position must fit ±16384; rotation must be finite at native precision.
- The form edits file, origin and angles. Unsupported imported fields can block Apply.
- No automatic file copying, path search, extension inference or child-content editing.
- Source changes stale the form. **Cancel** discards only the owned form.

See [compiled assemblies](instances.md) for supported content and name fixups.
