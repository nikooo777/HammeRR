[← Manual](README.md)

# Visibility clusters

Group open leaves into one visibility cluster, including dry, wet and portal leaves across visibility areas.

1. Draw a convex brush over the intended open volume.
2. Use **Tie to entity… → func_viscluster**.
3. Click **Tie selected brushes to entity**.
4. Click **Compile**, then open **Build details…**.
5. Read **Visibility** and **Authored visclusters** for emitted clusters, helpers,
   effective groups and grouped leaves.

Use a supported FGD through **Load FGD…** if the class is absent.
Supported child volumes also compile through referenced assemblies.

## Limits

- Use one visible convex brush per helper. Multi-brush helpers are unsupported.
- A leaf qualifies only when its checked intersection with the helper exceeds
  10% of the leaf's volume. Uncertain threshold cases stop the build. Solid leaves
  do not join groups.
- A leaf cannot qualify for two helpers. Ambiguous claims stop the build.
- Grouping preserves each leaf's area, water data, collision and ambient samples.
  It does not merge visibility areas or liquid volumes.
- PVS and PAS remain conservative and can expand. Grouping does not reduce the
  visibility solver's work.
- An active compile region must contain the complete visible helper brush.
  Crossing helpers stop the build; disjoint helpers are omitted.
- Existing geometry and work limits apply. Grouping does not repair leaks or invalid portals.

<!-- page-nav -->

---

[← Occluders](occluders.md) · [Manual](README.md) · [Status and limits →](status.md)
