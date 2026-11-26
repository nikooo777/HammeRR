[← Manual](README.md)

# Normal-map preview

Show small surface detail without adding geometry.

1. Apply a supported material or add a normal-mapped model. Try `shadertest/diffusebump` on a brush, `de_dust/groundsand_blend` on terrain or `models/props_wasteland/cargo_container01.mdl`.
2. Choose **Textured** in the viewport header.
3. Enable **Normal map preview**.
4. Check the status for unavailable materials.
5. Move the camera to inspect the surface.

| Ordinary shading | Normal-map preview |
| --- | --- |
| ![Cobbled hills under colored lights with ordinary shading](images/normal-maps-off.png) | ![The same cobbles with normal-map relief](images/normal-maps-on.png) |

For authored lights, set **Overlays → Lighting → Quality** to **Detail · per-fragment**. Fast lighting cannot use normal maps.
Use **Reload material assets** after changing material files.

## Preview model highlights

1. Add `models/props_combine/combine_mine01.mdl` or a supported Phong model.
2. Choose **Textured** and enable **Normal map preview**.
3. Open **Overlays**. Enable **Unshadowed authored lighting** under **Lighting**.
4. Select **Detail · per-fragment**, then move the camera around the model.

Opaque `VertexLitGeneric` models can use fixed-exponent Phong with the mask in
the bump texture alpha. The material needs explicit `$phong`, `$phongexponent`,
`$phongboost` and `$phongfresnelranges`. The highlight uses direct authored
lights. Exponent textures, alternate masks, environments and material proxies
are outside this profile.

## Limits

- Flat `LightmappedGeneric` brush faces and world or `func_detail` terrain. Terrain also supports plain two-base `WorldVertexTransition`.
- Terrain uses its curved geometry and prepared border normals. Rotated and mirrored face mapping is supported. Singular UVs and unsafe tangent frames are excluded.
- Two-base terrain blends normals with the painted terrain alpha. If only one normal map is declared, it covers both layers. Both base textures and all declared normal textures must load.
- Use static LDR RGB normal textures. SSBump, signed textures, proxies, detail and blend modulation are unavailable.
- Authored static and reference-pose models require **Normal** render mode, plain opaque `VertexLitGeneric`, and valid tangents in the VVD. The tangent cache holds up to 32 MiB.
- Two-base terrain and models require identity texture transforms. `LightmappedGeneric` requires an identity base transform and supports positive normal-map scale and translation with zero rotation.
- At most 1,048,576 expanded terrain triangle vertices per prepared scene. Other terrain stays explicitly unavailable for this effect.
- No environment reflections, animation or game brightness match.
- Unsupported surfaces keep their ordinary display and report why the effect is unavailable.

<!-- page-nav -->

---

[← Atlas-region texturing](atlas-texturing.md) · [Manual](README.md) · [Authored lighting preview →](lighting-preview.md)
