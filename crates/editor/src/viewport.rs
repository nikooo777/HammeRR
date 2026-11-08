//! The 3D viewport: fly-camera input and rendering into textures that Slint
//! composites.

use std::collections::HashMap;

use editor_core::session::compiled_terrain_snapshot::RenderGuard as TerrainSnapshotRenderGuard;
use editor_core::session::instance_contents::RenderGuard as InstanceContentsRenderGuard;
use editor_core::session::view_layout::{MAX_CANVAS_PIXELS, MAX_PANES};
use editor_core::shortcuts::Command;
use glam::Vec3;
use render::{
    Camera, GpuContext, GpuScene, MaterialLibrary, Projection, RenderTarget, Renderer,
    ResolvedSceneDisplay, SceneMesh, ViewOptions,
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

pub struct PaneRender {
    pub index: usize,
    pub camera: Camera,
    pub size: (u32, u32),
}

pub struct PaneFrame {
    pub index: usize,
    pub image: slint::Image,
    pub error: Option<String>,
    pub far_z: Option<f32>,
}

struct PaneState {
    targets: [Option<RenderTarget>; 2],
    next_target: usize,
    pixel_ratio: f32,
    camera: Option<Camera>,
    options: Option<ViewOptions>,
    size: Option<(u32, u32)>,
    visible: bool,
    dirty: bool,
    error: Option<String>,
    baked_surfaces_error: Option<String>,
    color_correction_error: Option<String>,
    sky3d_error: Option<String>,
}

impl Default for PaneState {
    fn default() -> Self {
        Self {
            targets: [None, None],
            next_target: 0,
            pixel_ratio: 1.0,
            camera: None,
            options: None,
            size: None,
            visible: false,
            dirty: true,
            error: None,
            baked_surfaces_error: None,
            color_correction_error: None,
            sky3d_error: None,
        }
    }
}

impl PaneState {
    fn needs_render(&self, request: &PaneRender, options: ViewOptions, pixel_ratio: f32) -> bool {
        self.dirty
            || self.targets[0].is_none()
            || self.camera != Some(request.camera)
            || self.options != Some(options)
            || self.size != Some(request.size)
            || self.pixel_ratio != pixel_ratio
    }

    fn observed(&mut self, request: &PaneRender, options: ViewOptions, pixel_ratio: f32) {
        self.camera = Some(request.camera);
        self.options = Some(options);
        self.size = Some(request.size);
        self.pixel_ratio = pixel_ratio;
        self.dirty = false;
    }

    fn message(&self) -> Option<String> {
        if let Some(error) = &self.error {
            return Some(error.clone());
        }
        let errors = [
            self.baked_surfaces_error.as_deref(),
            self.color_correction_error.as_deref(),
            self.sky3d_error.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        (!errors.is_empty()).then(|| errors.join("\n"))
    }
}

fn pane_options(mut options: ViewOptions, active: bool, pixel_ratio: f32) -> ViewOptions {
    options.handle_pixel_ratio = pixel_ratio;
    if !active {
        options.transform_handles = None;
        options.area_preview = None;
        options.paint_preview = None;
    }
    options
}

pub struct Viewport {
    renderer: Renderer,
    scene: Option<GpuScene>,
    instance_contents_guard: Option<InstanceContentsRenderGuard>,
    terrain_snapshot_guard: Option<TerrainSnapshotRenderGuard>,
    pub camera: Camera,
    pub options: ViewOptions,
    panes: [PaneState; MAX_PANES],
    active_pane: usize,
    held: HashMap<String, Command>,
    pub shortcut_input: crate::shortcut_input::State,
    looking: bool,
    last_mouse: Option<(f32, f32)>,
    left_press: Option<(f32, f32)>,
    scene_error: Option<String>,
}

impl Viewport {
    pub fn new(ctx: &GpuContext, camera: Camera) -> Self {
        Self {
            renderer: Renderer::new(ctx),
            scene: None,
            instance_contents_guard: None,
            terrain_snapshot_guard: None,
            camera,
            options: ViewOptions::default(),
            panes: std::array::from_fn(|_| PaneState::default()),
            active_pane: 0,
            held: HashMap::new(),
            shortcut_input: Default::default(),
            looking: false,
            last_mouse: None,
            left_press: None,
            scene_error: None,
        }
    }

    pub fn set_scene(
        &mut self,
        ctx: &GpuContext,
        mesh: &SceneMesh,
        library: &MaterialLibrary,
        instance_contents_guard: InstanceContentsRenderGuard,
        terrain_snapshot_guard: Option<TerrainSnapshotRenderGuard>,
    ) -> Result<(), String> {
        for pane in &mut self.panes {
            pane.baked_surfaces_error = None;
        }
        let uploaded = if mesh.baked_faces.is_some() {
            self.renderer.upload(ctx, mesh, library)
        } else {
            self.renderer
                .upload_replacing(ctx, self.scene.take(), mesh, library)
        };
        let scene = match uploaded {
            Ok(scene) => scene,
            Err(error) => {
                self.instance_contents_guard = None;
                self.terrain_snapshot_guard = None;
                self.scene_error = Some(error.clone());
                self.redraw();
                return Err(error);
            }
        };
        self.scene = Some(scene);
        self.instance_contents_guard = Some(instance_contents_guard);
        self.terrain_snapshot_guard = terrain_snapshot_guard;
        self.scene_error = None;
        self.redraw();
        Ok(())
    }

    pub fn redraw(&mut self) {
        for pane in &mut self.panes {
            pane.dirty = true;
        }
    }

    fn redraw_active(&mut self) {
        self.panes[self.active_pane].dirty = true;
    }

    pub fn set_active_pane(&mut self, index: usize) -> Result<(), String> {
        if index >= MAX_PANES {
            return Err("Viewport pane index is out of range".into());
        }
        if index != self.active_pane {
            self.release_all();
            self.active_pane = index;
            self.redraw();
        }
        Ok(())
    }

    pub fn sky3d_error(&self) -> Option<&str> {
        self.panes[self.active_pane].sky3d_error.as_deref()
    }

    pub fn color_correction_error(&self) -> Option<&str> {
        self.panes[self.active_pane]
            .color_correction_error
            .as_deref()
    }

    pub fn looking(&self) -> bool {
        self.looking
    }

    pub fn hold_camera(&mut self, identity: String, command: Command) {
        self.held.insert(identity, command);
    }

    pub fn release_camera(&mut self, identity: &str) -> bool {
        self.held.remove(identity).is_some()
    }

    pub fn clear_keyboard(&mut self) {
        self.held.clear();
        self.shortcut_input.reset();
    }

    pub fn sync_shortcuts(&mut self, stamp: String) {
        if self.shortcut_input.sync(stamp) {
            self.held.clear();
        }
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
            self.look_by(x - lx, y - ly, height);
        }
        self.last_mouse = Some((x, y));
    }

    /// Looks around in perspective or pans a planar view by a pointer movement in pixels.
    pub fn look_by(&mut self, dx: f32, dy: f32, height: f32) {
        if !self.looking || (dx == 0.0 && dy == 0.0) {
            return;
        }
        if self.camera.projection == Projection::Perspective {
            self.camera
                .look(-dx * LOOK_DEGREES_PER_PIXEL, -dy * LOOK_DEGREES_PER_PIXEL);
        } else {
            let units = self.camera.ortho_height / height.max(1.0);
            self.camera.position += (-self.camera.right() * dx + self.camera.up() * dy) * units;
        }
        self.redraw_active();
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
        self.redraw_active();
    }

    /// Clears held keys, e.g. when the viewport loses focus, so the camera
    /// doesn't keep flying.
    pub fn release_all(&mut self) {
        self.clear_keyboard();
        self.looking = false;
        self.last_mouse = None;
        self.left_press = None;
    }

    /// Advances the camera by `dt` seconds of held movement keys. Returns
    /// whether it moved.
    pub fn update(&mut self, dt: f32, fly_speed: f32, fast: bool) -> Result<bool, String> {
        let axis = |pos: Command, neg: Command| {
            f32::from(u8::from(self.held.values().any(|command| *command == pos)))
                - f32::from(u8::from(self.held.values().any(|command| *command == neg)))
        };
        let direction = [
            axis(Command::CameraForward, Command::CameraBackward),
            axis(Command::CameraRight, Command::CameraLeft),
            axis(Command::CameraUp, Command::CameraDown),
        ];
        let camera = editor_core::session::shortcuts::step_camera_navigation(
            self.camera,
            direction,
            dt,
            fly_speed,
            fast,
        )?;
        if camera == self.camera {
            return Ok(false);
        }
        self.camera = camera;
        self.redraw_active();
        Ok(true)
    }

    fn unavailable(&mut self, index: usize, error: String, baked: bool) -> PaneFrame {
        let pane = &mut self.panes[index];
        pane.targets = [None, None];
        pane.error = Some(error.clone());
        pane.baked_surfaces_error = baked.then(|| error.clone());
        pane.dirty = false;
        PaneFrame {
            index,
            image: slint::Image::default(),
            error: Some(error),
            far_z: None,
        }
    }

    fn check_panes(
        &self,
        panes: &[PaneRender],
        active: usize,
        pixel_ratio: f32,
        canvas: (u32, u32),
        session: &editor_core::session::Session,
    ) -> Result<(), String> {
        if !pixel_ratio.is_finite() || pixel_ratio <= 0.0 {
            return Err("Viewport pixel ratio must be finite and positive".into());
        }
        if active != session.active_view_pane() {
            return Err("Active viewport changed before rendering".into());
        }
        let rectangles = session.view_layout().rectangles(canvas)?;
        if panes.len() != rectangles.len() || panes.len() > MAX_PANES {
            return Err("Visible viewport layout changed before rendering".into());
        }
        let mut seen = [false; MAX_PANES];
        let mut pixels = 0u64;
        for pane in panes {
            if pane.index >= MAX_PANES || seen[pane.index] {
                return Err("Viewport pane indices must be distinct and in range".into());
            }
            seen[pane.index] = true;
            let rectangle = rectangles
                .iter()
                .find(|rect| rect.index == pane.index)
                .ok_or("Viewport pane is not visible in the current layout")?;
            if pane.size != (rectangle.width, rectangle.height) {
                return Err("Viewport pane dimensions changed before rendering".into());
            }
            let area = u64::from(pane.size.0)
                .checked_mul(u64::from(pane.size.1))
                .ok_or("Viewport pane pixel count overflowed")?;
            pixels = pixels
                .checked_add(area)
                .ok_or("Viewport pixel count overflowed")?;
        }
        let canvas_pixels = u64::from(canvas.0)
            .checked_mul(u64::from(canvas.1))
            .ok_or("Viewport canvas pixel count overflowed")?;
        if pixels > MAX_CANVAS_PIXELS || pixels > canvas_pixels {
            return Err(format!(
                "Visible viewports exceed the {MAX_CANVAS_PIXELS}-pixel render budget"
            ));
        }
        if !seen.get(active).copied().unwrap_or(false) {
            return Err("Active viewport must be visible".into());
        }
        Ok(())
    }

    fn draw_pane(
        &mut self,
        ctx: &GpuContext,
        request: &PaneRender,
        options: ViewOptions,
        display: ResolvedSceneDisplay,
        correction_error: Option<&str>,
    ) -> Result<(), String> {
        let pane = &mut self.panes[request.index];
        pane.observed(request, options, options.handle_pixel_ratio);
        pane.error = None;
        pane.baked_surfaces_error = None;
        pane.color_correction_error = None;
        pane.sky3d_error = None;
        if display.active
            && let Some(error) = correction_error
        {
            return Err(format!(
                "HDR scene color correction is unavailable: {error}"
            ));
        }
        let maximum = ctx.device.limits().max_texture_dimension_2d;
        if request.size.0 == 0
            || request.size.1 == 0
            || request.size.0 > maximum
            || request.size.1 > maximum
        {
            return Err(format!(
                "Viewport target requires 1 to {maximum} pixels per axis"
            ));
        }
        request
            .camera
            .validate(request.size.0 as f32 / request.size.1 as f32)?;
        let scene = self.scene.as_ref().ok_or("Viewport scene is unavailable")?;
        if pane.targets[0].is_none() {
            pane.targets = [
                Some(RenderTarget::try_new(ctx, request.size.0, request.size.1)?),
                Some(RenderTarget::try_new(ctx, request.size.0, request.size.1)?),
            ];
        }
        let target = pane.targets[pane.next_target]
            .as_ref()
            .ok_or("Viewport target is unavailable")?;
        self.renderer.render_resolved(
            ctx,
            scene,
            &request.camera,
            &options,
            &target.view,
            request.size,
            display,
        )?;
        pane.sky3d_error = self.renderer.last_sky3d_error().map(str::to_owned);
        let renderer_error = self.renderer.last_color_correction_error();
        pane.color_correction_error = match (correction_error, renderer_error) {
            (Some(source), Some(render)) if source != render => Some(format!("{source}\n{render}")),
            (Some(error), _) | (_, Some(error)) => Some(error.into()),
            (None, None) => None,
        };
        Ok(())
    }

    fn draw_panes(
        &mut self,
        ctx: &GpuContext,
        panes: &[PaneRender],
        drawing: &[bool; MAX_PANES],
        options: &[Option<ViewOptions>; MAX_PANES],
        displays: &[Option<ResolvedSceneDisplay>; MAX_PANES],
        correction_error: Option<&str>,
    ) {
        for request in panes.iter().filter(|request| drawing[request.index]) {
            let Some(options) = options[request.index] else {
                continue;
            };
            let Some(display) = displays[request.index] else {
                continue;
            };
            if let Err(error) = self.draw_pane(ctx, request, options, display, correction_error) {
                self.panes[request.index].error = Some(error);
            }
        }
    }

    fn verify_scene_sources(
        &self,
        session: &mut editor_core::session::Session,
    ) -> Result<(), String> {
        let guard = self
            .instance_contents_guard
            .as_ref()
            .ok_or("Viewport source is unavailable; refresh the scene")?;
        session.verify_instance_contents_render(guard)?;
        session.verify_compiled_terrain_snapshot_render(
            self.terrain_snapshot_guard.as_ref(),
            self.options,
        )
    }

    pub fn render_panes(
        &mut self,
        ctx: &GpuContext,
        panes: &[PaneRender],
        active: usize,
        pixel_ratio: f32,
        canvas: (u32, u32),
        session: &mut editor_core::session::Session,
    ) -> Vec<PaneFrame> {
        let visible = session.visible_view_panes();
        let mut frames = Vec::new();
        for index in 0..MAX_PANES {
            let pane = &mut self.panes[index];
            if !visible.iter().any(|visible| visible.index == index) && pane.visible {
                *pane = PaneState::default();
                frames.push(PaneFrame {
                    index,
                    image: slint::Image::default(),
                    error: None,
                    far_z: None,
                });
            }
        }
        let checked = self
            .set_active_pane(active)
            .and_then(|()| self.check_panes(panes, active, pixel_ratio, canvas, session));
        if let Err(error) = checked {
            frames.extend(
                visible
                    .iter()
                    .map(|pane| self.unavailable(pane.index, error.clone(), false)),
            );
            return frames;
        }
        for request in panes {
            let state = &mut self.panes[request.index];
            state.visible = true;
            if state.targets[0]
                .as_ref()
                .is_some_and(|target| target.size() != request.size)
            {
                state.targets = [None, None];
            }
        }
        if let Some(error) = self.scene_error.clone().or_else(|| {
            self.scene
                .is_none()
                .then(|| "Viewport scene is unavailable".into())
        }) {
            frames.extend(
                panes
                    .iter()
                    .map(|pane| self.unavailable(pane.index, error.clone(), false)),
            );
            return frames;
        }
        if let Err(error) = self.verify_scene_sources(session) {
            frames.extend(
                panes
                    .iter()
                    .map(|pane| self.unavailable(pane.index, error.clone(), false)),
            );
            return frames;
        }
        let mut drawing = [false; MAX_PANES];
        let mut options = [None; MAX_PANES];
        let mut displays = [None; MAX_PANES];
        let mut display_guards = Vec::new();
        for request in panes {
            let mut pane_view = pane_options(self.options, request.index == active, pixel_ratio);
            let display = match session.resolve_scene_display_for_render(pane_view, &request.camera)
            {
                Ok(display) => display,
                Err(error) => {
                    frames.push(self.unavailable(request.index, error, false));
                    continue;
                }
            };
            match session.fog_render_state_for_render(
                pane_view,
                &request.camera,
                Default::default(),
                display,
            ) {
                Ok(fog) => pane_view.fog = fog.fog,
                Err(error) => {
                    frames.push(self.unavailable(request.index, error, false));
                    continue;
                }
            }
            if display.active {
                display_guards.push((
                    request.index,
                    session.scene_display_render_guard(pane_view, request.camera, display),
                ));
            }
            options[request.index] = Some(pane_view);
            displays[request.index] = Some(display);
            drawing[request.index] =
                self.panes[request.index].needs_render(request, pane_view, pixel_ratio);
        }
        if !drawing.iter().any(|drawing| *drawing) {
            return frames;
        }
        let guard = match session.baked_surfaces_render_guard(self.options) {
            Ok(guard) => guard,
            Err(error) => {
                frames.extend(
                    panes
                        .iter()
                        .map(|pane| self.unavailable(pane.index, error.clone(), true)),
                );
                return frames;
            }
        };
        let (preview, correction_error) = match session.color_correction_for_render() {
            Ok(preview) => (preview, None),
            Err(error) => {
                for request in panes {
                    drawing[request.index] = options[request.index].is_some();
                }
                (None, Some(error))
            }
        };
        self.renderer.set_color_correction(preview.clone());
        self.draw_panes(
            ctx,
            panes,
            &drawing,
            &options,
            &displays,
            correction_error.as_deref(),
        );
        if let Err(error) = session.verify_color_correction_render(preview.as_ref()) {
            self.renderer.set_color_correction(None);
            for request in panes {
                drawing[request.index] = options[request.index].is_some();
            }
            self.draw_panes(ctx, panes, &drawing, &options, &displays, Some(&error));
        }
        if let Err(error) = session.verify_baked_surfaces_render(guard.as_ref(), self.options) {
            frames.extend(
                panes
                    .iter()
                    .map(|pane| self.unavailable(pane.index, error.clone(), true)),
            );
            return frames;
        }
        for (index, guard) in &display_guards {
            if drawing[*index]
                && let Err(error) = session.verify_scene_display_render(guard)
            {
                self.panes[*index].error = Some(error);
            }
        }
        if let Err(error) = self.verify_scene_sources(session) {
            frames.extend(
                panes
                    .iter()
                    .map(|pane| self.unavailable(pane.index, error.clone(), false)),
            );
            return frames;
        }
        for request in panes.iter().filter(|request| drawing[request.index]) {
            let pane = &mut self.panes[request.index];
            let image = if let Some(error) = &pane.error {
                Err(error.clone())
            } else {
                pane.targets[pane.next_target]
                    .as_ref()
                    .ok_or_else(|| "Viewport target was retired before display".to_string())
                    .and_then(|target| {
                        slint::Image::try_from(target.texture.clone())
                            .map_err(|error| format!("Could not display viewport texture: {error}"))
                    })
            };
            match image {
                Ok(image) => {
                    pane.next_target ^= 1;
                    frames.push(PaneFrame {
                        index: request.index,
                        image,
                        error: pane.message(),
                        far_z: render::fog::far_z(
                            options[request.index].and_then(|view| view.fog),
                            &request.camera,
                        ),
                    });
                }
                Err(error) => frames.push(self.unavailable(request.index, error, false)),
            }
        }
        frames
    }
}

pub fn key_text(key: Key) -> String {
    SharedString::from(key).to_string()
}
