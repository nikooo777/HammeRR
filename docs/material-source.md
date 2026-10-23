[← Manual](README.md)

# Material source

Read the material declaration and find its mounted files.

1. Open **Browse materials…**.
2. Select a material.
3. Click **Source…**.
4. Read **Resolved shader declaration** and the source files.
5. Inspect texture parameters and their mounted locations.
6. Click **Close** to return to the browser.

Patch materials include their resolved declaration and bounded include chain.
Water materials also report their capture dependencies and unsupported settings.
Use **Refresh source** to reread declarations. Use **Reload material assets** to refresh cached display images.

## Inspect image alpha

1. Select a material in **Browse materials…**.
2. Click **Alpha** below the preview.
3. Read the source alpha range and texture identity.
4. Click **Color** to restore the RGBA preview.

Black is alpha 0; white is alpha 255. The alpha view is opaque.
It shows decoded image alpha, not shader opacity, animation or native blending.

## Limits

- This is a source inspector, not a complete shader preview.
- Content mount order decides which file wins. Identical virtual paths can refer to different files in different mounts.
- Missing, malformed or over-budget source shows an error; partial text is marked as truncated.
- Inspection does not rewrite materials or reload a running game's assets.
- Restart the editor after replacing a mounted VPK index; display reload does not remount archives.
