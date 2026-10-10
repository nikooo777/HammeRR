//! The 3D viewport: fly-camera input and rendering into textures that Slint
//! composites.

use std::collections::HashSet;

use glam::Vec3;
use render::{Camera, GpuContext, GpuScene, RenderTarget, Renderer, SceneMesh, ViewOptions};
use slint::SharedString;
use slint::platform::Key;

const LOOK_DEGREES_PER_PIXEL: f32 = 0.2;
const DEFAULT_SPEED: f32 = 600.0;
const FAST_MULTIPLIER: f32 = 4.0;
const DOLLY_UNITS_PER_PIXEL: f32 = 2.0;

pub struct Viewport {
    renderer: Renderer,
    scene: Option<GpuScene>,
    pub camera: Camera,
    pub options: ViewOptions,
    /// Alternated each frame: handing Slint a different texture than last
    /// time is what makes it repaint.
    targets: [Option<RenderTarget>; 2],
    next_target: usize,
    held: HashSet<String>,
    looking: bool,
    last_mouse: Option<(f32, f32)>,
    pub speed: f32,
    dirty: bool,
}

impl Viewport {
    pub fn new(ctx: &GpuContext) -> Self {
        Self {
            renderer: Renderer::new(ctx),
            scene: None,
            camera: Camera::looking_at(Vec3::new(-512.0, -512.0, 512.0), Vec3::ZERO),
            options: ViewOptions::default(),
            targets: [None, None],
            next_target: 0,
            held: HashSet::new(),
            looking: false,
            last_mouse: None,
            speed: DEFAULT_SPEED,
            dirty: true,
        }
    }

    pub fn set_scene(&mut self, ctx: &GpuContext, mesh: &SceneMesh, camera: Camera) {
        self.scene = Some(self.renderer.upload(ctx, mesh));
        self.camera = camera;
        self.dirty = true;
    }

    pub fn redraw(&mut self) {
        self.dirty = true;
    }

    pub fn looking(&self) -> bool {
        self.looking
    }

    /// Returns whether the key is a camera control.
    pub fn key(&mut self, text: &str, down: bool) -> bool {
        let key = normalize_key(text);
        let handled = matches!(key.as_str(), "w" | "a" | "s" | "d" | "q" | "e") || key == shift();
        if handled {
            if down {
                self.held.insert(key);
            } else {
                self.held.remove(&key);
            }
        }
        handled
    }

    pub fn mouse_button(&mut self, button: i32, down: bool, x: f32, y: f32) {
        if button == 1 {
            self.looking = down;
            self.last_mouse = down.then_some((x, y));
        }
    }

    pub fn mouse_moved(&mut self, x: f32, y: f32) {
        if !self.looking {
            return;
        }
        if let Some((lx, ly)) = self.last_mouse {
            let (dx, dy) = (x - lx, y - ly);
            if dx != 0.0 || dy != 0.0 {
                self.camera
                    .look(-dx * LOOK_DEGREES_PER_PIXEL, -dy * LOOK_DEGREES_PER_PIXEL);
                self.dirty = true;
            }
        }
        self.last_mouse = Some((x, y));
    }

    /// Wheel dollies the camera, or changes fly speed while looking.
    pub fn wheel(&mut self, delta: f32) {
        if self.looking {
            self.speed = (self.speed * 2f32.powf(delta / 240.0)).clamp(25.0, 20_000.0);
        } else {
            self.camera.fly(Vec3::new(delta * DOLLY_UNITS_PER_PIXEL, 0.0, 0.0));
        }
        self.dirty = true;
    }

    /// Clears held keys, e.g. when the viewport loses focus, so the camera
    /// doesn't keep flying.
    pub fn release_all(&mut self) {
        self.held.clear();
        self.looking = false;
        self.last_mouse = None;
    }

    /// Advances the camera by `dt` seconds of held movement keys.
    pub fn update(&mut self, dt: f32) {
        let axis = |pos: &str, neg: &str| {
            f32::from(u8::from(self.held.contains(pos))) - f32::from(u8::from(self.held.contains(neg)))
        };
        let dir = Vec3::new(axis("w", "s"), axis("d", "a"), axis("e", "q"));
        if dir == Vec3::ZERO {
            return;
        }
        let fast = if self.held.contains(&shift()) { FAST_MULTIPLIER } else { 1.0 };
        self.camera.fly(dir.normalize() * self.speed * fast * dt);
        self.dirty = true;
    }

    /// Renders a new frame if anything changed, at `size` physical pixels.
    pub fn render(&mut self, ctx: &GpuContext, size: (u32, u32)) -> Option<slint::Image> {
        let size = (size.0.max(1), size.1.max(1));
        let resized = self.targets[0].as_ref().is_none_or(|t| t.size() != size);
        if !self.dirty && !resized {
            return None;
        }
        let scene = self.scene.as_ref()?;
        if resized {
            self.targets = [
                Some(RenderTarget::new(ctx, size.0, size.1)),
                Some(RenderTarget::new(ctx, size.0, size.1)),
            ];
        }
        let target = self.targets[self.next_target].as_ref()?;
        self.renderer
            .render(ctx, scene, &self.camera, &self.options, &target.view, size);
        self.next_target ^= 1;
        self.dirty = false;
        slint::Image::try_from(target.texture.clone()).ok()
    }
}

fn shift() -> String {
    SharedString::from(Key::Shift).to_string()
}

fn normalize_key(text: &str) -> String {
    text.to_lowercase()
}
