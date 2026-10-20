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

Use **Reload saved list** to read changes saved by another editor and recheck
file availability. A successful Reload replaces the list without opening a map
or applying retained build settings. A failed Reload preserves the last valid
list and shows the storage error. A missing map remains in the list so a
temporarily unavailable location does not erase its retained context.
**Remove from list** forgets the selected entry. To forget every entry, select
**Confirm clear list**, then click **Clear list**. These actions never delete map
files, change the open document or add undo steps. Saving or opening a forgotten
map can add it again. Reload clears the **Confirm clear list** acknowledgement.

If another editor changes the saved list, a stale writer preserves that file and
its local list, shows a conflict and blocks list changes. Click **Reload saved
list**, review the updated entries, then retry the action. A map that opened or
saved successfully stays open or saved even when remembering it fails. Reload
does not cancel an owned preview, change selection/camera/history or save a map.

Successful opens and saves move a map to the front. Repeated paths and symbolic
links to the same file share one entry. Changing **Build settings…** updates an
existing entry without changing its recency or making the map dirty. Failed opens
and primary-file saves leave the list unchanged. A history-storage failure appears
in this window and Activity; a successful primary-file save remains successful.

The equivalent MCP calls are `get_recent_maps`, `reload_recent_maps`,
`open_recent_map`, `remove_recent_map` and `clear_recent_maps`. Inspection does
not reload storage automatically. Pass the exact `path` returned by
`get_recent_maps`. Opening uses the normal `discard_changes` guard.

## Limits

- The list retains twenty maps. Each absolute UTF-8 path fits in 4096 bytes, and
  preferences fit in a 192 KiB regular file.
- Build context includes lighting, physics, sky quality, visibility, bounce count,
  smoothing angle, **Bake static-prop vertex lighting**, **Generate reflections**
  and **Pack sky assets**. Missing booleans in older records default to off. Game content and custom FGD files do
  not switch automatically. A different mounted game keeps the current build settings and
  retains the original entry's context until you explicitly change settings or
  save the map. Active compiles keep their current settings; reopen after the
  compile finishes to restore retained values.
- Recent preferences use `recent-maps.json` alongside recovery storage. Set
  `HAMMERR_RECENT_MAPS_DIR` to an absolute directory to separate this list;
  otherwise it follows `HAMMERR_RECOVERY_DIR` or the default recovery directory.
- Ordinary write/lock failures preserve the last list and can be retried after
  repair. Conflicts and malformed, unsupported or oversized preferences block
  changes until **Reload saved list** succeeds. Fix invalid storage externally,
  then Reload. If a write completed but its later verification failed, the error
  explicitly says the list was written; further changes require Reload.
- Participating Linux editors coordinate writes through a regular, persistent
  `.recent-maps.lock` sidecar. Lock contention fails promptly without waiting.
  The lock file must not be removed while editors are running. Manual writers,
  older editors and other programs that ignore it remain outside that guarantee;
  exact source checks reject their changes when observed. Coordinated writes
  are unavailable on other platforms. Directory synchronization after sudden
  power loss and automatic map relocation remain outside this feature.
