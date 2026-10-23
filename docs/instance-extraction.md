[← Manual](README.md)

# Extract an assembly

Export selected brushes and replace them with one child reference.

1. Save the parent map and create the destination directory.
2. Select eligible world brushes or complete ordinary `func_detail` owners in **Objects** mode.
3. Open **Extract selection to instance…**.
4. Enter a new relative VMF path.
5. Click **Prepare extraction**.
6. Review the source IDs, geometry counts and child file details.
7. Click **Extract**.
8. Save the changed parent map.

Use **Instance contents… → Refresh instance contents** to inspect the new reference.
**Cancel** releases the owned preparation. Cleanup errors remain visible.

## Limits

- The destination must be new, inside the saved root tree, with an explicit `.vmf` suffix.
- Existing files and symlinks reject. Extraction does not create directories or overwrite children.
- The pivot is **World origin (0, 0, 0)**.
- Selection order must preserve retained compiler brush and plane order.
- Terrain, runtime entities, existing references and unsupported metadata or inbound face bindings reject.
- Source, selection or path changes stale the preparation.
- Undo restores parent objects but does not delete the published child file.
