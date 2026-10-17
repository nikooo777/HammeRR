use super::*;

pub(in crate::mcp) struct Offscreen {
    pub(in crate::mcp) ctx: GpuContext,
    pub(in crate::mcp) renderer: Renderer,
    pub(in crate::mcp) scene: Option<(render::SceneKey, GpuScene)>,
    pub(in crate::mcp) issues: Vec<render::scene::SceneIssue>,
    pub(in crate::mcp) normal_maps: render::normal_maps::Report,
    pub(in crate::mcp) lighting: render::lighting::Report,
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
        }
    }
}

#[derive(Default)]
pub(in crate::mcp) struct OffscreenState {
    pub(in crate::mcp) renderer: Option<Offscreen>,
    pub(in crate::mcp) error: Option<String>,
}

pub(in crate::mcp) struct RenderedView {
    pub(in crate::mcp) normal_maps: render::normal_maps::Report,
    pub(in crate::mcp) lighting: render::lighting::Report,
    pub(in crate::mcp) png: Vec<u8>,
    pub(in crate::mcp) issues: Vec<render::scene::SceneIssue>,
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
        } = offscreen
            .renderer
            .as_mut()
            .expect("renderer was initialized");
        {
            let mut session = self.session();
            let key = view.scene_key(session.scene_serial());
            if scene.as_ref().is_none_or(|(previous, _)| *previous != key) {
                let mesh = session.build_scene_for_view(view);
                *issues = mesh.issues.clone();
                *normal_maps = mesh.normal_maps.clone();
                *lighting = mesh.lighting.report.clone();
                *scene = Some((key, renderer.upload(ctx, &mesh, session.materials())));
            }
        }
        let gpu_scene = &scene.as_ref().expect("scene was just built").1;
        let pixels = renderer.render_to_rgba(ctx, gpu_scene, camera, &view, size.0, size.1);
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(Cursor::new(&mut png), size.0, size.1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .and_then(|mut w| w.write_image_data(&pixels))
            .map_err(|error| format!("Could not encode render: {error}"))?;
        Ok(RenderedView {
            normal_maps: normal_maps.clone(),
            lighting: lighting.clone(),
            png,
            issues: issues.clone(),
        })
    }
}
