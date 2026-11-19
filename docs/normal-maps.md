[← Manual](README.md)

# Normal-map preview

Show small surface detail without adding geometry.

1. Apply a supported material. Try `shadertest/diffusebump` on a flat brush or `de_dust/groundsand_blend` on terrain.
2. Choose **Textured** in the viewport header.
3. Enable **Normal map preview**.
4. Check the status for unavailable materials.
5. Move the camera to inspect the surface.

| Ordinary shading | Normal-map preview |
| --- | --- |
| ![Cobbled hills under colored lights with ordinary shading](images/normal-maps-off.png) | ![The same cobbles with normal-map relief](images/normal-maps-on.png) |

For authored lights, set **Overlays → Lighting → Quality** to **Detail · per-fragment**. Fast lighting cannot use normal maps.
Use **Reload material assets** after changing material files.

## Limits

- Flat `LightmappedGeneric` brush faces and world or `func_detail` terrain. Terrain also supports plain two-base `WorldVertexTransition`. Models remain excluded.
- Terrain uses its curved geometry and prepared border normals. Rotated and mirrored face mapping is supported. Singular UVs and unsafe tangent frames are excluded.
- Two-base terrain blends normals with the painted terrain alpha. If only one normal map is declared, it covers both layers. Both base textures and all declared normal textures must load.
- Use static LDR RGB normal textures. SSBump, signed textures, proxies, detail and blend modulation are unavailable.
- Two-base terrain requires identity texture transforms. `LightmappedGeneric` requires an identity base transform and supports positive normal-map scale and translation with zero rotation.
- At most 1,048,576 expanded terrain triangle vertices per prepared scene. Other terrain stays explicitly unavailable for this effect.
- No specular, reflections, animation or game brightness match.
- Unsupported faces keep their base image and report why the effect is unavailable.

<!-- page-nav -->

---

[← Atlas-region texturing](atlas-texturing.md) · [Manual](README.md) · [Authored lighting preview →](lighting-preview.md)
