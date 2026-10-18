use glam::{DAffine3, DVec3};
use map_core::ObjectId;
use serde_json::{Value, json};

use super::{Actor, Session};

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    guide: Option<Guide>,
}

struct Guide {
    token: u64,
    generation: u64,
    revision: u64,
    scene: u64,
    entity: u32,
    attachment: usize,
}

pub(super) fn report(
    attachment: &source_formats::model::Attachment,
    index: usize,
    transform: DAffine3,
) -> Value {
    json!({
        "index":index,"name":attachment.name,"bone":attachment.bone,
        "flags":attachment.flags,"world_aligned":attachment.world_aligned(),
        "position":transform.translation.to_array(),
        "axes":[transform.matrix3.x_axis.to_array(),transform.matrix3.y_axis.to_array(),transform.matrix3.z_axis.to_array()]
    })
}

impl State {
    pub(super) fn materials_reloaded(&mut self, previous: u64, current: u64) {
        if let Some(guide) = &mut self.guide
            && guide.scene == previous
        {
            guide.scene = current;
        }
    }
}

impl Session {
    fn model_attachment(&self, entity: u32, index: usize) -> Result<Value, String> {
        if !std::ptr::eq(self.render_document(), &self.document) {
            return Err("Apply or cancel the active geometry preview first".into());
        }
        let source = self
            .document
            .entity(entity)
            .ok_or("Model entity is missing")?;
        if source.hidden {
            return Err(
                "Show the model entity before inspecting its attachment in the viewport".into(),
            );
        }
        let models = self.model_instances(&self.document);
        let instance = models
            .instances
            .iter()
            .find(|instance| instance.id == ObjectId::Entity(entity))
            .ok_or_else(|| {
                models
                    .issues
                    .get(&entity)
                    .cloned()
                    .unwrap_or("A supported model is required".into())
            })?;
        let transform = instance
            .attachment(index)
            .ok_or("Choose a valid one-based model attachment index")?;
        let attachment = &instance.asset.model.attachments[index - 1];
        Ok(report(attachment, index, transform))
    }

    pub fn show_model_attachment(
        &mut self,
        entity: u32,
        attachment: usize,
    ) -> Result<Value, String> {
        let point = self.model_attachment(entity, attachment)?;
        self.model_attachments.serial += 1;
        let token = self.model_attachments.serial;
        self.scene_serial += 1;
        self.model_attachments.guide = Some(Guide {
            token,
            generation: self.document_generation,
            revision: self.document.revision(),
            scene: self.scene_serial,
            entity,
            attachment,
        });
        self.view_serial += 1;
        Ok(
            json!({"token":token,"entity":format!("entity:{entity}"),"valid":true,"attachment":point}),
        )
    }

    fn checked_model_attachment_guide(&self, guide: &Guide) -> Result<Value, String> {
        if guide.generation != self.document_generation
            || guide.revision != self.document.revision()
            || guide.scene != self.scene_serial
        {
            return Err("Map, selection or model assets changed; show the attachment again".into());
        }
        self.model_attachment(guide.entity, guide.attachment)
    }

    pub fn model_attachment_guide(&self) -> Value {
        let Some(guide) = &self.model_attachments.guide else {
            return json!({"token":null,"valid":false});
        };
        match self.checked_model_attachment_guide(guide) {
            Ok(point) => {
                json!({"token":guide.token,"entity":format!("entity:{}",guide.entity),"valid":true,"attachment":point})
            }
            Err(error) => {
                json!({"token":guide.token,"entity":format!("entity:{}",guide.entity),"valid":false,"error":error})
            }
        }
    }

    pub fn clear_model_attachment(&mut self, token: u64) -> Result<(), String> {
        if !self
            .model_attachments
            .guide
            .as_ref()
            .is_some_and(|guide| guide.token == token)
        {
            return Err("Model attachment guide was cleared or replaced".into());
        }
        self.model_attachments.guide = None;
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    pub fn frame_model_attachment(
        &mut self,
        actor: Actor,
        entity: u32,
        attachment: usize,
        aspect: f32,
    ) -> Result<Value, String> {
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("Viewport aspect must be positive and finite".into());
        }
        let point = self.model_attachment(entity, attachment)?;
        let origin = position(&point);
        self.set_camera(self.camera.frame_bounds(
            (origin - DVec3::splat(18.0)).as_vec3(),
            (origin + DVec3::splat(18.0)).as_vec3(),
            aspect,
        ));
        self.log(
            actor,
            format!(
                "framed model attachment {}",
                point["name"].as_str().unwrap()
            ),
        );
        Ok(point)
    }

    pub(super) fn append_model_attachment_guide(&self, mesh: &mut render::SceneMesh) {
        let Some(guide) = &self.model_attachments.guide else {
            return;
        };
        if !self.working_object_visible(self.render_document(), ObjectId::Entity(guide.entity)) {
            return;
        }
        let Ok(point) = self.checked_model_attachment_guide(guide) else {
            return;
        };
        let origin = position(&point);
        for (index, color) in [
            [240, 90, 90, 255],
            [95, 220, 125, 255],
            [100, 155, 255, 255],
        ]
        .into_iter()
        .enumerate()
        {
            let axis = DVec3::from_array(std::array::from_fn(|component| {
                point["axes"][index][component].as_f64().unwrap()
            }))
            .normalize();
            for end in [origin, origin + axis * 16.0] {
                mesh.lines.push(render::scene::LineVertex {
                    position: end.as_vec3().to_array(),
                    color,
                });
            }
        }
        for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
            for sign in [-1.0, 1.0] {
                mesh.lines.push(render::scene::LineVertex {
                    position: (origin + axis * sign * 1.5).as_vec3().to_array(),
                    color: [255, 245, 190, 255],
                });
            }
        }
    }
}

fn position(report: &Value) -> DVec3 {
    DVec3::from_array(std::array::from_fn(|axis| {
        report["position"][axis].as_f64().unwrap()
    }))
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.guide
            .as_ref()
            .map(|guide| serde_json::json!([self.serial, guide.token]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn attachment_guides_share_pose_guards_ownership_and_source_preservation() {
        let Ok(game) = crate::startup::mount_game(None) else {
            return;
        };
        let mut session = Session::new(Ok(game));
        let ObjectId::Entity(id) = session
            .create_entity(
                Actor::Agent,
                "prop_dynamic",
                Some(DVec3::new(100.0, 200.0, 20.0)),
                BTreeMap::from([
                    ("model".into(), "models/kleiner.mdl".into()),
                    ("angles".into(), "0 90 0".into()),
                ]),
            )
            .unwrap()
        else {
            unreachable!()
        };
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let camera = session.camera();
        let info = session.inspect_entity_model(id).unwrap();
        let point = &info["attachments"][4];
        let base = session.build_scene();
        let serial = session.scene_serial();
        let token = session.show_model_attachment(id, 5).unwrap()["token"]
            .as_u64()
            .unwrap();
        assert!(session.scene_serial() > serial);
        assert_eq!(session.model_attachment_guide()["attachment"], *point);
        assert_eq!(session.build_scene().lines.len(), base.lines.len() + 12);
        assert_eq!(session.camera(), camera);
        assert!(session.show_model_attachment(id, 0).is_err());
        assert_eq!(session.model_attachment_guide()["token"], token);
        let replacement = session.show_model_attachment(id, 1).unwrap()["token"]
            .as_u64()
            .unwrap();
        assert!(session.clear_model_attachment(token).is_err());
        let serial = session.scene_serial();
        session.clear_model_attachment(replacement).unwrap();
        assert!(session.scene_serial() > serial);
        assert_eq!(session.build_scene().lines.len(), base.lines.len());
        session
            .frame_model_attachment(Actor::User, id, 5, 1.5)
            .unwrap();
        assert_ne!(session.camera(), camera);
        assert!(
            session
                .frame_model_attachment(Actor::Agent, id, 5, f32::NAN)
                .is_err()
        );
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        let token = session.show_model_attachment(id, 5).unwrap()["token"]
            .as_u64()
            .unwrap();
        session.refresh_model_assets(Actor::User);
        assert_eq!(session.model_attachment_guide()["valid"], false);
        assert_eq!(session.build_scene().lines.len(), base.lines.len());
        session.clear_model_attachment(token).unwrap();
    }
}
