[← Manual](README.md)

# Compiled lights

Read native light records and captured point/spot source identities from the last successful build.

1. Compile with **Direct LDR** or **Direct LDR + HDR**.
2. Open **Build details… → Inspect compiled lights…**.
3. Choose **Channel**.
4. Read **Record**, **Kind**, **Style**, **Origin**, **Intensity**, **Flags** and **Source**.
5. Select a row for **Source association**, attenuation, cone, normal and other stored fields.
6. Use **Previous lights** and **Next lights** to change pages.
7. Select a current **point** or **spot** record, then click **Frame light** to frame its stored origin in the viewport.

Use **Refresh inspection** after changing sources or building again.
The inspector is also available from **Build settings…** and explains when no build output is available.

**Frame light** keeps the current projection and frames a 32-unit box around the native origin. It rechecks the owned build and source guards when clicked. Map selection, history, settings and ordinary unapplied drafts remain unchanged. Finish or cancel an active viewport gesture or block preview before framing.

**Source association** identifies a captured root entity, or an instance's outer helper, leaf file, original entity and expanded entity for that build. **Occurrence** lists each authored helper ID and its referenced file in order from the root map to the leaf. Repeated uses of the same leaf file and entity remain distinct through their helper routes. Zero-intensity lights keep their identities; a wide authored spotlight may appear as a native point. Historical source IDs describe the build's original snapshot. They do not select or open today's source.

## Limits

- Record numbers belong to one build and channel. Source associations are captured during emission and checked against the exact channel table.
- Sky, surface and other native kinds show source association as unavailable. Older builds without metadata and inconsistent metadata keep readable native rows with an explicit association-unavailable reason.
- Occurrence routes capture up to four helper levels. Each helper ID belongs to the map containing that helper; its file identifies the next source. Routes describe the captured source snapshot. An expanded entity ID still distinguishes emitted entities within that build.
- Framing requires current output and an available channel. Historical records remain readable but cannot be framed.
- Only point and spot origins inside ±16384 units can be framed. Global, surface and unknown kinds are unavailable for framing.
- Values are native stored fields, without exposure or display normalization.
- Fullbright has no baked light tables. LDR builds have no HDR table.
- A valid table can contain zero records; missing or malformed tables fail explicitly.
- Intact stale output is historical. Missing, replaced or unverifiable output clears the displayed data.
- Inspection preserves the map, selection, camera and pending drafts.
- **Frame light** changes the camera only. It preserves source selection and adds no light marker or baked lighting preview.
