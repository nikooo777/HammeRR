use map_core::hollow::{self, Spec};
use map_core::{Document, EditError, ObjectId, subobject};
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};

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
    pub fn hollow_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.scene_serial,
            self.hollowing.serial
        )
    }

    pub fn clear_hollow_preview(&mut self) {
        if self.hollowing.preview.take().is_some() {
            self.hollowing.serial += 1;
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
        let mut document = self.document.clone();
        let result = document
            .edit("preview hollow", |tx| hollow::hollow(tx, ids, &spec))
            .map_err(|error| error.to_string())?;
        self.select(ids, SelectMode::Replace)?;
        self.cancel_block_preview();
        self.cancel_clip_preview();
        self.cancel_transform_preview();
        self.cancel_area_selection();
        self.clear_volume_selection();
        self.cancel_face_preview();
        self.clear_component_edit();
        self.clear_primitive_preview();
        self.clear_paint_stroke();
        self.clip_tool = false;
        self.paint.enabled = false;
        self.spatial.draw_block = false;
        self.hollowing.serial += 1;
        self.hollowing.preview = Some(Preview {
            document,
            result,
            token: self.hollowing.serial,
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
        json!({"preview":self.hollowing.preview.as_ref().map(|preview|json!({"token":preview.token.to_string(),"generation":preview.generation,"revision":preview.revision,"thickness":preview.spec.thickness,"outward":preview.spec.outward,"result":result_json(&preview.result)})),"limits":{"brushes":hollow::MAX_BRUSHES,"walls":hollow::MAX_PARTS,"minimum_thickness":0.125,"maximum_thickness":4096},"stamp":self.hollow_stamp()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;

    fn fixture() -> (Session, ObjectId) {
        let mut session = Session::new(Err("no assets needed".into()));
        let id = session
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(128.0),
                "brick/test",
                false,
            )
            .unwrap();
        (session, id)
    }

    #[test]
    fn hollow_preview_preserves_map_and_commits_exact_geometry_in_one_step() {
        let (mut session, id) = fixture();
        let original = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        session
            .preview_hollow(
                &[id],
                Spec {
                    thickness: 16.0,
                    outward: false,
                },
                &session.hollow_stamp(),
            )
            .unwrap();
        assert_eq!(session.document.vmf(), &original);
        assert_eq!(session.document.undo_labels().count(), history);
        let preview = session.hollowing.preview.as_ref().unwrap();
        let token = preview.token;
        let draft = preview.document.vmf().clone();
        assert_eq!(session.render_document().vmf(), &draft);
        assert!(
            session
                .preview_hollow(
                    &[id],
                    Spec {
                        thickness: 64.0,
                        outward: false
                    },
                    &session.hollow_stamp()
                )
                .is_err()
        );
        assert_eq!(session.render_document().vmf(), &draft);
        session.look_inside_hollow(token, 0).unwrap();
        assert_eq!(session.camera.position, DVec3::splat(64.0).as_vec3());
        assert!(session.look_inside_hollow(token, 1).is_err());
        session.commit_hollow(Actor::User, token).unwrap();
        assert_eq!(session.document.vmf(), &draft);
        assert_eq!(session.document.undo_labels().count(), history + 1);
        session.undo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &original);
        session.redo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &draft);
    }

    #[test]
    fn hollow_cancellation_and_competing_previews_guard_tokens() {
        let (mut session, id) = fixture();
        let original = session.document.vmf().clone();
        session
            .preview_hollow(
                &[id],
                Spec {
                    thickness: 16.0,
                    outward: false,
                },
                &session.hollow_stamp(),
            )
            .unwrap();
        let first = session.hollowing.preview.as_ref().unwrap().token;
        session
            .preview_hollow(
                &[id],
                Spec {
                    thickness: 8.0,
                    outward: true,
                },
                &session.hollow_stamp(),
            )
            .unwrap();
        let second = session.hollowing.preview.as_ref().unwrap().token;
        assert!(session.cancel_hollow(first).is_err());
        assert!(session.commit_hollow(Actor::Agent, first).is_err());
        session.cancel_hollow(second).unwrap();
        assert_eq!(session.render_document().vmf(), &original);
        session
            .preview_hollow(
                &[id],
                Spec {
                    thickness: 16.0,
                    outward: false,
                },
                &session.hollow_stamp(),
            )
            .unwrap();
        let token = session.hollowing.preview.as_ref().unwrap().token;
        session
            .create_primitive(Actor::Agent, Default::default(), true)
            .unwrap();
        assert!(session.commit_hollow(Actor::Agent, token).is_err());
        assert!(session.hollowing.preview.is_none());
    }
}
