use std::sync::{Arc, Weak};

use glam::{DVec3, Vec3};
use render::{SceneMesh, ViewOptions, scene::LineVertex};
use serde_json::{Value, json};

use super::{Actor, MAX_PLAYER_SUPPORT_SAMPLES, PlayerSupportSample, Ready, Session};

const HULL_COLOR: [u8; 4] = [80, 220, 255, 255];
const TRAJECTORY_COLOR: [u8; 4] = [255, 220, 90, 255];
const PROBE_COLOR: [u8; 4] = [255, 120, 100, 255];
const NORMAL_COLOR: [u8; 4] = [120, 255, 150, 255];
const NORMAL_LENGTH: f32 = 16.0;
const ENDPOINT_HALF_EXTENT: f32 = 2.0;
const MAX_SEGMENTS: usize = MAX_PLAYER_SUPPORT_SAMPLES + 20;
const POLICY: &str = "One selected observed player sample: actual movement-hull bounds at its native origin and the retained sampled-origin trajectory. The optional native diagnostic sweep is drawn from its explicit requested start to requested end; the endpoint marker and available fixed-length normal are anchored at the native returned hull-reference endpoint. That endpoint is not an actual contact point. Display normal normalization does not change retained native values. The guide does not identify an authored face/triangle, prove unsampled motion or replace observed fields with compiler/predictive geometry. Current build/runtime and compatible viewport are required; stale rows remain readable.";

#[derive(Default)]
pub(super) struct State {
    index: Option<usize>,
    visible: bool,
    serial: u64,
    presentation_key: Option<(bool, Option<String>)>,
}

pub struct RenderGuard {
    token: Option<u64>,
    serial: u64,
    ready: Option<Weak<Ready>>,
    displayed: bool,
}

pub(super) fn hull_bounds(sample: &PlayerSupportSample) -> Result<[Vec3; 2], String> {
    let origin = Vec3::from_array(sample.origin);
    let mins = origin + Vec3::from_array(sample.hull_mins);
    let maxs = origin + Vec3::from_array(sample.hull_maxs);
    if !origin.is_finite() || !mins.is_finite() || !maxs.is_finite() || !mins.cmplt(maxs).all() {
        return Err("Player sample has no finite ordered native movement hull".into());
    }
    Ok([mins, maxs])
}

fn normal_direction(normal: [f32; 3]) -> Option<Vec3> {
    let vector = DVec3::from_array(normal.map(f64::from));
    if !vector.is_finite() || vector == DVec3::ZERO {
        return None;
    }
    Some((vector / vector.length()).as_vec3())
}

fn validate_geometry(ready: &Ready, index: usize) -> Result<(), String> {
    hull_bounds(ready.sample(index)?)?;
    if ready.samples.is_empty() || ready.samples.len() > MAX_PLAYER_SUPPORT_SAMPLES {
        return Err("Player trajectory exceeds its retained sample bound".into());
    }
    if ready
        .samples
        .iter()
        .any(|sample| !Vec3::from_array(sample.origin).is_finite())
    {
        return Err("Player trajectory contains a nonfinite native origin".into());
    }
    if let Some(probe) = &ready.sample(index)?.probe {
        for vector in [
            probe.start,
            probe.requested_end,
            probe.native_start,
            probe.native_end,
        ] {
            if !Vec3::from_array(vector).is_finite() {
                return Err("Native diagnostic sweep has no finite display segment".into());
            }
        }
        if probe.plane.available && !Vec3::from_array(probe.plane.normal).is_finite() {
            return Err("Native diagnostic sweep has no finite available plane normal".into());
        }
    }
    Ok(())
}

fn append_segment(mesh: &mut SceneMesh, start: Vec3, end: Vec3, color: [u8; 4]) {
    for position in [start, end] {
        mesh.lines.push(LineVertex {
            position: position.to_array(),
            color,
        });
    }
}

impl Session {
    fn player_support_guide_error_for(&self, ready: &Ready, index: usize) -> Option<String> {
        self.player_support_frame_error(ready)
            .or_else(|| self.player_support_guide_geometry_error_for(ready, index))
    }

    fn player_support_guide_geometry_error_for(
        &self,
        ready: &Ready,
        index: usize,
    ) -> Option<String> {
        validate_geometry(ready, index).err().or_else(|| {
            let sample = ready.sample(index).ok()?;
            let bounds = hull_bounds(sample).ok()?;
            (!self.working_region_bounds_visible(Some(bounds.map(|point| point.as_dvec3()))))
                .then(|| "The selected player hull is outside the working region".into())
        })
    }

    fn player_support_guide_error(&self) -> Option<String> {
        let state = &self.player_support_observation;
        self.player_support_guide_error_for(state.ready.as_ref()?, state.guide.index?)
    }

    pub fn player_support_sample_guide(&self) -> Value {
        let state = &self.player_support_observation;
        let requested = state.guide.index.is_some();
        let error = self.player_support_guide_error();
        let displayed = requested && state.guide.visible && error.is_none();
        let sample = state
            .ready
            .as_ref()
            .zip(state.guide.index)
            .and_then(|(ready, index)| ready.sample(index).ok());
        let probe = sample.and_then(|sample| sample.probe.as_ref());
        json!({"requested":requested,"visible":state.guide.visible,"displayed":displayed,
            "token":state.token.map(|token|token.to_string()),"index":state.guide.index,"suppression_reason":error,
            "build_token":state.ready.as_ref().map(|ready|ready.build.token.to_string()),"profile":state.ready.as_ref().map(|ready|&ready.profile),
            "source":"observed_native","origin":sample.map(|sample|sample.origin),
            "hull_mins":sample.map(|sample|sample.hull_mins),"hull_maxs":sample.map(|sample|sample.hull_maxs),
            "hull_world_bounds":sample.and_then(|sample|hull_bounds(sample).ok()).map(|bounds|bounds.map(|point|point.to_array())),
            "trajectory_samples":state.ready.as_ref().map_or(0,|ready|ready.samples.len()),
            "trajectory_segments":state.ready.as_ref().map_or(0,|ready|ready.samples.len().saturating_sub(1)),
            "probe":probe.map(|probe|json!({"start":probe.start,"requested_end":probe.requested_end,"native_start":probe.native_start,
                "native_end":probe.native_end,"status":probe.status,"plane_available":probe.plane.available,
                "normal":probe.plane.normal,"normal_available":probe.plane.available && normal_direction(probe.plane.normal).is_some(),
                "normal_displayed":displayed && probe.plane.available && normal_direction(probe.plane.normal).is_some(),
                "endpoint_role":"native returned hull-reference endpoint; not an actual contact point","normal_length":NORMAL_LENGTH})),
            "colors":{"hull":HULL_COLOR,"trajectory":TRAJECTORY_COLOR,"probe":PROBE_COLOR,"normal":NORMAL_COLOR},
            "limits":{"guides":1,"trajectory_samples":MAX_PLAYER_SUPPORT_SAMPLES,"segments":MAX_SEGMENTS,"line_vertices":MAX_SEGMENTS*2},"policy":POLICY})
    }

    pub fn set_player_support_sample_guide(
        &mut self,
        actor: Actor,
        token: u64,
        index: usize,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.check_player_support_token(token)?;
        let ready = self
            .player_support_observation
            .ready
            .as_ref()
            .ok_or("Complete a player support observation first")?;
        if let Some(error) = self.player_support_guide_error_for(ready, index) {
            return Err(error);
        }
        self.player_support_observation.guide.index = Some(index);
        self.player_support_observation.guide.visible = true;
        self.invalidate_player_support_observation_render();
        self.log(
            actor,
            format!("Show observed player sample {index} hull and trajectory"),
        );
        Ok(self.player_support_sample_guide())
    }

    pub fn set_player_support_sample_guide_visible(
        &mut self,
        actor: Actor,
        token: u64,
        visible: bool,
    ) -> Result<Value, String> {
        self.check_player_support_token(token)?;
        if visible {
            self.ensure_not_closing()?;
            if self.player_support_observation.guide.index.is_none() {
                return Err("Choose an observed player sample guide first".into());
            }
            if let Some(error) = self.player_support_guide_error() {
                return Err(error);
            }
        }
        self.player_support_observation.guide.visible = visible;
        self.invalidate_player_support_observation_render();
        self.log(
            actor,
            if visible {
                "Show observed player guide"
            } else {
                "Hide observed player guide"
            },
        );
        Ok(self.player_support_sample_guide())
    }

    pub fn clear_player_support_sample_guide(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        self.check_player_support_token(token)?;
        self.retire_player_support_sample_guide();
        self.log(actor, "Cleared observed player guide");
        Ok(self.player_support_sample_guide())
    }

    pub(super) fn retire_player_support_sample_guide(&mut self) {
        let guide = &mut self.player_support_observation.guide;
        guide.index = None;
        guide.visible = false;
        self.invalidate_player_support_observation_render();
    }

    pub(super) fn invalidate_player_support_observation_render(&mut self) {
        self.player_support_observation.guide.serial =
            self.player_support_observation.guide.serial.wrapping_add(1);
        self.scene_serial = self.scene_serial.wrapping_add(1);
    }

    pub fn preflight_player_support_observation(&mut self) {
        let state = &self.player_support_observation;
        let key = (
            state.guide.visible && state.guide.index.is_some(),
            self.player_support_guide_error(),
        );
        if state.guide.presentation_key.as_ref() != Some(&key) {
            self.player_support_observation.guide.presentation_key = Some(key);
            self.invalidate_player_support_observation_render();
        }
    }

    pub fn player_support_observation_render_report(&self) -> Value {
        json!({"guide":self.player_support_sample_guide(),"policy":POLICY})
    }

    pub fn player_support_observation_render_guard(
        &self,
        _view: ViewOptions,
    ) -> Result<Option<RenderGuard>, String> {
        let state = &self.player_support_observation;
        let displayed = state.guide.visible
            && state.guide.index.is_some()
            && self.player_support_guide_error().is_none();
        if displayed {
            state
                .ready
                .as_ref()
                .ok_or("Observed player guide was retired")?
                .current(self)?;
        }
        Ok(Some(RenderGuard {
            token: state.token,
            serial: state.guide.serial,
            ready: state.ready.as_ref().map(Arc::downgrade),
            displayed,
        }))
    }

    pub fn verify_player_support_observation_render(
        &mut self,
        guard: Option<&RenderGuard>,
        _view: ViewOptions,
    ) -> Result<(), String> {
        let guard =
            guard.ok_or("Observed player guide custody is unavailable; refresh the scene")?;
        let result = (|| {
            let state = &self.player_support_observation;
            let same_ready = match (&state.ready, &guard.ready) {
                (Some(current), Some(retained)) => Weak::ptr_eq(&Arc::downgrade(current), retained),
                (None, None) => true,
                _ => false,
            };
            let displayed = state.guide.visible
                && state.guide.index.is_some()
                && self.player_support_guide_error().is_none();
            if state.token != guard.token
                || state.guide.serial != guard.serial
                || !same_ready
                || displayed != guard.displayed
            {
                return Err("Observed player guide changed during rendering; render again".into());
            }
            if displayed {
                state
                    .ready
                    .as_ref()
                    .ok_or("Observed player guide was retired")?
                    .current(self)?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.preflight_player_support_observation();
        }
        result
    }

    pub fn append_player_support_observation_guide(
        &self,
        mesh: &mut SceneMesh,
        _view: ViewOptions,
    ) {
        let state = &self.player_support_observation;
        if !state.guide.visible || self.player_support_guide_error().is_some() {
            return;
        }
        let Some((ready, index)) = state.ready.as_ref().zip(state.guide.index) else {
            return;
        };
        let Ok(sample) = ready.sample(index) else {
            return;
        };
        let Ok([mins, maxs]) = hull_bounds(sample) else {
            return;
        };
        mesh.add_bounds_lines(mins.as_dvec3(), maxs.as_dvec3(), HULL_COLOR);
        for pair in ready.samples.windows(2) {
            append_segment(
                mesh,
                Vec3::from_array(pair[0].origin),
                Vec3::from_array(pair[1].origin),
                TRAJECTORY_COLOR,
            );
        }
        if let Some(probe) = &sample.probe {
            append_segment(
                mesh,
                Vec3::from_array(probe.start),
                Vec3::from_array(probe.requested_end),
                PROBE_COLOR,
            );
            let endpoint = Vec3::from_array(probe.native_end);
            for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                append_segment(
                    mesh,
                    endpoint - axis * ENDPOINT_HALF_EXTENT,
                    endpoint + axis * ENDPOINT_HALF_EXTENT,
                    PROBE_COLOR,
                );
            }
            if probe.plane.available
                && let Some(direction) = normal_direction(probe.plane.normal)
            {
                append_segment(
                    mesh,
                    endpoint,
                    endpoint + direction * NORMAL_LENGTH,
                    NORMAL_COLOR,
                );
            }
        }
    }

    pub(super) fn player_support_sample_guide_geometry_available(
        &self,
        ready: &Ready,
        index: usize,
    ) -> bool {
        self.player_support_guide_geometry_error_for(ready, index)
            .is_none()
    }
}
