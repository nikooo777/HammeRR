[← Manual](README.md)

# Keyboard shortcuts

Assign up to two bindings to each viewport command.

1. Click **Viewport → Keyboard shortcuts…**.
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
| Display | Show grid, Snap to grid, Tool textures, Selection outlines |
| Transform | Select tool, Move handles, Rotate handles, Scale handles |
| Authoring | Draw brush, Clip mode |

## Limits

- Bindings apply to the focused viewport. Text fields retain normal editing keys.
- Projection and authoring commands start unbound.
- Changing Objects/Faces clears obsolete selection and drafts. Faces also stops brush drawing and clipping.
- Display toggles do not save grid or preview defaults.
- Invalid or changed storage requires Reload before another write.
- Shortcuts do not change map data or Undo history.
