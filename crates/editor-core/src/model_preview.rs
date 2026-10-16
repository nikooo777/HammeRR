use render::{Camera, GpuContext, MaterialLibrary, Renderer, SceneMesh, ViewOptions};

use crate::session::Session;

#[derive(Default)]
pub struct PreviewRenderer {
    gpu: Option<(GpuContext, Renderer)>,
}

impl PreviewRenderer {
    pub fn initialized(&self) -> bool {
        self.gpu.is_some()
    }

    pub fn render(
        &mut self,
        session: &Session,
        path: &str,
        skin: usize,
        body: u32,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        if !(64..=1024).contains(&width) || !(64..=1024).contains(&height) {
            return Err("Model preview dimensions must be between 64 and 1024 pixels".into());
        }
        let (mesh, materials, camera) =
            session.model_preview_scene(path, skin, body, width as f32 / height as f32)?;
        self.render_scene(
            &mesh,
            &materials,
            &camera,
            &ViewOptions {
                grid_visible: false,
                ..ViewOptions::default()
            },
            width,
            height,
        )
    }

    pub fn render_scene(
        &mut self,
        mesh: &SceneMesh,
        materials: &MaterialLibrary,
        camera: &Camera,
        view: &ViewOptions,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        if !(64..=1024).contains(&width) || !(64..=1024).contains(&height) {
            return Err("Preview dimensions must be between 64 and 1024 pixels".into());
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if self.gpu.is_none() {
                let context = GpuContext::new().map_err(|error| {
                    format!(
                        "Preview rendering unavailable: {error}; CPU inspection remains available"
                    )
                })?;
                let renderer = Renderer::new(&context);
                self.gpu = Some((context, renderer));
            }
            let (context, renderer) = self.gpu.as_mut().unwrap();
            renderer.clear_materials();
            let scene = renderer.upload(context, mesh, materials);
            Ok(renderer.render_to_rgba(context, &scene, camera, view, width, height))
        }))
        .unwrap_or_else(|_| {
            self.gpu = None;
            Err("Preview rendering failed; CPU inspection remains available".into())
        })
    }
}
