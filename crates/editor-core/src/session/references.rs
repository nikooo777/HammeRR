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

#[cfg(test)]
mod tests {
    use super::*;
    use source_formats::{fgd::Catalog, vmf::Vmf};

    fn fixture() -> Session {
        let mut session = Session::new(Err("no assets".into()));
        session.set_definitions(Actor::Agent, Catalog::parse(r#"
@PointClass = logic_relay [ targetname(target_source) : "Name" target(target_destination) : "Target" ]
"#).unwrap());
        session.replace_document(
            map_core::Document::new(
                Vmf::parse(
                    r#"
world { "id" "1" "classname" "worldspawn" }
entity { "id" "2" "classname" "logic_relay" "targetname" "relay" }
entity { "id" "3" "classname" "logic_relay" "target" "relay"
 connections { "OnTrigger" "relay,Trigger,,0.00,-1" } }
"#,
                )
                .unwrap(),
            ),
            None,
        );
        session
    }

    #[test]
    fn both_actors_share_reference_preview_cancel_commit_and_undo() {
        for actor in [Actor::Agent, Actor::User] {
            let mut session = fixture();
            let original = session.document.vmf().clone();
            assert_eq!(session.entity_references(2).unwrap().len(), 2);
            session.begin_entity_rename(2).unwrap();
            session
                .update_entity_rename("new_relay".into(), false, false)
                .unwrap();
            assert!(session.rename_preview().unwrap().error.is_none());
            assert_eq!(session.document.vmf(), &original);
            session.cancel_entity_rename();
            assert_eq!(session.document.undo_labels().count(), 0);
            session.begin_entity_rename(2).unwrap();
            session
                .update_entity_rename("new_relay".into(), false, false)
                .unwrap();
            session.commit_entity_rename(actor).unwrap();
            assert!(session.rename_draft().is_none());
            assert_eq!(session.document.undo_labels().count(), 1);
            assert_eq!(
                session.document.entity(3).unwrap().properties.get("target"),
                Some("new_relay")
            );
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &original);
            session.redo(actor).unwrap();
            assert_eq!(session.entity_references(2).unwrap().len(), 2);
        }
    }

    #[test]
    fn replaced_and_stale_drafts_cannot_mutate_or_cancel_other_work() {
        let mut session = fixture();
        session.begin_entity_rename(2).unwrap();
        let token = session.rename_draft().unwrap().token;
        session.begin_entity_rename(3).unwrap();
        assert!(session.check_rename_draft(token).is_err());
        session.begin_entity_rename(2).unwrap();
        session
            .update_entity_rename("new_relay".into(), false, false)
            .unwrap();
        session
            .set_entity_properties(Actor::Agent, 3, &[("custom".into(), Some("value".into()))])
            .unwrap();
        session.undo(Actor::Agent).unwrap();
        assert!(
            session
                .commit_entity_rename(Actor::User)
                .unwrap_err()
                .contains("changed")
        );
        session.begin_entity_rename(2).unwrap();
        session
            .update_entity_rename("new_relay".into(), false, false)
            .unwrap();
        session.set_definitions(
            Actor::Agent,
            Catalog::parse("@PointClass = logic_relay []").unwrap(),
        );
        assert!(
            session
                .commit_entity_rename(Actor::User)
                .unwrap_err()
                .contains("definitions changed")
        );
        session.new_map(Actor::Agent, true).unwrap();
        assert!(session.rename_draft().is_none());
    }

    #[test]
    fn changing_name_or_group_requires_fresh_review() {
        let mut session = fixture();
        session
            .set_entity_properties(Actor::User, 3, &[("notes".into(), Some("relay".into()))])
            .unwrap();
        let original = session.document.vmf().clone();
        session.begin_entity_rename(2).unwrap();
        session
            .update_entity_rename("new_relay".into(), false, true)
            .unwrap();
        assert!(!session.rename_draft().unwrap().accept_review);
        assert!(session.commit_entity_rename(Actor::Agent).is_err());
        assert_eq!(session.document.vmf(), &original);
        session
            .update_entity_rename("new_relay".into(), false, true)
            .unwrap();
        assert!(session.rename_draft().unwrap().accept_review);
        session
            .update_entity_rename("new_relay".into(), true, true)
            .unwrap();
        assert!(!session.rename_draft().unwrap().accept_review);
        session
            .update_entity_rename("new_relay".into(), true, true)
            .unwrap();
        session.commit_entity_rename(Actor::User).unwrap();
        assert_eq!(
            session.document.entity(3).unwrap().properties.get("notes"),
            Some("relay")
        );
    }
}
