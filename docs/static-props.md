[← Manual](README.md)

# Static props

Place static scenery with model assets, native collision choices and visibility membership based on the model's bounds.

1. Click **Entity…**, choose `prop_static`, then **Place entity** inside a sealed map.
2. Click **Browse…** beside **World Model**, choose a model built as a static prop, then **Use model**. Keep scale 1 and body 0. See [Models](models.md) for skin previews and model inspection.
3. Set **Position**, **Rotation** and **Skin** in the inspector. The compiler uses the same saved transform. Rotated props use a clipped, rotated box containing both the model's render bounds and collision bounds to determine which open world leaves can display them.
4. Click **Compile**. Static props use collision supplied by their model assets; the map's **Physics bodies** setting controls separately generated brush, terrain and fluid bodies.
5. Open **Build details…** and inspect **Static props**, **Model assets**, **Leaf references** and **Diagnostics**. Missing or unsupported requested assets stop publication and preserve the previous output. Fix the asset or properties and compile again.

![Choose a static model and skin](images/model-browser.png)

![Build results and diagnostics](images/build-status.png)

Limits: MDL versions 44/48, scale 1 and body 0. Native static-prop collision choices remain none, bounding box or model VPhysics; VPhysics needs the model's checked `.phy` asset. Leaf clipping uses header boxes rather than the exact mesh or physics hull, so it can retain extra leaves. It preserves the existing 0.01-unit world-axis boundary padding, omits solid leaves and keeps the existing unrotated membership. It does not change collision, shadows, materials or baked lighting.

HammeRR bounds static-prop leaf traversal and clipping to 16 million work units and four million cumulative reserved polygon points per build. A clipped fragment reserves at most 4,096 points and 256 faces. Exceeding these budgets, an invalid asset or a prop with no open leaf fails before publication. Existing native prop/model/leaf-table limits also apply. The source map and Undo history stay unchanged during compilation.

Agents use the same `create_entity`, `set_entity_properties`, `compile_map` and `start_compile` operations.

## Bake vertex lighting

Bake direct light into supported static models instead of relying on the game's
ambient samples. The result is stored in the BSP for every declared draw LOD.

1. Open **Build settings…** and choose **Build lighting → Direct LDR** or
   **Direct LDR + HDR**.
2. Choose **Indirect light → Direct only**, then enable
   **Bake static-prop vertex lighting**.
3. Click **Close**. Optional: select a prop and enable **Advanced properties**. Enter
   `disableselfshadowing` in **Property key**, enter `1` in **Value**, then
   press Enter. This removes only that prop's own geometry from its lighting
   rays. Use `0` to retain self shadows.
4. Click **Compile**. Unsupported requested props stop the
   build; **Diagnostics** identifies the affected object.
5. Open **Build details…** to inspect **Static-prop vertex lighting** and
   **Prop vertex lighting bytes** for the current published build.

![Static-prop vertex lighting in Build settings](images/build-prop-vertex-lighting.png)

Limits: this is an experimental, default-off direct-light bake for simple opaque
models. Body 0, root LOD 0, unit scale, one bodypart/model and plain selected
**VertexLitGeneric** materials are required. LDR and HDR are evaluated
independently. Self-shadow control applies to point/spot lights, sun and sky
ambient; other props, world brushes and terrain still block light. It does not
move embedded vertices or admit unsupported support contacts. See [the complete limits](compile-and-play.md#static-prop-vertex-lighting)
before enabling it on a larger map. Per-texel prop lightmaps and reflected prop
sources remain unavailable.

Agents enable the same setting with `set_compile_settings` and
`bake_static_prop_vertices: true`. Use `set_entity_properties` with
`set: {"disableselfshadowing": "1"}` for the same per-prop shadow choice.
