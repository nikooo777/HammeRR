[← Manual](README.md)

# Baked prop vertices

Inspect captured static-prop sources and baked vertex colors.

1. To read colors, enable **Bake static-prop vertex lighting** in **Build settings…**.
   Choose **Direct LDR** or **Direct LDR + HDR**.
2. Compile. Source addresses and framing also work without baked colors.
3. Open **Build details… → Inspect baked prop vertices…**.
4. Choose a compiled prop. For colors, choose **Channel** and a hardware section.
5. Read the captured source and **Vertex colors**.
6. Click **Frame prop origin** to center its compiled position.

Use **Previous props**, **Next props**, section controls and vertex controls to page through results.
Use **Refresh inspection** after a new build or source change.
Reduce **Props per page** if long captured source paths exceed the page budget.

Root sources show their authored entity ID. Instance sources show the outer helper,
every nested helper/file occurrence, the original leaf entity ID and the expanded
ID used by that build. Repeated references to the same child remain distinct.
Disabled props retain their source address even when they have no stored colors.
**Historical source** describes the captured build after source changes.

**Frame prop origin** centers a 32-unit point box. It keeps projection,
selection, history and ordinary drafts. It works for disabled or unbaked props
and does not need source metadata. It does not fit model bounds or select an
authored object. Finish active viewport work first. Refresh or compile again
after source changes.

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
