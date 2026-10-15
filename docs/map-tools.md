[← Manual](README.md)

# Organizing and checking maps

## Find objects

**Object report…** searches IDs, names, classes and materials. Filter by type (world brushes, point entities, brush entities…), by visibility, and optionally by an XYZ region.

Select or frame a single result, or all matches including those on later pages. Selecting can replace, add to, remove from or toggle the current selection.

## Hide and show

**Hide selected**, **Isolate selected**, **Show selected** and **Show all** change visibility. Visibility is saved in the map and undoable. **Restore visibility** returns to the previous state, including hidden brushes inside entities.

> [!IMPORTANT]
> Hidden objects are left out of compiled maps. Show them before building if they should be in the game.

![Object report with filters, selection modes and visibility actions](images/object-report.png)

[Layers…](layers.md) organizes objects into named, colored nested layers with scoped selection and the same map visibility controls.

## Map statistics

**Map statistics…** counts brush sides, world and entity brushes, point and brush entities, connections, materials, model references and terrain. Hidden objects are included, and the numbers update as the map changes.

Choose a breakdown to page through materials, entity classes, model paths, lightmap scales or terrain sizes, most frequent first.

These counts describe the editable map: brush sides rather than compiled faces, and model paths as written rather than verified files. Open previews aren't counted.

![Map totals and material usage](images/map-statistics.png)

## Check the map

1. Open **Check map…** and click **Run checks**.
2. Filter by severity and select an issue to read its explanation.
3. **Select issue** or **Focus issue** takes you to the problem.

The report marks itself out of date when the map or the entity definitions change.

Checks cover geometry, IDs, textures, entity logic, asset paths and player spawns. They include hidden objects and ignore open previews. Spawn checks warn when a visible spawn's standing player box overlaps solid world or `func_detail` brushes, including player clips, or a visible world or `func_detail` terrain sheet with **Player collision** enabled. Spawns that just touch the floor within 0.01 units are fine. Terrain parent volumes do not block; the check uses the authored surface triangles and identifies affected spawns and terrain faces.

Some issues can be repaired automatically. **Review repair…** shows the exact before and after values, and **Apply repair** fixes it in one undo step. This currently covers invalid texture projections and zero lightmap scales. Other issues link you to the content so you can fix it by hand.

Invalid material definitions, patch includes and display textures appear as
face-linked errors. Repair the content file and click **Run checks** again;
external asset edits do not automatically refresh a report.

Unknown terrain materials, unsupported flags or malformed geometry are reported as unchecked. Invalid terrain border-smoothing values appear as face issues; repair them through **Edit terrain… → Border smoothing**. Compiler warnings identify actual unsupported owners, flags or removed triangles.

Not checked yet: spawns blocked by models or other brush entities, removed terrain triangles, compiler LOD changes, runtime placement, gameplay objectives, full asset dependencies and engine limits. Compile and test in CS:S to find leaks and playability problems.

![Map issues with navigation and a repair preview](images/map-checks.png)

### Duplicate IDs

A VMF that contains duplicate object or face IDs is refused when opened or recovered. The error names the duplicate ID, and the currently open map is left untouched. HammeRR never renumbers imported objects silently, so fix the IDs in the file first.

## Design notes

**Design context…** keeps notes about intent, constraints and observations next to the geometry they describe. Agents read and write the same notes.

- **Map brief**: notes about the whole map.
- **Note on selection**: a note attached to objects.
- **New region**: a note attached to a box in the map, shown with a colored guide.

Search the notes, or find those relevant to the selection, a point or an area. Each note shows who wrote it and whether its objects changed since. **Save note** records one undo step.

- Copied objects keep their notes, marked for review. Deleting objects removes their notes, and undo restores both.
- Notes are editor-only and never compiled.
- Limits: 200-character summaries, 4 KiB per note, 1 MiB per map.
- Region change tracking currently covers the whole map. Other editors' handling of these notes hasn't been tested.

![Design notes, region bounds and edit controls](images/design-context.png)

## Autosave and recovery

**Recovery…** lists automatic snapshots, manual checkpoints and the versions replaced by each save.

- Autosave captures changed maps every two minutes by default, including unnamed maps. It never touches your VMF, its unsaved state or the undo history.
- Change the interval and how many snapshots to keep, or create a snapshot immediately.

To recover, select a snapshot and click **Open recovered copy**. It opens as an unsaved map; save it to keep it. Replacing unsaved work and deleting snapshots ask for confirmation first.

Recovery files live in `$XDG_STATE_HOME/hammerr/recovery`, or `~/.local/state/hammerr/recovery` if that variable isn't set. Set `HAMMERR_RECOVERY_DIR` to an absolute path to use another folder. Turning autosave off keeps manual snapshots and save backups. Failed snapshots are reported in Recovery and in Activity.

![Autosave settings, snapshots and recovery controls](images/recovery.png)
