use super::*;

pub(in crate::mcp) struct Offscreen {
    pub(in crate::mcp) ctx: GpuContext,
    pub(in crate::mcp) renderer: Renderer,
    pub(in crate::mcp) scene: Option<(render::SceneKey, GpuScene)>,
    pub(in crate::mcp) issues: Vec<render::scene::SceneIssue>,
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
            issues: Vec::new(),
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
    pub(in crate::mcp) issues: Vec<render::scene::SceneIssue>,
}

struct FrameMetadata {
    color_correction: Value,
    skybox: Value,
    working_region: Value,
    rope_preview: Value,
    material_reload: Value,
}

impl FrameMetadata {
    fn capture(session: &mut Session, view: ViewOptions, camera: Camera) -> Self {
        Self {
            color_correction: session.color_correction_report(),
            skybox: session.sky_preview_state(view, camera),
            working_region: session.working_region_summary(),
            rope_preview: session.rope_preview_report(view.rope_preview),
            material_reload: json!(session.material_reload_report()),
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
    ) -> ToolResult {
        let data = base64::engine::general_purpose::STANDARD.encode(self.png);
        let FrameMetadata {
            mut color_correction,
            skybox,
            working_region,
            rope_preview,
            material_reload,
        } = self.metadata;
        color_correction["applied"] =
            json!(color_correction["enabled"] == true && color_correction["ready"] == true);
        fog_preview["applied"] = json!(
            view.fog
                .is_some_and(|fog| fog.enabled() && fog.max_density() > 0.0)
                && view.surface == render::SurfaceView::Textured
                && camera.projection == Projection::Perspective
        );
        let preview_issues = self.issues.iter().map(|issue| json!({"face":{"solid":issue.face.solid,"side":issue.face.side},"message":issue.message})).collect::<Vec<_>>();
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
        let info = json!({ "color_correction":color_correction, "working_region":working_region, "camera": camera_json(&camera), "width": size.0, "height": size.1, "skybox":skybox, "sky3d_preview":self.sky3d_preview, "fog_preview":fog_preview, "rope_preview":rope_preview, "sprite_guides":self.sprite_guides, "instance_contents":self.instance_contents, "preview_issues":preview_issues, "normal_maps":{"report":self.normal_maps,"limits":render::normal_maps::limits()}, "lighting_preview":{"report":self.lighting,"limits":render::lighting::limits()}, "material_reload":material_reload, "view": rendered_view });
        Ok(CallToolResult::success(vec![
            ContentBlock::image(data, "image/png"),
            ContentBlock::text(info.to_string()),
        ]))
    }
}

impl EditorServer {
    pub(in crate::mcp) fn render_png(
        &self,
        camera: &Camera,
        size: (u32, u32),
        view: ViewOptions,
    ) -> Result<RenderedView, String> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.try_render_png(camera, size, view)
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
    ) -> Result<RenderedView, String> {
        self.session().color_correction_for_render()?;
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
        let (metadata, color_correction) = {
            let mut session = self.session();
            let color_correction = session.color_correction_for_render()?;
            renderer.set_color_correction(color_correction.clone());
            session.preflight_instance_contents();
            *sky3d_preview = session.sky3d_preview_state_for(view, *camera);
            let key = session.render_scene_key(view);
            if scene.as_ref().is_none_or(|(previous, _)| *previous != key) {
                let mesh = session.build_scene_for_view(view);
                *issues = mesh.issues.clone();
                *normal_maps = mesh.normal_maps.clone();
                *lighting = mesh.lighting.report.clone();
                *sprite_guides = session.sprite_guide_preview_state(view);
                *instance_contents = session.instance_contents_rendered_report();
                let previous = scene.take().map(|(_, scene)| scene);
                *scene = Some((
                    key,
                    renderer.upload_replacing(ctx, previous, &mesh, session.materials()),
                ));
            }
            (
                FrameMetadata::capture(&mut session, view, *camera),
                color_correction,
            )
        };
        let gpu_scene = &scene.as_ref().expect("scene was just built").1;
        let pixels =
            renderer.render_to_rgba_checked(ctx, gpu_scene, camera, &view, size.0, size.1)?;
        self.session()
            .verify_color_correction_render(color_correction.as_ref())?;
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

#[cfg(test)]
#[path = "rendering/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "rendering/color_correction_tests.rs"]
mod color_correction_tests;
