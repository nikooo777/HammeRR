use render::{Camera, ResolvedSceneDisplay, SceneDisplay, ViewOptions};
use serde_json::{Value, json};

use super::Session;

pub struct SceneDisplayRenderGuard {
    shared_view: ViewOptions,
    key: render::SceneKey,
    view: ViewOptions,
    camera: Camera,
    resolved: ResolvedSceneDisplay,
}

fn report(
    view: ViewOptions,
    camera: &Camera,
    resolved: Result<ResolvedSceneDisplay, String>,
) -> Value {
    let active = view.scene_display.active(camera, view.surface);
    let display = resolved.as_ref().ok();
    json!({
        "requested_mode":view.scene_display.name(),
        "effective_mode":display.map(|value|if value.active {"hdr"} else {"legacy"}),
        "active":display.is_some_and(|value|value.active),
        "requested_active":active,
        "requested_exposure_ev":view.scene_display.exposure_ev(),
        "effective_exposure_ev":display.and_then(|value|value.exposure_ev),
        "exposure_owner":display.map(|value|value.exposure_owner()),
        "display_curve":display.filter(|value|value.active).map(|_|render::scene_display::HDR_DISPLAY_CURVE),
        "ready":resolved.is_ok(),
        "error":resolved.err(),
        "camera":super::view_layout::camera_report(camera),
        "surface":view.surface.name(),
        "preview_only":true
    })
}

impl Session {
    pub fn scene_display_options(
        &self,
        current: SceneDisplay,
        requested_mode: Option<&str>,
        exposure_ev: Option<f64>,
    ) -> Result<SceneDisplay, String> {
        if exposure_ev.is_some_and(|value| !value.is_finite() || !(-8.0..=8.0).contains(&value)) {
            return Err("Scene exposure must be finite and between -8 and 8 EV".into());
        }
        let display = match requested_mode.unwrap_or(current.name()) {
            "legacy" => {
                if exposure_ev.is_some() {
                    return Err("Scene exposure EV requires HDR Scene display".into());
                }
                SceneDisplay::Legacy
            }
            "hdr" => SceneDisplay::Hdr {
                exposure_ev: exposure_ev
                    .map(|value| value as f32)
                    .or_else(|| current.exposure_ev())
                    .ok_or(
                        "Switching Scene display to HDR requires an explicit Scene exposure EV",
                    )?,
            },
            _ => return Err("Scene display mode must be legacy or hdr".into()),
        };
        display.validate()?;
        Ok(display)
    }

    pub fn set_scene_display(
        &mut self,
        requested_mode: Option<&str>,
        exposure_ev: Option<f64>,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        let mut view = self.view();
        view.scene_display =
            self.scene_display_options(view.scene_display, requested_mode, exposure_ev)?;
        self.set_view(view)?;
        Ok(self.scene_display_state())
    }

    pub fn resolve_scene_display_for_render(
        &self,
        view: ViewOptions,
        camera: &Camera,
    ) -> Result<ResolvedSceneDisplay, String> {
        let resolved = view.scene_display.resolve(camera, view.surface)?;
        if resolved.active {
            self.validate_compiled_details_hdr()?;
        }
        Ok(resolved)
    }

    pub fn validate_legacy_exposure_change(
        &self,
        current: f64,
        requested: f64,
        canonical_reset: bool,
    ) -> Result<(), String> {
        self.validate_legacy_exposure_change_for_render(
            self.view(),
            &self.camera(),
            current,
            requested,
            canonical_reset,
        )
    }

    pub fn validate_legacy_exposure_change_for_render(
        &self,
        view: ViewOptions,
        camera: &Camera,
        current: f64,
        requested: f64,
        canonical_reset: bool,
    ) -> Result<(), String> {
        if current == requested || (canonical_reset && requested == 0.0) {
            return Ok(());
        }
        if self
            .resolve_scene_display_for_render(view, camera)
            .is_ok_and(|display| display.active)
        {
            return Err("This pane uses HDR Scene exposure EV; set Scene exposure EV instead. Stored component exposures remain available in HDR-inactive panes".into());
        }
        Ok(())
    }

    pub(super) fn validate_view_update(
        &self,
        view: ViewOptions,
        camera: &Camera,
    ) -> Result<(), String> {
        view.scene_display.validate()?;
        for value in [view.lighting_exposure_ev, view.sky_exposure_ev] {
            if !value.is_finite() || !(-8.0..=8.0).contains(&value) {
                return Err("Component exposure must be finite and between -8 and 8 EV".into());
            }
        }
        self.validate_legacy_exposure_change_for_render(
            view,
            camera,
            f64::from(self.view.lighting_exposure_ev),
            f64::from(view.lighting_exposure_ev),
            false,
        )?;
        self.validate_legacy_exposure_change_for_render(
            view,
            camera,
            f64::from(self.view.sky_exposure_ev),
            f64::from(view.sky_exposure_ev),
            false,
        )?;
        Ok(())
    }

    pub fn scene_display_report_for_render(&self, view: ViewOptions, camera: &Camera) -> Value {
        report(
            view,
            camera,
            self.resolve_scene_display_for_render(view, camera),
        )
    }

    pub fn scene_display_report_for_resolved(
        &self,
        view: ViewOptions,
        camera: &Camera,
        display: ResolvedSceneDisplay,
    ) -> Value {
        report(view, camera, Ok(display))
    }

    pub fn scene_display_state(&self) -> Value {
        let view = self.view();
        let mut report = self.scene_display_report_for_render(view, &self.camera());
        report["active_pane"] = json!(self.active_view_pane());
        report["panes"] = json!(
            self.visible_view_panes()
                .into_iter()
                .map(|pane| {
                    let mut report = self.scene_display_report_for_render(view, &pane.camera);
                    report["index"] = json!(pane.index);
                    report["active_pane"] = json!(pane.index == self.active_view_pane());
                    report
                })
                .collect::<Vec<_>>()
        );
        report
    }

    pub fn scene_display_render_guard(
        &self,
        view: ViewOptions,
        camera: Camera,
        resolved: ResolvedSceneDisplay,
    ) -> SceneDisplayRenderGuard {
        SceneDisplayRenderGuard {
            shared_view: self.view,
            key: self.render_scene_key(view),
            view,
            camera,
            resolved,
        }
    }

    pub fn verify_scene_display_render(
        &self,
        guard: &SceneDisplayRenderGuard,
    ) -> Result<(), String> {
        self.ensure_not_closing()?;
        let mut key = self.render_scene_key(guard.view);
        key.rope_preview_serial = guard.key.rope_preview_serial;
        if self.view != guard.shared_view
            || key != guard.key
            || self.resolve_scene_display_for_render(guard.view, &guard.camera)? != guard.resolved
        {
            return Err(
                "Scene display or rendered source changed during the frame; render again".into(),
            );
        }
        Ok(())
    }
}
