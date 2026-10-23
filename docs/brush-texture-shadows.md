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

## Limits

- World and `func_detail` cutout surfaces are supported.
- Binary alpha tests only; partial transparency is unsupported.
- The bake samples mip 0 with nearest filtering. Game rendering can filter the texture differently.
- For brush models, texture shadows stay within the same model.
- Terrain alpha casters are unsupported.
- Unsupported materials, transforms or required assets stop the build.

See [static-prop mesh shadows](prop-shadows.md) for model casters.
