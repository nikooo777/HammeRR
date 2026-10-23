[← Manual](README.md)

# Static-prop mesh shadows

Use static model triangles to block baked light.

1. Place a compatible `prop_static` with **Entity…**.
2. Choose its model through **World Model → Browse…**.
3. Open **Build settings…**.
4. Choose **Direct LDR** or **Direct LDR + HDR**.
5. Compile, then inspect **Prop mesh shadows** and **Diagnostics**.

![Baked prop and brush shadows in the courtyard](images/courtyard-native-shadows.png)

Texture shadows require a model compiled with `$casttextureshadows`.
Flagged models can use supported alpha-tested or translucent texture shadows.
Unsupported requested texture-shadow effects fail the build.
Unflagged models cast opaque triangle shadows and report approximation warnings for unevaluated transparency.

## Limits

- Static LOD0 geometry only; runtime animation is not baked.
- Material, texture format and transform support is bounded.
- **Disable Shadows** excludes the prop from this shadow path.
- Shadow support does not imply vertex-lighting or asset-packing support.
- This page describes compiled shadows. The editor's authored-lighting preview is unshadowed.
