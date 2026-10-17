use super::super::*;

#[tool_router(router = atlas_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
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
        description = "Preview fitting each flat brush face independently into one material-atlas rectangle. Pass material, latest inspect_material_atlas snapshot and integer rectangle {x,y,width,height} in original texture-edge pixels (top-left origin). Omitted faces use selected faces; 1–256 world/brush-entity faces. Reuses texture-fit math and preserves axes, rotation, mirroring, geometry and lighting metadata. Terrain, invalid/zero/outside rectangles and collapsed mappings reject atomically without replacing a valid preview. Pass token to revise your own atlas draft; foreign previews reject. Review returned UV bounds and get_face_selection.atlas; finish with commit_face_preview/cancel_face_preview and the exact token. Apply adds one Undo step. Stale assets block commit; the owner can still Cancel. Foreign actors cannot commit or cancel. No texture-file or shader edits."
    )]
    pub(in crate::mcp) fn preview_atlas_fit(
        &self,
        Parameters(args): Parameters<PreviewAtlasFitArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let rectangle = crate::session::atlas_texturing::Rectangle {
                x: args.rectangle.x,
                y: args.rectangle.y,
                width: args.rectangle.width,
                height: args.rectangle.height,
            };
            let state = session.preview_atlas_fit(
                Actor::Agent,
                &faces,
                &args.material,
                rectangle,
                &args.snapshot,
                args.token.as_deref().map(parse_token).transpose()?,
            )?;
            Ok(json!({"atlas":state,"state":face_state(&session)}))
        })
    }
}
