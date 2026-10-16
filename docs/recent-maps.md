[← Manual](README.md)

# Recent maps

Reopen a map from a persistent list of your twenty most recent maps. Each entry
retains its full path, the game content directory used with it, and its build
settings. Reopening restores compatible build settings through the same operation
as **Build settings…**.

1. Click **Recent…** in the toolbar.
2. Select a map by its file name. Read its full path, availability, game directory
   and retained build settings below the list.
3. Click **Open selected map**. If the current map has unsaved edits or an
   unresolved partition, first select **Discard current unsaved edits when
   opening**. New edits clear this acknowledgement.
4. To stop, click **Cancel / close** or press Escape. Selecting a row or closing
   the window leaves the current map unchanged.

![Recent map paths and retained build settings](images/recent-maps.png)

Use **Refresh list** to recheck file availability. A missing map remains in the
list so a temporarily unavailable location does not erase its retained context.
**Remove from list** forgets the selected entry. To forget every entry, select
**Confirm clear list**, then click **Clear list**. These actions never delete map
files, change the open document or add undo steps. Saving or opening a forgotten
map can add it again.

Successful opens and saves move a map to the front. Repeated paths and symbolic
links to the same file share one entry. Changing **Build settings…** updates an
existing entry without changing its recency or making the map dirty. Failed opens
and primary-file saves leave the list unchanged. A history-storage failure appears
in this window and Activity; a successful primary-file save remains successful.

The equivalent MCP calls are `get_recent_maps`, `open_recent_map`,
`remove_recent_map` and `clear_recent_maps`. Pass the exact `path` returned by
`get_recent_maps`. Opening uses the normal `discard_changes` guard.

## Limits

- The list retains twenty maps. Each absolute UTF-8 path fits in 4096 bytes, and
  preferences fit in a 192 KiB regular file.
- Build context includes lighting, physics, sky quality, visibility, bounce count,
  smoothing angle, **Generate reflections** and **Pack sky assets**. Missing
  booleans in older records default to off. Game content and custom FGD files do
  not switch automatically. A different mounted game keeps the current build settings and
  retains the original entry's context until you explicitly change settings or
  save the map. Active compiles keep their current settings; reopen after the
  compile finishes to restore retained values.
- Recent preferences use `recent-maps.json` alongside recovery storage. Set
  `HAMMERR_RECENT_MAPS_DIR` to an absolute directory to separate this list;
  otherwise it follows `HAMMERR_RECOVERY_DIR` or the default recovery directory.
- Failed writes preserve the last list and can be retried with another save or
  list action. Malformed, unsupported or oversized preferences block changes so
  they cannot be overwritten. Fix the reported storage problem and restart.
- Separate editor processes do not coordinate preference writes. Directory
  synchronization after sudden power loss and automatic map relocation are
  outside this feature.
