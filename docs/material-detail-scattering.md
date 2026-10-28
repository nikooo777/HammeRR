[← Manual](README.md)

# Material detail scattering

Generate noncolliding detail models and sprites from exposed ground materials.

1. Put a detail dictionary in mounted game content.
2. Add `%detailtype` to the ground VMT, naming one dictionary type.
3. Open **World settings…**.
4. Set **Detail.vbsp file**, then click **Apply properties**.
5. Apply the material to exposed world/detail faces or terrain.
6. Compile with **Build lighting → Direct LDR**.
7. Inspect **Detail props**, **Model assets**, **Sprite entries** and diagnostics.
8. Use [Final compiled details](compiled-details.md) to inspect placement, or play the built map to inspect native appearance.

The absent dictionary path uses `detail.vbsp`.
A declared type must exist exactly once in that dictionary.

## Dictionary structure

A named type contains `density` and groups. Groups contain `alpha` and weighted entries.
Model entries specify `model`; sprite entries use the [detail atlas fields](detail-sprites.md).
Model details require static-prop models with supported `UnlitGeneric` materials.

## Terrain blends

Paint terrain alpha with **Edit terrain… → Paint blend** or **Fill blend**.
The material's detail dictionary groups determine how that alpha affects generated choices.
Geometry, source materials and authored blend rows remain unchanged by compilation.

## Generated lighting tint

Set **Generated detail lighting tint · linear RGB** in **World settings…** for supported attenuation.
Use **Remove override** to restore the absent setting.

## Limits

- Generation runs during compilation. The editor can show final placements and static baked styles. Runtime motion, style animation and native appearance require the game.
- Hidden geometry is excluded. Placement and output use bounded deterministic sampling.
- Missing types, malformed dictionaries, unsupported assets or exceeded budgets fail explicitly.
- Changes to consumed dictionaries, models, materials or sprite atlases require a new build.
- A mounted HL2 material can name a detail type absent from the CS:S dictionary; inspect and correct that mismatch.
- Dual-channel builds keep the HDR pass in one shared base color, with separate channel style extras.
- Decoration has no collision and does not replace structural geometry.
