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
    pub batch: Option<Vec<(u32, String)>>,
}

impl Session {
    pub fn begin_entity_rename_batch(&mut self, entities: &[u32]) -> Result<u64, String> {
        if self.document.vmf().entities.len() > references::MAX_BULK_RENAME_MAP_ENTITIES {
            return Err(format!(
                "Bulk rename admits maps with at most {} entities",
                references::MAX_BULK_RENAME_MAP_ENTITIES
            ));
        }
        let entities = if entities.is_empty() {
            self.selection
                .iter()
                .map(|id| match id {
                    map_core::ObjectId::Entity(id) => Ok(*id),
                    _ => Err("Select complete entities for bulk rename".to_owned()),
                })
                .collect::<Result<Vec<_>, _>>()?
        } else {
            entities.to_vec()
        };
        if entities.is_empty()
            || entities.len() > references::MAX_BULK_RENAME_ENTITIES
            || entities
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != entities.len()
        {
            return Err(format!(
                "Capture 1–{} distinct complete entities",
                references::MAX_BULK_RENAME_ENTITIES
            ));
        }
        let names = entities
            .iter()
            .map(|id| {
                let entity = self
                    .document
                    .entity(*id)
                    .ok_or_else(|| format!("Entity {id} no longer exists"))?;
                let name = entity.properties.get("targetname").unwrap_or_default();
                if name.len() > map_core::repeat::MAX_NAME_BYTES {
                    return Err(format!(
                        "Entity {id} name exceeds the {}-byte bulk rename limit",
                        map_core::repeat::MAX_NAME_BYTES
                    ));
                }
                Ok((*id, name.to_owned()))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let token = self
            .rename_serial
            .checked_add(1)
            .ok_or("Rename draft token capacity exhausted")?;
        self.rename_draft = Some(RenameDraft {
            token,
            entity: names[0].0,
            new_name: names[0].1.clone(),
            rename_group: false,
            accept_review: false,
            show_details: true,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions_serial: self.definitions_serial,
            batch: Some(names),
        });
        self.rename_serial = token;
        Ok(token)
    }

    fn checked_rename_batch(&self, token: u64) -> Result<&[(u32, String)], String> {
        self.check_rename_draft(token)?;
        let draft = self.rename_draft.as_ref().unwrap();
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
            || draft.definitions_serial != self.definitions_serial
        {
            return Err(
                "Map or definitions changed; cancel and capture the rename set again".into(),
            );
        }
        draft
            .batch
            .as_deref()
            .ok_or("The active rename draft is not a batch".into())
    }

    pub fn update_entity_rename_batch(
        &mut self,
        token: u64,
        names: &[(u32, String)],
    ) -> Result<u64, String> {
        let captured = self.checked_rename_batch(token)?;
        if names.len() != captured.len()
            || names
                .iter()
                .any(|(_, name)| name.len() > map_core::repeat::MAX_NAME_BYTES)
        {
            return Err(format!(
                "Supply every captured name, each at most {} UTF-8 bytes",
                map_core::repeat::MAX_NAME_BYTES
            ));
        }
        let requested = names
            .iter()
            .cloned()
            .collect::<std::collections::BTreeMap<_, _>>();
        if requested.len() != names.len()
            || names.len() != captured.len()
            || captured.iter().any(|(id, _)| !requested.contains_key(id))
        {
            return Err("Supply exactly one proposed name for every captured entity".into());
        }
        let names = captured
            .iter()
            .map(|(id, _)| (*id, requested[id].clone()))
            .collect::<Vec<_>>();
        if names == captured {
            return Ok(token);
        }
        let token = self
            .rename_serial
            .checked_add(1)
            .ok_or("Rename draft token capacity exhausted")?;
        let draft = self.rename_draft.as_mut().unwrap();
        draft.token = token;
        draft.batch = Some(names);
        self.rename_serial = token;
        Ok(token)
    }

    pub fn entity_rename_batch_preview(
        &self,
        token: u64,
    ) -> Result<references::BulkRenamePlan, String> {
        references::plan_many(
            &self.document,
            &self.definitions,
            self.checked_rename_batch(token)?,
        )
    }

    pub fn commit_entity_rename_batch(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<references::BulkRenamePlan, String> {
        let plan = self.entity_rename_batch_preview(token)?;
        if let Some(error) = &plan.error {
            return Err(error.clone());
        }
        if !plan
            .names
            .iter()
            .any(|name| name.old_name.as_deref() != Some(name.new_name.as_str()))
        {
            return Err("Enter at least one changed name before Apply renames".into());
        }
        self.edit(actor, "rename entity names and references", |tx| {
            references::apply_many(tx, &plan)
        })?;
        self.cancel_entity_rename();
        Ok(plan)
    }

    pub fn cancel_entity_rename_batch(&mut self, token: u64) -> Result<(), String> {
        self.check_rename_draft(token)?;
        if self.rename_draft.as_ref().unwrap().batch.is_none() {
            return Err("The active rename draft is not a batch".into());
        }
        self.cancel_entity_rename();
        Ok(())
    }

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
            batch: None,
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
        if draft.batch.is_some() {
            return Err("A batch rename is active; use the captured batch controls".into());
        }
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
        if draft.batch.is_some() {
            return Err("A batch rename is active; inspect the batch preview".into());
        }
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
