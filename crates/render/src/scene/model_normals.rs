use glam::DVec3;
use source_formats::model::Part;

use super::Vertex;
use crate::{materials::MaterialInfo, models::Instance, normal_maps::Report};

const MODEL_NORMAL_FRAME: u32 = 2;

#[derive(Default)]
pub(super) struct Admission {
    parts: usize,
    triangles: usize,
    error: Option<(String, String)>,
}

impl Admission {
    pub(super) fn begin_part(
        &mut self,
        part: &Part,
        material: &MaterialInfo,
        tool_textures: bool,
    ) -> bool {
        if material.tool && !tool_textures {
            return false;
        }
        let error = material.error.as_deref().or_else(|| {
            (material.missing && material.source_identity().is_none())
                .then_some("Model normal preview needs resolved selected materials")
        });
        if let Some(error) = error {
            self.error
                .get_or_insert_with(|| (material.name.clone(), error.to_owned()));
        } else if !material.declares_normals {
            return false;
        }
        self.parts += 1;
        self.triangles += part.indices.len() / 3;
        self.error.is_none()
    }

    pub(super) fn finish_part(&mut self, material: &str, result: Result<(), String>) -> bool {
        if let Err(error) = result {
            self.error = Some((material.to_owned(), error));
            return false;
        }
        true
    }

    pub(super) fn finish(self, instance: &Instance, vertices: &mut [Vertex], report: &mut Report) {
        if self.parts == 0 {
            return;
        }
        if let Some((material, reason)) = self.error {
            for vertex in vertices {
                vertex.tangent = [0.0; 4];
                vertex.texture_flags &= !MODEL_NORMAL_FRAME;
            }
            report.models.excluded_instances += 1;
            report.models.excluded_parts += self.parts;
            report.models.excluded_triangles += self.triangles;
            report.error(format!(
                "Model {} · {} · {material}: {reason}",
                instance.id, instance.asset.path
            ));
        } else {
            report.models.admitted_instances += 1;
            report.models.admitted_parts += self.parts;
            report.models.admitted_triangles += self.triangles;
        }
    }
}

pub(super) fn source_tangents(instance: &Instance) -> Result<&[[f32; 4]], String> {
    if instance.appearance.mode != source_formats::render_color::NORMAL {
        return Err("Model normal preview requires opaque Normal render mode".into());
    }
    let tangents = instance
        .asset
        .tangents
        .as_ref()
        .map_err(ToString::to_string)?;
    if tangents.len() != instance.asset.model.vertices.len() {
        return Err(
            "Model normal preview requires complete stored tangents for its rendered vertices"
                .into(),
        );
    }
    Ok(tangents)
}

pub(super) fn prepare_part(
    instance: &Instance,
    part: &Part,
    vertices: &mut [Vertex],
    tangents: &[[f32; 4]],
    transform: [f32; 4],
) -> Result<(), String> {
    crate::normal_maps::validate_uvs(
        part.indices
            .iter()
            .map(|&index| glam::DVec2::from_array(vertices[index as usize].uv.map(f64::from))),
        transform,
    )?;
    for &index in &part.indices {
        let index = index as usize;
        let vertex = &mut vertices[index];
        if vertex.tangent[3] != 0.0 {
            continue;
        }
        let [x, y, z, handedness] = tangents[index];
        let rotated = (instance.rotation * DVec3::new(f64::from(x), f64::from(y), f64::from(z)))
            .try_normalize()
            .ok_or("Model normal preview needs finite nonzero drawn tangent directions")?
            .as_vec3();
        vertex.tangent = [rotated.x, rotated.y, rotated.z, handedness];
        vertex.texture_flags |= MODEL_NORMAL_FRAME;
    }
    Ok(())
}
