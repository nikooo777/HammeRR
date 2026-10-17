use super::super::*;

#[tool_router(router = view_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the default-off unshadowed authored LDR point/spot lighting preview without graphics initialization. Reports admitted/excluded lights and flat brush receiver polygons, current render-document/working-region/material snapshot, bounded diagnostics and editor limits. Static unnamed style-0 lights only, maximum 32; overflow disables preview rather than choosing a prefix. Zero admitted lights make supported receivers black. Set shared lighting_preview, lighting_quality fast/detail and lighting_exposure_ev with set_view, or temporary render_view overrides. Fast interpolates vertex lighting and rejects enabled normal maps; Detail composes with supported normal maps. No sun, ambient, bounce, shadows, animation, native brightness or tone-map parity. Source/history/camera and pending geometry reviews remain unchanged.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_lighting_preview(&self) -> ToolResult {
        ok(self.session().lighting_preview_state())
    }

    #[tool(
        description = "Inspect shared normal_maps preview admission through the same CPU scene as the viewport. Defaults off; enable with set_view.normal_maps or override render_view.normal_maps. Reports admitted materials, mounted VMT/VTF provenance, exact unsupported-state errors, applied/rejected flat brush polygons and resource budgets. LightmappedGeneric and ordinary static RGB tangent-space normals only. Terrain, models, sprites/icons/helpers and non-Textured views are excluded. Fixed editor directional shading or enabled Detail unshadowed authored lights; no native game parity. Reload material assets clears cached successes/failures. No graphics initialization or source/history/camera mutation.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_normal_map_preview(&self) -> ToolResult {
        ok(self.session().normal_map_preview_state())
    }

    #[tool(
        description = "Inspect baked BSP face lightmaps from this session's last successful owned build, shared with Build status → Inspect baked lightmaps…. Read-only CPU operation; no GPU, game, map/history/selection/camera changes. Optional channel ldr/hdr (default ldr), exact zero-based compiled face, style_index and map_index (0 base, 1–3 directional bump maps). Omitted face selects the first valid lit face. Paginated face summaries use offset and limit (1–32). Returns explicit current/stale/unavailable build provenance, LDR/HDR availability, dimensions/styles/bytes, decoded linear RGBExp32 minimum/mean/maximum and stored base average, diagnostics and a PNG no larger than 128 pixels per side. Display conversion is documented separately from linear statistics and is not native tone mapping. No authored VMF face mapping is asserted. Changed/missing BSP files cannot be inspected; stale map/source/settings builds remain explicitly stale. No path argument.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_baked_lightmaps(
        &self,
        Parameters(args): Parameters<InspectBakedLightmapsArgs>,
    ) -> ToolResult {
        use crate::session::baked_lightmaps::{Channel, Query};
        let result = (|| {
            let query = Query {
                channel: Channel::parse(args.channel.as_deref().unwrap_or("ldr"))?,
                face: args.face,
                offset: args.offset.unwrap_or_default(),
                limit: args
                    .limit
                    .unwrap_or(crate::session::baked_lightmaps::MAX_PAGE_FACES),
                style_index: args.style_index.unwrap_or_default(),
                map_index: args.map_index.unwrap_or_default(),
            };
            self.session().inspect_baked_lightmaps(&query)
        })();
        let inspection = match result {
            Ok(inspection) => inspection,
            Err(error) => return fail(error),
        };
        let mut content = vec![ContentBlock::text(inspection.report.to_string())];
        if let Some(image) = inspection.image {
            let mut bytes = Vec::new();
            let mut encoder = png::Encoder::new(Cursor::new(&mut bytes), image.width, image.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let result = encoder
                .write_header()
                .and_then(|mut writer| writer.write_image_data(&image.pixels));
            if let Err(error) = result {
                return fail(error.to_string());
            }
            content.push(ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(bytes),
                "image/png",
            ));
        }
        Ok(CallToolResult::success(content))
    }

    #[tool(
        description = "Inspect the exact applied world skyname through the same read-only Session operation as World settings → Inspect applied sky…. Returns six material names, directory/VPK and Patch provenance, authored LDR/HDR references, VTF dimensions/formats, bounded read counts and actionable issues. ldr_ready/hdr_ready describe supported material/VTF structure only; they do not establish editor preview readiness, HDR radiance, actual game channel selection or portable sky packing. No family substitution, document/history/camera/draft changes or native processes. No arguments; refresh by calling again.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_sky_assets(
        &self,
        Parameters(_args): Parameters<InspectSkyAssetsArgs>,
    ) -> ToolResult {
        ok(json!(self.session().inspect_sky_assets()))
    }

    #[tool(
        description = "Inspect the shared Rope preview toggle, checked authored move_rope/keyframe_rope NextKey spans, static curve points, Slack/Width, bounded limits and link diagnostics. Enable through set_view.rope_preview or override one render_view. Neutral helper shading; no RopeMaterial fidelity, physics, collision, wind, runtime I/O, parenting or attachment binding. Clicking a span selects its source entity; endpoint helpers, area selection and framing remain available.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_rope_preview(&self) -> ToolResult {
        ok(self.session().rope_preview_state())
    }

    #[tool(
        description = "Inspect shared Fog preview settings and all authored env_fog_controller choices, parsed parameters and visible errors. Defaults off; no automatic controller selection. Snapshot guards reviewed settings. Static initial view-depth range fog in perspective Textured views only; sky/helpers unchanged. No native parity, radial/directional blend, far-Z clipping, HDR/zoom scaling, water, fog-volume switching, runtime I/O or transitions.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_fog_preview(&self) -> ToolResult {
        ok(self.session().fog_preview_state())
    }

    #[tool(
        description = "Apply Fog preview using the same Session operation as the Fog preview… dialog. Pass enabled, explicit env_fog_controller entity:<id> when enabling, and latest get_fog_preview.snapshot. Invalid/unsupported controllers and stale snapshots leave settings unchanged. Disabling always works with a fresh snapshot. Preview controls leave map/history unchanged. render_view accepts temporary fog_preview/fog_controller overrides."
    )]
    pub(in crate::mcp) fn set_fog_preview(
        &self,
        Parameters(args): Parameters<FogPreviewArgs>,
    ) -> ToolResult {
        run(|| {
            let controller = fog_controller_id(args.controller.as_deref())?;
            self.session()
                .set_fog_preview(args.enabled, controller, &args.snapshot)
        })
    }

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
        description = "Set shared viewport visibility and mode in any projection: lighting_preview enables bounded unshadowed static point/spot diffuse lighting; lighting_quality fast/detail and lighting_exposure_ev -8..8 control its editor display. Inspect get_lighting_preview for admitted/excluded coverage and limits. No sun/ambient/bounce/shadows or native-brightness parity. Modes: textured, flat, wireframe, lightmap_density or smoothing_groups. normal_maps defaults off and previews admitted flat-brush normal maps under fixed editor directional shading in Textured views; inspect get_normal_map_preview for diagnostics and limits. Terrain/models/resource guides are excluded; Detail unshadowed authored lighting composes with admitted normal maps. Flat shading ignores brush textures/transparency; wireframe shows geometry through solids. selection_outlines defaults on and controls selected object/face edges, marker outlines, model brackets and rope centerlines while preserving surfaces, selection tint, picking and diagnostic/tool guides. entity_images and entity_model_guides default on and control literal FGD icons/static Sprite resource images and studio/studioprop editor meshes respectively. Off retains ordinary editable boxes and defers guide asset reads; authored models, lights, ropes and other guides remain independent. Pure resource visibility preserves pending reviews. Helpers, selection and tool filters remain active. rope_preview shows bounded static authored NextKey rope spans with Slack/Width; inspect get_rope_preview for diagnostics and limits. No rope physics, parenting or attachment binding. sky_preview resolves world skyname as six mounted skybox faces selected by sky_channel (ldr default or explicit hdr), with sky_exposure_ev -8..8 applying only to HDR under the documented linear/Reinhard editor policy. This does not select game or build channels. The preview draws on sky surfaces in perspective Textured views with Tool textures enabled; unavailable faces retain the tool surface. No 3D sky-camera transform, fog or material animation; unavailable HDR never falls back to LDR. Choose a smoothing group 1–32; groups 25–32 are hard-edge memberships. Diagnostics inspect authored metadata, not baked lighting or final engine smoothing."
    )]
    pub(in crate::mcp) fn set_view(&self, Parameters(args): Parameters<SetViewArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let view = session.resolve_view_update(args.view_update())?;
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
        description = "Render the map to a PNG with the editor's renderer. Textured, flat, wireframe and diagnostic modes work in perspective or planar views, with entity helpers, grid and shared previews. Selection is highlighted in red. Optional surface and camera overrides leave shared settings unchanged; pass camera.focus to frame objects. additive_sprite_guides is an optional authored mode5 LDR editor-image approximation with constrained white/255/FX0 appearance. It requires entity_images, preserves literal icons and reports raw/additive policy and rejected appearance in sprite_guides; no native runtime/color-space parity.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn render_view(
        &self,
        Parameters(args): Parameters<RenderArgs>,
    ) -> ToolResult {
        let size = render_size(args.width, args.height);
        let (camera, mut view) = {
            let session = self.session();
            let camera = match args
                .camera
                .as_ref()
                .unwrap_or(&CameraSpec::default())
                .resolve(&session, size.0 as f32 / size.1 as f32)
            {
                Ok(camera) => camera,
                Err(error) => return fail(error),
            };
            let view = match session.resolve_view_update(args.view_update()) {
                Ok(view) => view,
                Err(error) => return fail(error),
            };
            (camera, view)
        };
        let mut fog_preview = {
            let session = self.session();
            let controller = match fog_controller_id(args.fog_controller.as_deref()) {
                Ok(controller) => controller,
                Err(error) => return fail(error),
            };
            match session.fog_render_state(args.fog_preview, controller) {
                Ok((fog, report)) => {
                    view.fog = fog;
                    report
                }
                Err(error) => return fail(error),
            }
        };
        let rendered = match self.render_png(&camera, size, view) {
            Ok(rendered) => rendered,
            Err(error) => return fail(error),
        };
        let data = base64::engine::general_purpose::STANDARD.encode(rendered.png);
        let skybox = self.session().sky_preview_state(view, camera);
        fog_preview["applied"] = json!(
            view.fog
                .is_some_and(|fog| fog.enabled() && fog.max_density() > 0.0)
                && view.surface == render::SurfaceView::Textured
                && camera.projection == Projection::Perspective
        );
        let material_reload = self.session().material_reload_report().cloned();
        let preview_issues = rendered.issues.iter().map(|issue| json!({"face":{"solid":issue.face.solid,"side":issue.face.side},"message":issue.message})).collect::<Vec<_>>();
        let working_region = self.session().working_region_summary();
        let rope_preview = self.session().rope_preview_report(view.rope_preview);
        let mut rendered_view = view_state(view);
        let extra = json!({"grid_visible": view.grid_visible, "grid_size": view.grid_size,
                "grid_elevation": view.grid_elevation,
                "block_preview": view.block_preview.map(|bounds| bounds.map(|point| point.to_array())),
                "clip_preview": view.clip_preview.map(|guide| json!({"normal": guide.normal.to_array(), "distance": guide.distance, "bounds": guide.bounds.map(|point| point.to_array())})),
                "transform_handles": view.transform_handles.map(|guide| json!({"mode": transform_mode_name(Some(guide.mode)), "origin": guide.origin.to_array(), "axes": guide.axes.map(|axis| axis.to_array()), "active": guide.active.map(render::handles::Handle::name)})),
                "area_preview": view.area_preview.map(|guide| json!({"ndc": guide.corners.map(|point| point.to_array()), "contained": guide.contained, "aspect": guide.aspect, "camera": camera_json(&guide.camera)}))});
        rendered_view
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let info = json!({ "working_region":working_region, "camera": camera_json(&camera), "width": size.0, "height": size.1, "skybox":skybox, "fog_preview":fog_preview, "rope_preview":rope_preview, "sprite_guides":rendered.sprite_guides, "preview_issues":preview_issues, "normal_maps":{"report":rendered.normal_maps,"limits":render::normal_maps::limits()}, "lighting_preview":{"report":rendered.lighting,"limits":render::lighting::limits()}, "material_reload":material_reload, "view": rendered_view });
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
            let hit = session.pick_with_rope_preview(&ray, args.rope_preview);
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
