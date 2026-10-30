[← Manual](README.md)

# Recent maps

Reopen one of the twenty most recent maps with compatible build settings.

1. Choose **File → Recent maps…**.
2. Select a map.
3. Check its full path and availability.
4. Click **Open selected map**.
5. Review any unsaved changes before discarding them.

New edits invalidate a prior discard acknowledgement.
Cancel keeps the current map.

## Manage the list

- **Reload saved list** reads outside changes and checks availability.
- **Remove from list** forgets the chosen entry.
- **Confirm clear list → Clear list** forgets all entries.

These actions do not delete map files.
Successful Open/Save moves the path to the front.
Missing maps remain listed until removed.

## Limits

- The list holds twenty maps. Symbolic links to the same file share an entry.
- Reopen retains the original source path for relative instance references.
- A missing or retargeted alias requires choosing the map again with **Open…**.
- Game content and custom FGD files do not switch automatically.
- Incompatible content leaves current build settings active.
- A storage conflict requires **Reload saved list** before another change.
- A map can save successfully even when updating its recent-list entry fails.
