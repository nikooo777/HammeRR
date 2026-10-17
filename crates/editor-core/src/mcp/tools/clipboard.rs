use super::super::*;

fn prefab_png(thumbnail: &crate::session::prefabs::Thumbnail) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(
        Cursor::new(&mut bytes),
        thumbnail.image.width,
        thumbnail.image.height,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&thumbnail.image.pixels))
        .map_err(|error| format!("Could not encode prefab thumbnail: {error}"))?;
    Ok(bytes)
}

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
        description = "Inspect a prefab thumbnail without initializing graphics. Optional style: flat (default, no texture reads) or textured (static opaque single-base LightmappedGeneric/UnlitGeneric/VertexLitGeneric, identity UV transform, no proxies/detail/second layers/display substitution). Returns actual geometry/bounds, style, asset fingerprints, cache state and resource limits or explicit diagnostics. Textured reads are bounded to 1 MiB per VMT/Patch, 16 MiB per VTF, 64 MiB per admission/revalidation pass and decoded mips, an 80 MiB initial identity-guard read bound, 256 sources and 2048-pixel axes. Shares detached Session preparation with Prefabs… thumbnail controls. Includes authored hidden geometry; excludes sky, normal maps and authored lighting. Exact prefab bytes and Textured asset bytes/winning sources are checked on cache retrieval. Textured failure never substitutes Flat. No map, selection, clipboard, camera, draft or history changes.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_prefab_thumbnail(
        &self,
        Parameters(args): Parameters<InspectPrefabThumbnailArgs>,
    ) -> ToolResult {
        run(|| {
            Ok(self
                .session()
                .prefab_thumbnail_report_with_style(&args.name, args.style.unwrap_or_default()))
        })
    }

    #[tool(
        description = "Render one prefab as a 320×240 PNG through Prefabs… → Render thumbnail / Refresh thumbnail's shared Session renderer. Optional style: flat (default) or textured; inspect_prefab_thumbnail reports exact admission limits. Both use detached real geometry/helpers and include hidden objects. Textured admits bounded static opaque base images with fixed editor shading; no normal maps, authored lighting, sky or native pixel-parity claim. Unsupported or missing assets return an error without an image or automatic Flat substitution; choose flat explicitly. Eight cached images total are keyed by source and style, rechecked against exact prefab/asset bytes, winning mounted sources and content/FGD epochs. refresh:true regenerates the requested style with current assets. No map, clipboard, selection, camera, draft or history changes.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn render_prefab_thumbnail(
        &self,
        Parameters(args): Parameters<RenderPrefabThumbnailArgs>,
    ) -> ToolResult {
        let thumbnail = match self.session().prefab_thumbnail_with_style(
            &args.name,
            args.refresh.unwrap_or(false),
            args.style.unwrap_or_default(),
        ) {
            Ok(thumbnail) => thumbnail,
            Err(error) => return fail(error),
        };
        let bytes = match prefab_png(&thumbnail) {
            Ok(bytes) => bytes,
            Err(error) => return fail(error),
        };
        Ok(CallToolResult::success(vec![
            ContentBlock::text(thumbnail.report.to_string()),
            ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(bytes),
                "image/png",
            ),
        ]))
    }

    #[tool(
        description = "Render one reviewed prefab library page as up to eight 320×240 PNG cards. Requires get_prefabs.snapshot; query/offset choose that catalog page, limit is1–8 (default8), style is flat (default) or textured. Uses the same lazy Session renderer and eight-image cache as Prefabs… → Render page. Source bytes, assets and final catalog are rechecked before returning. Unsupported or malformed entries have individual errors and no image; Textured never substitutes Flat. A stale catalog rejects the whole page. The first text block contains ordered entries with name and image_content_index into this result's content array, or an error. No map, clipboard, selection, placement review, camera or history changes.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn render_prefab_gallery(
        &self,
        Parameters(args): Parameters<RenderPrefabGalleryArgs>,
    ) -> ToolResult {
        let gallery = match self.session().render_prefab_gallery(
            args.query.as_deref().unwrap_or_default(),
            args.offset.unwrap_or_default(),
            args.limit
                .unwrap_or(crate::session::prefabs::MAX_GALLERY_ENTRIES),
            args.style.unwrap_or_default(),
            &args.snapshot,
        ) {
            Ok(gallery) => gallery,
            Err(error) => return fail(error),
        };
        let mut report = gallery.report;
        let mut images = Vec::new();
        for (index, entry) in gallery.entries.into_iter().enumerate() {
            if let Ok(thumbnail) = entry.thumbnail {
                match prefab_png(&thumbnail) {
                    Ok(bytes) => {
                        report["entries"][index]["image_content_index"] = json!(images.len() + 1);
                        images.push(ContentBlock::image(
                            base64::engine::general_purpose::STANDARD.encode(bytes),
                            "image/png",
                        ));
                    }
                    Err(error) => report["entries"][index]["error"] = json!(error),
                }
            }
        }
        let mut content = vec![ContentBlock::text(report.to_string())];
        content.extend(images);
        if let Err(error) = self.session().validate_prefab_gallery(
            &gallery.review,
            args.query.as_deref().unwrap_or_default(),
            args.offset.unwrap_or_default(),
            args.limit
                .unwrap_or(crate::session::prefabs::MAX_GALLERY_ENTRIES),
            args.style.unwrap_or_default(),
        ) {
            return fail(error);
        }
        Ok(CallToolResult::success(content))
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

#[cfg(test)]
mod authority_tests;
#[cfg(test)]
mod gallery_tests;
#[cfg(test)]
mod thumbnail_tests;
