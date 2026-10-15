[← Manual](README.md)

# Selecting

## Objects or faces

The **Selection** dropdown in the inspector switches between:

- **Objects**: brushes and entities. A brush entity is always selected as a whole.
- **Faces · materials & textures**: individual brush faces, for [texture work](textures.md).

Click to select, Ctrl+click to toggle. Loaded models use their visible triangles for point picking. Model entities belong to **Objects** mode; their mesh triangles are not editable brush faces.

## Cycle overlapping objects

Select an object behind another object without hiding either one.

1. Choose **Objects** in **Selection**.
2. Hold Alt and left-click where the objects overlap. If the current selection exactly matches one hit, the next hit in depth order replaces it; otherwise the nearest hit is selected.
3. Repeat Alt+click to advance through the hits and wrap to the first. The status bar shows the current overlap and its label.

![The rear brush selected by cycling through three overlapping objects](images/object-hit-cycling.png)

Brush entities count once even when the ray hits several child brushes. Native groups count once as their outermost visible selection unit. Enable **Bypass groups** in [Object groups…](groups.md) to cycle individual objects.

Cycling uses the same brush, terrain, model triangle and point-helper hits as ordinary object picking. Hidden objects and hidden tool brushes are skipped. Clicking empty space while holding Alt preserves the selection. Cycling replaces the selection; ordinary Ctrl+click retains its toggle behavior.

- The nearest 64 distinct object/group hits are available. If more overlap, the status bar reports the limit.
- Keep the pointer still until release. Alt-drag, Escape, navigation, viewport resizing, camera changes, map changes and selection changes cancel the gesture without falling through to an ordinary click.
- Cycling is an **Objects** operation. Finish active geometry previews first. Face selection, component picking, paint, placement picking, terrain tools, clipping and brush drawing retain their own gestures.
- Cycling changes selection without adding map undo history.

## Selection rules

Area, volume and paint selection share two settings:

- **Match**: **Intersects** accepts anything that touches the area. **Fully inside** requires all of an object's visible brushes to fit inside it.
- **Combine**: **Replace**, **Add**, **Remove** or **Toggle** the current selection.

Loaded model entities use their transformed asset bounds for area and volume matching. **Fully inside** requires that whole bounding box to fit; **Intersects** accepts a touching box. These tests include empty space inside a model’s bounds. Unsupported model assets retain helper bounds.

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

Terrain uses its displaced triangles for volume and area matching. Fully contained checks every surface vertex, including raised interior peaks. Other parent faces and the old parent volume do not match. To retain a triangle subset instead of complete faces, keep the volume preview active and use **Terrain triangles… → Select in volume**; see [Terrain](terrain.md).

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
- Terrain is selected through its actual displaced surface; ordinary parent faces and the old parent volume are excluded.

![Paint selection with a pixel-radius brush, candidate highlights and Apply/Cancel](images/paint-selection.png)
