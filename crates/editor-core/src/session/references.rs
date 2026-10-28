use map_core::references::{self, Reference, RenamePlan};
use serde::Serialize;

use super::{Actor, Session};

#[derive(Debug, Clone, Serialize)]
pub struct RenameDraft {
    pub token: u64,
    pub entity: u32,
    pub new_name: String,
    pub rename_group: bool,
    pub accept_review: bool,
    pub show_details: bool,
    pub generation: u64,
    pub revision: u64,
    pub definitions_serial: u64,
}

impl Session {
    pub fn rename_serial(&self) -> u64 {
        self.rename_serial
    }

    pub fn rename_draft(&self) -> Option<&RenameDraft> {
        self.rename_draft.as_ref()
    }

    pub fn entity_references(&self, entity: u32) -> Result<Vec<Reference>, String> {
        references::incoming(&self.document, &self.definitions, entity)
    }

    pub fn begin_entity_rename(&mut self, entity: u32) -> Result<(), String> {
        let entity = self
            .document
            .entity(entity)
            .ok_or("Entity no longer exists")?;
        self.rename_draft = Some(RenameDraft {
            token: self.rename_serial + 1,
            entity: entity.id,
            new_name: entity
                .properties
                .get("targetname")
                .unwrap_or_default()
                .into(),
            rename_group: false,
            accept_review: false,
            show_details: false,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions_serial: self.definitions_serial,
        });
        self.rename_serial += 1;
        Ok(())
    }

    pub fn check_rename_draft(&self, token: u64) -> Result<(), String> {
        if self
            .rename_draft
            .as_ref()
            .is_none_or(|draft| draft.token != token)
        {
            return Err("Rename draft changed. Reopen References and rename.".into());
        }
        Ok(())
    }

    pub fn update_entity_rename(
        &mut self,
        new_name: String,
        rename_group: bool,
        accept_review: bool,
    ) -> Result<(), String> {
        let draft = self
            .rename_draft
            .as_mut()
            .ok_or("Begin an entity rename first")?;
        let changed = draft.new_name != new_name || draft.rename_group != rename_group;
        draft.new_name = new_name;
        draft.rename_group = rename_group;
        draft.accept_review = !changed && accept_review;
        self.rename_serial += 1;
        Ok(())
    }

    pub fn rename_preview(&self) -> Result<RenamePlan, String> {
        let draft = self
            .rename_draft
            .as_ref()
            .ok_or("Begin an entity rename first")?;
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
            || draft.definitions_serial != self.definitions_serial
        {
            return Err("Map or definitions changed. Cancel and reopen the rename preview.".into());
        }
        references::plan(
            &self.document,
            &self.definitions,
            draft.entity,
            &draft.new_name,
            draft.rename_group,
        )
    }

    pub fn commit_entity_rename(&mut self, actor: Actor) -> Result<RenamePlan, String> {
        let plan = self.rename_preview()?;
        let accepted = self.rename_draft.as_ref().unwrap().accept_review;
        self.edit(actor, "rename entity references", |tx| {
            references::apply(tx, &plan, accepted)
        })?;
        self.cancel_entity_rename();
        Ok(plan)
    }

    pub fn rename_can_apply(&self, plan: &RenamePlan) -> bool {
        plan.error.is_none()
            && self.rename_draft.as_ref().is_some_and(|draft| {
                draft.accept_review || !plan.references.iter().any(|change| change.review)
            })
            && plan.entities.iter().any(|id| {
                self.document.entity(*id).is_some_and(|entity| {
                    entity.properties.get("targetname") != Some(plan.new_name.as_str())
                })
            })
    }

    pub fn set_rename_details(&mut self, show_details: bool) -> Result<(), String> {
        let draft = self
            .rename_draft
            .as_mut()
            .ok_or("Begin an entity rename first")?;
        if draft.show_details != show_details {
            draft.show_details = show_details;
            self.rename_serial += 1;
        }
        Ok(())
    }

    pub fn cancel_entity_rename(&mut self) {
        if self.rename_draft.take().is_some() {
            self.rename_serial += 1;
        }
    }
}
