[← Manual](README.md)

# Material detail scattering

Generate noncolliding detail models and sprites from exposed ground materials.

1. Put a detail dictionary in mounted game content.
2. Add `%detailtype` to the ground VMT, naming one dictionary type.
3. Open **World settings…**.
4. Set **Detail.vbsp file**, then click **Apply properties**.
5. Apply the material to exposed world/detail faces or terrain.
6. Compile with **Build lighting → Direct LDR** or **Direct LDR + HDR**.
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

1. Open **World settings…**.
2. Set **Generated detail lighting tint · linear RGB** to three integer channels from 0 to 255, for example `255 128 64`.
3. Click **Apply properties**, then compile again.

If the field is absent, enter `_generated_detail_lighting_tint` in **Custom property key**,
enter the triple in **Value**, then click **Stage value** before applying.

White (`255 255 255`) preserves baked lighting; 128 approximately halves a
linear channel. The tint applies to generated models, cards and shapes and
multiplies any dictionary-entry `_detail_lighting_tint`. Explicit detail
entities retain their own tint. Use **Remove override**, then **Apply properties**,
to restore the default white tint.

## Capture details in reflections

1. Configure [Generated reflections](reflections.md) and place the required cubemap samples.
2. In **Build settings…**, choose **Direct LDR** or **Direct LDR + HDR** and enable **Generate reflections**.
3. Compile to include supported final detail models, cards and shapes in the captures.

Capture uses mounted original assets. Detail model materials must be supported
static `UnlitGeneric` with an explicit static base texture; animated inputs,
proxies, ambiguous material/Patch declarations and other runtime effects reject.
Missing or unsupported inputs stop the build before lighting or capture, and
changed consumed assets stop publication.

## Limits

- Generation runs during compilation. The editor can show final placements and static baked styles. Runtime motion, style animation and native appearance require the game.
- Hidden geometry is excluded. Placement and output use bounded deterministic sampling.
- Missing types, malformed dictionaries, unsupported assets or exceeded budgets fail explicitly.
- Changes to consumed dictionaries, models, materials or sprite atlases require a new build.
- A mounted HL2 material can name a detail type absent from the CS:S dictionary; inspect and correct that mismatch.
- Dual-channel builds keep separate static LDR and HDR lighting.
- Native detail lighting uses a quantized gamma lookup, and sprites use 8-bit vertex colours. Separate HDR storage does not imply floating-point rendering precision or editor/game colour parity.
- Reflection generation always checks **Detail material file**, defaulting to `detail/detailsprites`, even in model-only scenes or when no details are emitted. Its supported static atlas material needs one square 2D texture no larger than 4096 pixels per side, with [native mip requirements](textures.md#native-texture-files). Procedural, render-target and animated inputs reject.
- Decoration has no collision and does not replace structural geometry.
