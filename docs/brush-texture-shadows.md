[← Manual](README.md)

# Brush texture shadows

Use base-texture alpha to shape or soften baked shadows from brushes and terrain.

1. Apply a supported alpha-tested or translucent `LightmappedGeneric` material to a world brush, `func_detail` brush or terrain sheet.
2. Set receiving-face [lightmap spacing](textures.md#face-lighting) small enough to resolve the pattern.
3. Open **Build settings…** and choose **Direct LDR** or **Direct LDR + HDR**.
4. Enable **Brush texture shadows**.
5. Compile and inspect **Diagnostics** in **Build details…**.
6. Open **Build details… → Baked surfaces…**, click **Prepare**, then **Show** to inspect compiled receivers and texture opacity.
7. Click **Play built map** to inspect the shadow in the game.

| Setting off | Setting on |
| --- | --- |
| ![A grate casts an opaque baked shadow](images/brush-cutout-shadow-off.png) | ![Light passes through the same grate to form a patterned shadow](images/brush-cutout-shadow-on.png) |

Both views use the same brush geometry and texture.
The setting changes lighting. Geometry, sealing, visibility and collision stay unchanged.

## Material opacity

Use a static base texture with one of these material forms:

| Form | Required values | Baked blocking |
| --- | --- | --- |
| Cutout | `$alphatest 1`, `$translucent 0`, `$alpha 1` | Each sampled texel either blocks or passes light at the checked alpha-test cutoff. |
| Partial transparency | `$translucent 1`, `$alphatest 0`, `$alpha 1` | Each sampled alpha byte contributes coverage from clear (0) to fully blocking (255). |

Omitted `$translucent` and `$alphatest` default to zero; omitted `$alpha` defaults to one.
The same checked base-texture transform and clamp/repeat addressing apply to both forms.
Terrain uses its base-grid texture coordinates. Painted terrain blend values do not control opacity.

Coverage adds across brush, terrain, brush-model and requested prop casters, up to full blocking.
For example, two crossings with alpha 64 and 128 leave 63/255 visibility.
A brush's separate entry and exit faces each contribute coverage, so two half-opaque faces can block completely.
Transmitted light keeps its color; the texture's RGB does not tint it.

The setting applies to direct lights and styles, sun and sky, diffuse bounces,
ambient probes, detail and static-prop vertex lighting, and material-emission transport.
Enable **Bake static-prop vertex lighting** when those vertex receivers are needed.

## Cast from a stationary root brush model

1. Select an eligible unnamed, unparented `func_brush` in the current map.
2. Enable **Advanced properties** and set its finite **Origin (X Y Z)**.
3. Set **Shadows** to **Yes**.
4. Open **Build settings…**, choose **Direct LDR** or **Direct LDR + HDR**, and enable **Brush texture shadows**.
5. Compile and inspect **Diagnostics** in **Build details…**.

The compiler applies the same cutout or partial-transparency sampler at the model's admitted stationary initial pose to world and other-model receivers. Model surfaces do not become reflected-light sources.

## Cast from a stationary child brush model

1. Open the child VMF as a normal map and select an eligible unnamed, unparented `func_brush`.
2. Enable **Advanced properties**, set its finite **Origin (X Y Z)**, and set **Shadows** to **Yes**. An existing supported origin brush may supply the effective pivot instead.
3. Apply a supported cutout or translucent material and save the child.
4. Open the parent map, open **Build settings…**, choose **Direct LDR** or **Direct LDR + HDR**, and enable **Brush texture shadows**.
5. Compile and inspect **Diagnostics** in **Build details…**.

Compilation consumes the effective origin and lowers the child's admitted initial pose before applying containing reference transforms. Each occurrence casts through its own transformed geometry and attached texture coordinates. The parent reference preview remains unavailable for child models.

## Shade another brush in the same child model

1. Open the child VMF and select the textured shadow brush and its receiving brushes.
2. Click **Tie to entity…**, choose `func_brush`, then click **Tie selected brushes to entity**.
3. Use an eligible unnamed, unparented model with a finite origin or supported origin brush, and save the child.
4. In the parent map, open **Build settings…**, choose **Direct LDR** or **Direct LDR + HDR**, and enable **Brush texture shadows**.
5. Compile and inspect **Diagnostics** in **Build details…**.

Texture opacity affects only lit receivers in the same model when the global Shadows field is absent or zero. Only an eligible **Shadows → Yes** request extends casting into world or other-model lighting. Malformed Shadows values reject. The parent reference preview remains unavailable.

## Limits

- World and `func_detail` brush and terrain surfaces are supported. Terrain sheets do not seal the map or become runtime brush-model terrain.
- The bake samples mip 0 with nearest filtering. Game rendering can filter the texture differently.
- **Baked surfaces…** displays these plain alpha materials with mip-0 nearest sampling and their checked cutoff or fractional alpha. Translucent display sorts whole batches by depth; intersecting surfaces can sort differently from the game.
- Without an explicit eligible **Shadows → Yes** request, root and child brush-model texture shadows stay within that model's own lit receivers. A model without an applicable baked receiver scope fails.
- Requested root and child texture-shadow models require **Brush texture shadows** in a lit mode. Fullbright and unsupported materials fail explicitly, including source-material errors outside the compile region.
- Only the supported plain `LightmappedGeneric` forms above are admitted. Authored `$alpha` values other than one, colored transmission, additive effects, vertex opacity, blend shaders and special compile contents are unsupported.
- Materials need a base texture, frame zero and checked transform values. Animated textures, proxies, conditional blocks and other unsupported parameters reject. The VTF must have one frame, face and depth slice.
- The original material and its scalar Patch include/edit chain must be unambiguous. Duplicate fields or Patch sections, unquoted conditional selectors and animated blocks stop the build before alpha decoding.
- Unsupported materials, transforms or required assets stop the build.

See [static-prop mesh shadows](prop-shadows.md) for model casters.
