[← Manual](README.md)

# Water

Create a horizontal world-water volume with connected depth metadata, inward-facing top and vertical-side materials, and optional fluid physics.

1. Use **Draw brush** or **Add brush** to create the water volume inside a sealed room. Keep it as a world brush.
2. Set **Selection → Objects**, enter `tools/toolsnodraw` in the material controls, and click **Apply**.
3. Set **Selection → Faces · materials & textures** and select the flat top face. Click **Browse materials…**, choose a water material, then **Use material** and **Apply**. The material's VMT must declare `$bottommaterial` to provide its below-water surface; this names another material, not a texture for the pool floor.
4. For an exposed vertical water side, select that side and apply a water material in the same way. Its own `$bottommaterial` supplies the inward-facing material. Sides using `tools/toolsnodraw` stay hidden.
5. Open **Build settings…**. For floating physics objects, choose **Physics bodies → CS:S engine physics**. Click **Close**, then **Compile** or **Run Map**.
6. Open **Build details…** and read **Diagnostics**. A missing `$bottommaterial` leaves the face one-sided and reports a warning. A named bottom material that cannot be loaded stops the build and preserves the previous output.

![Material browser used to choose the water's top material](images/material-browser.png)

![Build settings and diagnostics](images/build-status.png)

Connected water with the same height, material and contents shares its lowest connected depth. Different heights or top materials keep separate regions. The compiler adds one `water_lod_control` with distances of 1,000 and 2,000 if the map has none; authored controls are preserved. Hidden water is omitted.

Each generated inward face keeps its authored face's texture placement and native surface flags, uses that material's resolved `$bottommaterial`, and belongs to the submerged leaves. Solid covers and internal water-to-water boundaries add no inward faces. Coplanar vertical faces keep separate fog regions when their water heights or top materials differ. Generated faces do not receive overlays attached to authored faces. Material patches are resolved through their includes. `$underwateroverlay` stays a runtime setting of the top material.

Agents use the same material and compile operations: `create_block`, `select_faces`, `edit_faces`, `set_compile_settings`, `compile_map` and `start_compile`.

Limits: structural-world water with a visible horizontal top; inward faces cover the top and exposed vertical sides. HammeRR caps their combined generation at four million work units, 65,536 prospective fragments and one million cumulative winding point allocations. Conservative reservations include clipping and merging; exceeding a budget fails the build and preserves previous output. Native rendering of vertical inward faces still requires validation. Sloped sides, negative-Z bottom faces, depth-specific material packing, currents, custom damping, sloped or enclosed tops, detail/model liquids and broader native rendering validation remain unfinished. Conflicting overlapping liquids fail with a diagnostic. Editor previews use the material's editor texture; they do not render the game's water reflection, refraction or underwater view. Material assets must remain available in the game's mounted content. Compile regions must contain the complete water volume.
