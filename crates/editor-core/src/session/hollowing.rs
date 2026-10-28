use super::previews;
use map_core::hollow::{self, Spec};
use map_core::{Document, EditError, ObjectId, subobject};
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};

pub struct OpeningFace {
    pub id: map_core::surface::FaceId,
    pub normal: glam::DVec3,
    pub center: glam::DVec3,
    pub material: String,
}

pub struct Preview {
    pub document: Document,
    pub result: hollow::Result,
    token: u64,
    ids: Vec<ObjectId>,
    spec: Spec,
    generation: u64,
    revision: u64,
}

#[derive(Default)]
pub struct State {
    pub preview: Option<Preview>,
    serial: u64,
}

pub fn result_json(result: &hollow::Result) -> Value {
    json!({"objects":result.objects.iter().map(|id|match id {ObjectId::Solid(id)=>format!("solid:{id}"),ObjectId::Entity(id)=>format!("entity:{id}")}).collect::<Vec<_>>(),"brushes":result.brushes,"sources":result.sources,
        "cavities":result.cavities.iter().map(|cavity|json!({"source":cavity.source,"center":cavity.center.to_array(),"mins":cavity.mins.to_array(),"maxs":cavity.maxs.to_array()})).collect::<Vec<_>>()})
}

impl Session {
    pub fn hollow_faces(&self) -> Result<Vec<OpeningFace>, String> {
        let ids = self
            .component_solids()
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        if ids.len() > hollow::MAX_BRUSHES {
            return Err("Hollow at most 16 brushes at a time".into());
        }
        let mut faces = Vec::new();
        for id in ids {
            let (_, solid) =
                subobject::solid(&self.document, id).map_err(|error| error.to_string())?;
            let shape = subobject::topology(solid).map_err(|error| error.to_string())?;
            for face in shape.faces {
                let side = solid
                    .sides
                    .iter()
                    .find(|side| side.id == face.side)
                    .unwrap();
                faces.push(OpeningFace {
                    id: map_core::surface::FaceId {
                        solid: id,
                        side: side.id,
                    },
                    normal: map_core::Plane::from_points(side.plane.0).unwrap().normal,
                    center: face
                        .vertices
                        .iter()
                        .map(|index| shape.vertices[*index].position)
                        .sum::<glam::DVec3>()
                        / face.vertices.len() as f64,
                    material: side.material.clone(),
                });
            }
        }
        Ok(faces)
    }

    pub fn hollow_report(&self) -> Value {
        let mut report = self.hollow_state();
        match self.hollow_faces() {
            Ok(faces) => report["faces"] = json!(faces.iter().map(|face| json!({"solid":face.id.solid,"side":face.id.side,"normal":face.normal.to_array(),"center":face.center.to_array(),"material":face.material})).collect::<Vec<_>>()),
            Err(error) => report["source_error"] = json!(error),
        }
        report
    }

    pub fn hollow_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.scene_serial,
            self.geometry.hollowing.serial
        )
    }

    pub fn clear_hollow_preview(&mut self) {
        if self.geometry.hollowing.preview.take().is_some() {
            self.geometry.hollowing.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn preview_hollow(
        &mut self,
        ids: &[ObjectId],
        spec: Spec,
        stamp: &str,
    ) -> Result<(), String> {
        if stamp != self.hollow_stamp() {
            return Err("Map, selection or preview changed; inspect the hollow state again".into());
        }
        let mut document = self.document.preview_snapshot();
        let result = document
            .edit("preview hollow", |tx| hollow::hollow(tx, ids, &spec))
            .map_err(|error| error.to_string())?;
        self.select(ids, SelectMode::Replace)?;
        self.cancel_block_preview();
        self.clear_geometry_previews_except(&[previews::Kind::Hollow]);

        self.cancel_area_selection();
        self.clear_volume_selection();

        self.clip_tool = false;
        self.geometry.paint.enabled = false;
        self.spatial.draw_block = false;
        self.geometry.hollowing.serial += 1;
        self.geometry.hollowing.preview = Some(Preview {
            document,
            result,
            token: self.geometry.hollowing.serial,
            ids: ids.to_vec(),
            spec,
            generation: self.document_generation,
            revision: self.document.revision(),
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    fn checked_hollow(&self, token: u64) -> Result<&Preview, String> {
        let preview = self
            .geometry
            .hollowing
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Hollow preview was closed or replaced")?;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
        {
            return Err("Map changed; preview the hollow again".into());
        }
        Ok(preview)
    }

    pub fn commit_hollow(&mut self, actor: Actor, token: u64) -> Result<hollow::Result, String> {
        let preview = self.checked_hollow(token)?;
        let (ids, spec, expected) = (
            preview.ids.clone(),
            preview.spec.clone(),
            preview.result.clone(),
        );
        let solids = expected
            .brushes
            .iter()
            .map(|id| subobject::solid(&preview.document, *id).map(|(_, solid)| solid.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let result = self.edit(actor, "hollow brushes", |tx| {
            let result = hollow::hollow(tx, &ids, &spec)?;
            if result != expected
                || solids.iter().any(|solid| {
                    subobject::solid(tx.document(), solid.id)
                        .map_or(true, |(_, current)| current != solid)
                })
            {
                return Err(EditError::Invalid(
                    "Brush allocation changed; preview the hollow again".into(),
                ));
            }
            Ok(result)
        })?;
        self.select(&result.objects, SelectMode::Replace)?;
        Ok(result)
    }

    pub fn cancel_hollow(&mut self, token: u64) -> Result<(), String> {
        self.checked_hollow(token)?;
        self.clear_hollow_preview();
        Ok(())
    }

    pub fn look_inside_hollow(&mut self, token: u64, index: usize) -> Result<(), String> {
        let cavity = self
            .checked_hollow(token)?
            .result
            .cavities
            .get(index)
            .ok_or("Cavity index is outside this preview")?;
        let center = cavity.center.as_vec3();
        let mut camera =
            render::Camera::looking_at(center, center + glam::Vec3::new(1.0, 1.0, -0.2));
        camera.fov_y_degrees = self.camera.fov_y_degrees;
        self.set_camera(camera);
        Ok(())
    }

    pub fn hollow_state(&self) -> Value {
        json!({"handle":self.hollow_handle(),"preview":self.geometry.hollowing.preview.as_ref().map(|preview|json!({"token":preview.token.to_string(),"generation":preview.generation,"revision":preview.revision,"thickness":preview.spec.thickness,"outward":preview.spec.outward,"open_faces":preview.spec.open_faces.iter().map(|face|json!({"solid":face.solid,"side":face.side})).collect::<Vec<_>>(),"result":result_json(&preview.result)})),"limits":{"brushes":hollow::MAX_BRUSHES,"walls":hollow::MAX_PARTS,"minimum_thickness":0.125,"maximum_thickness":4096},"stamp":self.hollow_stamp()})
    }

    pub(super) fn hollow_handle(&self) -> Value {
        Self::edit_handle(
            "hollow",
            self.geometry
                .hollowing
                .preview
                .as_ref()
                .and_then(|preview| {
                    self.checked_hollow(preview.token)
                        .is_ok()
                        .then_some(preview.token)
                }),
        )
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.preview
            .as_ref()
            .map(|preview| serde_json::json!([self.serial, preview.token]))
    }
}
