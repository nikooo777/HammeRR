[← Manual](README.md)

# Visibility clusters

Group compatible open leaves into one visibility cluster.

1. Draw a convex brush over the intended open volume.
2. Use **Tie to entity… → func_viscluster**.
3. Click **Tie selected brushes to entity**.
4. Compile.
5. Inspect visibility-helper and cluster counts in **Build details…**.

Use a supported FGD through **Load FGD…** if the class is absent.
Supported child volumes also compile through referenced assemblies.

## Limits

- Grouping can increase visibility. It does not merge geometry, collision or ambient samples.
- Eligible dry open leaves must satisfy the checked membership rules.
- Unsupported wet, solid, partial or ambiguous memberships report diagnostics.
- Compile regions must retain the required volume and source geometry.
- This is an optimization control, not a repair for leaks or invalid portals.
