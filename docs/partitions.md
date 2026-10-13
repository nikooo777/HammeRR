[← Manual](README.md)

# Flat partitions

A flat partition draws a dividing line across a brush face and splits it into two **pieces**, without changing the brush yet. Once a face is divided, you can:

- **raise** the line into a roof ridge;
- **extrude** one or both pieces into new brushes;
- plan a different **top material** or **texture placement** for each piece;
- **keep** the division in the map and come back to it later.

| Pieces staged on a flat face | Pieces raised into a ridge |
| --- | --- |
| ![A dividing line on an unchanged brush face](images/component-partition-flat.png) | ![The pieces raised into two roof planes](images/component-partition-raised.png) |

## Divide a face

1. Open **Reshape brushes…**, choose **Faces** and select one face per brush. The face must be convex with 3–8 corners.
2. Choose the **Flat partition** operation.
3. Pick the two edges the line connects, or **Trim corner** to cut off a corner. A four-cornered face offers two straight-across directions and four corner trims.
4. Click **Stage flat partition**. The line appears and the brush stays unchanged.

To inspect the pieces, use **Highlight pieces** and **Frame highlighted pieces**, or click a piece in the viewport. Ctrl+click toggles a highlight and clicking empty space clears them.

![Flat partition controls with source corners, midpoints and highlighted pieces](images/component-partition-controls.png)

| Corner trim controls | Triangle and pentagon pieces, extruded |
| --- | --- |
| ![Trim-corner direction controls](images/partition-boundary-controls.png) | ![Triangle and pentagon pieces extruded above their source](images/partition-boundary-preview.png) |

| Dividing an eight-sided face | Two prisms above the octagon |
| --- | --- |
| ![Polygon cut choices and endpoint sliders](images/polygon-partition-controls.png) | ![Two styled polygon prisms above an octagonal face](images/polygon-partition-preview.png) |

## Move the dividing line

Enable **Adjust dividing edge** and move the two endpoints with the percentage fields or sliders, then click **Preview cut**. This makes unequal or diagonal pieces. **Reset cut to midpoint** puts the line back in the middle.

| Cut controls | Pieces from a diagonal cut, extruded |
| --- | --- |
| ![Percentage inputs and sliders for the two endpoints](images/partition-cut-controls.png) | ![A diagonal cut used to create two separately extruded volumes](images/partition-cut-preview.png) |

## Raise a ridge

When both pieces have four corners, enter the **Shared ridge height** and click **Preview ridge**. **Return to flat** goes back to the flat pieces. **Apply raised partition** commits the ridge.

- A moved dividing line works too, as long as both roof slopes stay flat and convex. Otherwise the ridge is refused and the flat preview stays.
- **Keep roof texture at corners** carries the flat texture onto both slopes.

| Ridge on a moved line | Roof slopes of different widths |
| --- | --- |
| ![Cut percentages and ridge controls](images/partition-adjusted-ridge-controls.png) | ![A moved dividing line raised with independent roof materials](images/partition-adjusted-ridge-preview.png) |

## Extrude pieces

**Extrude highlighted pieces** builds new brushes on top of one or both pieces.

1. Highlight the pieces to extrude.
2. Set a positive height. Optionally choose a wall material and keep the top texture attached.
3. Preview, then Apply. The faces of the new brushes are selected.

- The original brush stays unchanged, and its pieces can be reused.
- New brushes belong to the same world or brush entity as the source.
- Tops use the planned material (or the source face's), bases use the source face's material, and walls use the chosen wall material or inherit from the neighboring faces.
- Changing the highlighted pieces requires a new preview. **Return to flat** and **Cancel** discard the new brushes.

| Extrusion controls | Two pieces extruded to different heights |
| --- | --- |
| ![Height, wall material and top texture controls](images/partition-extrusion-controls.png) | ![Two pieces extruded above an unchanged source brush](images/partition-extrusion-preview.png) |

## Plan top materials

Enable **Planned top material** to give the highlighted pieces a different material for their future roof slope or extruded top. Choose one in the browser, or type its name and click **Preview material**.

- Each piece shows its material and whether it is inherited or planned. **Restore inheritance** reverts to the source face's material.
- The flat preview shows the planned materials without adding faces to the brush.
- Return to flat before changing a plan.

| Material controls | Two materials planned on one flat face |
| --- | --- |
| ![Planned material choices with preview and inheritance](images/partition-material-controls.png) | ![Two materials on a flat face without adding brush faces](images/partition-material-preview.png) |

## Plan top texture placement

Enable **Top texture placement** to shift, scale or rotate the texture of the highlighted pieces:

- shift in texels;
- scale in world units per texel (negative values mirror the texture);
- rotation in degrees.

Click **Preview placement** to see it on the flat face. **Use source placement** reverts. The placement is applied when you raise or extrude.

| Placement controls | Different placements on two pieces |
| --- | --- |
| ![Texture placement with units, rotation slider and inheritance](images/partition-texture-controls.png) | ![Different shifts, mirrored scales and rotation on two pieces](images/partition-texture-preview.png) |

## Keep for later

**Keep flat pieces** stores the division in the VMF, including the line position and any material and texture plans, without changing a single brush.

Later, even after saving and reopening the map, open **Saved flat pieces**:

- **Resume pieces** continues where you left off: inspect, raise or extrude.
- **Remove saved** deletes a stored division, even one whose face has since changed or been deleted.

Keeping, removing and applying are all undoable. Applying a raised partition uses up its stored division. Stored divisions survive mirroring and reopening, and they never affect compiled maps.

Finish an open partition (keep, apply or cancel it) before saving, compiling or playing.

![Saved flat partitions with Resume and Remove controls](images/component-partition-saved-controls.png)

## Limits

- Source faces must be convex with 3–8 corners. Each piece has 3–9 corners.
- Ridges need two four-cornered pieces. Other shapes can be kept or extruded.
- One batch handles 16 source brushes, 32 extruded pieces and 256 generated planes.
- A map can store divisions on up to 64 faces, using up to 64 KiB of data.
- Not yet available: inward extrusion, free-form dividing lines, and per-piece texture axes, fit and justification.
