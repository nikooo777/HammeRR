[← Manual](README.md)

# Physics and brush-model mass

Build collision bodies with the installed CS:S physics library. Brush-model
base masses come from the mounted material surface definitions and compiled
geometry. Individual collision faces retain their own contact properties.

1. Select a brush entity. Set **Selection** to **Objects** to apply one material
   across it, or **Faces · materials & textures** to assign individual faces.
2. Click **Browse materials…**, choose a material, then **Use material** and
   **Apply**. The material's `$surfaceprop` selects its physical properties;
   a valid material without that field uses the defined `default` property.
3. Open **Build settings…** and choose **Physics bodies → CS:S engine physics**.
   Click **Close**, then **Compile**.
4. Open **Build details…**. The entity diagnostic reports its base mass in
   kilograms and the surface property used. Repair any missing material,
   surface manifest or definition reported by the build, then compile again.

![Physics settings](images/build-settings.png)

For mixed-material models, the property covering the greatest emitted face
area determines the base mass. Equal areas keep the first emitted property.
This choice does not replace the per-face contact materials. Solid materials
use collision volume and density; shell materials use emitted surface area,
thickness and density. Base masses are capped at 50,000 kg. Positive
`func_physbox` **Mass Scale** and **Override Parameters** apply later in the game;
`phys_convert` **Mass Override** can replace the mass after conversion.

Limits: Linux64 with the verified installed CS:S physics library. Surface
definitions load in manifest order from the same game content mounts as
materials. Native inheritance and the declared density/thickness must agree.
Missing or malformed assets, undefined bases, invalid density/thickness and
unsupported native property indices stop the build and preserve previous
output. The bounded loader accepts at most 64 definition files, 8 MiB of text,
4,096 property definitions and 511-byte tokens. Native mass properties must
resolve to indices 0–127. The existing 127-name shared contact table still
applies. Secondary blended properties, custom density controls and broader
moving-body collision behavior remain outside this workflow.
