use super::super::*;

#[tool_router(router = view_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "List named camera views stored in this VMF, including projection, position, orientation, field of view and planar zoom. Also returns generation/revision guards. Up to 64 views; names up to 80 characters. Imported Hammer cameras remain preserved separately."
    )]
    pub(in crate::mcp) fn get_camera_views(&self) -> ToolResult {
        run(|| self.session().camera_views_json())
    }

    #[tool(
        description = "Save the current shared camera as a named view in the map. Optional replace_id replaces that view's camera and name. Adds one undo step; save the VMF to persist. Duplicate names are rejected. Optional generation/revision guards prevent stale edits."
    )]
    pub(in crate::mcp) fn save_camera_view(
        &self,
        Parameters(args): Parameters<CameraViewSaveArgs>,
    ) -> ToolResult {
        run(|| {
            let guard = camera_view_guard(args.generation, args.revision)?;
            let mut session = self.session();
            let id = session.save_camera_view(Actor::Agent, &args.name, args.replace_id, guard)?;
            Ok(json!({"id":id,"state":session.camera_views_json()?}))
        })
    }

    #[tool(
        description = "Rename a saved camera view without changing its camera. One undoable map edit; optional generation/revision guards from get_camera_views."
    )]
    pub(in crate::mcp) fn rename_camera_view(
        &self,
        Parameters(args): Parameters<CameraViewRenameArgs>,
    ) -> ToolResult {
        run(|| {
            let guard = camera_view_guard(args.generation, args.revision)?;
            let mut session = self.session();
            session.rename_camera_view(Actor::Agent, args.id, &args.name, guard)?;
            session.camera_views_json()
        })
    }

    #[tool(
        description = "Restore a named camera view into the shared viewport without changing map data, selection or undo history. Restores perspective or planar projection and zoom. Optional generation/revision guards reject stale view IDs."
    )]
    pub(in crate::mcp) fn restore_camera_view(
        &self,
        Parameters(args): Parameters<CameraViewIdArgs>,
    ) -> ToolResult {
        run(|| {
            let guard = camera_view_guard(args.generation, args.revision)?;
            let mut session = self.session();
            session.restore_camera_view(Actor::Agent, args.id, guard)?;
            Ok(camera_json(&session.camera()))
        })
    }

    #[tool(
        description = "Delete a named camera view in one undoable map edit. Optional generation/revision guards reject stale IDs. Does not move the current camera."
    )]
    pub(in crate::mcp) fn delete_camera_view(
        &self,
        Parameters(args): Parameters<CameraViewIdArgs>,
    ) -> ToolResult {
        run(|| {
            let guard = camera_view_guard(args.generation, args.revision)?;
            let mut session = self.session();
            session.delete_camera_view(Actor::Agent, args.id, guard)?;
            session.camera_views_json()
        })
    }

    #[tool(
        description = "Load an external .lin/.pts pointfile into the shared leak-path renderer. Requires a regular UTF-8 file ≤2 MiB containing 2–65536 lines of XYZ coordinates within ±1000000; blank lines allowed. Validates completely before replacing a path. Enables leak visibility without moving the camera or editing map/history. The path becomes stale after map edits and clears on map replacement or a current compile result."
    )]
    pub(in crate::mcp) fn load_pointfile(
        &self,
        Parameters(args): Parameters<PointfileLoadArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.load_pointfile(Actor::Agent, std::path::Path::new(&args.path))?;
            serde_json::to_value(session.pointfile_report(0, 50)?)
                .map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Inspect the shared external or current compiled leak path: source, stale/current state, token, current point, bounds, length and paginated XYZ points. Indices/offsets start at zero; limit defaults to 50, maximum 200. Stale external paths remain inspectable but cannot render or navigate. Shared with the Pointfile window.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_pointfile(
        &self,
        Parameters(args): Parameters<PointfileQueryArgs>,
    ) -> ToolResult {
        run(|| {
            serde_json::to_value(
                self.session()
                    .pointfile_report(args.offset, args.limit.unwrap_or(50))?,
            )
            .map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Unload the shared external or compiled leak path without deleting any file or editing the map. Optional token rejects another client's replacement path."
    )]
    pub(in crate::mcp) fn clear_pointfile(
        &self,
        Parameters(args): Parameters<PointfileTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.clear_pointfile(args.token.as_deref())?;
            serde_json::to_value(session.pointfile_report(0, 1)?).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Move the shared camera to a zero-based point index on the current path. Perspective aims along the next nonzero segment, or continues the last segment at the end; planar views center on the point and retain zoom. Optional token guards replacements; stale paths reject navigation. Does not edit map/history or selection."
    )]
    pub(in crate::mcp) fn navigate_pointfile(
        &self,
        Parameters(args): Parameters<PointfileNavigateArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.navigate_pointfile(args.index, args.token.as_deref())?;
            Ok(
                json!({"camera":camera_json(&session.camera()), "pointfile":session.pointfile_report(0, 1)?}),
            )
        })
    }

    #[tool(
        description = "Frame the entire current pointfile in the shared camera. Aspect is viewport width/height (default 1, range 0.01–100); optional token guards replacement. Stale paths and paths exceeding planar zoom limits reject framing without moving the camera. Shared with Frame path in the Pointfile window."
    )]
    pub(in crate::mcp) fn frame_pointfile(
        &self,
        Parameters(args): Parameters<PointfileActionArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.frame_pointfile(args.aspect.unwrap_or(1.0), args.token.as_deref())?;
            Ok(camera_json(&session.camera()))
        })
    }

    #[tool(
        description = "Set shared viewport visibility and mode in any projection: textured, flat, wireframe, lightmap_density or smoothing_groups. Flat shading ignores brush textures/transparency; wireframe shows geometry through solids. Helpers, selection and tool filters remain active. sky_preview resolves world skyname as six mounted LDR skybox faces on sky surfaces in perspective Textured views with Tool textures enabled; unavailable faces retain the tool surface. No 3D sky-camera transform, fog, HDR or material animation. Choose a smoothing group 1–32; groups 25–32 are hard-edge memberships. Diagnostics inspect authored metadata, not baked lighting or final engine smoothing."
    )]
    pub(in crate::mcp) fn set_view(&self, Parameters(args): Parameters<SetViewArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let mut view = session.surface_view_options(
                args.surface
                    .as_deref()
                    .map(render::SurfaceView::parse)
                    .transpose()?,
                args.smoothing_group,
            )?;
            if let Some(tools) = args.tool_textures {
                view.tool_textures = tools;
            }
            if let Some(leak) = args.leak_path {
                view.leak_path = leak;
            }
            if let Some(sky) = args.sky_preview {
                view.sky_preview = sky;
            }
            session.set_view(view);
            Ok(view_state(view))
        })
    }

    #[tool(
        description = "The user's current viewport camera.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_camera(&self) -> ToolResult {
        ok(camera_json(&self.session().camera()))
    }

    #[tool(
        description = "Move the user's viewport camera or switch perspective/top/front/side. Perspective supports position + look_at; planar views support center + ortho_height. Pass focus to frame object IDs."
    )]
    pub(in crate::mcp) fn set_camera(
        &self,
        Parameters(spec): Parameters<CameraSpec>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let camera = spec.resolve(&session, 1.0)?;
            session.set_camera(camera);
            session.log(Actor::Agent, "moved the camera");
            Ok(camera_json(&camera))
        })
    }

    #[tool(
        description = "Render the map to a PNG with the editor's renderer. Textured, flat, wireframe and diagnostic modes work in perspective or planar views, with entity helpers, grid and shared previews. Selection is highlighted in red. Optional surface and camera overrides leave shared settings unchanged; pass camera.focus to frame objects.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn render_view(
        &self,
        Parameters(args): Parameters<RenderArgs>,
    ) -> ToolResult {
        let size = render_size(args.width, args.height);
        let (camera, mut view) = {
            let session = self.session();
            match args
                .camera
                .unwrap_or_default()
                .resolve(&session, size.0 as f32 / size.1 as f32)
            {
                Ok(camera) => {
                    let view = args
                        .surface
                        .as_deref()
                        .map(render::SurfaceView::parse)
                        .transpose()
                        .and_then(|mode| session.surface_view_options(mode, args.smoothing_group));
                    match view {
                        Ok(view) => (camera, view),
                        Err(error) => return fail(error),
                    }
                }
                Err(message) => return fail(message),
            }
        };
        if let Some(tools) = args.tool_textures {
            view.tool_textures = tools;
        }
        if let Some(leak) = args.leak_path {
            view.leak_path = leak;
        }
        if let Some(sky) = args.sky_preview {
            view.sky_preview = sky;
        }
        let rendered = match self.render_png(&camera, size, view) {
            Ok(rendered) => rendered,
            Err(error) => return fail(error),
        };
        let data = base64::engine::general_purpose::STANDARD.encode(rendered.png);
        let skybox = skybox_state(&mut self.session());
        let material_reload = self.session().material_reload_report().cloned();
        let preview_issues = rendered.issues.iter().map(|issue| json!({"face":{"solid":issue.face.solid,"side":issue.face.side},"message":issue.message})).collect::<Vec<_>>();
        let info = json!({ "camera": camera_json(&camera), "width": size.0, "height": size.1, "skybox":skybox, "preview_issues":preview_issues, "material_reload":material_reload,
            "view": { "tool_textures": view.tool_textures, "sky_preview":view.sky_preview, "leak_path": view.leak_path, "surface": view.surface.name(), "smoothing_group": view.smoothing_group,
                "grid_visible": view.grid_visible, "grid_size": view.grid_size,
                "grid_elevation": view.grid_elevation,
                "block_preview": view.block_preview.map(|bounds| bounds.map(|point| point.to_array())),
                "clip_preview": view.clip_preview.map(|guide| json!({"normal": guide.normal.to_array(), "distance": guide.distance, "bounds": guide.bounds.map(|point| point.to_array())})),
                "transform_handles": view.transform_handles.map(|guide| json!({"mode": transform_mode_name(Some(guide.mode)), "origin": guide.origin.to_array(), "axes": guide.axes.map(|axis| axis.to_array()), "active": guide.active.map(render::handles::Handle::name)})),
                "area_preview": view.area_preview.map(|guide| json!({"ndc": guide.corners.map(|point| point.to_array()), "contained": guide.contained, "aspect": guide.aspect, "camera": camera_json(&guide.camera)})) } });
        Ok(CallToolResult::success(vec![
            ContentBlock::image(data, "image/png"),
            ContentBlock::text(info.to_string()),
        ]))
    }

    #[tool(
        description = "Identify the object and face at a pixel of an image rendered by render_view with the same camera and size.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn pick_at_pixel(
        &self,
        Parameters(args): Parameters<PickArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let (w, h) = render_size(args.width, args.height);
            let camera = args
                .camera
                .unwrap_or_default()
                .resolve(&session, w as f32 / h as f32)?;
            let ndc_x = 2.0 * (args.x + 0.5) / w as f32 - 1.0;
            let ndc_y = 1.0 - 2.0 * (args.y + 0.5) / h as f32;
            let (origin, direction) = camera.ray(ndc_x, ndc_y, w as f32 / h as f32);
            let ray = Ray {
                origin: origin.as_dvec3(),
                direction: direction.as_dvec3(),
            };
            let hit = session.pick(&ray);
            Ok(hit.map_or(json!({ "hit": null }), |hit| {
                hit_json(session.render_document(), &hit)
            }))
        })
    }

    #[tool(
        description = "Cast a ray and report the first visible object and face hit, e.g. to find the floor below a point.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn raycast(&self, Parameters(args): Parameters<RaycastArgs>) -> ToolResult {
        run(|| {
            if vec3(args.direction).length_squared() == 0.0 {
                return Err("direction must not be zero".into());
            }
            let mut session = self.session();
            let ray = Ray {
                origin: vec3(args.origin),
                direction: vec3(args.direction),
            };
            let hit = session.pick(&ray);
            Ok(hit.map_or(json!({ "hit": null }), |hit| {
                hit_json(session.render_document(), &hit)
            }))
        })
    }

    #[tool(
        description = "Inspect depth-ordered object hits along an explicit world ray without changing selection or history. Returns at most 64 candidates, exact total/truncation, actual hit object/brush/side/point/distance, and each expanded selection unit. Visible brush children deduplicate to owners, then outer object groups unless bypassed, before the cap. Usable model triangles replace helper boxes; hidden/tool filters match ordinary picking. Stable IDs break equal-depth ties. Objects mode only; active specialized tools and geometry previews reject.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_object_hit_candidates(
        &self,
        Parameters(args): Parameters<ObjectHitArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().object_hit_candidates(&Ray {
                origin: vec3(args.origin),
                direction: vec3(args.direction),
            })
        })
    }

    #[tool(
        description = "Cycle object selection along an explicit world ray through the shared Alt-click operation. Replace-only and stateless: if the exact current selection equals a candidate's expanded owner/group unit, select the next candidate and wrap; otherwise select the nearest. Empty rays leave selection unchanged. Recomputes current visible brush/model geometry and group bypass each call; returns chosen zero-based index plus candidates/total/truncation. Limited to the nearest 64 deduplicated units. No geometry or Undo edit; incompatible modes/tools/previews reject."
    )]
    pub(in crate::mcp) fn cycle_object_hit(
        &self,
        Parameters(args): Parameters<ObjectHitArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().cycle_object_hit(
                Actor::Agent,
                &Ray {
                    origin: vec3(args.origin),
                    direction: vec3(args.direction),
                },
            )
        })
    }
}
