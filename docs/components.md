[← Manual](README.md)

# Vertices, edges and faces

Reshape a brush through its corners, edges or faces.

## Pick components

1. Select brushes.
2. Open **Reshape brushes…**.
3. Set **Brush scope** to one brush or **All selected**.
4. Choose **Edges**, **Vertices** or **Faces**.
5. Click **Pick in viewport**.
6. Click components to select them.

Ctrl+click toggles a component. Empty space clears the selection.
Use **Add to scope** or **Remove from scope** for a custom brush set.
**Stop viewport picking** returns to object picking.

## Move

1. Choose **Move selection**.
2. Enter an XYZ offset.
3. Click **Preview move**.
4. Click **Apply move**.

Move handles provide the same operation. Release a drag to apply it.
**Use editor grid snapping** controls component snapping.
Enable **Keep textures at corners** to attach textures during the change.

A face must remain flat, and the brush must remain convex.
Moving one box corner often fails; move an edge or the entire face instead.

## Scale and rotate

1. Choose **Scale selection** or **Rotate selection**.
2. Enter positive scale factors or world-axis rotation angles.
3. Choose **Center of selected components** or **World position** under **Pivot**.
4. Preview the result.
5. Apply the result.

Unselected corners remain fixed.
**Discard preview** restores the prior component selection.

## Weld corners

1. Choose **Weld corners**.
2. Select at least two vertices per brush.
3. Choose **Average selected corners**, **Existing brush corner** or **World position**.
4. Click **Preview weld**.
5. Click **Apply weld**.

Collapsed faces are removed. Surviving faces retain their IDs and materials.
**Keep textures at surviving corners** preserves supported texture attachment.

## Split and raise

1. Choose **Split and raise**.
2. Select one face or edge per brush.
3. Set **Raise outward**.
4. Click **Preview split**.
5. Click **Apply split**.

A raised center or midpoint creates new triangular faces.
The original corners remain fixed.

## Roof ridge

1. Choose **Roof ridge**.
2. Select a four-corner face.
3. Choose the ridge direction.
4. Set its height.
5. Click **Preview ridge**.
6. Click **Apply ridge**.

![A flat top reshaped into two roof slopes](images/roof-ridge.png)

**Keep roof texture at corners** attaches the original texture to both slopes.
Use [Flat partitions](partitions.md) to divide a face before choosing a result.

## Limits

- 16 brushes, 512 selected components and 64 faces per brush.
- Split and ridge heights are 0.125–4096 units.
- Terrain uses its own [editing tools](terrain.md).
- Twisted, collapsed, concave or unrepresentable texture results reject before Apply.
