[← Manual](README.md)

# Previewing authored fog

Inspect an existing `env_fog_controller` and preview its static initial fog settings without changing the map or its undo history.

1. Create an `env_fog_controller` with **Entity…**, or open a map containing one. Use its entity properties to author `fogcolor` as three RGB integers from 0 to 255, `fogstart`, `fogend`, `fogmaxdensity` and `fogenable`.
2. Choose **Perspective** and **Textured** in the viewport.
3. Click **Fog preview…** under **VIEWPORT**. The dialog shows the current preview state and all authored controllers, including unsupported choices and their diagnostics.
4. Check **Enable fog preview**, then choose an **Active controller**. Review its color, range, maximum density and authored enabled state. A controller with `fogenable=0` draws no fog.
5. Click **Apply preview** to apply the reviewed choice. **Cancel** closes the dialog without changing preview settings. If map or view settings changed while reviewing, click **Refresh choices**, review again, and apply.
6. To turn it off, reopen **Fog preview…**, clear **Enable fog preview**, and click **Apply preview**.

![Fog controller selection and authored settings](images/fog-preview.png)

MCP uses the same Session operations. `get_fog_preview` returns the current settings, controller choices, parsed values, diagnostics and a snapshot. `set_fog_preview` accepts `enabled`, an explicit `controller` such as `entity:42`, and that snapshot. `get_map_info.fog_preview` reports the shared state. `render_view` accepts temporary `fog_preview` and `fog_controller` overrides; its metadata identifies the controller and whether fog was applied in the requested projection and mode. Overrides leave the shared choice unchanged.

## Limits

The preview starts off and requires an explicit controller selection. It does not invent a controller or select one from Master flags. Controllers are resolved from the authored map, independently of the working region. Changing or undoing authored settings updates the preview; a deleted or invalid selected controller draws no fog and reports the issue. Opening another map clears the selection and disables preview.

The supported approximation uses forward view depth in Hammer units. Its factor is `clamp((depth − start) / (end − start), 0, maxdensity)`. The squared factor blends the existing surface color toward the sRGB-decoded fog color in linear space. This follows the public SDK's range-factor and pixel-blend shape; it does not establish native CS:S parity. Transparent surfaces retain their existing alpha and draw order.

`fogstart` and `fogend` must be explicit finite values with `0 ≤ start < end` and a finite inverse range. RGB must contain exactly three integer channels. Density must be finite and between 0 and 1; enabled flags accept only 0 or 1. Missing `fogenable` uses the native initial default 0, and missing `fogmaxdensity` uses 1. Missing blend/radial flags use 0. Invalid values, duplicate case-insensitive keys, `fogblend=1`, `fogradial=1`, positive `farz`, and HDR/zoom scales other than 1 are rejected visibly.

Only perspective Textured surfaces and model meshes receive fog. Sky surfaces, the viewport background, entity helpers, selection outlines and grid retain their previous appearance. Flat shading, wireframe, diagnostic and planar views retain their prior output. Fog volumes, water fog, runtime I/O, transitions, directional secondary colors, radial fog, far-Z clipping, HDR/exposure and zoom scaling are outside this preview. `foglerptime` is checked as a finite nonnegative value; only the initial state is shown.

The primary references are Valve's [authored controller keys](https://github.com/ValveSoftware/source-sdk-2013/blob/master/src/game/server/fogcontroller.cpp) and [range fog and pixel blending](https://github.com/ValveSoftware/source-sdk-2013/blob/master/src/materialsystem/stdshaders/common_ps_fxc.h). The model and renderer do not change compiled fog behavior.
