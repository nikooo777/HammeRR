[← Manual](README.md)

# Map tools

Find objects, check map data and retain recovery copies.

## Find objects

1. Open **Object report…**.
2. Search IDs, names, classes or materials.
3. Filter by object type, visibility or bounds.
4. Select or frame the matching results.

**Hide selected**, **Isolate selected**, **Show selected**, **Show all** and **Restore visibility** control saved map visibility.
Use **Map statistics…** for object and geometry counts.

Use **Compare external map…** to [compare authored objects with a chosen VMF](map-comparison.md), then select or frame the complete matching current set.

## Check the map

1. Open **Check map…**.
2. Click **Run checks**.
3. Select a result.
4. Use **Select issue** or **Focus issue**.
5. Repair the source, then run checks again.

Use **Review repair… → Apply repair** for supported reviewed repairs, such as duplicate IDs.
Checks do not certify every compiler or engine behavior.

## Design notes

1. Open **Design context…**.
2. Choose **Map brief**, **Note on selection** or **New region**.
3. Enter **Summary** and **Intent, constraints and observations**. For a region, set **Minimum**, **Maximum** and **Guide color**.
4. Click **Save note**, or **Cancel edit** to discard the draft.
5. Use the note filters or search, then **Focus in map** to locate an attachment. **Show region guides** controls editor-only region outlines.

Each note allows 200 summary characters and 4 KiB of combined summary/body UTF-8 text. A map allows 1 MiB of note text and 256 regions. Region bounds must have positive extents within ±1,000,000 units.

Source changes mark notes as changed. Region notes observe the whole map, so a distant edit can change their status. Saving a note records the current source state; it does not retest the text.

Duplication copies notes and marks their observations changed. Deletion removes attached notes; **Undo** restores them. To remove one note, check **Remove this note**, then click **Delete note** or **Delete region**.

Notes use checked, segmented VMF world properties. Old HammeRR root-block notes migrate when opened and saved. Missing, altered or conflicting segments stop Open. Use the note editor to change notes; ordinary world-property editing hides and protects their storage.

> [!NOTE]
> Hammer++ retains these short world properties through its load/save format. Other editors must preserve all segments. Compile with HammeRR to exclude notes from the BSP; external compilers can emit them as world properties.

## Autosave and recovery

1. Open **Recovery…**.
2. Configure the available recovery settings, then click **Apply settings**.
3. Use **Save snapshot now** for an explicit copy.
4. Select a retained snapshot and click **Open recovered copy** to inspect it.

Use **Reload settings** after another editor changes recovery storage settings.

## Limits

- Recovery is separate from saving the source VMF.
- Missing, malformed or unwritable storage reports an error.
- A recovered copy does not silently overwrite the original map.
- Saved visibility, temporary editor hiding and working regions are separate controls.
- Source changes can stale checks or repairs; review again before applying them.

<!-- page-nav -->

---

[← Layers](layers.md) · [Manual](README.md) · [Extract an assembly →](instance-extraction.md)
