use super::super::*;

#[tool_router(router = atlas_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "List the map's saved atlas regions and catalog snapshot. At most 128 named regions retain canonical material, original image dimensions/identity, integer pixel rectangle and padding in editor-only VMF metadata. Names are unique per material ignoring case. This CPU-only read leaves map, history, camera, selection and previews unchanged. Unsupported or malformed metadata rejects without overwriting it.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn list_atlas_regions(&self) -> ToolResult {
        run(|| self.session().atlas_regions_state())
    }

    #[tool(
        description = "Save a named map-local atlas region using the same Session operation as Save region in Fit atlas region. Pass the latest inspect_material_atlas snapshot, latest list_atlas_regions catalog_snapshot, material, original-image rectangle and optional padding. Captures the inspected effective image identity and dimensions; does not fit faces or save a fit mode/reference. Adds one Undo step. Duplicate per-material names, invalid bounds/padding, stale source/catalog and any active geometry preview reject atomically. Apply or cancel geometry previews first. Texture pixels follow the existing explicit Reload material assets cache policy."
    )]
    pub(in crate::mcp) fn save_atlas_region(
        &self,
        Parameters(args): Parameters<SaveAtlasRegionArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().save_atlas_region(
                Actor::Agent,
                &args.name,
                &args.material,
                crate::session::atlas_texturing::FitRegion {
                    rectangle: crate::session::atlas_texturing::Rectangle {
                        x: args.rectangle.x,
                        y: args.rectangle.y,
                        width: args.rectangle.width,
                        height: args.rectangle.height,
                    },
                    padding: args.padding,
                },
                &args.snapshot,
                &args.catalog_snapshot,
            )
        })
    }

    #[tool(
        description = "Rename a saved atlas region by ID, guarded by the latest list_atlas_regions catalog_snapshot. Names need 1–80 characters without controls and must be unique per material ignoring case. Preserves its source identity, rectangle and padding. One Undo step; stale catalog, missing ID and active geometry previews reject atomically."
    )]
    pub(in crate::mcp) fn rename_atlas_region(
        &self,
        Parameters(args): Parameters<RenameAtlasRegionArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().rename_atlas_region(
                Actor::Agent,
                args.id,
                &args.name,
                &args.catalog_snapshot,
            )
        })
    }

    #[tool(
        description = "Delete a saved atlas region by ID with the latest list_atlas_regions catalog_snapshot. Removes only editor metadata in one Undo step, preserving face projections. Stale catalog, missing ID and active geometry previews reject atomically. Stale image identities can still be renamed or deleted."
    )]
    pub(in crate::mcp) fn delete_atlas_region(
        &self,
        Parameters(args): Parameters<DeleteAtlasRegionArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .delete_atlas_region(Actor::Agent, args.id, &args.catalog_snapshot)
        })
    }

    #[tool(
        description = "Choose a saved atlas region for the inspected material using ID, latest inspect_material_atlas snapshot and latest list_atlas_regions catalog_snapshot. Checks canonical material, original dimensions and effective image identity; material/image drift rejects without changing fields, faces or previews. Returns rectangle, padding, material and source snapshot to pass to existing preview_atlas_fit. Choose the transient fit mode/reference there, then use the same owned commit_face_preview/cancel_face_preview token flow. This CPU-only choice adds no history; image content follows explicit Reload material assets cache policy.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn choose_atlas_region(
        &self,
        Parameters(args): Parameters<ChooseAtlasRegionArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().choose_atlas_region(
                args.id,
                &args.material,
                &args.snapshot,
                &args.catalog_snapshot,
            )
        })
    }

    #[tool(
        description = "Inspect a material's supported atlas through the same Session operation as Face Textures → Fit atlas region…. Returns exact cached $basetexture dimensions, asset snapshot and a CPU PNG thumbnail ≤256 px. Static 2D single-layer LightmappedGeneric/UnlitGeneric only; rejects substituted %tooltexture, proxies/fallbacks, nonidentity base transforms and animated/volume/cubemap textures. Cached/fresh VMT identity mismatch requires Reload material assets. Does not edit the map, history, camera or selection; no graphics startup. New reads are bounded to 1 MiB per VMT/Patch and 32 MiB per VTF. Thumbnail and viewport share cached base pixels until explicit material reload.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_material_atlas(
        &self,
        Parameters(args): Parameters<InspectMaterialAtlasArgs>,
    ) -> ToolResult {
        let atlas = match self.session().inspect_material_atlas(&args.material) {
            Ok(atlas) => atlas,
            Err(error) => return fail(error),
        };
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(
            Cursor::new(&mut bytes),
            atlas.image.width,
            atlas.image.height,
        );
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let result = encoder
            .write_header()
            .and_then(|mut writer| writer.write_image_data(&atlas.image.pixels));
        if let Err(error) = result {
            return fail(error.to_string());
        }
        Ok(CallToolResult::success(vec![
            ContentBlock::text(atlas.report.to_string()),
            ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(bytes),
                "image/png",
            ),
        ]))
    }

    #[tool(
        description = "Preview flat brush faces fitted into one material-atlas rectangle. mode per_face (default) fits each face independently and preserves its frame. mode together requires explicit reference {solid,side} belonging to targets: same-facing coplanar faces share that reference's axes, skew, rotation and mirroring; combined bounds fit once, including gaps, replacing other target frames. Reference is only allowed with together. Pass material, latest inspect_material_atlas snapshot and integer rectangle {x,y,width,height} in original source-image edge pixels (top-left origin). Optional nonnegative integer padding (default 0) insets all four edges in source-image pixels and must leave positive usable width/height. The outer rectangle must stay within the actual image. Omitted faces use selection; 1–256 world/brush-entity faces. Geometry and nonprojection metadata are preserved. Terrain, noncoplanar/opposite-facing together targets, invalid regions, references and collapsed mappings reject atomically without replacing a valid preview. Pass token to revise your own atlas draft; foreign previews reject. Review returned usable UV bounds and get_face_selection.atlas; nonzero padding adds padding/usable_rectangle, together adds mode/reference. Default responses retain their former shape. Finish with commit_face_preview/cancel_face_preview and exact token. Apply adds one Undo step. Stale assets block commit; the owner can still Cancel. Foreign actors cannot commit or cancel. No texture-file or shader edits."
    )]
    pub(in crate::mcp) fn preview_atlas_fit(
        &self,
        Parameters(args): Parameters<PreviewAtlasFitArgs>,
    ) -> ToolResult {
        run(|| {
            use crate::session::atlas_texturing::{FitMode, FitOptions};

            let mode = match (args.mode, args.reference.as_ref()) {
                (AtlasFitModeArgs::PerFace, None) => FitMode::PerFace,
                (AtlasFitModeArgs::Together, Some(reference)) => FitMode::Together {
                    reference: face_id(reference),
                },
                (AtlasFitModeArgs::Together, None) => {
                    return Err("Together atlas fitting requires an explicit reference face".into());
                }
                (AtlasFitModeArgs::PerFace, Some(_)) => {
                    return Err("An atlas reference face is only valid with mode together".into());
                }
            };
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let region = crate::session::atlas_texturing::FitRegion {
                rectangle: crate::session::atlas_texturing::Rectangle {
                    x: args.rectangle.x,
                    y: args.rectangle.y,
                    width: args.rectangle.width,
                    height: args.rectangle.height,
                },
                padding: args.padding,
            };
            let state = session.preview_atlas_fit_options(
                Actor::Agent,
                &faces,
                &args.material,
                FitOptions { region, mode },
                &args.snapshot,
                args.token.as_deref().map(parse_token).transpose()?,
            )?;
            Ok(json!({"atlas":state,"state":face_state(&session)}))
        })
    }
}
