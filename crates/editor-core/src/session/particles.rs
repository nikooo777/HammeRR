use super::{Actor, Session};
use map_core::ObjectId;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

mod catalog;

const MAX_CHOICE_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
type EntityChanges = Vec<(u32, Vec<(String, Option<String>)>)>;

impl State {
    pub(super) fn has_draft(&self) -> bool {
        self.draft.is_some()
    }

    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.draft.as_ref().map(|draft| {
            serde_json::json!([
                self.serial,
                self.assets,
                draft.token,
                draft.entities,
                draft
                    .choice
                    .as_ref()
                    .and_then(|choice| choice["name"].as_str())
            ])
        })
    }
}

pub(super) const LIMITS: &str = "Definition metadata only; no animation or viewport preview. Global active particle manifest only; map manifests, includes, external element references and other DMX encodings/versions are unsupported.";

#[derive(Default)]
pub(super) struct State {
    catalog: Option<catalog::Catalog>,
    assets: u64,
    serial: u64,
    draft: Option<Draft>,
}

struct Draft {
    token: u64,
    entities: Vec<u32>,
    generation: u64,
    revision: u64,
    definitions: u64,
    scene: u64,
    assets: u64,
    content: u64,
    choice: Option<Value>,
}

impl State {
    pub(super) fn clear_document_draft(&mut self) {
        self.draft = None;
    }

    pub(super) fn materials_reloaded(&mut self, previous: u64, current: u64) {
        if let Some(draft) = &mut self.draft
            && draft.scene == previous
        {
            draft.scene = current;
        }
    }
}

fn target_report(token: u64, entities: &[u32]) -> Value {
    json!({"token":token,
        "entity":(entities.len()==1).then(||format!("entity:{}",entities[0])),
        "entities":entities.iter().map(|id|format!("entity:{id}")).collect::<Vec<_>>(),
        "count":entities.len()})
}

impl Session {
    fn particle_catalog(&mut self) -> Result<&catalog::Catalog, String> {
        if self.particles.catalog.is_none() {
            self.particles.catalog = Some(catalog::load(&self.game)?);
        }
        Ok(self.particles.catalog.as_ref().unwrap())
    }

    pub fn browse_particles(
        &mut self,
        query: &str,
        group: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        if query.len() > 256 || group.len() > 1024 || !(1..=100).contains(&limit) {
            return Err(
                "Use a query up to 256 bytes and a page of 1 to 100 particle systems".into(),
            );
        }
        if !["all", "used"].contains(&group)
            && !group.starts_with("folder:")
            && !group.starts_with("file:")
        {
            return Err("Choose all, used, folder:particles/… or file:particles/….pcf".into());
        }
        let used = self
            .document
            .vmf()
            .entities
            .iter()
            .filter(|entity| {
                entity
                    .classname()
                    .is_some_and(|class| class.eq_ignore_ascii_case("info_particle_system"))
            })
            .filter_map(|entity| entity.properties.get("effect_name"))
            .map(str::to_ascii_lowercase)
            .collect::<BTreeSet<_>>();
        let catalog = self.particle_catalog()?;
        let query = query.to_ascii_lowercase();
        let words = query.split_whitespace().collect::<Vec<_>>();
        let rows = catalog
            .entries
            .iter()
            .filter(|entry| {
                let text = format!(
                    "{} {} {}",
                    entry.definition.name,
                    entry.source,
                    entry.definition.material.as_deref().unwrap_or("")
                )
                .to_ascii_lowercase();
                words.iter().all(|word| text.contains(word))
            })
            .collect::<Vec<_>>();
        let mut groups = vec![
            json!({"id":"all","label":"All particle systems","count":rows.len()}),
            json!({"id":"used","label":"Used in this map","count":rows.iter().filter(|entry|used.contains(&entry.definition.name.to_ascii_lowercase())).count()}),
        ];
        let mut folders = BTreeMap::<String, usize>::new();
        let mut files = BTreeMap::<String, usize>::new();
        for entry in &rows {
            *folders
                .entry(entry.source.rsplit_once('/').unwrap().0.into())
                .or_default() += 1;
            *files.entry(entry.source.clone()).or_default() += 1;
        }
        groups.extend(folders.into_iter().map(|(folder,count)| json!({"id":format!("folder:{folder}"),"label":format!("Folder · {folder}"),"count":count})));
        groups.extend(
            files.into_iter().map(
                |(file, count)| json!({"id":format!("file:{file}"),"label":file,"count":count}),
            ),
        );
        let selected = rows
            .into_iter()
            .filter(|entry| match group {
                "all" => true,
                "used" => used.contains(&entry.definition.name.to_ascii_lowercase()),
                group if group.starts_with("folder:") => {
                    entry.source.rsplit_once('/').unwrap().0 == &group[7..]
                }
                _ => entry.source == group[5..],
            })
            .collect::<Vec<_>>();
        let entries = selected.iter().skip(offset).take(limit).map(|entry|json!({"name":entry.definition.name,"source":entry.source,"material":entry.definition.material,"precache":entry.precache,"ambiguous":catalog.ambiguous.contains(&entry.definition.name.to_ascii_lowercase())})).collect::<Vec<_>>();
        Ok(
            json!({"entries":entries,"groups":groups,"total":selected.len(),"offset":offset,"limit":limit,"manifest":"particles/particles_manifest.txt","files":catalog.files,"limits":LIMITS}),
        )
    }

    fn particle_entry(&mut self, name: &str) -> Result<catalog::Entry, String> {
        if name.is_empty() || name.len() > 255 || name.contains(['\"', '\0', '\r', '\n']) {
            return Err("Choose a listed particle system name up to 255 bytes".into());
        }
        let catalog = self.particle_catalog()?;
        let key = name.to_ascii_lowercase();
        if catalog.ambiguous.contains(&key) {
            return Err("This particle name has multiple definitions in the active manifest; resolve the duplicate definitions before choosing it".into());
        }
        catalog
            .aliases
            .get(&key)
            .map(|index| catalog.entries[*index].clone())
            .ok_or_else(|| "Particle system was not found in the global active manifest".into())
    }

    fn particle_report(entry: &catalog::Entry) -> Value {
        json!({"name":entry.definition.name,"source":entry.source,"precache":entry.precache,"material":entry.definition.material,"max_particles":entry.definition.max_particles,"functions":entry.definition.functions,"children":entry.definition.children,"encoded_bytes":entry.encoded_bytes,"elements":entry.elements,"fingerprint":format!("{:016x}",entry.fingerprint),"encoding":"binary","encoding_version":2,"format":"pcf","format_version":1,"limits":LIMITS})
    }

    pub fn inspect_particle(&mut self, name: &str) -> Result<Value, String> {
        self.particle_entry(name)
            .map(|entry| Self::particle_report(&entry))
    }

    fn checked_particle_choice(&self, token: u64) -> Result<&Draft, String> {
        let draft = self
            .particles
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Particle choice was closed or replaced")?;
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
            || draft.definitions != self.definitions_serial
            || draft.scene != self.scene_serial
            || draft.assets != self.particles.assets
            || draft.content != self.content_epoch
            || !std::ptr::eq(self.render_document(), &self.document)
        {
            return Err("Map, selection, definitions, content, particles or geometry preview changed; reopen the particle browser".into());
        }
        Ok(draft)
    }

    pub fn begin_particle_choice(&mut self, entity: u32, resume: bool) -> Result<Value, String> {
        self.begin_particle_choices(&[entity], resume)
    }

    fn particle_choice_targets(&self, entities: &[u32]) -> Result<Vec<u32>, String> {
        if entities.len() > super::bulk_entities::MAX_BULK_ENTITIES {
            return Err(format!(
                "Choose at most {} whole entities",
                super::bulk_entities::MAX_BULK_ENTITIES
            ));
        }
        let ids = entities
            .iter()
            .copied()
            .map(ObjectId::Entity)
            .collect::<Vec<_>>();
        let entities = self.whole_entity_targets(&ids)?;
        let mut errors = Vec::new();
        for &id in &entities {
            let target = self.document.entity(id).ok_or("Entity is missing")?;
            let checked = (|| {
                if !target.solids.is_empty()
                    || !target
                        .classname()
                        .is_some_and(|class| class.eq_ignore_ascii_case("info_particle_system"))
                {
                    return Err("choose an info_particle_system point entity".to_owned());
                }
                if ["classname", "effect_name"]
                    .iter()
                    .any(|key| target.properties.get_all(key).take(2).count() > 1)
                {
                    return Err("duplicate classname or effect_name fields must be repaired before choosing particles".into());
                }
                let property = target
                    .classname()
                    .and_then(|name| self.definitions.class(name))
                    .and_then(|class| class.property("effect_name"))
                    .ok_or(
                        "The mounted definitions do not provide info_particle_system.effect_name",
                    )?;
                if property.read_only {
                    return Err("The particle effect field is read-only".into());
                }
                Ok(())
            })();
            if let Err(error) = checked {
                errors.push(format!("entity:{id}: {error}"));
            }
        }
        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        Ok(entities)
    }

    fn checked_particle_changes(
        &self,
        entities: &[u32],
        name: &str,
    ) -> Result<EntityChanges, String> {
        self.particle_choice_targets(entities)?;
        let requested = [("effect_name".to_owned(), Some(name.to_owned()))];
        let mut changes = Vec::with_capacity(entities.len());
        let mut errors = Vec::new();
        for &id in entities {
            match self.checked_entity_property_changes(id, &requested) {
                Ok(checked) => changes.push((id, checked)),
                Err(error) => errors.push(format!("entity:{id}: {error}")),
            }
        }
        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        Ok(changes)
    }

    pub fn begin_particle_choices(
        &mut self,
        entities: &[u32],
        resume: bool,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        let entities = self.particle_choice_targets(entities)?;
        if resume
            && let Some(draft) = &self.particles.draft
            && draft.entities == entities
            && self.checked_particle_choice(draft.token).is_ok()
        {
            return self.particle_choice(draft.token);
        }
        if !std::ptr::eq(self.render_document(), &self.document) {
            return Err("Apply or cancel the active geometry preview first".into());
        }
        map_core::references::admit_snapshot_storage(
            self.document.vmf(),
            MAX_CHOICE_SNAPSHOT_BYTES,
            &mut || Ok(()),
        )?;
        let token = self
            .particles
            .serial
            .checked_add(1)
            .ok_or("Particle choice tokens exhausted")?;
        self.particles.serial = token;
        self.particles.draft = Some(Draft {
            token,
            entities,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            scene: self.scene_serial,
            assets: self.particles.assets,
            content: self.content_epoch,
            choice: None,
        });
        self.view_serial += 1;
        self.particle_choice(token)
    }

    pub fn choose_particle(&mut self, token: u64, name: &str) -> Result<Value, String> {
        self.ensure_not_closing()?;
        let entities = self.checked_particle_choice(token)?.entities.clone();
        let report = self.inspect_particle(name)?;
        self.checked_particle_changes(&entities, report["name"].as_str().unwrap())?;
        self.particles.draft.as_mut().unwrap().choice = Some(report);
        self.view_serial += 1;
        self.particle_choice(token)
    }

    pub fn particle_choice(&self, token: u64) -> Result<Value, String> {
        let draft = self.checked_particle_choice(token)?;
        let label = self
            .definitions
            .class("info_particle_system")
            .and_then(|class| class.property("effect_name"))
            .map_or("Particle effect", |property| property.label.as_str());
        let sources = draft.entities.iter().map(|id| {
            let entity = self.document.entity(*id).unwrap();
            json!({"entity":format!("entity:{id}"),"value":entity.properties.get("effect_name"),
                "hidden":entity.hidden,"editor_hidden":self.editor_object_hidden(ObjectId::Entity(*id))})
        }).collect::<Vec<_>>();
        let common = sources
            .iter()
            .all(|source| source["value"] == sources[0]["value"]);
        let source = if common {
            sources[0]["value"].clone()
        } else {
            Value::Null
        };
        let value = draft.choice.as_ref().map_or_else(
            || {
                if common {
                    json!(source.as_str().unwrap_or(""))
                } else {
                    Value::Null
                }
            },
            |choice| choice["name"].clone(),
        );
        let mut report = target_report(token, &draft.entities);
        report["valid"] = json!(true);
        report["key"] = json!("effect_name");
        report["title"] = json!("Particle system");
        report["label"] = json!(label);
        report["value"] = value;
        report["source_value"] = source;
        report["source_fields"] = json!(sources);
        report["mixed"] = json!({"effect_name":!common});
        report["particle"] = json!(draft.choice);
        report["limits"] = json!(LIMITS);
        Ok(report)
    }

    pub fn current_particle_choice(&self) -> Value {
        let Some(draft) = &self.particles.draft else {
            return json!({"token":null,"valid":false});
        };
        self.particle_choice(draft.token).unwrap_or_else(|error| {
            let mut report = target_report(draft.token, &draft.entities);
            report["valid"] = json!(false);
            report["error"] = json!(error);
            report
        })
    }

    pub fn cancel_particle_choice(&mut self, token: u64) -> Result<(), String> {
        if !self
            .particles
            .draft
            .as_ref()
            .is_some_and(|draft| draft.token == token)
        {
            return Err("Particle choice was closed or replaced".into());
        }
        self.particles.draft = None;
        self.view_serial += 1;
        Ok(())
    }

    pub fn apply_particle_choice(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.ensure_not_closing()?;
        let draft = self.checked_particle_choice(token)?;
        let entities = draft.entities.clone();
        let choice = draft
            .choice
            .clone()
            .ok_or("Choose a particle system first")?;
        let name = choice["name"].as_str().unwrap().to_owned();
        let changes = self.checked_particle_changes(&entities, &name)?;
        let fresh = catalog::load(&self.game)?;
        let key = name.to_ascii_lowercase();
        if fresh.ambiguous.contains(&key) {
            return Err(
                "Particle name became ambiguous; reload particles and reopen the choice".into(),
            );
        }
        let entry = fresh
            .aliases
            .get(&key)
            .map(|index| &fresh.entries[*index])
            .ok_or("Particle system is no longer in the active manifest")?;
        if Self::particle_report(entry) != choice {
            return Err(
                "Particle definition changed; reload particles and reopen the choice".into(),
            );
        }
        self.edit(actor, "choose particle for entities", |tx| {
            for (id, changes) in &changes {
                map_core::edit::set_properties(tx, *id, changes)?;
            }
            Ok(())
        })?;
        self.particles.draft = None;
        self.view_serial += 1;
        let mut report = target_report(token, &entities);
        report["key"] = json!("effect_name");
        report["value"] = json!(name);
        Ok(report)
    }

    pub fn reload_particle_assets(&mut self) {
        self.particles.catalog = None;
        self.particles.assets += 1;
        self.view_serial += 1;
    }
}
