//! The 3D viewport: fly-camera input and rendering into textures that Slint
//! composites.

use std::collections::HashSet;

use glam::Vec3;
use render::{
    Camera, GpuContext, GpuScene, MaterialLibrary, Projection, RenderTarget, Renderer, SceneMesh,
    ViewOptions,
};
use slint::SharedString;
use slint::platform::Key;

const LOOK_DEGREES_PER_PIXEL: f32 = 0.2;
const DOLLY_UNITS_PER_PIXEL: f32 = 2.0;
/// A left press and release closer than this (logical pixels) is a click.
pub(crate) const CLICK_SLOP: f32 = 4.0;

/// A left click in the viewport, in normalized device coordinates.
pub struct Click {
    pub ndc: (f32, f32),
    pub aspect: f32,
    pub control: bool,
}

pub struct Viewport {
    renderer: Renderer,
    scene: Option<GpuScene>,
    pub camera: Camera,
    pub options: ViewOptions,
    /// Alternated each frame: handing Slint a different texture than last
    /// time is what makes it repaint.
    targets: [Option<RenderTarget>; 2],
    next_target: usize,
    pixel_ratio: f32,
    held: HashSet<String>,
    looking: bool,
    last_mouse: Option<(f32, f32)>,
    left_press: Option<(f32, f32)>,
    dirty: bool,
    scene_error: Option<String>,
    baked_surfaces_error: Option<String>,
    color_correction_error: Option<String>,
}

impl Viewport {
    pub fn new(ctx: &GpuContext, camera: Camera) -> Self {
        Self {
            renderer: Renderer::new(ctx),
            scene: None,
            camera,
            options: ViewOptions::default(),
            targets: [None, None],
            next_target: 0,
            pixel_ratio: 1.0,
            held: HashSet::new(),
            looking: false,
            last_mouse: None,
            left_press: None,
            dirty: true,
            scene_error: None,
            baked_surfaces_error: None,
            color_correction_error: None,
        }
    }

    pub fn set_scene(
        &mut self,
        ctx: &GpuContext,
        mesh: &SceneMesh,
        library: &MaterialLibrary,
    ) -> Result<(), String> {
        self.baked_surfaces_error = None;
        let uploaded = if mesh.baked_faces.is_some() {
            self.renderer.upload(ctx, mesh, library)
        } else {
            self.renderer
                .upload_replacing(ctx, self.scene.take(), mesh, library)
        };
        let scene = match uploaded {
            Ok(scene) => scene,
            Err(error) => {
                self.scene_error = Some(error.clone());
                return Err(error);
            }
        };
        self.scene = Some(scene);
        self.scene_error = None;
        self.dirty = true;
        Ok(())
    }

    pub fn redraw(&mut self) {
        self.dirty = true;
    }

    pub fn sky3d_error(&self) -> Option<&str> {
        self.renderer.last_sky3d_error()
    }

    pub fn color_correction_error(&self) -> Option<&str> {
        self.color_correction_error.as_deref()
    }

    pub fn baked_surfaces_error(&self) -> Option<&str> {
        self.baked_surfaces_error.as_deref()
    }

    pub fn looking(&self) -> bool {
        self.looking
    }

    /// Returns whether the key is a camera control.
    pub fn key(&mut self, text: &str, down: bool) -> bool {
        let key = text.to_lowercase();
        let handled = matches!(key.as_str(), "w" | "a" | "s" | "d")
            || (self.camera.projection == Projection::Perspective
                && matches!(key.as_str(), "q" | "e"))
            || key == key_text(Key::Shift);
        if handled {
            if down {
                self.held.insert(key);
            } else {
                self.held.remove(&key);
            }
        }
        handled
    }

    /// Returns a click when the left button is released close to where it
    /// was pressed.
    pub fn mouse_button(
        &mut self,
        button: i32,
        down: bool,
        (x, y): (f32, f32),
        viewport: (f32, f32),
        control: bool,
    ) -> Option<Click> {
        match (button, down) {
            (1, _) => {
                self.looking = down;
                self.last_mouse = down.then_some((x, y));
                None
            }
            (0, true) => {
                self.left_press = Some((x, y));
                None
            }
            (0, false) => {
                let (px, py) = self.left_press.take()?;
                if (x - px).hypot(y - py) > CLICK_SLOP || viewport.0 <= 0.0 || viewport.1 <= 0.0 {
                    return None;
                }
                Some(Click {
                    ndc: (2.0 * x / viewport.0 - 1.0, 1.0 - 2.0 * y / viewport.1),
                    aspect: viewport.0 / viewport.1,
                    control,
                })
            }
            _ => None,
        }
    }

    pub fn mouse_moved(&mut self, x: f32, y: f32, height: f32) {
        if !self.looking {
            return;
        }
        if let Some((lx, ly)) = self.last_mouse {
            let (dx, dy) = (x - lx, y - ly);
            if dx != 0.0 || dy != 0.0 {
                if self.camera.projection == Projection::Perspective {
                    self.camera
                        .look(-dx * LOOK_DEGREES_PER_PIXEL, -dy * LOOK_DEGREES_PER_PIXEL);
                } else {
                    let units = self.camera.ortho_height / height.max(1.0);
                    self.camera.position +=
                        (-self.camera.right() * dx + self.camera.up() * dy) * units;
                }
                self.dirty = true;
            }
        }
        self.last_mouse = Some((x, y));
    }

    /// Wheel dollies the perspective camera or zooms a planar view.
    pub fn wheel(&mut self, delta: f32, ndc: (f32, f32), aspect: f32) {
        if self.camera.projection != Projection::Perspective {
            let before = self.camera.plane_point(ndc, aspect, 0.0).unwrap();
            self.camera.ortho_height = (self.camera.ortho_height * 2f32.powf(-delta / 480.0))
                .clamp(Camera::MIN_ORTHO_HEIGHT, Camera::MAX_ORTHO_HEIGHT);
            let after = self.camera.plane_point(ndc, aspect, 0.0).unwrap();
            self.camera.position += before - after;
        } else {
            self.camera
                .fly(Vec3::new(delta * DOLLY_UNITS_PER_PIXEL, 0.0, 0.0));
        }
        self.dirty = true;
    }

    /// Clears held keys, e.g. when the viewport loses focus, so the camera
    /// doesn't keep flying.
    pub fn release_all(&mut self) {
        self.held.clear();
        self.looking = false;
        self.last_mouse = None;
        self.left_press = None;
    }

    /// Advances the camera by `dt` seconds of held movement keys. Returns
    /// whether it moved.
    pub fn update(&mut self, dt: f32, fly_speed: f32) -> bool {
        let axis = |pos: &str, neg: &str| {
            f32::from(u8::from(self.held.contains(pos)))
                - f32::from(u8::from(self.held.contains(neg)))
        };
        let dir = Vec3::new(axis("w", "s"), axis("d", "a"), axis("e", "q"));
        if dir == Vec3::ZERO {
            return false;
        }
        let fast = if self.held.contains(&key_text(Key::Shift)) {
            editor_core::navigation_preferences::FAST_MULTIPLIER
        } else {
            1.0
        };
        let amount = dir.normalize() * fly_speed * fast * dt;
        if self.camera.projection == Projection::Perspective {
            self.camera.fly(amount);
        } else {
            self.camera.position += self.camera.up() * amount.x + self.camera.right() * amount.y;
        }
        self.dirty = true;
        true
    }

    /// Renders a new frame if anything changed, at `size` physical pixels.
    pub fn render(
        &mut self,
        ctx: &GpuContext,
        size: (u32, u32),
        pixel_ratio: f32,
        session: &mut editor_core::session::Session,
    ) -> Option<slint::Image> {
        if self.scene_error.is_some() {
            return None;
        }
        let size = (size.0.max(1), size.1.max(1));
        let resized = self.targets[0].as_ref().is_none_or(|t| t.size() != size);
        if !self.dirty && !resized && self.pixel_ratio == pixel_ratio {
            return None;
        }
        let scene = self.scene.as_ref()?;
        self.baked_surfaces_error = None;
        let guard = match session.baked_surfaces_render_guard(self.options) {
            Ok(guard) => guard,
            Err(error) => {
                self.baked_surfaces_error = Some(error);
                return None;
            }
        };
        self.color_correction_error = None;
        let preview = match session.color_correction_for_render() {
            Ok(preview) => preview,
            Err(error) => {
                self.color_correction_error = Some(error);
                None
            }
        };
        self.renderer.set_color_correction(preview.clone());
        if resized {
            self.targets = [
                Some(RenderTarget::new(ctx, size.0, size.1)),
                Some(RenderTarget::new(ctx, size.0, size.1)),
            ];
        }
        let target = self.targets[self.next_target].as_ref()?;
        let options = ViewOptions {
            handle_pixel_ratio: pixel_ratio,
            ..self.options
        };
        self.renderer
            .render(ctx, scene, &self.camera, &options, &target.view, size);
        if let Some(error) = self.renderer.last_color_correction_error() {
            self.color_correction_error = Some(error.to_owned());
        }
        if let Err(error) = session.verify_color_correction_render(preview.as_ref()) {
            self.renderer.set_color_correction(None);
            self.renderer
                .render(ctx, scene, &self.camera, &options, &target.view, size);
            self.color_correction_error = Some(error);
        }
        if let Err(error) = session.verify_baked_surfaces_render(guard.as_ref(), self.options) {
            self.baked_surfaces_error = Some(error);
            return None;
        }
        self.next_target ^= 1;
        self.pixel_ratio = pixel_ratio;
        self.dirty = false;
        slint::Image::try_from(target.texture.clone()).ok()
    }
}

pub fn key_text(key: Key) -> String {
    SharedString::from(key).to_string()
}
