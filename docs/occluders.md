[← Manual](README.md)

# Occluders

Compile invisible blockers that can hide covered renderable objects in the game.

1. Create 1–16 convex helper brushes inside permanent opaque cover.
2. Apply `tools/toolsoccluder` to contributing faces.
3. Use `tools/toolsnodraw` on unused faces.
4. Select the brushes and use **Tie to entity… → func_occluder**.
5. Set **Advanced properties → Start Active** to **Yes** or **No**.
6. Click **Compile**, then open **Build details…**.
7. Read **Occluders**, **Polygons**, **Vertex references** and **Diagnostics**.

The compiler builds one exterior boundary across all admitted helpers.
Each helper keeps its own control record, **Start Active** value, name and connections.
Shared faces belong to the first contributing brush in compile order.
Active and inactive helpers both take part in this pass.

A valid helper can have no contributing faces.
Global clipping can also remove a duplicate or fully covered boundary.
The compiler keeps an empty record for each such helper. It does not renumber later records.
**Occluders** counts these records; **Polygons** and **Vertex references** count retained geometry.

For root-map control, name the helper and target it with `Activate`, `Deactivate` or `Toggle` connections.
Supported unnamed child occluders can compile through [referenced assemblies](instances.md).
These inputs enable or disable retained polygons. They do not restore faces removed during compilation.

## Limits

- Helpers stay fixed in world space. They provide no visible wall or collision geometry.
- Each helper's retained polygons must remain inside one visibility area. Split helpers that cross areas.
- Disconnected islands, oblique edge/point contacts, shared faces and checked shallow overlaps retain their exterior boundaries.
- Source boundaries and generated cuts must keep their topology after native vertex conversion. Collapsed gaps, lost slivers, nonplanar stored rings and ambiguous conversions fail explicitly. Even a small authored offset can differ from an exact shared boundary.
- Checked arithmetic and shared work, point and fragment limits remain. Invalid geometry is not replaced with an empty record.
- Child runtime parenting and named control are unsupported.
- Child helper counts do not imply parent viewport geometry or picking.
- Native sky-view occlusion uses the player's connected-area mask. A helper in a separate sky-only area does not hide that sky's props.

<!-- page-nav -->

---

[← Material detail scattering](material-detail-scattering.md) · [Manual](README.md) · [Visibility clusters →](visclusters.md)
