[← Manual](README.md)

# Final compiled details

Show the final models, cards and cross/tri shapes from a successful build.
The layer includes material-scattered details and uses their native BSP placements.

1. Click **Compile**.
2. Open **Build details… → Compiled details…**. You can also use **Build settings… → Compiled details…**.
3. Click **Prepare details**. Use **Cancel preparation** to stop the reader.
4. Click **Show details**. A missing asset or unsupported material stops the complete layer.
5. Set **Preview lighting → Baked LDR** to show stored static colors. Use **Editor shading** for the normal preview.
6. Use **Refresh inspection**, **Previous records** and **Next records** to inspect placements and stored lighting.
7. Use **Hide details** to keep the preparation, or **Clear details** to release it.

![Final compiled cards and shapes in the editor](images/compiled-details.png)

Full and vertical cards face the current camera. **Prepare details** and **Clear details** reset lighting to **Editor shading**.

![Stored base colors on final model, card, cross and tri placements](images/compiled-detail-colors.png)

## Limits

- Compile again after map, build-setting or tracked asset changes. Then prepare the new output.
- Details do not affect authored selection, picking or Undo. Native ordinals belong to one build.
- Editing tools, geometry drafts and Faces mode hide the layer. Working region filters whole native props; authored hide/show stays separate.
- Only the current session's retained BSP is eligible. Arbitrary BSP import is unavailable.
- Models use skin 0 and body 0. Materials require supported static `UnlitGeneric` textures.
- **Baked LDR** uses the native base color with static style 0 at full weight. A record with runtime light styles stops the complete mode; the previous mode stays active.
- Baked colors clamp to the editor display range. Native gamma, dynamic lights and HDR are not reproduced.
- Sway, avoidance, runtime mirroring and fading are not displayed.
- A changed model still in use requires **Reload models**, another build and preparation.
- The layer accepts at most 16,384 records, 64 model assets and 128 materials. Exceeded asset or geometry budgets fail explicitly.

Use `inspect_compiled_details` and `set_compiled_details` through MCP.
