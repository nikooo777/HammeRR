[← Manual](README.md)

# Baked prop vertices

Read the exact vertex-color bytes emitted for static props.

1. Enable **Bake static-prop vertex lighting** in **Build settings…**.
2. Choose **Direct LDR** or **Direct LDR + HDR**.
3. Compile.
4. Open **Build details… → Inspect baked prop vertices…**.
5. Choose **Channel**, a compiled prop and a hardware section.
6. Read **Vertex colors**.

Use **Previous props**, **Next props**, section controls and vertex controls to page through results.
Use **Refresh inspection** after a new build or source change.

## Limits

- Prop ordinals and hardware sections belong to one build, not authored entity IDs.
- Packed values are not an editor appearance preview or a screen-brightness measurement.
- Missing channels remain unavailable; they do not substitute another channel.
- Intact stale output is marked historical. Missing, replaced or corrupt owned output is unavailable.
- Inspection does not alter the map or build settings.
