[← Manual](README.md)

# Build recipes

Save build settings for later use.

1. Open **Build settings…**.
2. Choose a **Build recipe**.
3. Click **Apply recipe**.
4. Review the settings, then click **Close**.
5. Click **Compile** or **Run map**.

**Fast preview** favors quick checks. **Final lighting** selects a lit build.
Both leave generated reflections disabled.
Recipes retain **Brush texture shadows** and **Colored texture shadows**.
Older recipes leave colored transport off.
Recipes also retain **Postprocess order**. Older recipes use lighting before
physics.

## Choose postprocess order

1. Open **Build settings…**.
2. Set **Postprocess order** to **Lighting then physics** or **Physics then lighting**.
3. Read the effective plan and any skipped-stage reasons below it.
4. Click **Close**, then **Compile** or **Run map**.

The choice applies immediately to the next build. Physics first can report a
physics failure before spending time baking lighting. Geometry and visibility
always run first; packing, reflection capture and publication keep their fixed
order afterward. The selected build settings decide which stages run.

## Save a custom recipe

1. Set the desired build values, including **Postprocess order**.
2. Enter **Recipe name**.
3. Click **Save current as new**.

Use **Replace with current** to update the selected custom recipe.
Use **Rename recipe** or **Delete recipe** to manage it.
**Reload recipes** reads changes saved by another editor.

## Limits

- Up to 32 custom recipes; names are 1–80 UTF-8 bytes.
- Recipes store build values, not map content, machine runtime paths or compile regions.
- Applying a recipe does not start a build or change a running job.
- Active builds freeze settings and recipe edits. Build details retain the
  settings and postprocess plan captured when that job started.
- External commands and arbitrary compiler-stage ordering are unavailable.
- Stale or unreadable storage rejects writes until reloaded or repaired.

<!-- page-nav -->

---

[← Material emission](material-emission.md) · [Manual](README.md) · [Compile regions →](compile-regions.md)
