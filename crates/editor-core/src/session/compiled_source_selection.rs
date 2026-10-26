use compiler::entity_origins::Origin;
use map_core::ObjectId;
use serde_json::{Value, json};

use super::{Actor, SelectMode, Session};

pub(super) struct SourceTarget {
    id: ObjectId,
    kind: &'static str,
}

pub(super) fn captured_root_target(origin: &Origin) -> SourceTarget {
    let (id, kind) = match origin {
        Origin::Root { entity } => (*entity, "root_entity"),
        Origin::Instance { instance, .. } => (*instance, "containing_reference"),
    };
    SourceTarget {
        id: ObjectId::Entity(id),
        kind,
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
        self.select(&[target.id], SelectMode::Replace)?;
        self.log(actor, format!("selected compiled source {}", target.id));
        Ok(
            json!({"target":crate::identity::id_string(target.id),"source_kind":target.kind,"mode":"objects","selected_count":1}),
        )
    }
}
