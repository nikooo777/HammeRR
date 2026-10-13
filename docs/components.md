[← Manual](README.md)

# Vertices, edges and faces

Reshape brushes by editing their corners (vertices), edges and faces directly. Every operation keeps brushes convex and valid; anything that would break a brush is rejected before the map changes.

## Pick components

1. Select brushes and open **Reshape brushes…**.
2. Set the **Brush scope**:
   - one brush, chosen by ID, material and position, or
   - **All selected**, up to 16 brushes.

   **Add to scope** and **Remove from scope** build a custom set. Large brush entities list their brushes in pages. **Inspect this brush** narrows the scope to one brush.
3. Choose **Edges**, **Vertices** or **Faces**, then click **Pick in viewport**.
4. Click to select, Ctrl+click to toggle, click empty space to clear. The component list gives exact access to every part, including overlapping ones.

Blue markers show available corners and edges; orange marks the selection. Corners and edges can be picked through surfaces. Faces pick the nearest visible surface. Hidden brushes can't be picked.

**Stop viewport picking**, **Cancel** or Escape returns to normal object picking and discards any unapplied change.

| One brush from a large entity | Brushes from several pages |
| --- | --- |
| ![Inspecting one brush from the last page of a large brush entity](images/component-brush-scope.png) | ![Adding brushes from separate pages to the scope](images/component-brush-subset.png) |

Then choose an operation: **Move selection**, **Scale selection**, **Rotate selection**, **Weld corners**, **Split and raise**, **Roof ridge** or **Flat partition**. Flat partitions have [their own page](partitions.md).

## Move

Move the selected components with the handles or by typing an offset.

- **Handles**: drag a colored arrow to move along one axis, a plane square to move along two, or the center square to move within the view. Release to apply; Escape cancels. **Show viewport move handles** toggles them.
- **Typed offset**: enter an XYZ offset, click **Preview move**, then **Apply move**.

**Use editor grid snapping** applies the grid to both. Offsets are always measured from the brush as it was before the preview, so adjusting a preview never adds up.

Faces must stay flat and the brush convex. Moving a single corner of a box usually isn't possible; move its edge or the whole face instead.

| Move controls | Top face moved upward |
| --- | --- |
| ![Face, vertex and edge selection with exact offsets](images/brush-components.png) | ![Selected top face moved upward with viewport handles](images/component-preview.png) |

### Keeping textures attached

By default, textures stay fixed in world space while you reshape. Enable **Keep textures at corners** to stretch and rotate the texture with each face. If Source's texture projection can't represent the result, the change is rejected: turn the option off or move the whole face.

| Texture fixed in world space | Texture kept at the corners |
| --- | --- |
| ![Texture repeats as the brush grows taller](images/texture-world.png) | ![Texture stretches with the taller brush](images/texture-attached.png) |

## Scale and rotate

- **Scale selection** takes a positive factor per axis: 1 leaves an axis unchanged, 0.5 halves it, 2 doubles it.
- **Rotate selection** takes degrees around world X, then Y, then Z.
- **Pivot** is the **Center of selected components** or a **World position**.

Unselected corners stay where they are. Scaling a box's top face down makes a frustum; rotating it makes a sloped roof.

Click **Preview scale/rotation**, then **Apply**. **Discard preview** and **Cancel** work as usual, and **Keep textures at corners** is available. Twisted, collapsed or concave results are rejected.

| Scale controls | Rotate controls |
| --- | --- |
| ![Scale factors and pivot](images/component-scale-controls.png) | ![Degree inputs and custom world pivot](images/component-rotate-controls.png) |

| Scaled top face (frustum) | Rotated top face (sloped roof) |
| --- | --- |
| ![Top-face scaling leaves the bottom corners fixed](images/component-scale-preview.png) | ![Top-face rotation creates a sloped roof](images/component-rotate-preview.png) |

## Weld corners

Merge several corners into one.

1. Choose **Weld corners**, pick **Vertices**, and select at least two corners in each brush.
2. Under **Merge corners at**, choose:
   - **Average selected corners**: each brush welds to its own average point;
   - **Existing brush corner**: weld onto one of the original corners;
   - **World position**: exact coordinates, starting at the selected corners' center.
3. Click **Preview weld**, then **Apply weld**.

For example, welding a box's four top corners makes a pyramid.

- Faces that collapse are removed. The preview reports how many and highlights the new corner.
- Remaining faces keep their IDs, material and texture projection.
- **Keep textures at surviving corners** keeps textures attached. The merged corner averages the texture coordinates of the corners it replaces.
- **Discard preview** restores the original corner selection.

| Weld controls | Four corners welded into a pyramid |
| --- | --- |
| ![Weld controls with Apply and Cancel](images/brush-welding.png) | ![Four box corners welded into a pyramid apex](images/component-weld-preview.png) |

| Exact weld position | Pyramid with an off-center apex |
| --- | --- |
| ![World-position weld inputs](images/brush-weld-position.png) | ![A pyramid welded to a custom apex](images/component-weld-position.png) |

## Split and raise

Add a corner in the middle of a face or edge and push it outward.

1. Choose **Split and raise** and select one face or edge per brush.
2. Set **Raise outward** in units or with the slider.
3. Click **Preview split**, then **Apply split**.

- A face gets a new corner at its center, raised along the face's normal. An edge gets one at its midpoint, raised between its two faces.
- Existing corners stay put; the affected faces become triangles that inherit the original material, texture projection and lighting.
- **Keep textures at corners** stretches each triangle's texture toward the raised corner.
- After Apply, the new corner is selected so you can keep shaping it.

**Limits:** a rise of 0.125–4096 units, one component per brush, 64 faces per brush. Results that cross neighboring faces are rejected.

| Face split controls | Face split into four triangles |
| --- | --- |
| ![Raised corner controls with rise slider](images/brush-face-split.png) | ![A face split into four triangles around a raised corner](images/component-face-split.png) |

| Edge split controls | Raised edge midpoint |
| --- | --- |
| ![Edge split controls](images/brush-edge-split.png) | ![A raised edge midpoint shared by six new triangles](images/component-edge-split.png) |

## Roof ridge

Raise a four-sided face into two roof slopes.

1. Choose **Roof ridge** and select one four-cornered face per brush.
2. Pick the ridge direction for each brush and set the height.
3. Click **Preview ridge**, then **Apply ridge**.

The midpoints of two opposite edges rise together and form the ridge; the original corners stay put. The new faces inherit the source face's material and texture projection. **Keep roof texture at corners** carries the flat face's texture onto both slopes. After Apply, both ridge corners are selected.

| Ridge controls | Two roof slopes |
| --- | --- |
| ![Roof ridge direction and height controls](images/brush-roof-ridge.png) | ![Two roof planes joined at the raised edge midpoints](images/component-roof-ridge.png) |

## Limits

- Up to 512 selected components across 16 brushes, with up to 64 sides per brush.
- Every change is checked before the map changes. Duplicate IDs, open or redundant geometry and collapsed edges are rejected.
- Terrain (displacements) can't be reshaped with these tools yet.
