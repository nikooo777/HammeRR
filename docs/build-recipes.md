[← Manual](README.md)

# Build recipes

Save build settings for later use.

1. Open **Build settings…**.
2. Choose a **Build recipe**.
3. Click **Apply recipe**.
4. Review the settings, then click **Close**.
5. Click **Compile** or **Run Map**.

**Fast preview** favors quick checks. **Final lighting** selects a lit build.
Both leave generated reflections disabled.

## Save a custom recipe

1. Set the desired build values.
2. Enter **Recipe name**.
3. Click **Save current as new**.

Use **Replace with current** to update the selected custom recipe.
Use **Rename recipe** or **Delete recipe** to manage it.
**Reload recipes** reads changes saved by another editor.

## Limits

- Up to 32 custom recipes; names are 1–80 UTF-8 bytes.
- Recipes store build values, not map content, machine runtime paths or compile regions.
- Applying a recipe does not start a build or change a running job.
- Stale or unreadable storage rejects writes until reloaded or repaired.
