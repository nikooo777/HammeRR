[← Manual](README.md)

# Visibility clusters

Group open leaves into one visibility cluster, including dry, wet and portal leaves across visibility areas.

1. Draw one or more convex brushes over the intended open volume.
2. Use **Tie to entity… → func_viscluster**.
3. Click **Tie selected brushes to entity**.
4. Click **Compile**, then open **Build details…**.
5. Read **Visibility** and **Authored visclusters** for emitted clusters, helpers,
   effective groups and grouped leaves.

Use a supported FGD through **Load FGD…** if the class is absent.
Supported child volumes also compile through referenced assemblies.

## Limits

- Use 1–16 visible convex brushes per helper. Their union can be concave or disconnected.
  Overlapping volume counts once.
- A leaf qualifies only when its checked intersection with the complete union exceeds
  10% of the leaf's volume. Uncertain threshold cases stop the build. Solid leaves
  do not join groups.
- A leaf cannot qualify for two helpers. Ambiguous claims stop the build.
- Grouping preserves each leaf's area, water data, collision and ambient samples.
  It does not merge visibility areas or liquid volumes.
- PVS and PAS remain conservative and can expand. Grouping does not reduce the
  visibility solver's work.
- An active compile region must contain every visible helper brush.
  Crossing helpers stop the build; disjoint helpers are omitted.
- Existing geometry and work limits apply. Grouping does not repair leaks or invalid portals.
- A helper supports 64 sides per brush, 256 source sides and 8192 source hull points.
  Its checked union supports 256 retained fragments and 65,536 retained points.
  Uncertain splits or exceeded work and allocation limits stop the build.

<!-- page-nav -->

---

[← Occluders](occluders.md) · [Manual](README.md) · [Status and limits →](status.md)
