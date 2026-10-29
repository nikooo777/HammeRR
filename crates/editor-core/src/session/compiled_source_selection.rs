use compiler::entity_origins::Origin;
use map_core::{ObjectId, surface::FaceId};
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};

pub(super) enum SourceTarget {
    Object { id: ObjectId, kind: &'static str },
    Face(FaceId),
}

pub(super) fn captured_root_target(origin: &Origin) -> SourceTarget {
    let (id, kind) = match origin {
        Origin::Root { entity } => (*entity, "root_entity"),
        Origin::Instance { instance, .. } => (*instance, "containing_reference"),
    };
    SourceTarget::Object {
        id: ObjectId::Entity(id),
        kind,
    }
}

pub(super) fn captured_face_target(
    origin: &compiler::face_origins::Origin,
) -> Result<SourceTarget, &'static str> {
    match origin {
        compiler::face_origins::Origin::Root { solid, side, .. } => {
            Ok(SourceTarget::Face(FaceId {
                solid: *solid,
                side: *side,
            }))
        }
        compiler::face_origins::Origin::Instance { instance, .. } => Ok(SourceTarget::Object {
            id: ObjectId::Entity(*instance),
            kind: "containing_reference",
        }),
        compiler::face_origins::Origin::GeneratedRegion { .. } => {
            Err("Generated region faces have no selectable authored source")
        }
        compiler::face_origins::Origin::Unresolved { .. } => {
            Err("The compiled face source is unresolved")
        }
    }
}

impl Session {
    pub(super) fn compiled_source_selection_ready(&self) -> Result<(), String> {
        self.ensure_not_closing()?;
        self.inspection_viewport_ready("selecting a compiled source")?;
        if self.geometry.entity_placement.has_draft()
            || self.geometry.instance_collapse.worker.busy()
            || !std::ptr::eq(self.render_document(), &self.document)
        {
            return Err(
                "Finish or cancel active geometry drafts before selecting a compiled source".into(),
            );
        }
        Ok(())
    }

    pub(super) fn select_compiled_source_target(
        &mut self,
        actor: Actor,
        target: SourceTarget,
    ) -> Result<Value, String> {
        self.compiled_source_selection_ready()?;
        let result = match target {
            SourceTarget::Object { id, kind } => {
                self.select(&[id], SelectMode::Replace)?;
                self.log(actor, format!("selected compiled source {id}"));
                json!({"target":crate::identity::id_string(id),"source_kind":kind,"mode":"objects","selected_count":1})
            }
            SourceTarget::Face(face) => {
                self.select_faces(&[face], SelectMode::Replace)?;
                self.log(
                    actor,
                    format!(
                        "selected compiled source solid:{} side:{}",
                        face.solid, face.side
                    ),
                );
                json!({"target":{"solid":face.solid,"side":face.side},"source_kind":"root_face","mode":"faces","selected_count":1})
            }
        };
        Ok(result)
    }
}
