[← Manual](README.md)

# Selecting

## Objects or faces

The **Selection** dropdown in the inspector switches between:

- **Objects**: brushes and entities. A brush entity is always selected as a whole.
- **Faces · materials & textures**: individual brush faces, for [texture work](textures.md).

Click to select, Ctrl+click to toggle. Every tool on this page works in both modes.

## Selection rules

Area, volume and paint selection share two settings:

- **Match**: **Intersects** accepts anything that touches the area. **Fully inside** requires all of an object's visible brushes to fit inside it.
- **Combine**: **Replace**, **Add**, **Remove** or **Toggle** the current selection.

Hidden objects and hidden tool brushes are skipped. Previews highlight the candidates without changing the selection or the undo history.

## Area selection

Left-drag on empty space in any view, away from any handle. The rectangle selects through the full depth of the view, not only the surfaces you can see. Ctrl+drag toggles.

## Volume selection

Select everything inside exact XYZ bounds.

1. Open **Select a volume…**.
2. Start from the selection or map bounds, or type the minimum and maximum coordinates.
3. Choose objects or faces, the match rule and the combine mode, then click **Preview matches**.
4. The blue box and highlights stay visible while you move the camera. **Frame box** shows the whole region.
5. **Select matches** applies the selection. **Cancel** leaves the selection unchanged.

| Volume controls | Region preview |
| --- | --- |
| ![Volume bounds, match rules and cancellation](images/volume-selection.png) | ![Blue volume guide and matching brush](images/volume-preview.png) |

## Paint selection

Brush over visible objects or faces to select them.

1. Choose **Paint selection** in the viewport toolbar and set the **Radius** in screen pixels.
2. Drag across the view. The brush picks the nearest visible object or face under it. Hold Ctrl to toggle instead.
3. Release, or click **Apply selection**, to apply. Escape or **Cancel** discards the stroke.

- Passing over the same object twice counts it once.
- The brush samples every 2 pixels, so very thin features between samples can be missed.
- In face mode, point helpers are ignored.
- Imported terrain is selected through its supporting brush faces.

![Paint selection with a pixel-radius brush, candidate highlights and Apply/Cancel](images/paint-selection.png)
