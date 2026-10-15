[← Manual](README.md)

# Water

Create a horizontal world-water volume with connected depth metadata, a material for viewing the surface from below, and optional fluid physics.

1. Use **Draw brush** or **Add brush** to create the water volume inside a sealed room. Keep it as a world brush.
2. Set **Selection → Objects**, enter `tools/toolsnodraw` in the material controls, and click **Apply**.
3. Set **Selection → Faces · materials & textures** and select the flat top face. Click **Browse materials…**, choose a water material, then **Use material** and **Apply**. The material's VMT must declare `$bottommaterial` to provide its below-water surface; this names another material, not a texture for the pool floor.
4. Open **Build settings…**. For floating physics objects, choose **Physics bodies → CS:S engine physics**. Click **Close**, then **Compile** or **Run Map**.
5. Open **Build details…** and read **Diagnostics**. A missing `$bottommaterial` leaves the surface one-sided and reports a warning. A named bottom material that cannot be loaded stops the build and preserves the previous output.

![Material browser used to choose the water's top material](images/material-browser.png)

![Build settings and diagnostics](images/build-status.png)

Connected water with the same height, material and contents shares its lowest connected depth. Different heights or top materials keep separate regions. The compiler adds one `water_lod_control` with distances of 1,000 and 2,000 if the map has none; authored controls are preserved. Hidden water is omitted.

The generated underside keeps the top face's texture placement and native surface flags, uses the resolved `$bottommaterial`, and belongs to the submerged leaves. It does not receive overlays attached to the authored top. Material patches are resolved through their includes. `$underwateroverlay` stays a runtime setting of the top material.

Agents use the same material and compile operations: `create_block`, `select_faces`, `edit_faces`, `set_compile_settings`, `compile_map` and `start_compile`.

Limits: horizontal structural-world water tops only. Below-water faces on exposed sides, depth-specific material packing, currents, custom damping, sloped or enclosed tops, detail/model liquids and broader native rendering validation remain unfinished. Conflicting overlapping liquids fail with a diagnostic. Editor previews use the material's editor texture; they do not render the game's water reflection, refraction or underwater view. Material assets must remain available in the game's mounted content. Compile regions must contain the complete water volume.
