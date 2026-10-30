[← Manual](README.md)

# Keyboard shortcuts

Assign up to two bindings to each viewport command.

1. Choose **Edit → Preferences → Keyboard shortcuts…**.
2. Enter a label, command ID or binding in **Search commands**.
3. Select a command.
4. Set **Primary** and **Alternate**.
5. Set **Ctrl** and **Shift** as required.
6. Resolve reported duplicate or reserved bindings.
7. Click **Save shortcuts**.

Save applies the complete form now and at startup.
Choose **Unbound** to remove a binding.
**Clear search** restores the full command list.
Cancel discards unsaved changes, including hidden search results.

## Commands

| Area | Commands |
| --- | --- |
| Editing | Undo, Redo, Frame selection, Delete selection, Duplicate selection |
| Projection | Perspective, Top, Front, Side |
| Selection | Objects, Faces |
| Display | Show grid, Snap to grid, Tool textures, Selection outlines, Hide selected in editor, Show all editor-hidden |
| Transform | Select tool, Move handles, Rotate handles, Scale handles |
| Authoring | Draw brush, Clip mode |

## Limits

- Bindings apply to the focused viewport. Text fields retain normal editing keys.
- Ctrl+K always opens **Search commands**. Like Ctrl+N, Ctrl+O, Ctrl+S, Ctrl+C, Ctrl+X and Ctrl+V, it cannot be assigned.
- Projection, authoring and editor-visibility commands start unbound.
- Hide requires selected objects and committed geometry. Show all restores only temporary editor-hidden objects, including during a preview.
- Changing Objects/Faces clears obsolete selection and drafts. Faces also stops brush drawing and clipping.
- Display toggles do not save grid or preview defaults.
- Invalid or changed storage requires Reload before another write.
- Saving bindings does not edit the map. Executing a command has the same effect as its editor control.
