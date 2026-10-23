[← Manual](README.md)

# Editor basics

Use the viewport to inspect the map. Use the inspector to change the selection.

![A textured courtyard in the viewport](images/showcase-editor.png)

## Navigate

| Action | Control |
| --- | --- |
| Fly | Hold the right mouse button; use WASD and Q/E. |
| Fly faster | Hold Shift while flying. |
| Planar pan / zoom | Right-drag / mouse wheel. |
| Change projection | **Perspective**, **Top · XY**, **Front · XZ**, **Side · YZ**. |
| Frame the selection | **F** or **Frame · F**. With no selection, frame the map. |
| Select / toggle | Click / Ctrl+click. |
| Select an area | Drag on empty space; release to select. |
| Duplicate / delete | Ctrl+D / Del. |
| Undo / redo | Ctrl+Z / Ctrl+Y. |
| Cancel an active tool | Escape. |

These keys are factory bindings. See [Keyboard shortcuts](keyboard-shortcuts.md).
Use **Fly speed** or scroll while flying to change speed.

## Apply a preview

1. Select the objects.
2. Open the required tool.
3. Set its values.
4. Click **Preview**.
5. Inspect the result.
6. Click **Apply** to keep it, or **Cancel** to discard it.

Applied map edits create one Undo step. A conflicting edit invalidates the preview.
Users and agents share the selection, previews and Undo history.

## Grid and snapping

1. Set **Grid** to the required spacing.
2. Enable **Snap** for viewport placement and dragging.
3. Enable **Show grid** to display the grid.

Grid spacing is 0.125–4096 units. Numeric fields retain the values you enter.
To align existing objects, use **Transform selection → Preview snap to grid**.
See [Brushes and transforms](brushes.md#snap-selection-to-grid).

## Viewport modes

Choose a mode under **Viewport**.

| Mode | Result |
| --- | --- |
| **Textured** | Mounted game materials. |
| **Flat shading** | Solid geometry without textures. |
| **Wireframe** | Brush edges and terrain triangles. |
| **Lightmap density** | Authored lighting sample spacing. |
| **Smoothing groups** | Membership in the selected smoothing group. |

| Flat shading | Wireframe |
| --- | --- |
| ![Courtyard geometry without textures](images/courtyard-flat.png) | ![Courtyard brush edges](images/courtyard-wireframe.png) |

Clear **Tool textures** to hide tool brushes from display and picking.
Enable **Directional surface shading** to make slopes easier to see.
This fixed editor light does not show baked lighting.

## Selection outlines

1. Select an object or face.
2. Clear **Selection outlines** to hide its edge lines.
3. Enable **Selection outlines** to restore them.

The selection and its surface tint remain active. Tool guides remain visible.

## Saved camera views

1. Move the camera to the required view.
2. Open **Saved views…**.
3. Enter a name.
4. Click **Save current as new**.
5. Select a saved view.
6. Click **Go to view**.

Saved views are stored in the map. Use **Replace with current**, **Rename** or **Delete view** to manage them.
Each map supports 64 views, with names up to 80 characters.

## Saved defaults

Grid, preview and navigation defaults have separate dialogs.

1. Open **Grid defaults…**, **Preview defaults…** or **Navigation defaults…**.
2. Click **Save current as defaults** to save the current settings.
3. Click **Apply saved defaults** to use saved settings now.
4. Click **Reload defaults** after an outside change or storage error.

**Reset saved defaults** saves factory settings. Apply them separately.
Ordinary viewport changes do not save defaults.
Unreadable or changed storage blocks writes until Reload succeeds or you explicitly Reset.
A write-verification error can mean the file was written. Reload before retrying.

## Closing the editor

1. Use the window close control.
2. Review **Unsaved changes and drafts** when it appears.
3. Choose **Yes** to discard them, or **No**/**Cancel** to continue editing.

Save or apply anything you need before Close.
Changed state requires another review. Busy work can require waiting or cancellation.
Closing stops game launches and sound previews started by the editor. Cleanup errors remain visible.
See [Open documents](open-documents.md) for multiple maps.

## Limits

- Preview modes do not reproduce all game rendering.
- Lightmap density shows authored spacing, not compiled samples.
- Ordinary camera movement and view settings belong to the session. Saved views belong to the map.
- Preference files are limited to 4 KiB. Symlinked or nonregular preference files reject.
