[← Manual](README.md)

# Prefab library

Save brush and entity assemblies for reuse across maps. Prefabs are portable VMF
files; placement uses the same checked preview, naming and Undo behavior as Paste.

## Save an assembly

1. Set **Selection** to **Objects**, then select the brushes and entities to keep.
2. Open **Prefabs…** in the inspector and choose **Save selection…**.
3. Enter a **Prefab name** and click **Save prefab**. The library creates a new
   VMF file without changing the map, selection, clipboard or map Undo. Existing
   names are never overwritten. **Cancel** or Escape discards the form.
4. Use **Find a prefab** to search names. **Previous** and **Next** page results;
   **Refresh library** reads changes made outside the editor.

![Prefab search, assembly details and placement controls](images/prefab-library.png)

## Inspect an assembly's shape

1. Choose an entry in **Prefab library**. Counts and source diagnostics are
   available immediately, including when graphics are unavailable.
2. Click **Render thumbnail**. **Flat geometry · 320 × 240** shows the actual
   brush, terrain and model shape with the normal editor helpers for point
   entities. The isolated preview includes authored hidden objects and omits
   textures and sky. It does not change the map, selection, clipboard, camera,
   active placement review or Undo.
3. Click **Refresh thumbnail** to regenerate pixels. **Refresh library** checks
   external VMF edits; changed, removed or invalid sources clear the old image
   and show a diagnostic. Missing models and referenced instances produce an
   explicit error without a substitute image.

![Isolated flat geometry thumbnail beside prefab search and placement](images/prefab-thumbnail.png)

## Place an assembly

1. Choose a prefab to inspect its object, group, layer and face counts, size,
   source center and outgoing-link count.
2. Enter X, Y and Z under **Placement position · world units**. With the default
   **Source pivot** of **Bounds center**, this moves the original authored bounds
   center to that position.
3. Enter **Rotation · world XYZ degrees** and **Scale · positive XYZ factors**.
   Scale applies first, then rotation about the source pivot. Defaults of zero
   degrees and factors of one retain centered placement. Nonidentity scale
   requires an assembly containing only brushes or brush entities.
4. Choose **Source origin** to place the source VMF's origin at the destination,
   or **Custom** and enter source-world X, Y and Z. The chosen source pivot moves
   to **Placement position**; it remains fixed while scaling and rotating.
5. Enable **Keep external links by name** if outgoing links should connect to
   named objects in the destination map. Otherwise those links prevent preview.
6. Click **Preview placement…**. The shared **Paste objects** window shows the
   placement, generated names and exact external targets/matches. Review the map
   and this information before choosing **Apply paste**.
7. **Cancel**, Escape or closing Paste discards that owned placement. **Apply
   paste** records one Undo step and selects the pasted objects.

![Rotation, scale and a custom source pivot](images/prefab-transformed-placement.png)

![Shared Paste review for the transformed assembly](images/prefab-transformed-paste.png)

Textures remain attached through scale, rotation and placement. Point-entity
origins and supported Source orientation fields rotate together; models retain
their existing authored scale. The detached clipboard contains the transformed
assembly. Paste's **Offset** adds a world translation to that prepared geometry;
return to **Prefabs…** to change its rotation, scale or source pivot.

Required native groups and layers receive fresh IDs, supported internal links
follow new unique names, and object notes remain attached. The prefab file and
source map are unchanged. An already loaded preview remains usable if its library
file is subsequently deleted. Renaming or replacing its source file marks the
review stale: choose the current library entry and preview again before applying.

## Rename a library entry

1. Choose an entry and click **Rename…**.
2. Edit **Prefab name**. The existing name is prefilled; letters, numbers, spaces,
   dashes and underscores use the same limits as Save. Case-only changes are
   allowed. An existing name, including a case variant, cannot be overwritten.
3. Click **Rename prefab**. **Cancel**, Escape or closing the window keeps the
   original filename. Refreshing the library cannot renew an already open form;
   after a stale-state error, cancel and start **Rename…** again.
4. Choose the renamed entry and use **Preview placement…** to replace any older
   Paste review from that entry.

![Rename form with its original entry and editable prefab name](images/prefab-rename.png)

Rename changes only the library filename. VMF bytes, object/face/group/layer IDs
and external asset references stay intact. Map geometry, selection, clipboard
payload and Undo remain unchanged. Library changes are separate from map Undo.

## Delete a library entry

1. Choose an entry and click **Delete prefab…**.
2. Review its name, then click **Delete prefab**. **Cancel** keeps the file.

Deletion removes only that library VMF. Placed map copies remain. Library file
operations are separate from map Undo.

## Limits

- Up to 128 VMFs, 8 MiB per file and 64 MiB total; directory scans stop at 512
  entries. Names use 1–80 UTF-8 bytes of letters, numbers, spaces, dashes or
  underscores, without surrounding whitespace. Symlink roots/entries are refused.
- Storage defaults to `prefabs/` within the editor's recovery/state folder,
  normally `~/.local/state/hammerr/recovery/prefabs`. `HAMMERR_PREFABS_DIR` selects
  another absolute folder. The window reports the active folder. Independent
  editor instances and external writers are not coordinated; changed file
  metadata invalidates older actions. Refresh after edits made outside the editor.
- Capture and placement retain [clipboard limits](clipboard.md#limits), including
  16 top-level sources, checked references, metadata budgets and coordinate bounds.
  Component-only copies, saved partition intents and private Hammer formats remain
  unsupported. Source world settings, map/region notes and cameras are excluded.
- Rotation and source-pivot values must be finite and within ±65536. Scale factors
  must be finite and strictly positive. Prepared geometry, final geometry and the
  world placement offset must fit the editor's ±65536 coordinate bounds.
- Nonidentity scale rejects point/model entities. Rotation/scale reject opaque
  entity blocks and unknown brush, face or displacement fields/blocks, plus
  unsupported spatial entity properties such as `movedir`, FGD vectors/axes and
  ambiguous vector-like custom values. Brush-entity orientation fields are also
  refused. Ordinary centered translation preserves the existing clipboard
  metadata policy. Invalid placement retains the previous clipboard and preview.
- Assets remain external references. Mount the required game/custom content;
  the library does not package textures or models.
- Thumbnails render one selected entry on request and cache at most eight
  320 × 240 images. Every retrieval rechecks exact source bytes, file identity
  and content/FGD settings. Limits are 2 MiB source text, 128 brushes, 1024 sides,
  64 sides per brush, 65536 displacement vertices, eight model paths, sixteen
  model instances, 64 material bindings, 200000 vertices and 600000 indices.
  Larger valid assemblies can still use ordinary placement. Use **Model details…**
  → **Reload models** after repairing external model files. Graphics
  failures retain CPU inspection and can be retried with **Render thumbnail**.
- Rename requires Linux and a filesystem supporting atomic no-replace rename;
  unsupported operations fail without a copy/delete fallback. Independent writers
  can still race source-file changes after snapshot validation; existing targets
  are never overwritten. Preview source identity/size/mtime changes are checked
  before Apply, including changes made by another Session.
- Thumbnail grids, textured thumbnails, direct replacement from selection,
  local-frame placement, referenced instances and linked editing are unavailable.
  Applied copies can be transformed independently through the normal controls.
