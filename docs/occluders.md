[← Manual](README.md)

# Occluders

Compile invisible blockers that can hide covered renderable objects in the game.

1. Create 1–16 convex helper brushes inside permanent opaque cover.
2. Apply `tools/toolsoccluder` to contributing faces.
3. Use `tools/toolsnodraw` on unused faces.
4. Select the brushes and use **Tie to entity… → func_occluder**.
5. Set **Advanced properties → Start Active** to **Yes** or **No**.
6. Compile.
7. Inspect **Occluders**, **Polygons**, **Vertex references** and **Diagnostics**.

For root-map control, name the helper and target it with `Activate`, `Deactivate` or `Toggle` connections.
Supported unnamed child occluders can compile through [referenced assemblies](instances.md).

## Limits

- Helpers are invisible and provide no wall or collision geometry.
- Each helper must remain inside one visibility area; split helpers that cross areas.
- Supported islands, contacts and overlaps retain exterior polygons and gaps.
- Child runtime parenting and named control are unsupported.
- Child helper counts do not imply parent viewport geometry or picking.
