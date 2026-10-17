[← Manual](README.md)

# Normal-map preview

Preview surface detail from a supported brush material's normal texture under
the editor's directional light.

1. Select **Textured** in **Viewport** and apply a material with a supported
   `$bumpmap` to a flat brush face.
2. Enable **Normal map preview** in **Viewport**.
3. Inspect the status below the switch. It identifies applied polygons and
   unavailable materials; unsupported faces retain their base image.
4. Move the camera to inspect the surface. This changes the view of the same
   material detail without editing geometry or lighting.
5. After repairing or changing material files, open **Browse materials…** and
   click **Reload material assets**. Disable **Normal map preview** to return
   to ordinary shading.

![Flat brush normal-map preview](images/normal-map-preview.png)

Limits:

- Only flat `LightmappedGeneric` world/detail/brush-entity polygons are covered.
  Terrain, models, resource images, helpers and tool surfaces are excluded.
  Other viewport modes perform no normal admission.
- This is fixed editor directional shading. Authored lights, specular,
  reflections, animation and native game brightness are independent.
- One static RGB/BGR/RGBA/BGRA8888 or DXT1/3/5 normal texture is supported.
  Signed/swizzled, HDR, SSBump and specialized VTF sampling flags are unsupported.
  RGB Z must be at least 128; alpha is ignored. Sampling uses linear RGB,
  linear filtering, repetition and the base mip.
- Base texture transforms must be identity. Bump transforms allow positive
  axis scale and translation, with zero rotation. Proxies, fallback blocks,
  detail/second normal maps, bump frames and editor texture overrides reject.
- Texture axes are limited to 1024 pixels, VTF reads to 16 MiB and VMT/Patch
  reads to 64 KiB each. The cache retains at most 128 admissions and 64 MiB
  decoded pixels. Base/transformed UVs must remain within ±1,000,000.
- Material reload clears cached successes and failures. Externally changed VMT
  declarations require reload before they can be combined with a cached base.
  Toggling the preview preserves map data, camera, selection, history and
  pending geometry reviews. It is not part of saved Preview defaults.
