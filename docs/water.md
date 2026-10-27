[← Manual](README.md)

# Water and slime

Create horizontal world-liquid volumes with native depth, inward surfaces and optional fluid physics.

## Create a pool

1. Draw the water volume inside a sealed room. Keep it as a world brush.
2. In object mode, apply `tools/toolsnodraw` to the brush.
3. Choose **Selection → Faces · materials & textures**.
4. Apply a water material to the horizontal top face.
5. Open **Build settings…**.
6. Choose **Physics bodies → CS:S engine physics** for floating physics objects.
7. Compile, then inspect **Build details… → Diagnostics**.
8. Play the built map to inspect reflection, refraction and the underwater view.

![A compiled pool with native reflection and refraction](images/showcase-native-water.png)

The material's `$bottommaterial` names its below-water surface material, not the solid pool floor.
A missing declaration reports a one-sided surface. An unreadable named material stops the build.

| Above water | Below water |
| --- | --- |
| ![Native water reflection and refraction](images/water-native-close.png) | ![The pool viewed underwater](images/water-native-underwater.png) |

## Create a slime pool

1. Draw the volume inside a sealed room. Keep it as a world brush.
2. Apply `tools/toolsnodraw` to the brush.
3. Choose **Selection → Faces · materials & textures**.
4. Apply a supported opaque slime material to the horizontal top face.
5. Choose **Physics bodies → CS:S engine physics** in **Build settings…** for floating physics objects.
6. Compile and inspect **Build details… → Diagnostics**.

![A compiled opaque slime volume in CS:S](images/slime-native.png)

The material must declare `%compileslime 1` and name itself in `$bottommaterial`. For example, `slime/pool` requires `$bottommaterial slime/pool`.

Slime keeps its own native contents and uses the same material on both sides of every exposed liquid face.
Static scalar `LightmappedGeneric`, `UnlitGeneric` and scalar `Patch` chains are supported with neutral opacity settings.
Every slime material must explicitly name itself in `$bottommaterial` for native below-view lookup. Use the material name without `materials/`, `.vmt`, extra whitespace or alternate path spelling; letter case may differ. A Patch must name the applied top material, not its include parent.
Keep alpha at one and additive, translucent and vertex-alpha effects off. Conditional, proxy and animated blocks are unsupported.
The material keeps its lighting and opaque envmap effects; Water-shader reflection and refraction are unavailable.
Enable **Pack liquid assets** for the supported static subset described below. Other slime material assets must remain mounted.

## Inspect water material dependencies

1. Select the material in **Browse materials…**.
2. Click **Source…**.
3. Read **Water dependencies** and its state.
4. Inspect compiler materials, runtime materials, files and unsupported settings.
5. Use **Refresh source** after changes.

**Supported static inventory** describes supported dependencies. Other states explain incomplete or unsupported inventories.
Inspection does not pack assets.

## Pack static liquid assets

1. Use a supported static Water declaration or opaque slime material and dependencies.
2. Enable **Pack liquid assets** in **Build settings…**.
3. Compile and inspect **Liquid assets** in **Build details…**.

Packing supports a checked static subset, not every animated stock Water shader.
Static envmaps require matching supported LDR and HDR cubemap files. Unsupported dependencies fail explicitly.

Slime packing supports static opaque `LightmappedGeneric`, `UnlitGeneric` and unambiguous scalar `Patch` chains with `%compileslime 1`, an explicit `$basetexture` and the exact native self-bottom declaration. It includes every original material and declared `$basetexture`, `$bumpmap` and `%tooltexture` VTF, including textures overridden by a Patch. These textures must be supported static one-frame 2D VTFs, version 7.0–7.4, with depth one and `first_frame 0`.
Neutral opacity settings, `$surfaceprop`, `%keywords`, static fog/color controls, UV transforms and authored `$reflectivity` are admitted. Fog switches and `%notooltexture` use literal `0` or `1`; fog distances must be finite. Colors and reflectivity use normalized square-bracket RGB triples. UV transforms use the supported center/scale/rotate/translate syntax with finite components within ±1024 and no repeated operations. A named static `$envmap` also packs its original LDR and HDR cube files, including declarations overridden by a Patch. Both cubes require the existing VTF 7.4 layout with seven stored square faces, `first_frame 0` and matching width, height and mip count; the HDR sibling must use the supported HDR format. Effective `$envmapframe` must be absent or zero. Dynamic `env_cubemap`, missing cube siblings, other parameters, detail textures, additional material references and effects stop packing. A Patch chain cannot edit the same key in multiple materials. Missing dependencies and unsupported texture layouts stop publication without replacing the previous BSP.

Assets outside the packed closure must remain mounted in the game.

## Steady physical current

1. Open **World settings…**.
2. Find **Liquid current · world units/s**.
3. Set **X**, **Y** and **Z**.
4. Click **Apply properties**.
5. Compile with **CS:S engine physics**.

The vector length must not exceed 128 units/s. It applies uniformly to eligible physical objects in water and slime, not player movement.
Use **Remove override** to restore the absent setting.

## Liquid pressure damping

Set **Liquid pressure damping** in **World settings…**, then click **Apply properties**.
The coefficient is 0–0.01 and applies uniformly to eligible water and slime with engine physics.
Use **Remove override** to restore the absent setting.

## Stationary invisible water

1. Draw an axis-aligned box in dry open space, separated from other geometry.
2. Apply `tools/toolsnodraw` to every face.
3. Use **Tie to entity…** with custom class `func_water_analog`.
4. Click **Tie selected brushes to entity**.
5. Compile with **CS:S engine physics**.

At most two separate root helpers are supported, with one cuboid each.
Keep rotation, start position and flags at zero.
Volumes must not touch each other, other geometry or retained water/slime.
Movement, parenting, rendered surfaces and authored world current/damping are unsupported.

## Limits

- Structural-world water and slime require visible horizontal tops. Complete volumes must remain inside a compile region.
- Exposed vertical sides and negative-Z bottoms can generate inward surfaces from their own bottom materials.
- Sloped liquid tops, sloped inward Water surfaces and detail/model liquids remain unsupported.
- Water-shader slime, non-neutral opacity, conditional/proxy blocks, missing or different slime below-view materials and mixed WATER/SLIME contents in one brush are unsupported.
- Connected compatible liquids share depth metadata. Conflicting overlapping liquids fail.
- Current and damping are map-wide; per-pool controls remain unavailable.
- The editor shows the water material's editor image, not native reflection or refraction.
- Invalid controls, missing required assets and exceeded geometry budgets fail without replacing the previous BSP.
