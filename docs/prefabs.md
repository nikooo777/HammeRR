[← Manual](README.md)

# Prefab library

Save and reuse brush/entity assemblies as VMF files.

## Save an assembly

1. Select whole objects.
2. Open **Prefabs…**.
3. Click **Save selection…**.
4. Enter **Prefab name**.
5. Click **Save prefab**.

## Browse previews

1. Enter a search in **Find a prefab**.
2. Choose **Flat** or **Textured**.
3. Click **Render page**.
4. Select a card.

Use **Previous** and **Next** for other pages.
**Refresh thumbnail** rereads the current preview.
Unavailable materials remain reported.

## Place an assembly

1. Select the prefab.
2. Set **Placement position · world units**.
3. Set **Rotation · world XYZ degrees**.
4. Set **Scale · positive XYZ factors**.
5. Choose **Source origin** when the VMF origin must align with the destination.
6. Review external links.
7. Click **Preview placement…**.
8. Inspect the shared paste preview.
9. Click **Apply paste**.

Cancel discards only the pending placement.
Use **Rename…** or **Delete prefab…** to manage the library.
Review the selected name before confirming deletion.

## Limits

- 128 VMFs, 8 MiB per file and 64 MiB total.
- Positive geometry scaling cannot resize point/model entities.
- Unsupported references follow the [clipboard rules](clipboard.md#references).
- Thumbnails show supported editor geometry, not compiled game lighting.
- A changed file requires a new preview before Apply.
