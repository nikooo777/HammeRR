[← Manual](README.md)

# Hollow, carve and clip

Turn solid geometry into rooms, openings and cut shapes.

Select brushes to show **Brush tools** in the Inspector. The same commands are in the **Brush** menu.

![Brush tools in the Inspector](images/inspector-brush.png)

## Hollow

1. Select the source brushes.
2. Open **Hollow brushes…**.
3. Set wall thickness.
4. Choose **Inside the original size** or **Outside the original size**.
5. Check the faces to remove under **Openings**.
6. Click **Preview walls**.
7. Inspect the cavity with **Look inside**.
8. Click **Create walls**.

![A hollow room with its roof and front removed](images/hollow-room.png)

Inside keeps the outer dimensions. Outside keeps the original interior dimensions.
Each source needs at least one wall. Open shells need other geometry to seal a playable map.

## Carve

1. Select cutters and targets.
2. Open **Carve brushes…**.
3. Choose **Cutter · shape to subtract** and **Target brushes**.
4. Enable **Use multiple cutter brushes** for more than one cutter.
5. Under **New cut faces**, choose cutter settings, corresponding target settings, or a named material.
6. For a named material, enter its name or click **Browse materials…**.
7. Click **Preview carve** and inspect the targets.
8. Click **Apply carve**.

![A doorway cut with a box and cylindrical cutter](images/carved-doorway.png)

Cutters remain in the map. Move or delete them afterward.
The default copies cutter surface settings. Target mode requires a unique
original target face parallel to each new face. Named material mode keeps the
cutter's texture placement and lighting settings. Unsupported matches reject
the complete preview; use cutter or named material mode for oblique cuts.
**Discard preview** keeps the source choices. **Cancel** closes the tool.

## Clip

1. Select brushes. Use **Select → Object selection → Solids** to choose a child
   brush inside an entity.
2. Click **Clip selected brushes**.
3. For terrain, choose a mode under **Terrain cut**.
4. Draw a line in a planar view, enter a plane, or choose **Pick three points**.
5. Choose **Arrow side**, **Opposite side** or **Both sides**.
6. Inspect the preview.
7. Click **Apply cut**.

![A diagonal cut creates a sloping brush](images/clipped-brush.png)

Use **Move plane** or **Rotate plane** to adjust the cut with handles.
**Cancel cut** or <kbd>Escape</kbd> discards the preview.
Changing **Terrain cut** discards the current preview, picked points and plane
positioning without changing the map or Undo history.
A child-brush cut keeps its entity and unselected siblings. Apply selects the
resulting child brushes. Selecting a whole entity clips all crossing children.

### Cut terrain on its existing grid

1. Select power-3/4 world or `func_detail` terrain brushes. Include any
   noncrossing neighbors you authorize the operation to partition.
2. Click **Clip selected brushes**.
3. Under **Terrain cut**, choose **Existing grid**.
4. Enable **Partition selected neighbors** if retained quarter-grid joins need
   extra partitions. It is off by default.
5. Set a plane through a complete middle row or column. Power 4 also permits
   quarter-grid lines: row or column 4 or 12 of its 16 cells.
6. Choose **Arrow side**, **Opposite side** or **Both sides**.
7. Inspect the native patches and separate counts of cut brushes and partitioned
   neighbors, then click **Apply cut**.

Each crossing source produces two patches per retained half for a middle cut,
at one lower power. A quarter cut produces four power-2 patches on its short
side and six power-2/3 patches on its long side. **Both sides** produces ten.
Partitioned neighbors add their own patches.
Grid spacing, paint samples and triangle flags stay the same.
The cut follows the displaced surface, not its backing brush.
It does not create a wall beneath the cut.
Matching neighbor borders are checked before Apply.
**Partition selected neighbors** refines only required regions of selected
noncrossing power-3/4 neighbors to retain native full/half joins. Regions can
reach native power 2: at most one level for power 3 and two for power 4.
The operation propagates through required selected neighbors within the shared
limits. Unselected and unrelated selected terrain stay unchanged.
Missing selection, insufficient native power or an unrepresentable result
rejects the complete request. The option never enables itself after a
failed cut. Changing it discards the preview; changing **Terrain cut** turns it off.
The preview also checks retained points at native game precision. A cut that
cannot keep those points and their side of the plane rejects.
Visible map instances use freshly resolved child terrain for these checks. Save
a new parent first. Child maps stay unchanged; prepare another preview after
child edits or a parent save.

![A painted terrain cut preserves its joined neighboring patches](images/terrain-grid-cut.png)

### Retile flat terrain

Use this mode to cut an eligible planar surface across its existing grid.

1. Select planar power-2/3/4 world or `func_detail` terrain. Its base cage
   must be planar; tilted cages and affine tangential offsets are supported.
   Select all patches that the cut must split.
2. Click **Clip selected brushes**.
3. Under **Terrain cut**, choose **Retile flat terrain**.
4. Draw a line, enter a plane, or choose **Pick three points**.
5. Choose **Arrow side**, **Opposite side** or **Both sides**.
6. Inspect the new native patches, then click **Apply cut**.

Each retained three-to-five-corner region becomes power-2 quadrilateral patches.
Four-corner regions stay one patch when their joins permit. Select the results
to cut them again. Compatible joined patches are checked as one component.
Joined surfaces may have different slopes on a common tilted or world-axis base
plane when they share one exact coordinate projection and pass native normal checks.
Each surface must follow one exact affine mapping from its base cage to a
visible plane. Paint must vary linearly across the surface, with native float
alpha from 0 to 255 that preserves its raw and native packed fields exactly.
Triangle flags and collision materials retain their original regions. Retiling
keeps these fields, material, texture placement and smoothing. It does not flatten a
curved or nonlinearly painted surface. **Existing grid** never switches modes
after a failed cut.

This mode also accepts qualified sources that are already coarsened by native LOD.

![A diagonal cut retiles flat terrain into native quad patches](images/terrain-flat-cut.png)

![Repeated cuts preserve a sloped terrain sheet](images/terrain-sloped-cuts.png)

![Repeated cuts preserve a two-texture terrain paint field](images/terrain-painted-cuts.png)


### Retile grid cells

Cut curved terrain along a complete existing row or column, including qualified
tilted bases.

1. Select the world or `func_detail` terrain to cut. Include noncrossing neighbors
   you authorize for whole-neighbor retiling.
2. Click **Clip selected brushes**.
3. Under **Terrain cut**, choose **Retile grid cells**.
4. Enable **Partition selected neighbors** if a retained full original edge needs
   a selected neighbor retiled.
5. Set a plane through one complete internal row or column of the visible surface.
6. Choose **Arrow side**, **Opposite side** or **Both sides**.
7. Inspect the cut and neighbor patches, then click **Apply cut**.

Each retained original cell becomes one power-2 patch. The original piecewise
surface, UVs, paint and triangle labels must stay exact. Nonlinear paint is
allowed when each new triangle retains its original raw and packed blend fields.
Normals and runtime LOD are regenerated. This mode does not cut through part of an original cell.
Qualified unchanged neighbors may remain at a corner, or lose an old edge contact
when the cut removes that entire join. Their authored grids stay unchanged.
With **Partition selected neighbors**, each required selected noncrossing
neighbor is retiled in full, with one power-2 patch per original cell. Corner-only,
severed and unrelated selected neighbors stay unchanged.

![Painted curved terrain retains its blend through grid-cell cuts](images/terrain-cell-cut.png)

### Cut through curved cells

Cut the original terrain triangles with a free plane.

1. Select world or `func_detail` terrain brushes. Include noncrossing neighbors
   you authorize for whole-neighbor retiling.
2. Click **Clip selected brushes**.
3. Under **Terrain cut**, choose **Retile surface triangles**.
4. Under **Blend samples**, choose **Exact fields**, **Native float** or
   **Nearest byte**.
5. Enable **Partition selected neighbors** if a retained full original edge needs
   a selected neighbor retiled.
6. Draw or enter the cut plane.
7. Choose **Arrow side**, **Opposite side** or **Both sides**.
8. Inspect the cut and neighbor patches and blend-error bounds, then click **Apply cut**.

Cut cells become quad fans. Whole retained cells become quarter-cell patches.
Geometry, UVs, triangle flags and collision-material regions stay exact.
**Partition selected neighbors** retiles each required selected noncrossing
neighbor in full, with four power-2 patches per original cell. Partial retained
edges or incompatible fan density can still reject. All output shares the
128-patch limit; **Both sides** may exceed it even when one retained side fits.
The option is off by default. Changing it discards the preview; changing
**Terrain cut** turns it off. A failed request preserves the previous settings
and valid draft.
**Blend samples** chooses how inserted paint samples are stored:

| Choice | Inserted samples | Raw error limit | Native packed error limit |
| --- | --- | --- | --- |
| **Exact fields** (default) | Preserve raw and all four native packed fields exactly. | 0 | 0 |
| **Native float** | Store fractional values at native float32 precision. | 2⁻¹⁷/255 | (1 + 3·2⁻¹⁷)/255 |
| **Nearest byte** | Round to the nearest integer, ties to even. | 0.5/255 | 1.5/255 |

Original samples stay unchanged. Fractional paint is supported when the selected
policy can certify it. A bound above its limit rejects the cut. The preview reports the policy, actual raw error bound,
largest error bound across the four native packed profiles, and their limits.
These bounds describe this cut's blend fields. Repeated rounding can accumulate
error; final pixels also depend on materials and lighting.
Changing **Blend samples** discards the old preview. Changing **Terrain cut**
resets **Blend samples** to **Exact fields**. **Native float** and **Nearest byte**
are available only for **Retile surface triangles**. A failed request keeps the
previous valid draft; no policy switches automatically after a failure.

![Both painted curved-terrain halves moved apart after a through-cell cut](images/terrain-surface-cut.png)

## Limits

- Hollow: 16 brushes, 64 faces per brush, 256 walls; thickness 0.125–4096 units.
- Carve: 1–8 cutters, 16 targets, 256 pieces, 64 faces per piece.
- Carving away every child brush of an entity rejects.
- Terrain cannot be hollowed or carved. **Existing grid** requires a power-3/4
  world or `func_detail` patch and a complete middle grid line, or a power-4
  quarter line.
  Matching full/half borders must retain their sample spacing and LOD state.
  Coarsened source and output patches must retain their native masks, positions,
  collision triangles and maximal render triangles. Unselected neighbors must
  keep their complete native state.
- **Existing grid** rejects other cut lines, power-2 splitting, quarter-edge
  links that cannot be resolved by **Partition selected neighbors**, changed LOD
  geometry and overlapping or unmatched borders. Neighbor partitioning refines
  required regions down to power 2 within the shared limits; it cannot partition
  unselected terrain or refine below native power 2.
  Missing or unsupported child sources stop the preview. Saved-hidden references
  are omitted; display filters do not remove compiler neighbors.
- **Retile flat terrain** requires a common planar base and orientation, including
  tilted bases. Joined planar surfaces can have different slopes when the component
  shares one exact coordinate projection and passes native arithmetic, orientation
  and conditioning checks. Curved neighbors, overlaps and unsupported partial joins reject.
  Paint must remain affine. A selected source with native LOD before the cut needs
  constant paint and uniform raw, native and physical triangle labels. It and all
  its outputs must pass exact LOD geometry, UV and full-region coverage checks.
  A source that was not coarsened before the cut cannot acquire output LOD.
  Unchanged neighbors must keep their complete LOD state.
  Nonzero offset normals and opaque metadata reject.
  Native positions, shared samples and texture fields must remain exact.
  Normal inputs must pass the native arithmetic and conditioning checks.
  Normal bits and final pixels can differ after retiling.
- Source paint must be representable as native float32 alpha from 0 to 255.
  **Exact fields** preserves raw alpha and the same native packed-alpha field
  under all four rounding profiles. Each output triangle must retain the original flags and, with `$surfaceprop2`,
  the original collision-material alpha class over its complete area. A triangle
  that crosses incompatible source regions rejects. Edge-only contacts do not
  transfer labels. Rounding-dependent collision classifications reject.
- Retiling requires direct static `LightmappedGeneric` or plain
  `WorldVertexTransition` materials. Both transition layers use their original
  textures and the primary mapping size. Texture inputs must be untransformed,
  ordinary single-frame VTFs with positive power-of-two dimensions no larger than 32768. Material and texture references
  use case-insensitive lookup.
  Use relative names without extensions or a `materials/` prefix.
  For `LightmappedGeneric`, an optional static `$detail` layer supports blend modes 0–3 and a factor from
  zero to one. Its scale must be an exact native float, written without an
  exponent in at most 32 characters. Omitted scale uses the native value 4.
  Each scaled UV sample must stay exact; some otherwise valid scales reject.
  Patch materials, other blend shaders, base transforms, animated or runtime textures
  and unsupported resources reject. Changed source images or materials require
  another preview before Apply.
- **Retile grid cells** requires an exact planar base with a rectangular cage in
  its base-plane coordinate chart. Tilted bases must pass the same native geometry
  and normal checks. Curved height and non-affine interior tangential warps are
  supported. Original projected borders may form simple convex or concave polygons.
  Border segments must not cross, touch unrelated segments or backtrack.
  Connected originals must share the exact base chart and orientation.
  Complete footprint checks must
  preserve their native contacts and complete shared seam polylines.
  The cut fails if footprints overlap or form extra unsewn contacts. Output patches retain
  straight, strictly ordered convex quad projections. Local cell and normal checks
  can still reject the cut. Every original
  and output triangle must preserve orientation and pass the exact native geometry
  and normal checks. Paint samples must preserve each original triangle's raw and
  packed blend fields exactly. Native texture rules match **Retile flat terrain**.
  Unchanged curved neighbors may retain a certified corner or have an old edge
  contact fully severed. They must share the exact base chart and orientation,
  with matching native contact samples, complete seam polylines and certified normals.
  Their authored grids and metadata stay unchanged.
  **Partition selected neighbors** explicitly permits whole retiling of selected
  noncrossing neighbors required by a retained matched full original edge.
  Every neighbor must pass complete source, field, material and metadata checks.
  Missing selection, unsupported partial edges, incompatible native density,
  affected LOD coarsening, contact mismatches, corner
  capacity overflow, folded projections, self-intersecting borders,
  extra contacts, unsupported degenerate contacts and uncertain
  native normal inputs reject.
  Each retained cell adds 32 native triangles. Powers 2–4 are subject to the
  shared output and work limits.
- **Retile surface triangles** requires a planar base with a convex cage in its
  exact coordinate chart. It shares the geometry, paint, texture, normal and
  contact checks of **Retile grid cells**. Output must fit native edge
  and corner limits without LOD coarsening. Skew quarter grids, high-valence
  corners, unrepresentable raw paint and changed native packed fields in
  **Exact fields** can refuse. The error names the unrepresentable field.
  **Native float** and **Nearest byte** do not relax geometry, UV, triangle-label
  or collision-material checks.
- Clipping accepts a valid ten-word VMF allowed-vertex cache. New patches reset
  present caches and keep absent caches absent. Malformed caches reject the cut.
  Native LOD masks still pass all checks.
- Terrain cuts accept 16 sources, including partitioned neighbors, and 128 output patches. Unsupported bindings,
  notes, native mask states and opaque payloads reject before the map changes.
- Numerically uncertain cuts reject. They do not move retained points to fit
  the new patch cages.
- Select one child in **Solids** mode, or the whole `func_detail` entity.
  Targeted crossing brushes and explicitly authorized required neighbors can
  change. Runtime brush-model terrain rejects.
- A connected cut checks at most 128 neighboring faces. Terrain and resolved
  instance context share a 64 MiB snapshot budget.
- Invalid, collapsed or excessively thin results reject before the map changes.

<!-- page-nav -->

---

[← Brushes and transforms](brushes.md) · [Manual](README.md) · [Polygon brushes →](polygon-brushes.md)
