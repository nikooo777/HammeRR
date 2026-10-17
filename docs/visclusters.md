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
4. Disable any active compile region and click **Compile**.
5. Open **Build details…**. The visibility summary distinguishes solved leaf
   clusters from emitted clusters and reports helpers, effective groups and
   grouped leaves. A helper selecting zero or one open leaf reports no reduction.

![Build details with authored visibility clusters](images/build-visclusters.png)

MCP uses the same `tie_to_entity` operation with `classname:"func_viscluster"`
and the same `compile_map` or `start_compile` build paths. No additional setting
or recipe option enables grouping. Builds consume the frozen unsaved map snapshot.

The volume qualifies an existing open leaf when their actual convex intersection
is strictly greater than 10% of the leaf's volume. AABB overlap and the leaf
center do not decide membership. Helper boundaries do not cut the BSP tree.
Near-threshold numerical uncertainty rejects the build and asks you to move
the boundary; it does not lower the overlap threshold.

Limits: full-map builds, 16 helpers and one visible convex brush
per helper, with at most 64 authored sides. The combined leaf/helper intersection
is limited to 128 planes and 2,048 polygon points. Geometry and grouping share
the bounded visibility work allowance. These are compiler admission limits.
Displacements, origin materials, runtime parents/angles, liquid/areaportal
members and groups crossing visibility areas reject. Two helpers qualifying for
the same leaf reject even if the helper brushes themselves do not overlap.
Hide or remove an invalid helper to compile without it. Multi-brush unions and
active compile regions remain unsupported.

The compiler solves its existing physical leaf graph, then unions source and
destination cluster groups and recomputes audibility. Every baseline visible
or audible leaf stays included. Individual leaves retain their bounds, faces,
collision and LDR/HDR ambient samples. Helpers do not render, collide, become
runtime entities or seed leak flood.

This version reduces emitted cluster count when helpers group multiple leaves.
It does not promise less compile work or smaller compressed visibility data;
denser shared visibility rows may increase the final lump. Exact Valve compiler
floating-point selection, runtime speed and native pixel parity remain unevaluated.
