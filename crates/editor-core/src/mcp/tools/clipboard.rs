use super::super::*;

#[tool_router(router = clipboard_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the detached object clipboard, current snapshot/content revision, counts, external links, bounds, Cut-name restoration availability and owned paste preview. Does not read the system clipboard. UI: Paste…. Copy/Cut capture committed objects; paste stays detached across map replacement. Limits: 16 sources, 256 objects including owned brushes, 8192 sides, 256 groups, 8 MiB text and 64 nesting levels. Native layer ancestors and memberships transfer with fresh independent IDs. Saved partitions and numeric/uncertain/script-driven links reject. Private Hammer clipboard formats are not implemented.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_clipboard(&self) -> ToolResult {
        run(|| Ok(self.session().clipboard_report()))
    }

    #[tool(
        description = "Copy whole selected or explicit objects to a detached clipboard using get_clipboard.snapshot. Preserves map geometry, history, camera and selection. destination session (default) captures only the shared Session clipboard; system also publishes UTF-8 VMF text to the computer clipboard. Failed capture/publication preserves the previous payload and draft. UI: Copy (system). Use export_clipboard to transfer between AI sessions."
    )]
    pub(in crate::mcp) fn copy_objects(
        &self,
        Parameters(args): Parameters<ClipboardCaptureArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let system = match args.destination.as_deref().unwrap_or("session") {
                "session" => false,
                "system" => true,
                _ => return Err("destination must be session or system".into()),
            };
            session.capture_clipboard(Actor::Agent, &ids, &args.snapshot, false, system)
        })
    }

    #[tool(
        description = "Capture and Cut whole objects using get_clipboard.snapshot. Fully validates the detached payload and deletion first, then publishes the requested session/system clipboard before removing sources in one Undo step. Native publication failure never deletes objects. Same-map paste can restore original names only while the post-Cut map and definitions remain unchanged. Incoming links may remain broken until Paste or Undo; inspect them before Cut. UI: Cut (system)."
    )]
    pub(in crate::mcp) fn cut_objects(
        &self,
        Parameters(args): Parameters<ClipboardCaptureArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let system = match args.destination.as_deref().unwrap_or("session") {
                "session" => false,
                "system" => true,
                _ => return Err("destination must be session or system".into()),
            };
            session.capture_clipboard(Actor::Agent, &ids, &args.snapshot, true, system)
        })
    }

    #[tool(
        description = "Import bounded UTF-8 VMF text into the shared object clipboard with get_clipboard.snapshot. Provide text OR system:true to explicitly read the computer clipboard. Geometry, history and selection stay unchanged; invalid import retains the old payload and preview. Whole VMF text transfers brushes/entities and their native group ancestors/object notes; map world settings, cameras, regions and unrelated map metadata are excluded. Text/private formats are reported explicitly. UI: Paste… → Read system clipboard."
    )]
    pub(in crate::mcp) fn import_clipboard(
        &self,
        Parameters(args): Parameters<ClipboardImportArgs>,
    ) -> ToolResult {
        run(|| {
            if args.text.is_some() == args.system.unwrap_or(false) {
                return Err("Provide text or system:true, exclusively".into());
            }
            self.session()
                .import_clipboard(Actor::Agent, args.text.as_deref(), &args.snapshot)
        })
    }

    #[tool(
        description = "Export the exact get_clipboard.payload.content_revision as UTF-8 VMF text. destination response (default) returns text for cross-session import; system explicitly publishes to the computer clipboard. Retains native clipboard ownership for the editor lifetime. No map/history changes. UI: Paste… → Publish to system clipboard."
    )]
    pub(in crate::mcp) fn export_clipboard(
        &self,
        Parameters(args): Parameters<ClipboardExportArgs>,
    ) -> ToolResult {
        run(|| {
            let system = match args.destination.as_deref().unwrap_or("response") {
                "response" => false,
                "system" => true,
                _ => return Err("destination must be response or system".into()),
            };
            self.session()
                .export_clipboard(args.content_revision, system)
        })
    }

    #[tool(
        description = "Preview the detached clipboard in the current map using get_clipboard.snapshot. Omitted/zero offset pastes in place; offset uses world XYZ. Allocates fresh object/side/group/layer IDs, preserves nested groups and layers and object notes, and remaps exact internal references to unique names. External links reject unless keep_external_links:true; inspect payload.external_links first. restore_cut_names is permitted only for an unchanged same-map Cut; otherwise unique names apply. render_view/topology share the draft; saving and compiling use committed geometry. Invalid requests keep the last valid preview. UI: Paste… → Preview paste."
    )]
    pub(in crate::mcp) fn preview_paste(
        &self,
        Parameters(args): Parameters<PasteArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let restore_names = args
                .restore_cut_names
                .unwrap_or_else(|| session.can_restore_cut_names());
            session.preview_paste(
                map_core::transfer::Options {
                    offset: args.offset.map(vec3).unwrap_or(DVec3::ZERO),
                    texture_lock: args.texture_lock.unwrap_or(true),
                    keep_external: args.keep_external_links.unwrap_or(false),
                    restore_names,
                },
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Apply the exact owned paste token in one Undo step. Replays and checks geometry, IDs, groups, names, references and metadata against the preview, then selects pasted objects. Changed/replaced map, clipboard, definitions or foreign drafts reject atomically. UI: Paste… → Apply paste."
    )]
    pub(in crate::mcp) fn apply_paste(
        &self,
        Parameters(args): Parameters<RepeatArrayTokenArgs>,
    ) -> ToolResult {
        run(|| self.session().apply_paste(Actor::Agent, args.token))
    }

    #[tool(
        description = "Cancel only the exact owned paste token, preserving the committed map/history and any replacement preview. UI: Paste… → Cancel, Escape or close."
    )]
    pub(in crate::mcp) fn cancel_paste(
        &self,
        Parameters(args): Parameters<RepeatArrayTokenArgs>,
    ) -> ToolResult {
        run(|| self.session().cancel_paste(args.token))
    }

    #[tool(
        description = "List the personal portable-VMF prefab library and inspect a chosen assembly. query/offset/limit page entries (default64/max64); selected decodes one prefab with counts, authored bounds and external links. Returns snapshot for mutations and placement. No map, clipboard or history changes. Limits:128 VMFs,8 MiB each,64 MiB total,512 scanned entries. Assets stay external; private Hammer formats and instances are unavailable. UI: Prefabs….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_prefabs(
        &self,
        Parameters(args): Parameters<PrefabsArgs>,
    ) -> ToolResult {
        run(|| {
            Ok(self.session().prefab_report(
                args.query.as_deref().unwrap_or_default(),
                args.offset.unwrap_or(0),
                args.limit.unwrap_or(64),
                args.selected.as_deref(),
            ))
        })
    }

    #[tool(
        description = "Save committed whole objects as a new named portable VMF prefab with get_prefabs.snapshot. ids defaults to current object selection. Names use1–80 UTF-8 bytes of letters/numbers/spaces/dashes/underscores. Captures the checked clipboard scope, groups/layers, notes and supported references; no map/history/clipboard change. Existing files are never overwritten. UI: Prefabs… → Save selection… → Save prefab."
    )]
    pub(in crate::mcp) fn save_prefab(
        &self,
        Parameters(args): Parameters<SavePrefabArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            session.save_prefab(Actor::Agent, &args.name, &ids, &args.snapshot)
        })
    }

    #[tool(
        description = "Preview a library assembly with get_prefabs.snapshot. position (world XYZ, default0,0,0) receives the source pivot (default original authored bounds center; optional custom pivot). Positive XYZ scale applies before world XYZ rotation in degrees; defaults preserve centered translation. Nonidentity scale requires brushes/brush entities. Unsupported spatial properties and opaque geometry/entity blocks reject rotated/scaled placement. Textures stay attached. Privately validates before replacing clipboard/drafts; source files/map/history remain unchanged. Returned paste.preview.token uses shared apply_paste/cancel_paste/render_view. IDs/names/references/groups/layers/notes remap; external links require keep_external_links:true. UI: Prefabs… → Preview placement…."
    )]
    pub(in crate::mcp) fn preview_prefab(
        &self,
        Parameters(args): Parameters<PreviewPrefabArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().preview_prefab_placement(
                Actor::Agent,
                &args.name,
                crate::session::prefabs::Placement {
                    position: args.position.map(vec3).unwrap_or(DVec3::ZERO),
                    rotation: args.rotation.map(vec3).unwrap_or(DVec3::ZERO),
                    scale: args.scale.map(vec3).unwrap_or(DVec3::ONE),
                    pivot: args.pivot.map(vec3),
                },
                args.keep_external_links.unwrap_or(false),
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Remove the exact named VMF from the personal prefab library using get_prefabs.snapshot. Placed map copies, map history and owned detached paste previews remain intact. File removal is separate from map Undo. Stale library/map/selection state rejects. UI: Prefabs… → Delete prefab… → Delete prefab; Cancel preserves it."
    )]
    pub(in crate::mcp) fn delete_prefab(
        &self,
        Parameters(args): Parameters<PrefabEntryArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .delete_prefab(Actor::Agent, &args.name, &args.snapshot)
        })
    }

    #[tool(
        description = "Rename an exact prefab library entry with get_prefabs.snapshot. new_name uses the same bounded portable names as Save; case-only changes are allowed. Atomic Linux no-replace rename preserves VMF bytes, IDs and asset references; existing destination names, including case variants, reject without changing either entry. No map/selection/clipboard payload/history changes. Existing Paste previews from that entry become stale and must be reopened from the renamed prefab; deletion alone retains detached previews. File operations are separate from map Undo. UI: Prefabs… → Rename… → Rename prefab; Cancel/Escape preserves the library."
    )]
    pub(in crate::mcp) fn rename_prefab(
        &self,
        Parameters(args): Parameters<RenamePrefabArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .rename_prefab(Actor::Agent, &args.name, &args.new_name, &args.snapshot)
        })
    }
}
