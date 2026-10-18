use super::{Actor, Session};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

mod catalog;
#[cfg(any(test, feature = "test-support"))]
pub(crate) mod fixtures;

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.draft.as_ref().map(|draft| {
            serde_json::json!([
                self.serial,
                self.assets,
                draft.token,
                draft
                    .choice
                    .as_ref()
                    .and_then(|choice| choice["name"].as_str())
            ])
        })
    }
}

#[cfg(test)]
mod tests;

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
    entity: u32,
    generation: u64,
    revision: u64,
    definitions: u64,
    scene: u64,
    assets: u64,
    choice: Option<Value>,
}

impl State {
    pub(super) fn materials_reloaded(&mut self, previous: u64, current: u64) {
        if let Some(draft) = &mut self.draft
            && draft.scene == previous
        {
            draft.scene = current;
        }
    }
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
            || !std::ptr::eq(self.render_document(), &self.document)
        {
            return Err("Map, selection, definitions, particles or geometry preview changed; reopen the particle browser".into());
        }
        Ok(draft)
    }

    pub fn begin_particle_choice(&mut self, entity: u32, resume: bool) -> Result<Value, String> {
        if resume
            && let Some(draft) = &self.particles.draft
            && draft.entity == entity
            && self.checked_particle_choice(draft.token).is_ok()
        {
            return self.particle_choice(draft.token);
        }
        if !std::ptr::eq(self.render_document(), &self.document) {
            return Err("Apply or cancel the active geometry preview first".into());
        }
        let target = self.document.entity(entity).ok_or("Entity is missing")?;
        if !target.solids.is_empty()
            || !target
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("info_particle_system"))
        {
            return Err("Choose an info_particle_system point entity".into());
        }
        let property = self
            .definitions
            .class("info_particle_system")
            .and_then(|class| class.property("effect_name"))
            .ok_or("The mounted definitions do not provide info_particle_system.effect_name")?;
        if property.read_only {
            return Err("The particle effect field is read-only".into());
        }
        self.particles.serial += 1;
        let token = self.particles.serial;
        self.particles.draft = Some(Draft {
            token,
            entity,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            scene: self.scene_serial,
            assets: self.particles.assets,
            choice: None,
        });
        self.view_serial += 1;
        self.particle_choice(token)
    }

    pub fn choose_particle(&mut self, token: u64, name: &str) -> Result<Value, String> {
        self.checked_particle_choice(token)?;
        let report = self.inspect_particle(name)?;
        self.particles.draft.as_mut().unwrap().choice = Some(report.clone());
        self.view_serial += 1;
        Ok(json!({"token":token,"valid":true,"particle":report}))
    }

    pub fn particle_choice(&self, token: u64) -> Result<Value, String> {
        let draft = self.checked_particle_choice(token)?;
        let target = self.document.entity(draft.entity).unwrap();
        let source = target.properties.get("effect_name").unwrap_or("");
        let label = target
            .classname()
            .and_then(|class| self.definitions.class(class))
            .and_then(|class| class.property("effect_name"))
            .map_or("Particle effect", |property| property.label.as_str());
        Ok(
            json!({"token":token,"valid":true,"entity":format!("entity:{}",draft.entity),"key":"effect_name","title":"Particle system","label":label,"value":draft.choice.as_ref().and_then(|choice|choice["name"].as_str()).unwrap_or(source),"particle":draft.choice,"limits":LIMITS}),
        )
    }

    pub fn current_particle_choice(&self) -> Value {
        let Some(draft) = &self.particles.draft else {
            return json!({"token":null,"valid":false});
        };
        self.particle_choice(draft.token)
            .unwrap_or_else(|error| json!({"token":draft.token,"valid":false,"error":error}))
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
        let draft = self.checked_particle_choice(token)?;
        let entity = draft.entity;
        let choice = draft
            .choice
            .clone()
            .ok_or("Choose a particle system first")?;
        let name = choice["name"].as_str().unwrap().to_owned();
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
        self.set_entity_properties(actor, entity, &[("effect_name".into(), Some(name.clone()))])?;
        self.particles.draft = None;
        self.view_serial += 1;
        Ok(json!({"entity":format!("entity:{entity}"),"key":"effect_name","value":name}))
    }

    pub fn reload_particle_assets(&mut self) {
        self.particles.catalog = None;
        self.particles.assets += 1;
        self.view_serial += 1;
    }
}
