[← Manual](README.md)

# Brush texture shadows

Use a brush texture's cutouts to shape baked shadows.

1. Apply a supported alpha-tested `LightmappedGeneric` material to the brush.
2. Set receiving-face [lightmap spacing](textures.md#face-lighting) small enough to resolve the pattern.
3. Open **Build settings…** and choose **Direct LDR** or **Direct LDR + HDR**.
4. Enable **Brush texture shadows**.
5. Compile and inspect **Diagnostics** in **Build details…**.
6. Click **Play built map** to inspect the shadow.

| Setting off | Setting on |
| --- | --- |
| ![A grate casts an opaque baked shadow](images/brush-cutout-shadow-off.png) | ![Light passes through the same grate to form a patterned shadow](images/brush-cutout-shadow-on.png) |

Both views use the same brush geometry and texture.
The setting changes lighting. Geometry, sealing, visibility and collision stay unchanged.

## Cast from a stationary root brush model

1. Select an eligible unnamed, unparented `func_brush` in the current map.
2. Enable **Advanced properties** and set its finite **Origin (X Y Z)**.
3. Set **Shadows** to **Yes**.
4. Open **Build settings…**, choose **Direct LDR** or **Direct LDR + HDR**, and enable **Brush texture shadows**.
5. Compile and inspect **Diagnostics** in **Build details…**.

The compiler applies the same binary cutout sampler at the model's admitted stationary initial pose to world and other-model receivers. Model surfaces do not become reflected-light sources.

## Cast from a stationary child brush model

1. Open the child VMF as a normal map and select an eligible unnamed, unparented `func_brush`.
2. Enable **Advanced properties**, set its finite **Origin (X Y Z)**, and set **Shadows** to **Yes**. An existing supported origin brush may supply the effective pivot instead.
3. Apply the supported binary cutout material and save the child.
4. Open the parent map, open **Build settings…**, choose **Direct LDR** or **Direct LDR + HDR**, and enable **Brush texture shadows**.
5. Compile and inspect **Diagnostics** in **Build details…**.

Compilation consumes the effective origin and lowers the child's admitted initial pose before applying containing reference transforms. Each occurrence casts through its own transformed geometry and attached texture coordinates. The parent reference preview remains unavailable for child models.

## Shade another brush in the same child model

1. Open the child VMF and select the cutout brush and its receiving brushes.
2. Click **Tie to entity…**, choose `func_brush`, then click **Tie selected brushes to entity**.
3. Use an eligible unnamed, unparented model with a finite origin or supported origin brush, and save the child.
4. In the parent map, open **Build settings…**, choose **Direct LDR** or **Direct LDR + HDR**, and enable **Brush texture shadows**.
5. Compile and inspect **Diagnostics** in **Build details…**.

The cutout affects only lit receivers in the same model when the global Shadows field is absent or zero. Only an eligible **Shadows → Yes** request extends casting into world or other-model lighting. Malformed Shadows values reject. The parent reference preview remains unavailable.

## Limits

- World and `func_detail` cutout surfaces are supported.
- Binary alpha tests only; partial transparency is unsupported.
- The bake samples mip 0 with nearest filtering. Game rendering can filter the texture differently.
- Without an explicit eligible **Shadows → Yes** request, root and child brush-model texture shadows stay within that model's own lit receivers. A model without an applicable baked receiver scope fails.
- Requested root and child cutout models require **Brush texture shadows** in a lit mode. Fullbright and unsupported materials fail explicitly, including source-material errors outside the compile region.
- Cutouts require the supported plain `LightmappedGeneric` material, `$alphatest 1`, a base texture and checked cutoff/transform values. Fractional translucency, additive/window effects and special compile contents are unsupported.
- Terrain alpha casters are unsupported.
- Unsupported materials, transforms or required assets stop the build.

See [static-prop mesh shadows](prop-shadows.md) for model casters.
