use glam::{DVec3, Vec3};
use render::Camera;
use serde_json::Value;
use source_formats::bsp::{Bsp, Edge, Face, Model, Plane};

use super::super::{Actor, Session, baked_build, inspection_framing};
use super::{Channel, read_owned_bsp};

const FACE_PLANE_EPSILON: f64 = compiler::winding::ON_EPSILON * 2.0;
const NORMAL_LENGTH_EPSILON: f64 = 0.001;

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

fn model_range(model: &Model, faces: usize) -> Result<std::ops::Range<usize>, String> {
    let first = usize::try_from(model.first_face)
        .map_err(|_| "Compiled model has a negative first face")?;
    let count = usize::try_from(model.face_count)
        .map_err(|_| "Compiled model has a negative face count")?;
    let end = first
        .checked_add(count)
        .filter(|&end| end <= faces)
        .ok_or("Compiled model face range exceeds this channel's face table")?;
    Ok(first..end)
}

fn face_bounds(bsp: &Bsp, channel: Channel, ordinal: usize) -> Result<[Vec3; 2], String> {
    let faces = bsp
        .lighting_faces(channel.kind())
        .map_err(|error| error.to_string())?;
    let face = faces
        .get(ordinal)
        .ok_or("Compiled face index exceeds this channel's face count")?;
    if face.dispinfo >= 0 {
        return Err("Terrain faces cannot be framed from their flat compiled base face".into());
    }
    if face.dispinfo != -1 {
        return Err("Compiled face has an invalid displacement index".into());
    }
    let models = bsp.records::<Model>().map_err(|error| error.to_string())?;
    let world = models.first().ok_or("Compiled Model0 is unavailable")?;
    if world.origin != [0.0; 3] {
        return Err("Compiled Model0 has a nonzero or invalid origin".into());
    }
    if !model_range(world, faces.len())?.contains(&ordinal) {
        return Err("Only world/detail faces in compiled Model0 can be framed; model-local faces are unsupported".into());
    }
    for model in models.iter().skip(1) {
        if model_range(model, faces.len())?.contains(&ordinal) {
            return Err("Compiled face belongs to overlapping model ranges".into());
        }
    }
    winding_bounds(bsp, face)
}

fn winding_bounds(bsp: &Bsp, face: &Face) -> Result<[Vec3; 2], String> {
    let planes = bsp.records::<Plane>().map_err(|error| error.to_string())?;
    let plane = planes
        .get(usize::from(face.plane))
        .ok_or("Compiled face plane is out of bounds")?;
    let normal = DVec3::from_array(plane.normal.map(f64::from));
    if !normal.is_finite()
        || !plane.dist.is_finite()
        || (normal.length_squared() - 1.0).abs() > NORMAL_LENGTH_EPSILON
        || face.side > 1
        || !face.area.is_finite()
        || face.area <= 0.0
    {
        return Err("Compiled face has an invalid plane, side or area".into());
    }
    let first = usize::try_from(face.first_edge)
        .map_err(|_| "Compiled face has a negative first surfedge")?;
    let count = usize::try_from(face.edge_count)
        .ok()
        .filter(|&count| count >= 3)
        .ok_or("Compiled face requires at least three surfedges")?;
    let surfedges = bsp.surfedges().map_err(|error| error.to_string())?;
    let end = first
        .checked_add(count)
        .ok_or("Compiled face surfedge range overflow")?;
    let ring = surfedges
        .get(first..end)
        .ok_or("Compiled face surfedge range is out of bounds")?;
    let edges = bsp.records::<Edge>().map_err(|error| error.to_string())?;
    let vertices = bsp.vertices().map_err(|error| error.to_string())?;
    let mut first_vertex = None;
    let mut previous_end = None;
    let mut mins = Vec3::splat(f32::INFINITY);
    let mut maxs = Vec3::splat(f32::NEG_INFINITY);
    let mut area = DVec3::ZERO;
    for &surfedge in ring {
        let index = usize::try_from(surfedge.unsigned_abs())
            .map_err(|_| "Compiled edge index is too large")?;
        let edge = edges
            .get(index)
            .ok_or("Compiled surfedge references an out-of-range edge")?
            .0;
        let [start, end] = if surfedge < 0 {
            [edge[1], edge[0]]
        } else {
            edge
        };
        if start == end || previous_end.is_some_and(|previous| previous != start) {
            return Err(
                "Compiled face surfedges do not form a connected nonzero edge chain".into(),
            );
        }
        let mut points = [Vec3::ZERO; 2];
        for (point, index) in points.iter_mut().zip([start, end]) {
            *point = Vec3::from_array(
                *vertices
                    .get(usize::from(index))
                    .ok_or("Compiled edge references an out-of-range vertex")?,
            );
            if !point.is_finite()
                || point.abs().max_element() > map_core::checks::SOURCE_COORDINATE_LIMIT as f32
            {
                return Err(
                    "Compiled face vertices must be finite and inside Source bounds".into(),
                );
            }
            if (normal.dot(point.as_dvec3()) - f64::from(plane.dist)).abs() > FACE_PLANE_EPSILON {
                return Err("Compiled face vertices do not lie on its flat plane".into());
            }
        }
        if points[0] == points[1] {
            return Err("Compiled face contains a zero-length edge".into());
        }
        first_vertex.get_or_insert(start);
        previous_end = Some(end);
        mins = mins.min(points[0]);
        maxs = maxs.max(points[0]);
        area += points[0].as_dvec3().cross(points[1].as_dvec3());
    }
    if first_vertex != previous_end || area.length_squared() == 0.0 {
        return Err("Compiled face requires a closed nondegenerate edge chain".into());
    }
    Ok([mins, maxs])
}
