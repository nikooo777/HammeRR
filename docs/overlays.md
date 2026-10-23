[← Manual](README.md)

# Overlays

Project a material onto compiled surfaces. The editor shows the projection outline.

## Create a fitted overlay

1. Select visible coplanar world or `func_detail` faces in face mode, including at least one quad.
2. Open **Overlay…**.
3. Choose **Material** and optional **Name**.
4. Set **Render order**, texture ranges and fade distances.
5. Click **Fit / Preview**.
6. Inspect the outline and bound-face count, then click **Apply overlay**.

The first eligible quad supplies the projection axes. One overlay spans all selected faces, up to 64 sources. Native rendering clips it to each bound face, leaving unbound gaps clear. Selecting one quad keeps the original fit.

**Cancel** discards the owned preview. Compile and play to inspect the material projection.

![One native overlay spans two panels and leaves the unbound gap clear](images/overlay-native.png)

## Refit or rebind

1. Select an existing eligible overlay and open **Overlay…**.
2. For multiple bound faces, choose a quad under **Reference face**. Its current edges set the projection axes.
3. Click **Refit selected overlay** to fit all existing bound faces.
4. Review the new outline and bound-face count, then click **Apply refit**.

The bound faces must remain visible, flat and coplanar. Refit preserves their exact side IDs. It follows moved geometry only when requested; it does not infer the original reference face or preserve an earlier in-plane orientation. A single-face overlay keeps its original refit behavior.

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

## Limits

- Multi-face creation requires flat coplanar faces facing the same direction and an eligible quad to anchor the projection.
- Multi-face refit requires an explicit eligible quad reference from the existing bindings and the same source limits as creation.
- Rebind and corner tools require one eligible visible ordinary quad binding.
- Terrain and brush-model targets are unavailable for fitted authoring.
- Compiled imported overlays support broader world/detail and terrain targets.
- Corners may extend beyond a bound face; native rendering clips the projection to that face.
- Moving geometry does not automatically refit or rebind an overlay.
- Compile regions must retain the complete projection and every required target.
- Unsupported, missing or ambiguous bindings fail explicitly.
