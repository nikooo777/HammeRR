[← Manual](README.md)

# Transaction history

Inspect edits made by you, agents and system operations in the current map.
Return to the state after a known transaction through the same undo and redo
history used by the main toolbar.

1. Click **History…** beside **Undo** and **Redo** in the main toolbar.
2. Read the applied and redo counts and the current state at the top. Rows are
   newest first, including transactions currently available to redo. Each row
   shows its author, label, state ID and counts of added, modified and removed
   objects. **Newer** and **Older** move through pages of 50 transactions.
3. Click a transaction to inspect its changed object IDs and map sections.
   The selected state is the map immediately after that transaction. Choose
   **Initial state** for the map before its first transaction in this session.
4. Review the displayed undo or redo count, then click **Go to selected state**.
   The result states the direction, exact transaction count and resulting
   state. The redo chain remains available; a new edit replaces that chain
   according to the normal undo behavior.
5. Use **Cancel selection** to clear the target or **Close** to leave the window.
   Escape also closes it. Selecting, inspecting, paging and closing leave the
   source map and history unchanged. **Refresh** reloads the displayed history.

![Current-document transaction inspection and exact navigation](images/history.png)

Limits: history is retained only for the current open document. Opening or
replacing the map starts a new history; reopening a saved VMF does not restore
past transactions. Rows describe actual object-level changes and changed map
sections, with up to 16 object IDs and 256 label characters per transaction.
They do not claim property-by-property diffs or historical screenshots. An
unknown author is shown explicitly when provenance was not recorded.

Navigation supports up to 256 transactions at once. For a longer jump, choose
an intermediate transaction. Apply or cancel active geometry, brush, selection,
measurement and entity-placement drafts before navigating, including placements
still waiting for a surface pick. Navigation waits while a build, launch
or shutdown is in progress. Intervening map edits, saves or document replacement
invalidate the selected target; select it again from the refreshed history.

Navigation uses ordinary document undo/redo and selection cleanup. It leaves the
camera in place and does not save the file. A changed document remains unsaved
until saved, including a return to source content that was previously saved.
Other open editing dialogs retain their existing stale-draft guards.

MCP exposes the same operations as `get_history` and `navigate_history`.
Inspection returns a bounded page, the current history snapshot and exact state
IDs. Navigation requires that snapshot and a returned state ID, or `"0"` for
the initial state; stale and discarded branch targets reject without changes.
