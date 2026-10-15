use render::{GpuContext, Renderer, ViewOptions};

use crate::session::Session;

#[derive(Default)]
pub struct PreviewRenderer {
    gpu: Option<(GpuContext, Renderer)>,
}

impl PreviewRenderer {
    pub fn render(
        &mut self,
        session: &Session,
        path: &str,
        skin: usize,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        if !(64..=1024).contains(&width) || !(64..=1024).contains(&height) {
            return Err("Model preview dimensions must be between 64 and 1024 pixels".into());
        }
        let (mesh, materials, camera) =
            session.model_preview_scene(path, skin, width as f32 / height as f32)?;
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if self.gpu.is_none() {
                let context = GpuContext::new().map_err(|error| error.to_string())?;
                let renderer = Renderer::new(&context);
                self.gpu = Some((context, renderer));
            }
            let (context, renderer) = self.gpu.as_mut().unwrap();
            renderer.clear_materials();
            let scene = renderer.upload(context, &mesh, &materials);
            Ok(renderer.render_to_rgba(
                context,
                &scene,
                &camera,
                &ViewOptions {
                    grid_visible: false,
                    ..ViewOptions::default()
                },
                width,
                height,
            ))
        }))
        .unwrap_or_else(|_| {
            self.gpu = None;
            Err("Model preview rendering failed; CPU inspection remains available".into())
        })
    }
}
