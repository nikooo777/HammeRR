[← Manual](README.md)

# Static-prop mesh shadows

Direct LDR traces a static prop's actual model triangles. Open space in a model stays open to light even when it lies inside the model's render or collision bounds. Models authored with `$casttextureshadows` also let light through supported alpha-test cutouts. Shadow casting is independent of the prop's collision choice.

1. Click **Entity…**, choose `prop_static`, then **Place entity**. Use **Browse…** beside **World Model** to choose its model and skin. Keep `disableshadows` at `0` to let it cast shadows.
2. Click **Build settings…**, set **Build lighting** to **Direct LDR**, and choose **Direct only** or the desired number of diffuse bounces under **Indirect light**.
3. Click **Close**, then **Compile**.
4. Open **Build details…**. **Prop mesh shadows** reports the contributing props, triangle count, disabled props, alpha-test triangles and alpha textures. Read **Diagnostics** for unsupported material behavior or missing assets.

![Build settings and diagnostics](images/build-status.png)

Point lights, spotlights, sun, sky ambient, diffuse bounce rays and model ambient probes share the same opaque or supported alpha-test mesh blockers. Baked brush, terrain and brush-entity receivers all use those blockers. The native `disableshadows` property disables them; hidden props are omitted. The selected skin determines the shadow material. Fullbright does not load shadow meshes or alpha textures.

Alpha-test shadows require the model's native `CAST_TEXTURE_SHADOWS` flag, set when the model is compiled with `$casttextureshadows`; there is no separate editor toggle. For flagged models, selected-skin **VertexLitGeneric** and **UnlitGeneric** materials with `$alphatest` enabled use `$basetexture` alpha. Supported ordinary opaque materials keep opaque shadows. Requested unsupported texture-shadow behavior fails the build with an entity-specific diagnostic and preserves the previous BSP.

Agents use `set_compile_settings` with `lighting: "direct_ldr"`, then `compile_map` or `start_compile`. Build results expose `prop_shadow_props`, `prop_shadow_models`, `prop_shadow_triangles`, `prop_shadow_disabled` and `prop_shadow_work`; the last is preparation work, included in total `lighting_work`. Cutout results add `prop_shadow_alpha_triangles`, `prop_shadow_alpha_materials`, `prop_shadow_alpha_textures` and `prop_shadow_alpha_bytes`. These count transformed cutout triangles, selected material slots per model, unique VTF masks and their decoded alpha bytes. The CLI uses the same compiler behavior.

## Limits

- MDL versions 44/48, VVD version 4 and DX90 VTX version 7 use the shared bounded model decoder. Shadows use LOD0, body 0 and the native translation/rotation. Static-prop scaling and nonzero bodygroups are unsupported.
- All shadow triangles are two-sided. Models without `CAST_TEXTURE_SHADOWS` retain opaque shadows and existing approximation warnings for alpha/translucent/additive materials, unsupported shaders, material animation/conditional blocks and missing materials. Missing or malformed mesh files fail the build instead of substituting a bounding box.
- Cutout rays interpolate triangle UVs, then sample mip 0 with nearest texels. VTF clamp S/T flags clamp each axis; other axes repeat. `$alphatestreference` accepts 0–1. An absent reference uses 0.5 for VertexLitGeneric and 0.7 for UnlitGeneric; explicit zero uses 0.5. Alpha equal to or above the reference blocks light. Filtered edges and fractional/translucent transmission are unsupported; binary cutouts do not reproduce native VRAD's fractional texture coverage.
- Cutout `$basetexturetransform` supports finite **center**, **scale**, **translate** and **rotate 0**, with each value within ±1024. Rotated transforms, nonzero `$frame`, animated/cubemap/volume textures, missing or corrupt requested assets and unsupported opacity parameters fail explicitly. Translucency, additive effects, vertex/distance alpha, alpha modulation and material animation/conditional blocks are unsupported for requested texture shadows. Self-illumination, base-alpha environment masks, seamless projection and additional material effects are unsupported for cutouts.
- Props block bounced and ambient rays but do not reflect light as sources. Per-vertex/per-texel prop baking, animated-prop shadows and HDR remain unfinished.
- Ambient probe positions avoid each shadow-casting prop's mesh bounds. This conservative placement can exclude open space inside large or concave bounds; leaves without a safe candidate produce a diagnostic.
- At most 262,144 transformed prop triangles and 256 MiB of model/texture input are accepted per build. Individual asset files are limited to 64 MiB; material files to 64 KiB, with at most eight patch includes and 65,536 material lookups. Cutout textures are limited to 4096 pixels per axis and 64 MiB of decoded alpha across the build. Mesh preparation, alpha decoding and ray traversal share the cancellable 100-million-unit lighting budget.
- A failed or cancelled build preserves the previous BSP. Disable shadows on decoration that does not need them to reduce work.
