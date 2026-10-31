[← Manual](README.md)

# Atlas-region texturing

Fit flat faces independently or together, or automatically pack separate face UV islands into a rectangular part of an existing material image.

1. Select faces under **Selection → Faces · materials & textures**.
2. Open **Edit face textures…**, then **Fit atlas region…**.
3. Enter **Material**, then click **Inspect material**.
4. Drag between two edges on the inspected image, or enter **X**, **Y**, **Width** and **Height** in source image pixels.
5. Set **Padding** in whole source image pixels. Use `0` for the full rectangle.
6. Check **Usable area**. Cyan outlines the rectangle; yellow outlines the padded usable area.
7. Click **Preview atlas fit**.
8. Inspect the faces, then click **Apply atlas fit**.

The top-left image edge is `0, 0`. The default **Each face independently** mode fits the usable area separately for every face. Its texture direction, skew, rotation and mirroring stay unchanged.
For example, a `32 × 16` rectangle at `16, 8` with padding `2` uses `28 × 12` pixels at `18, 10`.
**Cancel** discards the preview. Changed fields require another preview.

Dragging rounds to whole source image pixel edges and works in either direction.
Start inside the image; continuing beyond an edge clips the rectangle to the image.
An empty click or line keeps the previous rectangle. **Escape** during a drag restores it without closing the dialog or discarding an existing face preview.
Release, then click **Preview atlas fit** before applying. Reinspect or reopen the dialog after a source change.

![Two equal faces use the full image on the left and an eight-pixel inset on the right](images/atlas-padding.png)

Left: no padding. Right: an eight-pixel inset excludes the blue image border.

## Fit faces together

1. Select coplanar faces facing the same direction. Open **Fit atlas region…**.
2. Under **Fit mode**, choose **Fit coplanar faces together**.
3. Choose **Reference face**. Its texture direction, skew, rotation and mirroring become the shared frame for every target face.
4. Inspect the material and set the rectangle and padding, then click **Preview atlas fit**.
5. Inspect the continuous mapping, then click **Apply atlas fit**.

The combined projected bounds fit the usable area once. Separated faces are allowed; gaps consume image space.
Other target projection frames are replaced. Geometry, lightmap settings, smoothing and other face data stay unchanged.
Changing **Fit mode** or **Reference face** requires another preview before applying.

![One image spans two faces; the center edge does not restart the mapping](images/atlas-continuity.png)

## Pack face islands

1. Select flat faces, then open **Fit atlas region…**.
2. Under **Fit mode**, choose **Pack face islands**.
3. Inspect **Material** and choose the destination rectangle.
4. Set **Padding** for the outer region, in source image pixels.
5. Set **Island UV size**. Use `1` to keep current texel sizes; `0.5` halves both dimensions.
6. Set **Island padding** in whole source image pixels.
7. Click **Preview atlas fit**. Check the green allocations and face mapping.
8. Click **Apply atlas fit**, or **Cancel**. Apply creates one Undo step.

![Three face islands packed into one Source trim sheet](images/atlas-packing.png)

Each flat face is one island. Packing preserves aspect, axes, skew, rotation and mirroring.
Use **Fit coplanar faces together** for continuous surfaces.

Large islands are placed first. Reordering the same faces gives the same layout.
Islands keep their requested size and orientation. If no layout fits, reduce
**Island UV size** or **Island padding**, or choose a larger region.
A rejected update keeps the previous preview.

Packing reuses the chosen atlas image. Image and material generation remain separate.
Fractional texel spans reserve enough whole pixels; the projection keeps its aspect.

## Save and reuse named regions

1. Inspect the material, then draw or enter the rectangle and **Padding**.
2. Enter **Region name**, then click **Save region** before starting a face preview.
3. When fitting this material again, choose it under **Saved region** and click **Use saved region**.
4. Check the restored rectangle and padding, choose **Fit mode** and **Reference face** as needed, then click **Preview atlas fit**.
5. Inspect the faces, then click **Apply atlas fit**, or **Cancel** to discard the face preview.

Saved regions stay in this map, including after saving and reopening it. Each region keeps the canonical material, original image dimensions and identity, rectangle and padding. Fit mode, reference face, island UV size and island padding remain current fitting choices.
Names are unique for each material, ignoring case. Up to 128 regions can be saved in a map, with names up to 80 characters.
Saved child VMFs can retain valid catalogs when used as [compiled assemblies](instances.md). Their regions remain in the child and do not become parent regions or compiled map data.

To rename a region, select it, edit **Region name**, then click **Rename region**. **Delete region** removes the selected saved region. Each save, rename or delete has its own Undo step and leaves face projections unchanged. Apply or cancel any geometry preview before changing saved regions.

After **Reload material assets**, unchanged images can reuse their regions. Changed materials, image contents or dimensions reject reuse and show **(source changed)** in the list. Inspect the image and save a new region; rename or delete an old one as needed. External texture changes follow the same explicit reload policy as the image and viewport.

## Limits

- Flat world, detail and brush-entity faces only; terrain and collapsed projections reject.
- One static base image with a supported `LightmappedGeneric` or `UnlitGeneric` declaration.
- No tool overrides, proxies, extra layers or image editing.
- The rectangle must have positive size and stay inside the original image.
- Padding must leave a positive usable width and height; invalid updates preserve the existing preview.
- Together fitting requires a reference from the target selection and same-facing coplanar faces. It does not unfold across corners.
- Packing uses one unrotated rectangular allocation per face, including unused polygon corners; it does not preserve continuity across islands. Allocation search is bounded to 1024 free rectangles and fails explicitly if that budget is exceeded.
- Up to 256 faces. Padding changes the fit, not the image. Choose suitable image gutters and padding for the material's filtering and mipmaps.
