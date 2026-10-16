[← Manual](README.md)

# Build recipes

Build recipes recall a complete set of native compiler settings. Use the built-in recipes for geometry or lighting checks, or save named combinations for a project. The same recipes are available to agents through MCP.

## Choose a recipe

1. Click **Build settings…** in the toolbar. After a build, **Build details…** opens the same window.
2. Choose **Fast preview**, **Final lighting**, or a saved recipe under **Build recipe**. The text below shows the selected recipe's settings. Choosing an entry leaves the current settings unchanged.
3. Click **Apply recipe**. **Current recipe** identifies the applied recipe; the individual controls below show its settings.
4. Click **Close**, then **Compile** or **Run Map**. Applying a recipe does not start a build.

| Recipe | Lighting | Reflections | Physics | Visibility | Sampling | Indirect light | Brush smoothing |
| --- | --- | --- | --- | --- | --- | --- | --- |
| **Fast preview** | Fullbright | Off | No physics bodies | Fast (conservative) | Fast (32 directions) | Direct only | 0° |
| **Final lighting** | Direct LDR | Off | CS:S engine physics | Detailed (portal clipping) | Detailed (512 directions) | Four diffuse bounces | 45° |

**Final lighting** uses the supported detailed settings of the experimental compiler. It requires the installed CS:S collision library and the `hammerr-physics` helper. Adjust **Physics bodies** to **No physics bodies** and save a custom recipe when collision bodies are unnecessary. See [Compile and play](compile-and-play.md) for the compiler's current capabilities.

![Named build recipes and their current settings](images/build-recipes.png)

## Save or change a custom recipe

1. Set **Build lighting**, **Generate reflections**, **Physics bodies**, **Visibility quality**, **Indirect light**, **Brush smoothing angle**, and **Sampling quality** as needed. Reflection generation requires **Direct LDR** or **Direct LDR + HDR**; the lighting mode chooses LDR alone or both channels. A changed value clears the applied recipe label to **Custom settings**.
2. Enter a unique **Recipe name**, then click **Save current as new**. The new recipe stores all seven current values. Capture runtime paths remain separate machine settings; see [Generated reflections](reflections.md).
3. To update an existing custom recipe, choose it under **Build recipe**, adjust the individual settings, and click **Replace with current**. **Recipe name** also becomes the replacement's name.
4. To change only its name, choose the recipe, edit **Recipe name**, then click **Rename recipe**. To remove it, click **Delete recipe**; current compiler settings remain available.
5. Click **Cancel recipe edit** to discard the pending name and selection edits. Settings already applied or changed with the individual controls remain in effect.

Custom recipes save on this computer separately from VMF files and map undo. The status text reports session-only storage or storage failures. Restart to reload recipes after another editor changes their storage file. A failed write keeps the previous registry and file intact; correct a write error and retry. Writes preserve existing file permissions and follow valid file symlinks. A load error blocks custom changes until storage is corrected and the editor restarts. Built-in recipes remain available.

Both built-in recipes disable reflection generation. Older custom recipes that lack this setting also load with generation disabled.

## Builds and cancellation

1. Open **Build details…** while compiling to inspect progress and **Recipe at build start**.
2. Click **Cancel build** to stop the active job. Wait for it to finish cancelling before changing recipes or settings.
3. Click **Play built map** to launch a successful current build without compiling again.

Builds freeze their settings and recipe identity at start. Recipe changes are blocked while a build runs or cancels. Renaming or deleting a recipe afterward preserves the completed job's captured name. Different compiler values make previous output stale; restoring the same values makes that output current again when the map snapshot still matches. Recipe names alone do not invalidate output.

## Limits

- Built-in recipes cannot be replaced, renamed, or deleted. Keep at most **32** custom recipes, with unique names ignoring case and **1–80 UTF-8 bytes** per name. Surrounding whitespace is trimmed; control characters are rejected.
- Recipe files are versioned JSON bounded to **65,536 bytes**. `HAMMERR_BUILD_RECIPES` selects an alternative file; otherwise recipes use `build-recipes.json` in the recovery storage directory. `HAMMERR_RECOVERY_DIR` therefore also isolates default recipe storage.
- Custom recipes store supported native compiler settings. Geometry and output publication remain required. Lighting can be disabled with **Fullbright**, physics with **No physics bodies**. Arbitrary executables, shell commands, extra compiler stages, stage reordering, and output-path recipes are unavailable.
- Loading recipes does not apply one automatically. New sessions begin with **Fast preview** settings. A recipe name is current only while its applied values remain unchanged.
- **Final lighting** retains Direct LDR and the compiler's existing lighting, visibility, collision and budget limits. Custom recipes may retain **Direct LDR + HDR**; baked prop lightmaps and broader engine support remain unavailable.
- Cross-process changes detected before a write are rejected; simultaneous writes are not coordinated with a shared lock. Recipe storage is local to this computer and is not embedded in the map.
