[← Manual](README.md)

# Open documents

Keep two maps open, with separate windows, selections, cameras and Undo histories.

1. Click **Open documents…**.
2. Click **New document** or **Open document…**.
3. Use **Show document** to display a listed map.
4. Use **Close document** to close the chosen map.

Review unsaved changes and drafts before discarding them.
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

## Limits

- Two documents share one mounted content environment and saved preference catalogs.
- Each document retains its own build settings, jobs and recovery state.
- Conflicting source/output files and aliases cannot be reserved twice.
- Failed cleanup retains the document's claims and error.
- Tabs, bulk Quit and workspace restoration are unavailable.
- MCP callers must pass the opaque `document_id` when two maps are open.

See [MCP documents](mcp.md#documents) for routing and Close tokens.
