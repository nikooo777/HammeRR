[← Manual](README.md)

# Brushes and transforms

## Drawing brushes

In Top, Front or Side:

1. Click **Draw brush**.
2. Set **Start** and **Depth** for the axis that points into the view (Z in Top, Y in Front, X in Side).
3. Drag out the footprint. The preview shows its snapped size.
4. Release to create the brush. Escape cancels; **Done** returns to selection.

In Perspective, **Add brush** places a default block.

Use [Polygon brushes](polygon-brushes.md) to draw a convex or concave outline and extrude it into editable solids.

## Transform handles

Choose **Move handles**, **Rotate handles** or **Scale handles** in the viewport toolbar, then drag:

- **Move**: an arrow moves along one axis; a square moves within a plane.
- **Rotate**: a ring rotates around its axis.
- **Scale**: an axis handle scales along one axis; the center handle scales uniformly.

Release to apply. Escape or **Cancel** discards the drag.

- **World axes** / **Local axes** sets the handle orientation. Local axes follow the first selected brush or the entity's angles.
- **Snap** uses the grid for moving and scaling. **Angle snap** sets the rotation step.

## Exact transforms

Check **Transform selection** in the inspector and choose **Move**, **Rotate**, **Scale**, **Mirror** or **Shear**.

| Mode | Input |
| --- | --- |
| **Move** | An XYZ offset in units. |
| **Rotate** | Degrees around X, Y and Z. |
| **Scale** | A positive factor per axis. Brushes are resized; entity origins are spaced apart. |
| **Mirror** | The axes to reflect across. See [Mirror](#mirror). |
| **Shear** | One coordinate shifted in proportion to another. See [Shear](#shear). |

The **Pivot** can be the **Selection center**, the **World origin** or **Custom** coordinates. The pivot and axis choices also apply to the handles.

Typed transforms are exact and ignore grid snapping. While a transform preview is open, from the handles, the inspector or an agent, the inspector shows its values and a summary of every operation in it. Click **Apply transform** or **Cancel** before starting another.

### Snap selection to grid

Move an off-grid selection onto the grid without changing its shape or the spacing between objects:

1. Select the objects and set **Grid** in the viewport toolbar.
2. Check **Transform selection** in the inspector.
3. Click **Preview snap to grid**. Its helper text names the **Anchor: selection bounds minimum**.
4. Review the **Anchor** coordinates and movement in the transform summary. The anchor is the minimum corner of the combined world bounds.
5. Click **Apply transform** for one Undo step, or **Cancel** to discard the preview.

This explicit action uses the current grid spacing even when **Snap** is off. All selected objects move by the same XYZ offset; individual vertices and entity origins are not rounded separately. Bounds include displaced terrain, loaded models or their helper fallback. **Follow move and rotation** controls texture attachment. An anchor already on the grid produces no map edit; floating-point roundoff counts as aligned. Halfway coordinates round away from zero. World/local axes and custom pivots do not change this world-space anchor.

**Reload models** cancels a pending snap preview because the bounds may change. Click **Preview snap to grid** again after reloading assets. Missing or unsupported model assets use their helper bounds; aligning those helpers does not align a model that becomes available later.

Agents use `preview_snap_to_grid`, then the same transform inspection, commit and cancellation tools.

### Textures during transforms

Two independent options control what happens to textures:

- **Follow move and rotation**: the texture moves and turns with the brush. When off, the texture stays fixed in world space.
- **Stretch textures when scaling**: the texture stretches with the brush. When off, it keeps its size and repeats as the brush grows.

With both off, the brush keeps its original world-space texture projection. The scale handles' toolbar has the same options.

| Texture keeps its size | Texture stretches with the brush |
| --- | --- |
| ![A resized brush with repeated texture tiles](images/object-texture-repeat.png) | ![The same resized brush with stretched texture tiles](images/object-texture-stretch.png) |

### Mirror

1. Choose **Mirror** and check **Mirror X**, **Mirror Y** and/or **Mirror Z**.
2. Choose world or local axes and a pivot.
3. Click **Mirror selection**.

- **Mirror textures with geometry** flips the texture with each face. When off, faces keep the world projection.
- Brush entities keep their brushes, and their origin is mirrored. Face IDs are preserved.
- A selection that includes point entities or terrain is rejected without changes.
- The result must stay within ±65,536 units. The playable area is smaller; see [Compile and play](compile-and-play.md#coordinate-limits).

### Shear

Shear shifts one coordinate in proportion to another, for example to slant a wall.

Set **Shift coordinate** X, **By coordinate** Y and **Factor** 0.5: every point moves along X by half its Y distance from the pivot.

- Factors range from −64 to 64, and the two axes must differ. A factor of 0 changes nothing.
- **Keep textures attached while shearing** slants the texture with the faces. When off, faces keep the world projection.
- Brush entities keep their brushes, and their origin moves with the geometry. Point entities and terrain are rejected.

| Before shear | After shear with attached textures |
| --- | --- |
| ![An oblique brush before shearing](images/brush-shear-before.png) | ![The same brush sheared along X by its Z coordinate](images/brush-shear-after.png) |

## Brush shapes

Build wedges, cylinders, cones, spheres and arches from exact dimensions.

1. Open **Brush shapes…**.
2. Choose **Wedge / ramp**, **Cylinder**, **Cone**, **Faceted sphere** or **Arch / ring**.
3. Enter the bounds and the height axis, and browse for a material.
4. Set the segment count for cylinders, cones and spheres. Arches also take an inset, a start angle and a sweep.
5. Click **Preview shape**. **Focus preview** frames it.
6. **Add shape** creates and selects the brushes.

- Curves are polygons that fit inside the bounds. The preview reports the actual size and face count.
- Arch bounds describe the full ellipse: a 180° sweep builds half of it and 360° builds a ring.
- Arches are made of several convex brushes. Every shape is ordinary VMF geometry that you can keep editing.

| Shape controls | Shapes built by an agent |
| --- | --- |
| ![Brush shape dimensions, material preview and cancellation controls](images/brush-shapes.png) | ![Wedge, cylinder, cone, sphere and segmented arch](images/brush-primitives.png) |
