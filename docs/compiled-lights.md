[← Manual](README.md)

# Compiled lights

Read native light records and their captured authored sources from the last successful build.

1. Compile with **Direct LDR** or **Direct LDR + HDR**.
2. Open **Build details… → Inspect compiled lights…**.
3. Choose **Channel**.
4. Read **Record**, **Kind**, **Style**, **Origin**, **Intensity**, **Flags** and **Source**.
5. Select a row for **Source association**, including an emitting face's original material, and other stored fields.
6. Use **Previous lights** and **Next lights** to change pages.
7. Select a current **point**, **spot** or **surface** record, then click **Frame light** to frame its stored origin or emission centroid.
8. For an available current source, click **Select source**, **Select source face** or **Select containing reference**.
9. To frame an authored sun/sky source, select its source, then use **Frame** in the viewport.
10. For a **point**, **spot** or **surface** row, click **Inspect source visibility**
    to open its stored source cluster in the [visibility inspector](visibility.md).

Use **Refresh inspection** after changing sources or building again.
The inspector is also available from **Build settings…** and explains when no build output is available.

**Frame light** keeps the current projection and frames a 32-unit box around the native origin. It rechecks the owned build and source guards when clicked. Map selection, history, settings and ordinary unapplied drafts remain unchanged. Finish or cancel an active viewport gesture or block preview before framing.

**Source association** identifies a captured light entity for point, spot and known sun/sky records. For generated surface lights, it identifies the compiled emitting face, original material and authored solid/side instead of inventing a light entity. Instance sources include their outer helper, leaf file and complete **Occurrence** route. Each route lists authored helper IDs and referenced files from root to leaf, distinguishing repeated uses of the same source.

Zero-intensity lights keep their identities; a wide authored spotlight may appear as a native point. Historical source addresses describe the build's original snapshot and cannot select today's source.

**Select source face** selects the exact root face in **Faces** mode.
**Select source** selects one root light entity in **Objects** mode. For an
instanced source, **Select containing reference** selects its outer root helper
in **Objects** mode. These actions replace selection and keep the camera, map,
Undo history and property drafts. Finish geometry drafts and viewport work first.
Selection does not expand groups, reveal hidden objects or open child files.
Missing source metadata blocks selection; native framing keeps its separate rules.

For a selected root face, **Inspect baked lightmaps… → Inspect selected authored face**
finds its compiled matches. MCP can also find an instance face's matches using
the exact captured occurrence; see [MCP](mcp.md).

**Inspect source visibility** uses the record's stored cluster from the same BSP.
It does not classify the light's position again. Historical records can be
inspected; showing visibility bounds requires a current build. PVS and PAS show
potential visibility or audibility. They do not show native light winners,
falloff, ray occlusion or exact finite-area visibility.

## Limits

- Record numbers belong to one build and channel. Source associations are captured during emission and checked against the exact channel table.
- Older builds without metadata and inconsistent metadata keep readable native rows with an explicit association-unavailable reason. A surface needs exactly one resolved authored root or instance face; multiple contributors and generated or unresolved sources remain unavailable.
- With **Native direct emission**, `surface` records are material-emission centroid point approximations. Their stored origin, normal and intensity do not establish finite-area response, exact visibility or which records the game selects.
- Occurrence routes capture up to four helper levels. Each helper ID belongs to the map containing that helper; its file identifies the next source. Routes describe the captured source snapshot. An expanded entity ID still distinguishes emitted entities within that build.
- Framing requires current output and an available channel. Historical records remain readable but cannot be framed.
- Only finite point/spot origins and surface centroids inside ±16384 units can be framed. Directional sky and unknown kinds have no frameable native point. For sun/sky, use an available authored source instead.
- Source visibility rejects directional sky, unknown kinds, unbaked channels,
  invalid record numbers and invalid source clusters.
- Values are native stored fields, without exposure or display normalization.
- Fullbright has no baked light tables. LDR builds have no HDR table.
- A valid table can contain zero records; missing or malformed tables fail explicitly.
- Intact stale output is historical. Missing, replaced or unverifiable output clears the displayed data.
- Inspection preserves the map, selection, camera and pending drafts.
- **Frame light** changes the camera only. It preserves source selection and adds no light marker or baked lighting preview.
