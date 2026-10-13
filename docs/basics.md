[← Manual](README.md)

# Editor basics

![The editor with a courtyard built by an AI agent](images/editor.png)

The window has four parts:

- **Toolbar** (top): file actions, Compile, Run Map, Game Log, Undo/Redo, brush creation, Delete, the **Tool textures** and **Leak path** toggles, and the MCP address.
- **Viewport toolbar**: projection, framing, saved views, grid and snapping, and the active viewport tool.
- **Inspector** (right): viewport mode, selection mode, and the tools and properties for whatever is selected.
- **Activity** (bottom right): recent edits, each labeled **you** or **agent**.

## Previews, Apply and Cancel

Almost every tool in HammeRR works the same way:

1. **Set up the operation.** The viewport shows a preview. The map doesn't change yet, and nothing goes into the undo history.
2. **Apply** commits the whole operation as **one undo step**.
3. **Cancel**, Escape, or closing the dialog throws the preview away.

If an input would produce invalid geometry, the last valid preview stays on screen and Apply waits until the input is fixed. If the map changes while a preview is open (for example, because an agent edited it), the preview is invalidated instead of being applied to the wrong objects.

Undo and redo are **Ctrl+Z** and **Ctrl+Y**.

## Working with an AI agent

You and any connected agent share one editing session: the map, the selection, the camera, open previews and the undo history. An agent's edits appear live and show up in Activity, and you can undo them like your own. An agent can set up a preview for you to apply, or the other way around. See [AI agents and MCP](mcp.md).

## Navigation

| Action | Control |
| --- | --- |
| Fly (perspective) | Hold the right mouse button; move with WASD and Q/E. |
| Switch projection | **Perspective**, **Top · XY**, **Front · XZ** or **Side · YZ** in the viewport toolbar. |
| Pan / zoom (Top, Front, Side) | Right-drag / mouse wheel. Zoom follows the cursor. |
| Frame the selection | **F** or **Frame · F**. Frames the whole map when nothing is selected. |
| Select / toggle | Click / Ctrl+click. |
| Select an area | Left-drag on empty space. Ctrl+drag toggles. |
| Nudge the selection | Arrow keys and PgUp/PgDn. Hold Shift to nudge by at most one unit. |
| Duplicate / delete | Ctrl+D / Del. |
| Undo / redo | Ctrl+Z / Ctrl+Y. |
| Cancel the active tool | Escape. |

In Top, Front and Side, the arrow keys move within the view's plane and PgUp/PgDn move along its depth.

Framing fits the viewport's width and height and keeps the camera's direction and field of view.

## Grid and snapping

**Grid** sets the spacing, from 0.125 to 4096 units. **Snap** and **Show grid** are independent. Values typed into numeric fields are always exact; they are never snapped.

## Viewport modes

Choose a mode in the **Viewport** section of the inspector. Every mode works in every projection.

| Mode | Shows |
| --- | --- |
| **Textured** | The game's materials. |
| **Flat shading** | Solid geometry without textures or transparency. |
| **Wireframe** | Brush edges, including terrain triangles. Use it to see through solids, or for the classic Hammer look in Top, Front and Side. |
| **Lightmap density** | The lighting grid set on each face. See [face lighting](textures.md#face-lighting). |
| **Smoothing groups** | Faces that belong to a chosen smoothing group. |

Light icons, selection highlights and editing guides stay visible in every mode. The density and smoothing modes show the settings you've authored, not baked lighting.

Uncheck **Tool textures** in the toolbar to hide brushes that use tool materials (clip, nodraw, and so on). Hidden tool brushes are also skipped by selection.

| Flat shading | Wireframe |
| --- | --- |
| ![Untextured geometry and a light helper](images/flat-view.png) | ![Brush edges and the same light helper](images/wireframe-view.png) |

## Saved camera views

Save named camera positions and jump back to them.

1. Open **Saved views…** next to Frame.
2. Type a name and choose **Save current as new**.
3. Later, select the view and choose **Go to view**. This restores the projection, position, direction, field of view and planar zoom.

**Rename**, **Replace with current** and **Delete view** manage the list. Saved views are stored in the VMF, and changes to them are undoable. Going to a view moves only the camera.

- Up to 64 views per map, with names up to 80 characters.
- Camera records written by Hammer are kept separately and preserved.

![Named camera views with restore and edit controls](images/saved-views.png)
