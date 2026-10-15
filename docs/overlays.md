[← Manual](README.md)

# Overlays

Overlays project a material onto selected world surfaces. Use them for markings, graffiti and signs without adding brush geometry. HammeRR compiles existing `info_overlay` entities imported from a VMF.

## Build an imported overlay

1. Open a VMF that already contains an overlay, sealed world geometry and a player spawn.
2. Open **Object report…**, search for `info_overlay`, and select the overlay to inspect its properties.
3. Choose **Material**, **Render Order** (0–3), **U Start**, **U End**, **V Start** and **V End** as needed. Higher render orders draw after lower ones; reversed U/V ranges mirror the texture.
4. Set **Start Fade Dist** and **End Fade Dist**. An end distance of `0` disables distance fading. A negative start distance delegates the fade start to the game.
5. Click **Compile**. Open **Build details…** in the bar below the toolbar to inspect **Overlays**, **Surface references** and any **Diagnostics**.
6. Click **Run Map** to see the projection in CS:S.

![A compiled graffiti overlay on the floor in Counter-Strike: Source](images/overlays.png)

Brush-face bindings follow compiled face splitting and lightmap subdivision. Adjacent faces with different overlay memberships remain separate. World brushes, `func_detail` surfaces and imported displacements are supported. Named overlays receive a runtime accessor with the compiled overlay ID; the VMF keeps its original entity and properties.

[Compile regions](compile-regions.md#include-overlays-and-visibility-helpers)
keep the complete imported projection independently of its marker position.
Include every target surface; an omitted or buried target stops the build
without replacing the previous BSP.

## Limits

- Overlay placement, corner handles, viewport projection and automatic rebinding after geometry edits are not available yet. The imported **Overlay Basis** and **Overlay Point** fields are read-only.
- Targets must be visible, exposed world, detail or terrain faces. Hidden, absent, non-rendering and brush-model targets fail compilation. Hidden overlays are omitted.
- Budgets: 1,024 overlays, 64 source sides and 64 compiled surface references per overlay, and 65,536 total source or face bindings. Subdivision can increase the compiled face count; use fewer targets or a larger lightmap scale if it exceeds the budget. These are bounded compiler budgets, not a claim about every engine limit.
- Basis axes must be finite perpendicular unit vectors. Projection corners must form a convex quadrilateral within the compile coordinate bounds. Invalid material paths, projections or fade distances keep the previous BSP.
- Water overlay transitions, brush-model overlays and `infodecal` compilation remain unfinished.
