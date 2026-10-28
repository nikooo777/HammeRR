[← Manual](README.md)

# Open documents

Keep two maps open, with separate windows, selections, cameras and Undo histories.

1. Click **Open documents…**.
2. Click **New document** or **Open document…**.
3. Use **Show document** to display a listed map. Use **Show silently** to keep keyboard focus in your current window.
4. Use **Close document** to close the chosen map.

Review unsaved changes and drafts before discarding them.
If Close reports active work, inspect **Active** in the document row. Wait for
the named request or worker, or cancel it with its normal control.
Closing the last document exits the editor.
The ordinary **New** and **Open…** controls replace their current map instead.

## Save all named maps

1. Open **Open documents…**.
2. Click **Save all named maps**.
3. Inspect every document result.

The operation saves named, dirty maps without opening file choosers.
A clean named map has its source checked without being rewritten.
A dirty unnamed map needs **Save as…** first.
Pending drafts need Apply or Cancel first.
Successful saves remain saved even when another map fails.

## Quit all maps

1. Open **Open documents…** and click **Quit all…**.
2. Review every map, including hidden maps, unsaved changes and pending drafts.
3. Click **Quit all reviewed documents**. If changes need discarding, click **Discard reviewed changes and quit all** instead.

**Cancel quit** keeps all maps open. Closing the last visible window with hidden
maps opens this same review. A changed or busy map requires a fresh review.
Failed cleanup retains that map's claims and reports the error.

## Limits

- **Show silently** requires Hyprland with Lua window rules. It adds one runtime rule for HammeRR background windows; no configuration file changes. An unavailable rule causes an error before display.
- Two documents share one mounted content environment and saved preference catalogs.
- Each document retains its own build settings, jobs and recovery state.
- Conflicting source/output files and aliases cannot be reserved twice.
- Failed cleanup retains the document's claims and error.
- Tabs and workspace restoration are unavailable.
- MCP callers must pass the opaque `document_id` when two maps are open.

See [MCP documents](mcp.md#documents) for routing and Close tokens.
