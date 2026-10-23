[← Manual](README.md)

# Cut, copy and paste

Transfer a detached set of objects within or between maps.

1. Choose **Objects** under **Selection**.
2. Select the objects.
3. Click **Copy** or **Cut**.
4. Open the destination map if required.
5. Click **Paste…**.
6. Check **Destination** and the object counts.
7. Choose **In place** or **Offset** under **Placement**.
8. Set **Keep textures attached** as required.
9. Review external links.
10. Click **Preview paste**.
11. Inspect the result.
12. Click **Apply paste**.

Viewport shortcuts are Ctrl+C, Ctrl+X and Ctrl+V.
Text fields retain their normal text shortcuts.
Cancel discards the paste preview; a completed Cut remains a separate Undo step.

## References

Supported internal references follow the new unique names.
External references require **Keep external links by name**.
For an unchanged same-map Cut, **Restore cut names** can reconnect incoming references.
An intervening edit, Undo or definition reload invalidates that receipt.

## Limits

- System clipboard transfer uses UTF-8 VMF text. Private Hammer formats are unsupported.
- Clipboard access can be unavailable on a headless display. The retained session payload remains explicit.
- Ambiguous, scripted and unsupported numeric references reject.
- Saved face partitions are outside clipboard transfer.
- A source edit does not change an already copied payload.
