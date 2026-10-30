[← Manual](README.md)

# Rope preview

Show the static shape of linked rope entities.

1. Place `move_rope` and `keyframe_rope` entities.
2. Give each endpoint a unique **Name**.
3. Set each source's `NextKey` to the next endpoint's name.
4. Set `Slack`, `Width` and `Subdiv` on each source.
5. Leave the final endpoint's `NextKey` empty.
6. Enable **Rope preview** in **Overlays**.
7. Check the span count and diagnostics.

![A rope hangs between two masonry posts](images/rope-span.png)

Click a span to select its source entity. Edit an endpoint to update the curve.
Clear **Rope preview** to hide the curves without changing the map.

## Frame and select a span

1. Enable **Rope preview**, then select a span or its source endpoint.
2. Press **F** to frame that source's helper and complete outgoing span, including its width.
3. Use a marquee or **Select a volume…** around the sagging curve to select its source.
4. In the volume dialog, choose **Fully inside the box** to require the entire helper and outgoing span, or **Intersects the box** to accept overlap. Marquee uses **Fully inside** and **Intersects**.

Framing, object inspection and area/volume selection share conservative axis-aligned bounds around the displayed sampled curve and half its **Width**.
A span belongs to its source. Selecting only its target does not include the incoming span; that target keeps its own helper and outgoing span.
Changing **Rope preview** retires pending marquee and volume reviews. Rejected, hidden or working-region-excluded spans retain ordinary endpoint behavior.

Transform pivots, snap anchors, placement and paint keep their ordinary endpoint-helper bounds. Raw map statistics also remain source bounds.

## Limits

- The shape is a static downward parabola, not a physics simulation.
- Slack: 0–65536 units. Width: 1–64 units. Subdiv: 0–8.
- Missing values use FGD defaults: 25, 2 and 2.
- Missing, ambiguous, cyclic or self-links report errors. Both endpoints must be visible.
- Parenting, attachment binding, dangling ropes and unsupported interpolators are unavailable.
- No rope material, wind, collision, stiffness, breakage or runtime animation preview.
- Bounds-based selection can include empty space inside the curve's axis-aligned box; it does not test exact tube intersection.
