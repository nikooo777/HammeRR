use super::{Vec3, Vertex};
use crate::lighting::Report;
use crate::materials::MaterialInfo;
use crate::models::Instance;
use source_formats::model::Part;

pub(super) struct Admission {
    parts: usize,
    triangles: usize,
    error: Option<(Option<String>, String)>,
}

impl Admission {
    pub(super) fn new(instance: &Instance) -> Self {
        Self {
            parts: 0,
            triangles: 0,
            error: (instance.appearance.mode != source_formats::render_color::NORMAL).then(|| {
                (None, "Model authored lighting requires opaque Normal render mode; Texture opacity keeps ordinary shading".into())
            }),
        }
    }

    pub(super) fn part(
        &mut self,
        part: &Part,
        material: &MaterialInfo,
        vertices: &[Vertex],
        tool_textures: bool,
    ) {
        self.parts += 1;
        if tool_textures || !material.tool {
            self.triangles += part.indices.len() / 3;
        }
        if self.error.is_some() {
            return;
        }
        let reason = if material.tool || material.sky {
            Some("Tool and sky model materials are excluded from authored lighting")
        } else if material.base.is_none() || material.missing {
            Some("Model authored lighting needs an available base display texture")
        } else {
            material.model_lighting_error.as_deref()
        };
        if let Some(reason) = reason {
            self.error = Some((Some(material.name.clone()), reason.into()));
            return;
        }
        for triangle in part.indices.as_chunks::<3>().0 {
            let packed = triangle.map(|index| vertices[index as usize]);
            if let Some(reason) = crate::lighting::triangle_error(
                packed.map(|vertex| Vec3::from_array(vertex.position)),
                packed.map(|vertex| Vec3::from_array(vertex.normal)),
            ) {
                self.error = Some((Some(material.name.clone()), reason.into()));
                return;
            }
        }
    }

    pub(super) fn finish(self, instance: &Instance, vertices: &mut [Vertex], report: &mut Report) {
        if let Some((material, reason)) = self.error {
            report.models.excluded_instances += 1;
            report.models.excluded_parts += self.parts;
            report.models.excluded_triangles += self.triangles;
            let entity = match instance.id {
                map_core::ObjectId::Entity(id) => Some(id),
                _ => None,
            };
            report.diagnostic(
                entity,
                material.as_deref(),
                &format!(
                    "{}: {reason}; the whole selected model instance keeps ordinary shading",
                    instance.asset.path
                ),
            );
        } else {
            report.models.admitted_instances += 1;
            report.models.admitted_parts += self.parts;
            report.models.admitted_triangles += self.triangles;
            for part in instance.asset.model.parts_for_body(instance.body) {
                for &index in &part.indices {
                    vertices[index as usize].lighting_receiver = 1;
                }
            }
        }
    }
}
