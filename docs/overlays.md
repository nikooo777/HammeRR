[← Manual](README.md)

# Overlays

Project a material onto compiled surfaces. The editor shows the projection outline.

## Create a fitted overlay

1. Select one visible world or `func_detail` quad in face mode.
2. Open **Overlay…**.
3. Choose **Material** and optional **Name**.
4. Set **Render order**, texture ranges and fade distances.
5. Click **Fit / Preview**.
6. Inspect the outline, then click **Apply overlay**.

**Cancel** discards the owned preview. Compile and play to inspect the material projection.

## Refit or rebind

Use **Refit selected overlay** to fit an existing eligible overlay to its current face.
Click **Apply refit** after reviewing the new outline.

To change its target:

1. Select a destination quad.
2. Open **Overlay…** and choose an existing overlay from the rebind list.
3. Click **Preview rebind**.
4. Inspect the old and new bindings.
5. Click **Apply rebind**.

Refit and rebind preserve the existing overlay identity, settings and connections.

## Edit projection corners

1. Select one eligible `info_overlay`.
2. Open **Overlay… → Edit selected corners**.
3. Edit **Corner 0** through **Corner 3** in the displayed local U/V frame.
4. Click **Preview corners**, then **Apply corners**.

Use **Viewport handles** to drag supported corners directly.
**Cancel handles** discards that interaction. Crossed or collapsed corners reject.

## Compile imported overlays

1. Open the map and inspect `info_overlay` properties.
2. Set material, render order, texture ranges and fades as required.
3. Compile.
4. Inspect **Overlays**, **Surface references** and **Diagnostics** in **Build details…**.
5. Play the built map.

Compiled bindings follow supported face splitting and lightmap subdivision.
Supported imported water transitions use their own checked native records.

## Referenced assemblies

Supported unnamed child overlays can target direct visible world/detail and displacement faces in the same occurrence.
The parent contents preview shows target geometry, not the projected material.

![A native overlay projects graffiti onto the compiled wall](images/overlay-native.png)

## Limits

- Fitted authoring and refit/rebind/corner tools require one eligible visible ordinary quad binding.
- Compiled imported overlays support broader world/detail and terrain targets.
- Corners may extend beyond a bound face; native rendering clips the projection to that face.
- Moving geometry does not automatically refit or rebind an overlay.
- Compile regions must retain the complete projection and every required target.
- Unsupported, missing or ambiguous bindings fail explicitly.
