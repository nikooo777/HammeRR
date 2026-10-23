[← Manual](README.md)

# Compiled lights

Read native light records from the last successful owned build.

1. Compile with **Direct LDR** or **Direct LDR + HDR**.
2. Open **Build details… → Inspect compiled lights…**.
3. Choose **Channel**.
4. Read **Record**, **Kind**, **Style**, **Origin**, **Intensity** and **Flags**.
5. Select a row for attenuation, cone, normal and other stored fields.
6. Use **Previous lights** and **Next lights** to change pages.

Use **Refresh inspection** after changing sources or building again.
The inspector is also available from **Build settings…** and explains when no owned output exists.

## Limits

- Record numbers belong to one build and channel, not authored entities.
- Values are native stored fields, without exposure or display normalization.
- Fullbright has no baked light tables. LDR builds have no HDR table.
- A valid table can contain zero records; missing or malformed tables fail explicitly.
- Intact stale output is historical. Missing, replaced or unverifiable output clears the displayed data.
- Inspection preserves the map, selection, camera and pending drafts.
