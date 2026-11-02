[← Manual](README.md)

# Keyboard shortcuts

Assign two binding slots to each of 36 editor commands. Each slot can use one
stroke or a two-stroke sequence, with logical keys or physical key positions.

1. Choose **Edit → Preferences → Keyboard shortcuts…**.
2. Enter a label, command ID or binding in **Search commands**.
3. Select a command. Its **Primary** and **Alternate** slots appear below the
   command list. Scroll to reach all controls.
4. Set **Binding type** to **Single stroke** or **Two-stroke sequence**.
   Choose **Unbound** to remove the slot.
5. For **First stroke**, choose **Logical key** or **Physical key** under
   **Key type**, then choose **Key**. Set **Ctrl**, **Shift**, **Alt** and
   **Meta** as required.
6. For a sequence, configure **Second stroke** the same way. Each stroke has
   its own key type and exact modifiers.
7. Resolve the reported incomplete, duplicate, ambiguous or reserved bindings.
8. Click **Save shortcuts** to write and activate the complete form.

Logical keys follow the keyboard layout. Physical keys use the selected key
position, such as **KeyW**, regardless of the character produced by that key.
Changing **Key type** clears that stroke's **Key** selection; choose the key again.
To enter a sequence, press its first stroke, release it, then press its second
stroke within one second. **Escape** cancels a pending sequence. A mismatched
second stroke, focus change, text editing, another dialog or changed shortcut
settings also cancel it.

**Clear search** restores the full command list. Searching does not change the
selected command or discard edits, including invalid or filtered-out bindings.
A changed runtime configuration leaves a dirty form intact and blocks its save;
**Reload saved** explicitly discards the form and loads saved bindings.
**Reset saved shortcuts** writes and activates factory bindings. **Cancel**
closes the panel and discards unapplied edits; it does not undo a completed save,
reload or reset.

## Commands

| Area | Commands |
| --- | --- |
| File and clipboard | New map, Open map, Save map, Save map as, Copy objects, Cut objects, Paste objects |
| Camera | Camera forward, Camera backward, Camera left, Camera right, Camera up, Camera down |
| Editing | Undo, Redo, Frame selection, Delete selection, Duplicate selection |
| Projection | Perspective, Top, Front, Side |
| Selection | Objects, Faces |
| Display | Show grid, Snap to grid, Tool textures, Selection outlines, Hide selected in editor, Show all editor-hidden |
| Transform | Select tool, Move handles, Rotate handles, Scale handles |
| Authoring | Draw brush, Clip mode |

File and clipboard factory bindings are <kbd>Ctrl</kbd>+<kbd>N</kbd>, <kbd>Ctrl</kbd>+<kbd>O</kbd>, <kbd>Ctrl</kbd>+<kbd>S</kbd>,
<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>S</kbd>, <kbd>Ctrl</kbd>+<kbd>C</kbd>, <kbd>Ctrl</kbd>+<kbd>X</kbd> and <kbd>Ctrl</kbd>+<kbd>V</kbd>, respectively. They run
the same editor controls, including existing dialogs and review steps.

Camera factory bindings are **W**, **S**, **A**, **D**, **E** and **Q**,
respectively. Camera directions allow single strokes only. Their binding
**Shift** stays off because holding **Shift** independently boosts movement
speed. Held movement stops when the originating key is released, even if its
modifiers have changed.

## Limits

- Text fields and focused controls keep native handling. Configured commands
  run after the focused control rejects the key. Buttons may consume Space or
  Enter; an unbound Tab moves focus. Pointing at the viewport gives it keyboard
  focus unless text is being edited. The desktop must deliver the combination.
- **Escape** stays reserved. Exact <kbd>Ctrl</kbd>+<kbd>K</kbd> opens command search; modified
  alternatives such as <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>K</kbd> can be assigned.
- Arrow keys and **PageUp**/**PageDown**, with no modifier or only **Shift**,
  keep fixed nudging behavior. Include **Ctrl**, **Alt** or **Meta** to bind them.
- A single stroke cannot also be a sequence prefix. Conflicting logical and
  physical bindings reject when they could match the same key input.
- Projection, selection-mode, display, transform, authoring and editor-visibility
  commands start unbound. Older files retain their existing bindings and add
  the new file, clipboard and camera factory bindings.
- Hide requires selected objects and committed geometry. Show all restores only
  temporary editor-hidden objects, including during a preview.
- Changing Objects/Faces clears obsolete selection and drafts. Faces also stops
  brush drawing and clipping. Display toggles do not save grid or preview defaults.
- Invalid or changed storage requires **Reload saved** before another write,
  or explicit **Reset saved shortcuts**. Failures preserve the current bindings
  and unsaved form values.
- Saving bindings does not edit the map. Executing a command has the same effect
  as its editor control.

<!-- page-nav -->

---

[← Compare external maps](map-comparison.md) · [Manual](README.md) · [Navigation defaults →](navigation-defaults.md)
