[← Manual](README.md)

# Static-prop mesh shadows

Direct LDR traces a static prop's actual model triangles. Open space in a model stays open to light even when it lies inside the model's render or collision bounds. Shadow casting is independent of the prop's collision choice.

1. Click **Entity…**, choose `prop_static`, then **Place entity**. Use **Browse…** beside **World Model** to choose its model and skin. Keep `disableshadows` at `0` to let it cast shadows.
2. Click **Build settings…**, set **Build lighting** to **Direct LDR**, and choose **Direct only** or the desired number of diffuse bounces under **Indirect light**.
3. Click **Close**, then **Compile**.
4. Open **Build details…**. **Prop mesh shadows** reports the contributing props, triangle count and disabled props. Read **Diagnostics** for unsupported material behavior or missing model geometry.

![Build settings and diagnostics](images/build-status.png)

Point lights, spotlights, sun, sky ambient, diffuse bounce rays and model ambient probes share the same opaque mesh blockers. Baked brush, terrain and brush-entity receivers all use those blockers. The native `disableshadows` property disables them; hidden props are omitted. The selected skin determines the material diagnostics. Fullbright does not load shadow meshes.

Agents use `set_compile_settings` with `lighting: "direct_ldr"`, then `compile_map` or `start_compile`. Build results expose `prop_shadow_props`, `prop_shadow_models`, `prop_shadow_triangles`, `prop_shadow_disabled` and `prop_shadow_work`; the last is preparation work, included in total `lighting_work`. The CLI uses the same compiler behavior.

## Limits

- MDL versions 44/48, VVD version 4 and DX90 VTX version 7 use the shared bounded model decoder. Shadows use LOD0, body 0 and the native translation/rotation. Static-prop scaling and nonzero bodygroups are unsupported.
- Shadows are opaque and two-sided. Texture alpha, translucency, additive effects, shaders other than VertexLitGeneric/UnlitGeneric/LightmappedGeneric and material animation/conditional blocks are not evaluated; affected selected materials produce warnings. Missing materials also produce an opaque-shadow warning. Missing or malformed mesh files fail the build instead of substituting a bounding box.
- Props block bounced and ambient rays but do not reflect light as sources. Per-vertex/per-texel prop baking, animated-prop shadows and HDR remain unfinished.
- Ambient probe positions avoid each shadow-casting prop's mesh bounds. This conservative placement can exclude open space inside large or concave bounds; leaves without a safe candidate produce a diagnostic.
- At most 262,144 transformed prop triangles and 256 MiB of model input are accepted per build. Individual model files are limited to 64 MiB; material files to 64 KiB, with at most eight patch includes and 65,536 material lookups. Preparation and ray traversal share the cancellable 100-million-unit lighting budget.
- A failed or cancelled build preserves the previous BSP. Disable shadows on decoration that does not need them to reduce work.
