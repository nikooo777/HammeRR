[← Manual](README.md)

# Baked prop vertices

Read the exact vertex-color bytes emitted for static props and their captured authored source addresses.

1. Enable **Bake static-prop vertex lighting** in **Build settings…**.
2. Choose **Direct LDR** or **Direct LDR + HDR**.
3. Compile.
4. Open **Build details… → Inspect baked prop vertices…**.
5. Choose **Channel**, a compiled prop and a hardware section.
6. Read the source address above **Vertex colors**, then inspect the stored colors.

Use **Previous props**, **Next props**, section controls and vertex controls to page through results.
Use **Refresh inspection** after a new build or source change.
Reduce **Props per page** if long captured source paths exceed the page budget.

Root sources show their authored entity ID. Instance sources show the outer helper,
every nested helper/file occurrence, the original leaf entity ID and the expanded
ID used by that build. Repeated references to the same child remain distinct.
Disabled props retain their source address even when they have no stored colors.
**Historical source** describes the captured build after source changes.

## Limits

- Physical paths that cannot be reported show stale provenance and omitted path fields.
- Prop ordinals and hardware sections belong to one build. Source addresses identify captured authored entities; they do not resolve, select or frame current objects.
- Missing or unverified source metadata is reported separately; valid native prop and color rows remain available.
- Captured prop metadata is limited to 8 MiB, including shared occurrence storage once. Source copies are admitted before allocation within 128 KiB per report; each report is limited to 256 KiB.
- Hardware vertices are not mapped to model vertex positions.
- Packed values are not an editor appearance preview or a screen-brightness measurement.
- Missing channels remain unavailable; they do not substitute another channel.
- Intact stale output is marked historical. Missing, replaced or corrupt build output is unavailable.
- Inspection does not alter the map or build settings.
