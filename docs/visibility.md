[← Manual](README.md)

# Visibility inspection

Inspect the world leaves, portals and visibility sets from your last successful build.

1. Compile the map, then open **Build details… → Inspect visibility…**.
2. Use **Target → Summary** to list compiled world leaves. Choose a row to inspect that leaf's native bounds, final cluster and incident portals.
3. Choose **PVS** for visible destination clusters or **PAS** for audible destination clusters, then click **Inspect target**. Choose a destination row to inspect that cluster's member leaves.
4. Use **Previous rows**, **Next rows**, **Previous destinations** and **Next destinations** to change pages. You can also choose **Leaf**, **Cluster** or **Portal**, enter its **Ordinal**, and click **Inspect target**.
5. Select a leaf or portal and click **Frame selection**. **Show selected guide** displays its cyan bounds box or portal ring in the viewport. **Clear guide** removes it.
6. Use **Refresh inspection** after another build or source change.

![Compiler-owned portal ring in the viewport](images/visibility-portals.png)

Framing preserves your camera projection. Inspection, framing and guides preserve selection, history and unapplied drafts. Finish or cancel active viewport gestures and block previews before framing or showing a guide.

## Limits

- Leaf boxes are the native bounds, not the exact convex cells. Portal rings show compiler geometry rather than rendered faces. Only one selected guide is shown at a time.
- Leaf and cluster ordinals and retained portal slots belong to one build. These are not authored object IDs. Portal slots can have gaps because exterior, solid and unreachable endpoints are excluded.
- Visibility-cluster helpers can place several leaves in one final cluster. Their internal portal boundaries remain inspectable.
- PVS and PAS are separate native rows. They describe compiled visibility and audibility, not current game occlusion or sound mixing. The captured Fast or Detailed build mode remains authoritative.
- Fullbright builds still have visibility. Lighting channels do not change this inspector.
- Intact older output remains readable as historical data. Stale, missing or replaced output cannot be framed or shown as a guide. Source/output guards are checked when inspecting or changing the guide and during scene updates; unavailable output clears rows and totals.
- Freshness covers the committed map, settings, mounted-content epoch and retained source guards. It does not capture every world material. Displayed drafts were not compiled.
- Retention is limited to 64 MiB, 65,536 native leaves, 262,144 portals, 1,048,576 points and 4,096 points per portal. Exceeding a metadata limit makes inspection unavailable while ordinary compilation can still succeed. Queries return at most 64 rows per page.
- This inspector has no external BSP/PRT file input, authored selection mapping or viewport picking of compiled cells.
