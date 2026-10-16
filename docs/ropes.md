[← Manual](README.md)

# Previewing authored ropes

Show a bounded static approximation of authored rope spans while keeping the rope entities available for ordinary selection and editing.

1. Open a map containing `move_rope` and `keyframe_rope` entities, or create them with **Entity…**. Give each linked endpoint a unique `targetname`.
2. In the source entity's properties, set `NextKey` to the next rope endpoint's exact `targetname`. Author `Slack`, `Width` and `Subdiv` on the source of each span. Leave `NextKey` empty on the final `keyframe_rope`.
3. Under **VIEWPORT**, check **Rope preview**. The status shows the number of static spans and any unresolved or unsupported link. Perspective and planar views support the preview in every viewport mode.
4. Click a span to select its source entity. The existing endpoint helpers remain visible and selectable. Edit its properties to change the span, or hide an endpoint to suppress it. **Undo** restores the authored settings and preview together.
5. Clear **Rope preview** to hide the curves. The toggle changes the shared view without modifying the map, selection or undo history.

MCP uses the same Session operations. `set_view` accepts `rope_preview`; `get_rope_preview` returns the shared setting, checked links, sampled centerlines, authored width/slack, diagnostics and limits. `get_map_info.rope_preview` reports the same state. `render_view` accepts a temporary `rope_preview` override and includes the effective report in its metadata. Pass the same optional override to `pick_at_pixel` when picking that image; neither override changes the shared view.

![Static rope span using authored slack and width](images/rope-preview.png)

## Limits

Preview starts off. Each source connects only to one exact, case-insensitive `NextKey` target among authored `move_rope` or `keyframe_rope` entities. Missing, ambiguous, wildcard, self and cyclic links produce diagnostics and no substitute geometry. Both endpoints must be visible and inside the working region. A source outside the region contributes no span. Tool-texture filtering leaves these entity helpers available.

`NextKey` and `targetname` identifiers accept at most 256 UTF-8 bytes. Oversized identifiers and duplicate case-insensitive targetname keys fail visibly; accepted names are matched in full. Diagnostic excerpts retain at most 64 bytes of a name plus an ellipsis, including when preview is off. The imported source text remains unchanged.

The static downward parabola is fitted to the sampled length `endpoint distance + Slack`. It is an authoring approximation, not a simulated or native engine pose. `Slack` accepts 0–65536 units, `Width` accepts 1–64 units, and integer `Subdiv` accepts 0–8. Missing values use the installed Source FGD defaults: 25, 2 and 2 respectively. Each span uses 8–64 curve segments with a six-sided tube, and at most 512 visible rope nodes are inspected. Excess nodes are counted in the report. Endpoints and the complete tube must stay within ±65536 units. Invalid or duplicate consumed properties fail visibly.

Only `Type=0`, `PositionInterpolator=2` for `move_rope`, and linked endpoints with `Dangling=0` are supported. Positive Slack between vertically aligned endpoints requires a physics pose and is diagnosed instead of inferring a lateral shape. Parenting and attachment binding are unsupported. The neutral helper appearance does not render `RopeMaterial`, `TextureScale` or `Barbed`. Wind, collision, stiffness, breakage, animation, runtime I/O and physics are not evaluated. The compiler and saved VMF behavior are unchanged.

A tube pick selects its source entity and competes with visible map geometry by depth. Group and selection behavior remain shared with other object picks. Face selection ignores rope tubes. Area selection and framing continue to use endpoint helpers, so they do not follow a sagging curve's full extent.
