use super::*;

pub(in crate::mcp) struct Offscreen {
    pub(in crate::mcp) ctx: GpuContext,
    pub(in crate::mcp) renderer: Renderer,
    pub(in crate::mcp) scene: Option<(render::SceneKey, GpuScene)>,
    pub(in crate::mcp) compiled_terrain_snapshot_guard:
        Option<crate::session::compiled_terrain_snapshot::RenderGuard>,
    pub(in crate::mcp) player_support_observation_guard:
        Option<crate::session::player_support_observation::RenderGuard>,
    pub(in crate::mcp) issues: render::scene::SceneIssues,
    pub(in crate::mcp) normal_maps: render::normal_maps::Report,
    pub(in crate::mcp) lighting: render::lighting::Report,
    pub(in crate::mcp) sprite_guides: Value,
    pub(in crate::mcp) instance_contents: Value,
    pub(in crate::mcp) sky3d_preview: Value,
}

impl Offscreen {
    pub(in crate::mcp) fn new(ctx: GpuContext) -> Self {
        Self {
            renderer: Renderer::new(&ctx),
            ctx,
            scene: None,
            compiled_terrain_snapshot_guard: None,
            player_support_observation_guard: None,
            issues: Default::default(),
            normal_maps: Default::default(),
            lighting: Default::default(),
            sprite_guides: Value::Null,
            instance_contents: Value::Null,
            sky3d_preview: Value::Null,
        }
    }
}

#[derive(Default)]
pub(in crate::mcp) struct OffscreenState {
    pub(in crate::mcp) renderer: Option<Offscreen>,
    pub(in crate::mcp) error: Option<String>,
}

pub(in crate::mcp) struct RenderedView {
    metadata: FrameMetadata,
    pub(in crate::mcp) instance_contents: Value,
    pub(in crate::mcp) sky3d_preview: Value,
    pub(in crate::mcp) normal_maps: render::normal_maps::Report,
    pub(in crate::mcp) lighting: render::lighting::Report,
    pub(in crate::mcp) sprite_guides: Value,
    pub(in crate::mcp) png: Vec<u8>,
    pub(in crate::mcp) issues: render::scene::SceneIssues,
}

struct FrameMetadata {
    scene_display: Value,
    lighting_preview: Value,
    selection_presentation: Value,
    helper_presentation: Value,
    color_correction: Value,
    skybox: Value,
    working_region: Value,
    rope_preview: Value,
    material_reload: Value,
    compiled_details: Value,
    baked_surfaces: Value,
    compiled_terrain_snapshot: Value,
    player_support_observation: Value,
}

impl FrameMetadata {
    fn capture(
        session: &mut Session,
        view: ViewOptions,
        camera: Camera,
        display: render::ResolvedSceneDisplay,
        lighting: &render::lighting::Report,
    ) -> Self {
        Self {
            scene_display: session.scene_display_report_for_resolved(view, &camera, display),
            lighting_preview: session
                .lighting_preview_state_for_render(view, camera, display, lighting),
            selection_presentation: json!(session.selection_presentation(view)),
            helper_presentation: session.helper_presentation(view),
            color_correction: session.color_correction_report(),
            skybox: session.sky_preview_state_for_render(view, camera, display),
            working_region: session.working_region_summary(),
            rope_preview: session.rope_preview_report(view.rope_preview),
            material_reload: json!(session.material_reload_report()),
            compiled_details: json!({"state":session.compiled_details_observation(),"lighting":session.compiled_details_lighting_report_for_render(view,camera,display)}),
            baked_surfaces: session.baked_surfaces_render_report_for_render(view, camera, display),
            compiled_terrain_snapshot: session.compiled_terrain_snapshot_render_report(),
            player_support_observation: session.player_support_observation_render_report(),
        }
    }
}

impl RenderedView {
    pub(in crate::mcp) fn into_tool_result(
        self,
        camera: Camera,
        size: (u32, u32),
        view: ViewOptions,
        mut fog_preview: Value,
        include_details: bool,
    ) -> ToolResult {
        let data = base64::engine::general_purpose::STANDARD.encode(self.png);
        let FrameMetadata {
            scene_display,
            mut lighting_preview,
            selection_presentation,
            helper_presentation,
            mut color_correction,
            skybox,
            working_region,
            rope_preview,
            material_reload,
            compiled_details,
            baked_surfaces,
            compiled_terrain_snapshot,
            player_support_observation,
        } = self.metadata;
        color_correction["applied"] =
            json!(color_correction["enabled"] == true && color_correction["ready"] == true);
        let color_applied = render::fog::for_color(view.fog, &camera, view.surface)
            .is_some_and(|fog| fog.enabled() && fog.max_density() > 0.0);
        let far_z = render::fog::far_z(view.fog, &camera);
        fog_preview["applied"] = json!(color_applied);
        fog_preview["color_applied"] = json!(color_applied);
        fog_preview["far_clip_applied"] = json!(far_z.is_some());
        fog_preview["far_z"] = json!(far_z);
        let preview_issues = self.issues.entries.iter().map(|issue| json!({"brush":issue.solid(),"face":issue.face().map(|face|json!({"solid":face.solid,"side":face.side})),"message":issue.message()})).collect::<Vec<_>>();
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
        lighting_preview["report"] = json!(self.lighting);
        let mut info = json!({ "scene_display":scene_display,"selection_presentation":selection_presentation,"helper_presentation":helper_presentation,"baked_surfaces":baked_surfaces,"compiled_details":compiled_details,"compiled_terrain_snapshot":compiled_terrain_snapshot,"player_support_observation":player_support_observation, "color_correction":color_correction, "working_region":working_region, "camera": camera_json(&camera), "width": size.0, "height": size.1, "skybox":skybox, "sky3d_preview":self.sky3d_preview, "fog_preview":fog_preview, "rope_preview":rope_preview, "sprite_guides":self.sprite_guides, "instance_contents":self.instance_contents, "preview_issues":preview_issues, "preview_issues_omitted": self.issues.omitted, "normal_maps":{"report":self.normal_maps,"limits":render::normal_maps::limits()}, "lighting_preview":lighting_preview, "material_reload":material_reload, "view": rendered_view });
        if !include_details {
            omit_render_catalogues(&mut info);
        }
        Ok(CallToolResult::success(vec![
            ContentBlock::image(data, "image/png"),
            ContentBlock::text(info.to_string()),
        ]))
    }
}

fn omit_render_catalogues(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for key in ["limits", "policy", "policies"] {
                fields.remove(key);
            }
            for value in fields.values_mut() {
                omit_render_catalogues(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                omit_render_catalogues(value);
            }
        }
        _ => {}
    }
}

impl EditorServer {
    pub(in crate::mcp) fn render_png(
        &self,
        camera: &Camera,
        size: (u32, u32),
        view: ViewOptions,
        display: render::ResolvedSceneDisplay,
        display_guard: &crate::session::SceneDisplayRenderGuard,
    ) -> Result<RenderedView, String> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.try_render_png(camera, size, view, display, display_guard)
        })) {
            Ok(result) => result,
            Err(payload) => {
                let detail = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("graphics backend panic");
                let error = format!("Rendering failed: {detail}");
                let mut state = self
                    .offscreen
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                state.renderer = None;
                state.error = Some(error.clone());
                Err(error)
            }
        }
    }
}

impl EditorServer {
    pub(in crate::mcp) fn rendering_status(&self) -> Value {
        let state = self
            .offscreen
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        json!({"initialized": state.renderer.is_some(), "adapter": state.renderer.as_ref().map(|renderer| renderer.ctx.adapter.get_info().name), "error": state.error})
    }
}

impl EditorServer {
    pub(in crate::mcp) fn try_render_png(
        &self,
        camera: &Camera,
        size: (u32, u32),
        view: ViewOptions,
        display: render::ResolvedSceneDisplay,
        display_guard: &crate::session::SceneDisplayRenderGuard,
    ) -> Result<RenderedView, String> {
        {
            let mut session = self.session();
            session.preflight_carve_preview();
            session.verify_carve_preview_render()?;
            session.preflight_terrain_removal_preview();
            session.verify_terrain_removal_render()?;
            session.preflight_face_terrain_context();
            session.verify_face_terrain_context_render()?;
        }
        self.session().color_correction_for_render()?;
        self.session().verify_scene_display_render(display_guard)?;
        self.session()
            .verify_reference_images_for_render(camera.projection)?;
        let mut offscreen = self
            .offscreen
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if offscreen.renderer.is_none() {
            let ctx = match GpuContext::new() {
                Ok(ctx) => ctx,
                Err(error) => {
                    let error = format!(
                        "Rendering unavailable: {error}. Map editing and compilation remain available."
                    );
                    offscreen.error = Some(error.clone());
                    return Err(error);
                }
            };
            offscreen.renderer = Some(Offscreen::new(ctx));
            offscreen.error = None;
        }
        let Offscreen {
            ctx,
            renderer,
            scene,
            compiled_terrain_snapshot_guard,
            player_support_observation_guard,
            issues,
            normal_maps,
            lighting,
            sprite_guides,
            instance_contents,
            sky3d_preview,
        } = offscreen
            .renderer
            .as_mut()
            .expect("renderer was initialized");
        let (
            metadata,
            color_correction,
            baked_surfaces_guard,
            reference_images_guard,
            instance_contents_guard,
        ) = {
            let mut session = self.session();
            let color_correction = session.color_correction_for_render()?;
            renderer.set_color_correction(color_correction.clone());
            session.preflight_carve_preview();
            session.verify_carve_preview_render()?;
            session.preflight_terrain_removal_preview();
            session.verify_terrain_removal_render()?;
            session.preflight_face_terrain_context();
            session.verify_face_terrain_context_render()?;
            session.preflight_instance_contents();
            session.preflight_compiled_visibility();
            session.preflight_compiled_details();
            session.preflight_baked_surfaces();
            session.preflight_compiled_terrain_snapshot();
            session.preflight_player_support_observation();
            session.preflight_compiled_prop_leaf_bounds();
            session.verify_scene_display_render(display_guard)?;
            *sky3d_preview = session.sky3d_preview_state_for(view, *camera);
            let key = session.render_scene_key(view);
            if scene.as_ref().is_none_or(|(previous, _)| *previous != key) {
                let prepared_guard = session.compiled_terrain_snapshot_render_guard(view)?;
                let player_guard = session.player_support_observation_render_guard(view)?;
                let mesh = session.build_scene_for_view(view);
                session.verify_compiled_terrain_snapshot_render(prepared_guard.as_ref(), view)?;
                session.verify_player_support_observation_render(player_guard.as_ref(), view)?;
                let uploaded = if mesh.baked_faces.is_some() {
                    renderer.upload(ctx, &mesh, session.materials())?
                } else {
                    renderer.upload_replacing(
                        ctx,
                        scene.take().map(|(_, scene)| scene),
                        &mesh,
                        session.materials(),
                    )?
                };
                *issues = mesh.issues.clone();
                *normal_maps = mesh.normal_maps.clone();
                *lighting = mesh.lighting.report.clone();
                *sprite_guides = session.sprite_guide_preview_state(view);
                *instance_contents = session.instance_contents_rendered_report();
                *scene = Some((session.render_scene_key(view), uploaded));
                *compiled_terrain_snapshot_guard = prepared_guard;
                *player_support_observation_guard = player_guard;
            }
            let retained_guard = compiled_terrain_snapshot_guard
                .as_ref()
                .ok_or("Cached native terrain guide custody is unavailable; render again")?;
            session.verify_compiled_terrain_snapshot_render(Some(retained_guard), view)?;
            session.verify_player_support_observation_render(
                player_support_observation_guard.as_ref(),
                view,
            )?;
            let instance_contents_guard =
                session.instance_contents_render_guard(view.instance_contents);
            (
                FrameMetadata::capture(&mut session, view, *camera, display, lighting),
                color_correction,
                session.baked_surfaces_render_guard(view)?,
                session.reference_images_render_guard(camera.projection)?,
                instance_contents_guard,
            )
        };
        self.session()
            .verify_instance_contents_render(&instance_contents_guard)?;
        self.session().verify_compiled_terrain_snapshot_render(
            compiled_terrain_snapshot_guard.as_ref(),
            view,
        )?;
        self.session().verify_player_support_observation_render(
            player_support_observation_guard.as_ref(),
            view,
        )?;
        let gpu_scene = &scene.as_ref().expect("scene was just built").1;
        let pixels = renderer.render_to_rgba_checked_resolved(
            ctx, gpu_scene, camera, &view, size.0, size.1, display,
        )?;
        self.session().verify_scene_display_render(display_guard)?;
        self.session().verify_carve_preview_render()?;
        self.session().verify_terrain_removal_render()?;
        self.session().verify_face_terrain_context_render()?;
        self.session()
            .verify_color_correction_render(color_correction.as_ref())?;
        self.session()
            .verify_reference_images_render(&reference_images_guard)?;
        self.session()
            .verify_baked_surfaces_render(baked_surfaces_guard.as_ref(), view)?;
        self.session().verify_compiled_terrain_snapshot_render(
            compiled_terrain_snapshot_guard.as_ref(),
            view,
        )?;
        self.session().verify_player_support_observation_render(
            player_support_observation_guard.as_ref(),
            view,
        )?;
        self.session()
            .verify_instance_contents_render(&instance_contents_guard)?;
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(Cursor::new(&mut png), size.0, size.1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .and_then(|mut w| w.write_image_data(&pixels))
            .map_err(|error| format!("Could not encode render: {error}"))?;
        Ok(RenderedView {
            metadata,
            instance_contents: instance_contents.clone(),
            sky3d_preview: sky3d_preview.clone(),
            normal_maps: normal_maps.clone(),
            lighting: lighting.clone(),
            sprite_guides: sprite_guides.clone(),
            png,
            issues: issues.clone(),
        })
    }
}
