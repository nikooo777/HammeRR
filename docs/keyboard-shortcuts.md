[← Manual](README.md)

# Keyboard shortcuts

Choose two bindings each for Undo, Redo, Frame selection, Delete selection and
Duplicate selection, Perspective, Top, Front and Side projection commands, and
Objects and Faces selection modes. Projection and selection-mode commands start
**Unbound**, including when an older saved shortcuts
file loads. Bindings work while the viewport has keyboard focus; text fields
keep their normal editing keys.

1. In **Viewport**, click **Keyboard shortcuts…**.
2. Choose a command, including **Perspective**, **Top**, **Front**, **Side**,
   **Objects** or **Faces**. Scroll the command list to reach later rows.
   Compare **Current form** with **Factory bindings**.
3. Set **Primary** and **Alternate** using the key dropdowns and **Ctrl** and
   **Shift** switches. Choose **Unbound** to remove a slot.
4. Resolve any duplicate or reserved binding shown below the controls. To swap
   bindings, edit both commands before saving.
5. Click **Save shortcuts** to activate the complete form now and at startup.
   Click the viewport before using a binding. Its **Frame** hint reflects the
   current binding.
   Projection bindings use the same per-view cameras as the viewport's projection
   controls and affect the document whose viewport received the key.
   **Objects** and **Faces** match the **Selection** dropdown: changing modes
   clears the selection and obsolete drafts or gestures. Entering Faces also
   stops clipping and brush drawing. Either command exits component editing,
   including when its selection mode is already active.
6. **Reload saved** loads and activates the saved file. **Reset saved shortcuts**
   explicitly writes and activates factory values. **Cancel**, Escape or closing
   the window discards unsaved form changes; it does not undo a saved change.

![Command bindings, modifier controls and storage status](images/keyboard-shortcuts.png)

Limits:

- Keys are logical A–Z, F1–F12, Delete and Backspace, with exact Ctrl/Shift
  modifiers. Alt/Meta bindings, physical scan codes and key sequences are absent.
- Escape, arrows, PageUp/PageDown, camera W/A/S/D/Q/E with or without Shift, and
  Ctrl+N/O/S/C/X/V remain reserved. File, clipboard, camera movement and tool
  bindings cannot be changed here. Command availability still follows editing
  context.
- An unavailable saved file shows an error and preserves active bindings.
  Save remains disabled until Reload succeeds or you explicitly Reset.
  Outside changes require another inspection. Symlinked/nonregular targets and
  files over 16 KiB reject; malformed storage is never overwritten by ordinary
  Save. Save and Reset coordinate with other editors through a nonblocking
  lock; a busy lock preserves the file and active bindings. Reload before
  retrying. Changed directories require explicit Reload. Programs that ignore
  the lock can still race with publication. Coordinated writes require Linux;
  other platforms reject publication.
- If a write completes but verification fails, the dialog reports the completed
  write and its error. The intended saved bindings remain active; Reload to
  inspect storage before continuing.
- Bindings are local preferences. Saving them does not edit the map or Undo
  history. The factory shortcuts in [Editor basics](basics.md) remain useful
  when no saved configuration exists.
- Objects/Faces commands change only the originating document's selection mode;
  camera, map data and Undo history stay unchanged. Repeating the active mode
  keeps ordinary selections and drafts, while still exiting component editing.
