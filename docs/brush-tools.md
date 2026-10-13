[← Manual](README.md)

# Hollow, carve and clip

## Hollow

Turn solid brushes into rooms with walls.

1. Select solid brushes or brush entities and open **Hollow brushes…**.
2. Set the wall thickness and choose:
   - **Inside the original size**: the outer shape stays the same.
   - **Outside the original size**: the original shape becomes the empty interior.
3. Under **Openings**, check the faces that should stay open, such as the roof. Each brush needs at least one wall.
4. Click **Preview walls**. **Look inside** moves the camera into the cavity (pick its number when hollowing several brushes). Wireframe mode also shows the walls from outside.
5. **Create walls** replaces the originals.

- Thickness is measured perpendicular to each face, including slopes.
- At an opening the original boundary stays in place, so an open roof keeps the side walls at full height.
- Each wall is an ordinary convex brush. Inner faces inherit the material, texture mapping and lighting settings of the face they came from. Brush entities keep ownership of their walls.
- A shell with openings needs other geometry to seal the map.

**Limits:** 16 source brushes, 64 sides per brush, 256 walls, and a thickness from 0.125 to 4096 units. Terrain can't be hollowed.

| Hollow controls | Inside the preview |
| --- | --- |
| ![Wall thickness, opening faces and cancellable shell preview](images/hollow-brushes.png) | ![Textured interior of a hollowed brush before applying](images/hollow-interior.png) |

![An open roof and front face, with full-height side walls](images/hollow-open-shell.png)

## Carve

Subtract one or more shapes from other brushes.

1. Select the cutter and the brushes to carve, then open **Carve brushes…**.
2. Choose the **Cutter · shape to subtract** and check the **Target brushes**.
3. Click **Preview carve**. The preview shows the resulting pieces and lists brushes that will be removed entirely or stay unchanged.
4. **Apply carve** commits it. **Discard preview** or **Cancel** leaves the map alone.

To subtract several shapes at once, enable **Use multiple cutter brushes** and give each selected brush the **Cutter** or **Target** role. Overlapping cutters remove their shared volume once, and their order doesn't matter.

- Cutters stay in the map. Move or delete them afterwards.
- Original faces keep their settings. New faces take the material, texture projection and lighting of the cutter face that created them.
- A world brush that lies entirely inside a cutter disappears. Carving away every brush of an entity is refused.

**Limits:** 1–8 convex cutters, 16 targets, 256 pieces, and 64 faces per brush. Terrain can't be carved.

| Carving controls | Carved pieces around the cutter |
| --- | --- |
| ![Cutter selector, target checkboxes and preview actions](images/brush-carving.png) | ![Six carved pieces around the retained cutter](images/carve-preview.png) |

| Multiple cutters | Result with the cutters hidden |
| --- | --- |
| ![Cutter and target roles with a batch preview](images/multiple-cutter-controls.png) | ![Two combined cutter shapes leave a stepped cavity](images/multiple-cutter-cavities.png) |

## Clip

Cut brushes with a plane.

1. Select brushes or a brush entity and click **Clip selected brushes**.
2. Define the cut in one of three ways:
   - draw a line in Top, Front or Side;
   - type a plane normal and distance;
   - choose **Pick three points** and click three points on visible brush surfaces. You can switch views between clicks.
3. Choose which part to keep: **Arrow side**, **Opposite side** or **Both sides**. Optionally browse for the material of the new cut faces.
4. **Apply cut** commits it. **Cancel cut** or Escape discards it.

With three points, orange markers show each point, and the third point (not in line with the other two) creates the preview. **Remove last point** and **Start again** revise them; **Use plane / line** switches back to the other methods. Points follow the editor's snapping.

To fine-tune a cut, choose **Move plane** or **Rotate plane** under **Handles** and drag an arrow, a square or a ring. **Snap** uses the grid and the displayed angle step. Releasing keeps the preview; **Cancel positioning** or Escape during a drag puts the plane back.

- Existing faces keep their texture projections. Brushes the plane doesn't cross stay unchanged.
- Terrain that the plane crosses can't be clipped yet.

| Positioning the cut | Rotating the plane |
| --- | --- |
| ![Cut-plane move handles](images/cut-plane-move.png) | ![Rotation rings on the cut preview](images/cut-plane-rotate.png) |

![Three-point cut with point markers and the plane preview](images/three-point-cut.png)
