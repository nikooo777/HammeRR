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
3. Enter the relevant design context.
4. Click **Save note**, or **Cancel**.

Notes are map metadata for humans and MCP clients. They do not compile into gameplay entities.
The complete design-context payload is limited to 32 KiB.

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
