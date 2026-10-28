use glam::{DVec3, Vec3};
use render::Camera;
use serde_json::Value;
use source_formats::bsp::{Bsp, FaceGeometry};

use super::super::{Actor, Session, baked_build, inspection_framing};
use super::{Channel, read_owned_bsp};

#[derive(Debug)]
pub struct FramedFace {
    pub build: Value,
    pub channel: &'static str,
    pub face: usize,
    pub bounds: [[f32; 3]; 2],
    pub camera: Camera,
}

impl Session {
    pub fn frame_compiled_face(
        &mut self,
        actor: Actor,
        build_token: String,
        channel: Channel,
        face: usize,
        aspect: f32,
    ) -> Result<FramedFace, String> {
        self.ensure_not_closing()?;
        inspection_framing::validate_aspect(aspect)?;
        self.inspection_viewport_ready("framing a compiled face")?;
        let state = self.baked_lightmap_build_state();
        if state["token"].as_str() != Some(build_token.as_str()) {
            return Err(
                "Build token does not match the owned successful build; inspect again".into(),
            );
        }
        if state["current"] != true || state["status"] != "current" {
            return Err("Framing requires a current successful owned build; refresh inspection or compile again".into());
        }
        let build = self.compiled.as_ref().ok_or("No successful owned build")?;
        let bsp = read_owned_bsp(build)?;
        if let Some(error) = baked_build::source_issue(build) {
            return Err(error);
        }
        if let Ok(origins) = &build.face_origins {
            origins.validate_channel(&bsp, channel.kind())?;
        }
        let [mins, maxs] = face_bounds(&bsp, channel, face)?;
        let camera = self.inspection_frame_camera(mins, maxs, aspect, "Compiled face")?;
        let rechecked = baked_build::recheck(build);
        if let Some(error) = rechecked
            .unavailable
            .or_else(|| rechecked.stale.into_iter().next())
        {
            return Err(error);
        }
        self.set_camera(camera);
        self.log(
            actor,
            format!("framed compiled {} face {face}", channel.name()),
        );
        Ok(FramedFace {
            build: state,
            channel: channel.name(),
            face,
            bounds: [mins.to_array(), maxs.to_array()],
            camera,
        })
    }
}

pub(super) fn face_bounds(
    bsp: &Bsp,
    channel: Channel,
    ordinal: usize,
) -> Result<[Vec3; 2], String> {
    let geometry = FaceGeometry::new(bsp, channel.kind()).map_err(|error| error.to_string())?;
    let (model, positions) = geometry
        .positions(ordinal)
        .map_err(|error| error.to_string())?;
    let transforms = compiler::initial_brush_model_transforms(bsp)?;
    let transform = transforms
        .get(model)
        .ok_or("Compiled face has no native initial model pose")?;
    let mut mins = DVec3::splat(f64::INFINITY);
    let mut maxs = DVec3::splat(f64::NEG_INFINITY);
    for position in &positions {
        let point = transform.transform_point3(DVec3::from_array(position.map(f64::from)));
        if !point.is_finite()
            || point.abs().max_element() > map_core::checks::SOURCE_COORDINATE_LIMIT
        {
            return Err(
                "Compiled face initial-pose vertices must be finite and inside Source bounds"
                    .into(),
            );
        }
        mins = mins.min(point);
        maxs = maxs.max(point);
    }
    if !mins.is_finite() || !maxs.is_finite() {
        return Err("Compiled face has no finite final geometry".into());
    }
    Ok([mins.as_vec3(), maxs.as_vec3()])
}
