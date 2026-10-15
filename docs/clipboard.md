[← Manual](README.md)

# Cut, copy and paste

Transfer whole brushes and entities within a map or between maps. A captured clipboard owns its data independently of the source map. Paste previews the destination before adding objects in one Undo step.

1. Switch **Selection** to **Objects** and select the objects to transfer. Finish component editing first.
2. Click **Copy** to retain the originals, or **Cut** to remove them in one Undo step. These buttons sit beside **Paste…** in the inspector. With the viewport focused, **Ctrl+C** and **Ctrl+X** do the same. Text inputs retain their ordinary text shortcuts.
3. Open the destination map when transferring between maps. The captured objects remain available after source edits or map replacement.
4. Click **Paste…**, or press **Ctrl+V** with the viewport focused. The window reads system clipboard VMF text unless it is resuming a paste preview. Check **Destination** and the object, group and face counts. If the system clipboard cannot be imported, the window reports the error and shows the retained session payload.
5. Choose **Placement**: **In place** retains the source world coordinates; **Offset** enables the exact **X**, **Y** and **Z** world-unit offsets.
6. Use **Keep textures attached** to move texture projections with the geometry. Disable it to leave projections in world space.
7. Inspect the external-link list. **Keep external links by name** explicitly retains those outgoing references, including links with no current destination match. Otherwise external links prevent preview. Exact internal links follow the newly generated names.
8. For an unchanged same-map Cut, **Restore cut names** reconnects existing incoming links using the original names. Other pastes generate unique names. Any map edit, Undo, map replacement or entity-definition change disables this Cut-name receipt.
9. Click **Preview paste**. Inspect the map and the name/link summary, then click **Apply paste**. Changed settings require another preview. **Cancel**, Escape and closing the window discard only that owned preview. A completed Cut remains a separate Undo step.

![Paste placement, texture attachment and link handling](images/clipboard-paste.png)

**Read system clipboard** explicitly refreshes the captured payload from another application's VMF text. **Publish to system clipboard** exports the retained payload as VMF text without changing the map. Copy and Cut require successful system publication before replacing the session clipboard or deleting sources.

Agents use the same operations: `get_clipboard`, `copy_objects`, `cut_objects`, `import_clipboard`, `export_clipboard`, `preview_paste`, `apply_paste` and `cancel_paste`. Headless sessions can transfer exported text directly without a desktop clipboard.

Use the [Prefab library](prefabs.md) to keep named assemblies across editor
restarts and place them through the same Paste review.

## Limits

- Capture 1–16 top-level objects, with at most 256 objects including owned brushes, 8,192 sides and 256 native groups plus 256 required layers. A brush has at most 256 sides. Payload text is bounded to 8 MiB, 64 nesting levels and 131,072 KeyValues nodes. Authoring coordinates and the complete pasted geometry stay within ±65,536 units.
- Native group and layer ancestors, partial/multiple memberships, owned-brush links, ordinary/displacement geometry, object notes, authorship and observations transfer. Unknown object metadata remains verbatim; opaque embedded IDs or scripts are not interpreted. Captures use native file order.
- Direct layer links on physical group records, saved partition intents on copied brushes, overlays, numeric node/face references, ambiguous names or internal links, and named sets with script/dynamic-I/O dependencies reject. Resolve these before copying; failure preserves the last valid payload and preview.
- External links default to rejection. Keeping them by name may connect to matching destination objects. Generated names that would acquire existing destination or payload references reject. No external reference is silently deleted.
- Original-name restoration requires the exact unchanged same-map post-Cut revision and entity definitions. Imported text cannot create a Cut receipt. Cross-map pastes use unique names.
- Transfer uses whole objects and an exact world offset. Rotation, scaling and component-only clipboard edits are unavailable. Use ordinary transforms after applying.
- Whole VMF text imports its brushes, entities, required native groups/layers and attached object notes. Source world settings, cameras, map notes, regions and unrelated map metadata are excluded.
- Native clipboard access supports UTF-8 VMF text through available X11 or Wayland data-control support. Private Hammer/Hammer++ clipboard formats are unsupported. The editor retains clipboard ownership while it runs; a clipboard manager may retain it afterward.
