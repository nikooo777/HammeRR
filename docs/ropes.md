[← Manual](README.md)

# Rope preview

Show the static shape of linked rope entities.

1. Place `move_rope` and `keyframe_rope` entities.
2. Give each endpoint a unique **Name**.
3. Set each source's `NextKey` to the next endpoint's name.
4. Set `Slack`, `Width` and `Subdiv` on each source.
5. Leave the final endpoint's `NextKey` empty.
6. Enable **Rope preview** in **Viewport**.
7. Check the span count and diagnostics.

![A rope hangs between two masonry posts](images/rope-span.png)

Click a span to select its source entity. Edit an endpoint to update the curve.
Clear **Rope preview** to hide the curves without changing the map.

## Limits

- The shape is a static downward parabola, not a physics simulation.
- Slack: 0–65536 units. Width: 1–64 units. Subdiv: 0–8.
- Missing values use FGD defaults: 25, 2 and 2.
- Missing, ambiguous, cyclic or self-links report errors. Both endpoints must be visible.
- Parenting, attachment binding, dangling ropes and unsupported interpolators are unavailable.
- No rope material, wind, collision, stiffness, breakage or runtime animation preview.
- Selection bounds and area selection follow endpoint helpers, not the sagging curve.
