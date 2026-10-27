[← Manual](README.md)

# Extract an assembly

Export selected brushes to a child VMF and replace them with one reference while
keeping their placement and textures in the parent. A local pivot gives the child
assembly a useful origin for later movement and rotation.

1. Save the parent map and create the destination directory.
2. Select eligible world brushes or complete ordinary `func_detail` owners in **Objects** mode.
3. Open **Extract selection to instance…**.
4. Enter a new relative VMF path.
5. Choose **Pivot**: **World origin (0, 0, 0)** keeps the original child coordinates; **Selection centre** uses the center of the selected brush bounds; **Custom** uses the world-space **X**, **Y** and **Z** you enter.
6. Click **Prepare extraction**.
7. Review the pivot coordinates, source IDs, geometry counts and child file details.
8. Click **Extract**.
9. Save the changed parent map.

Use **Instance contents… → Refresh instance contents** to inspect the new reference.
**Cancel** releases the owned preparation. Cancel and prepare again to change a
reviewed pivot or destination. Cleanup errors remain visible.

![Transformed child references to one brush assembly](images/instance-assemblies.png)

## Limits

- The destination must be new, inside the saved root tree, with an explicit `.vmf` suffix.
- Existing files and symlinks reject. Extraction does not create directories or overwrite children.
- **World origin (0, 0, 0)** remains the default. **Custom** requires finite coordinates.
- The pivot and both local and expanded brushes must fit Source compile bounds.
- Geometry, texture axes/scales and metadata must restore exactly after serialization and expansion. Native texture and lightmap projections must be bit-identical; authored decimal shift values can differ through rounding when the compiled mapping stays unchanged. If preparation rejects changed projections or bounds, use another pivot or **World origin (0, 0, 0)**.
- Selection order must preserve retained compiler brush and plane order.
- Terrain, runtime entities, existing references and unsupported metadata or inbound face bindings reject.
- Source, selection or path changes stale the preparation.
- Undo restores parent objects but does not delete the published child file.
