[← Manual](README.md)

# Open documents

Keep two maps open in one editor process. Each document has its own window,
selection, camera, drafts, Undo history, build settings and recovery state.

1. Click **Open documents…** in a map window's toolbar.
2. Click **New document**, or **Open document…** and choose a VMF file. These
   actions create another map window. The existing **New**, **Open…**, Recent
   maps and recovery actions still replace the map in their original window.
3. Use **Show document** to present an open map. The list shows its path,
   unsaved state, visibility, lifecycle and cached work status.
4. Click **Close document**, or close that map window. Review any unsaved map
   changes and drafts before choosing whether to discard them. Closing one map
   leaves its peer available; closing the last document exits the editor.

![Two independent maps in the Open documents list](images/open-documents.png)

Desktop MCP-created documents start with an actual hidden window. Use **Show document**
or MCP `show_document` to present them. Show a hidden peer before closing the
last visible map window; Close otherwise refuses and leaves the map open.

The first release allows two documents using one mounted content environment.
An already claimed source or conflicting output is rejected. Successfully
closing a document releases its source/output claims so it can be opened again,
even if an external client retains its closed handle. A failed cleanup stays in
the list with its error, retains capacity and claims, and rejects further edits.
Final process shutdown reports retained cleanup errors.

Shared saved settings and catalogs remain shared. Current editor state and
render caches belong to their fixed documents. Two documents are an admission
limit, not a promise about total memory use. This release has separate windows;
there are no tabs, bulk Quit/Save All, workspace restore, or different mounted
content profiles in the same process.

MCP uses `list_documents`, `new_document`, `open_document`, `show_document` and
`close_document`. Pass the opaque `document_id` from the list to document tools.
Omitting it works only when exactly one live document remains; focus and
last-used document never choose an MCP target. Close uses the exact
`get_map_info.document_close.review_token` and explicit discard consent for
reviewed unsaved loss. Closing one of two live documents keeps the shared
transport available; final Close retains its exact output and process-exit
ordering.
