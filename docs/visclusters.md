[← Manual](README.md)

# Authored visibility clusters

`func_viscluster` combines selected BSP leaves into one visibility cluster.
Use it when several nearby open leaves should share visibility. Geometry and
collision stay separate. Visibility can expand after grouping.

1. Create one ordinary convex brush covering the intended volume.
2. Select it and click **Tie to entity…**.
3. Choose or enter `func_viscluster`, then click **Tie selected brushes to entity**.
   If needed, use **Load FGD…** to load the installed CS:S Source SDK
   `source2009/bin/cstrike.fgd`; its included `base.fgd` defines this solid class.
4. For a region build, include the complete helper brush in **Region build…**,
   click **Preview region**, then **Apply region**. Click **Compile**.
5. Open **Build details…**. The visibility summary distinguishes solved leaf
   clusters from emitted clusters and reports helpers, effective groups and
   grouped leaves. A helper selecting zero or one open leaf reports no reduction.

![Build details with authored visibility clusters](images/build-visclusters.png)

MCP uses the same `tie_to_entity` operation with `classname:"func_viscluster"`
and the same `compile_map` or `start_compile` build paths. No additional setting
or recipe option enables grouping. Builds consume the frozen unsaved map snapshot.

## Referenced volumes

1. Create and save the volume in a child VMF using the same **Tie to entity…**
   steps above.
2. In the parent map, use **Instance reference…** to place the child. Move or
   rotate the reference to position the volume; keep the child helper's own
   entity angles absent or zero.
3. Click **Compile**, then open **Build details…**. **Visibility helpers:**
   counts expanded child volumes before region selection. The visibility summary
   counts retained root and child helpers after that selection.
4. Open **Instance contents…** and click **Refresh instance contents** to inspect
   child sources and counts. The volumes have no reference surfaces, icons,
   picking bounds or framing.

## Membership and limits

The volume qualifies an existing open leaf when their actual convex intersection
is strictly greater than 10% of the leaf's volume. AABB overlap and the leaf
center do not decide membership. Helper boundaries do not cut the BSP tree.
Near-threshold numerical uncertainty rejects the build and asks you to move
the boundary; it does not lower the overlap threshold.

Full-map and compile-region builds support 16 retained helpers across the root
and all child occurrences. Each helper requires one visible convex brush with
at most 64 authored sides. The combined leaf/helper intersection
is limited to 128 planes and 2,048 polygon points. Geometry and grouping share
the bounded visibility work allowance. These are compiler admission limits.
Displacements, origin materials, runtime parents/angles, liquid/areaportal
members and groups crossing visibility areas reject. Two helpers qualifying for
the same leaf reject even if the helper brushes themselves do not overlap.
Hide or remove an invalid helper to compile without it. Multi-brush unions
remain unsupported. In a region build, contained helpers stay whole, hidden or
wholly exterior helpers are omitted, and crossing helpers stop publication. The
marker origin does not select the volume. Membership uses the sealed regional
leaves, so its groups can differ from a full-map build.

In referenced assemblies, Hidden, all-hidden-brush and region-exterior
helpers are omitted. Crossing helpers reject. Source geometry and each
intermediate transform still validate before regional omission; expansion has
its own work allowance. Failures identify the root reference, child file and
original helper. See [Referenced assemblies](instances.md) for child field limits.

The compiler solves its existing physical leaf graph, then unions source and
destination cluster groups and recomputes audibility. Every baseline visible
or audible leaf stays included. Individual leaves retain their bounds, faces,
collision and LDR/HDR ambient samples. Helpers do not render, collide, become
runtime entities or seed leak flood.

This version reduces emitted cluster count when helpers group multiple leaves.
It does not promise less compile work or smaller compressed visibility data;
denser shared visibility rows may increase the final lump. Exact Valve compiler
floating-point selection, runtime speed and native pixel parity remain unevaluated.
