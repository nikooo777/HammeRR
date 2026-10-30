[← Manual](README.md)

# Static props

Compile static scenery models with visibility, collision options and lighting.

## Place and build

1. Use **Entity…** to place `prop_static`.
2. Click **Browse…** beside **World Model**.
3. Choose a compatible static model, then click **Use model**.
4. Set **Position**, **Rotation** and **Skin**.
5. Compile and inspect **Static props**, **Model assets**, **Leaf references** and **Diagnostics**.

![A static barrel beside brush-built crates in the compiled courtyard](images/showcase-native-arch.png)

Use **Not Solid**, **Use Bounding Box** or **Use VPhysics** under **Collisions**, as supported.
VPhysics needs compatible model collision data from a matching `.phy` file.
The **Physics bodies** setting controls compiled brush and terrain bodies separately.

## Cast exact emission shadows

Use supported opaque prop triangles to block [material emission](material-emission.md#exact-brush-shadows).

1. Leave **Disable Shadows** disabled on the prop.
2. Open **World settings…** and set **Emission file**.
3. Enable **Exact emission visibility**, then click **Apply properties**.
4. Compile with **Direct LDR** or **Direct LDR + HDR**.
5. Open **Build details…**. Check **Emission visibility policy** and **Diagnostics**.

Shadows use actual triangles from body 0 and LOD 0 with the selected **Skin**
and compiled pose, including supported referenced props. Materials require plain
opaque **VertexLitGeneric** or supported **Patch** chains. Exact mode checks the
original model and material inputs. Missing, unsupported or alpha materials and
zero-area triangles stop the build. The previous BSP stays intact.

**Disable Shadows** removes that prop's shadow geometry. Disabling receiver self
shadows omits only that receiving prop's own triangles. Other props, world and
terrain still block light. Static props cannot emit intrinsic material light.

## Pack static prop assets

1. Enable **Pack static prop assets** in **Build settings…**.
2. Compile.
3. Check **Static-prop assets** and diagnostics in **Build details…**.

Packing includes the checked model companions and supported material dependencies.
Unsupported materials or missing required assets stop publication.
Supported `Patch` materials can inherit through up to eight include edges,
with at most one `$basetexture` `insert` or `replace` block per Patch. Packing retains
every original material and declared texture, including overridden textures.
Deeper edits win within the same edit type; replacements apply after all inserts.

## Bake vertex lighting

Bake a static style-0 base while supported point/spot switching and patterns
remain runtime lighting in the game.

1. Choose **Direct LDR** or **Direct LDR + HDR**.
2. Enable **Bake static-prop vertex lighting**.
3. Choose direct lighting or one to four diffuse bounces.
4. Compile, then inspect **Static-prop vertex lighting** and **Prop vertex lighting bytes**.
5. Use **Run map** to see supported light switching and patterns in the game.

Reflected reception uses the preceding lighting pass from world, terrain and
eligible stationary **Shadows → Yes** root or assembled-child brush models.
The existing zero-to-four bounce depth and independent LDR/HDR channels apply.
VHV stores only the static style-0 base. The game adds supported point/spot
switching and patterns as runtime direct lighting.
The editor's baked preview shows the stored base; it does not animate these
runtime contributions. Switched indirect ambient and animated VHV remain unsupported.

Vertex lighting requires a stricter model/material subset than ordinary prop placement.
Unsupported model or material effects stop the build.
Default-alpha-test receivers accept the supported nested basetexture Patch edits.
Every hardware vertex keeps its lighting colors; receiver textures are read only
when shadow casting, packing or enabled reflected sources need them.
Use [baked prop inspection](baked-prop-vertices.md) to read the emitted samples.

## Reflect light from static props

Use eligible vertex-lit static props as reflected style-0 light sources.

1. Open **Build settings…** and choose **Direct LDR** or **Direct LDR + HDR**.
2. Enable **Bake static-prop vertex lighting**.
3. Enable **Reflect light from static props**.
4. Choose **Direct only** or one to four diffuse bounces under **Indirect light**.
5. Compile, then check **Reflected static-prop sources** in **Build details…**.

Sources use their actual root-LOD triangles, selected skin and UVs, and generated
vertex colors. Source counts include each admitted placed occurrence once.
Maps and vertex receivers use the chosen bounce depth together. Ambient and
details sample final output. LDR and HDR remain independent.

The option defaults off and requires both prerequisites. Turn it off before
disabling vertex lighting or choosing **Fullbright**; invalid changes keep the
previous settings. Eligible sources may have **Disable Shadows** enabled, and
receiver self-shadow choices remain effective.

Only admitted opaque or strict binary-cutout vertex-lighting materials qualify.
Missing source reflectivity, required textures or unsupported material effects
stop the build. A prop with **Disable Vertex lighting** enabled is not a source.
Runtime direct styles remain separate. Switched indirect ambient, intrinsic
prop emission, arbitrary model shaders and animated sources remain unsupported.

## Visibility and lighting options

Set fade distances, fade scale and DX-level range in **Advanced properties**.
Use **Disable Shadows**, **Disable Vertex lighting** and supported self-shadow or normal controls as required.
Keep defaults unless the intended game behavior needs a change.

## Native lighting origin

Set **Lighting Origin** to a named eligible point entity for supported native lighting-origin control.
Referenced assemblies apply their name-fixup rules before resolving that target.
Missing or ambiguous required targets fail explicitly.

## Ignore receiver normals

Use **Ignore surface normal for computing vertex lighting** only for the supported vertex-lighting path.
This changes receiver weighting, not shadow geometry or source normals.

## Props in referenced assemblies

Supported child static props compile with transformed positions, angles, skins and checked names.
Inspect **Instance contents…** before building. Child source files must remain available.

## Limits

- Models must be built as static props, with supported companion files and body 0.
- Static props retain unit scale, white tint and normal render-mode restrictions.
- Leaf membership uses conservative rotated header bounds, not exact model hulls.
- Packing does not change vertex lighting or shadow settings.
- Packed chains must end at `VertexLitGeneric` with one canonical `$basetexture`. Opaque terminals can add valid `$reflectivity`; strict cutouts can add only `$alphatest 1`.
- Other terminal parameters, authored alpha references and proxies remain unsupported.
- Cutout vertex-lighting receivers require one canonical basetexture and literal `$alphatest 1` at the default cutoff, with the same bounded Patch grammar as packing.
- Cutout vertex lighting is experimental.
- Missing or unsupported required data fails rather than substituting another asset.
- Sampled textures must meet the [native texture limits](textures.md#native-texture-files).
