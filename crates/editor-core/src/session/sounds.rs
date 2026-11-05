use std::collections::{BTreeMap, BTreeSet};

use compiler::asset_sources::{BuildMaterialSources, CapturedBuildAsset};
use map_core::ObjectId;
use serde_json::{Value, json};
use source_formats::sound;

use super::{Actor, Session};

mod audio;
mod catalog;

impl State {
    pub(super) fn has_draft(&self) -> bool {
        self.draft.is_some()
    }

    pub(super) fn clear_document_draft(&mut self) {
        self.draft = None;
    }

    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.draft.as_ref().map(|draft| {
            serde_json::json!([
                self.serial,
                draft.token,
                draft.entities,
                draft.key,
                draft
                    .choice
                    .as_ref()
                    .map(|choice| (&choice.value, choice.variant))
            ])
        })
    }
}

impl State {
    pub(super) fn close_playback(&self) -> serde_json::Value {
        serde_json::json!({
            "sequence": self.playback_serial,
            "playback": self.playback.as_ref().map(audio::Playback::close_identity),
            "volume": self.volume,
        })
    }
}

const MAX_AUDIO_BYTES: usize = 16 * 1024 * 1024;
const MAX_FIELDS: usize = 128;

type EntityChanges = Vec<(u32, Vec<(String, Option<String>)>)>;

pub struct Preview {
    pub bytes: Vec<u8>,
    pub report: Value,
}

pub(super) struct State {
    catalog: Option<catalog::Catalog>,
    assets: u64,
    serial: u64,
    draft: Option<Draft>,
    playback: Option<audio::Playback>,
    playback_serial: u64,
    volume: f32,
}

impl Default for State {
    fn default() -> Self {
        Self {
            catalog: None,
            assets: 0,
            serial: 0,
            draft: None,
            playback: None,
            playback_serial: 0,
            volume: 0.5,
        }
    }
}

struct Draft {
    token: u64,
    entities: Vec<u32>,
    key: Option<String>,
    fields: Vec<Field>,
    generation: u64,
    revision: u64,
    definitions: u64,
    scene: u64,
    assets: u64,
    content: u64,
    choice: Option<Choice>,
}

#[derive(Clone, serde::Serialize)]
struct Field {
    key: String,
    label: String,
}

#[derive(Clone)]
struct Choice {
    value: String,
    variant: usize,
    report: Value,
    sources: BuildMaterialSources,
    sequence: u64,
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
    fn sound_catalog(&mut self) -> Result<&catalog::Catalog, String> {
        if self.sounds.catalog.is_none() {
            self.sounds.catalog = Some(catalog::load(&self.game)?);
        }
        Ok(self.sounds.catalog.as_ref().unwrap())
    }

    pub fn browse_sounds(
        &mut self,
        query: &str,
        group: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        if query.len() > 256 || group.len() > 1024 || !(1..=100).contains(&limit) {
            return Err("Use a query up to 256 bytes and a page of 1 to 100 sounds".into());
        }
        if !["all", "files", "scripts", "used"].contains(&group)
            && !group.starts_with("folder:")
            && !group.starts_with("script:")
        {
            return Err("Choose all, files, scripts, used, folder:… or script:…".into());
        }
        let used = self
            .document
            .vmf()
            .entities
            .iter()
            .flat_map(|entity| {
                let class = entity
                    .classname()
                    .and_then(|name| self.definitions.class(name));
                entity
                    .properties
                    .iter()
                    .filter(move |(key, _)| {
                        class
                            .and_then(|class| class.property(key))
                            .is_some_and(|property| property.kind == "sound")
                    })
                    .map(|(_, value)| value.to_ascii_lowercase())
            })
            .collect::<BTreeSet<_>>();
        let catalog = self.sound_catalog()?;
        let query = query.to_ascii_lowercase();
        let words = query.split_whitespace().collect::<Vec<_>>();
        let rows = catalog
            .entries
            .iter()
            .filter(|entry| {
                let text = format!("{} {}", entry.value, entry.source.as_deref().unwrap_or(""))
                    .to_ascii_lowercase();
                words.iter().all(|word| text.contains(word))
            })
            .collect::<Vec<_>>();
        let mut folders = BTreeMap::<String, usize>::new();
        for entry in &rows {
            *folders.entry(entry.group()).or_default() += 1;
        }
        let mut groups = vec![
            json!({"id":"all","label":"All sounds","count":rows.len()}),
            json!({"id":"files","label":"Sound files","count":rows.iter().filter(|entry|entry.kind()=="file").count()}),
            json!({"id":"scripts","label":"Sound scripts","count":rows.iter().filter(|entry|entry.kind()=="script").count()}),
            json!({"id":"used","label":"Used in this map","count":rows.iter().filter(|entry|used.contains(&entry.value.to_ascii_lowercase())).count()}),
        ];
        groups.extend(folders.iter().map(
            |(id, count)| json!({"id":id,"label":id.split_once(':').unwrap().1,"count":count}),
        ));
        let selected = rows
            .into_iter()
            .filter(|entry| match group {
                "all" => true,
                "files" => entry.kind() == "file",
                "scripts" => entry.kind() == "script",
                "used" => used.contains(&entry.value.to_ascii_lowercase()),
                _ => entry.group() == group,
            })
            .collect::<Vec<_>>();
        let entries=selected.iter().skip(offset).take(limit).map(|entry|json!({"value":entry.value,"kind":entry.kind(),"source":entry.source,"variants":entry.cue.as_ref().map_or(1,|cue|cue.waves.len()),"ambiguous":catalog.ambiguous.contains(&entry.value.to_ascii_lowercase())})).collect::<Vec<_>>();
        Ok(
            json!({"groups":groups,"entries":entries,"total":selected.len(),"offset":offset,"limit":limit,"warnings":catalog.warnings}),
        )
    }

    fn sound_entry(&mut self, value: &str) -> Result<catalog::Entry, String> {
        if value.len() > 1024 {
            return Err("Sound value exceeds 1024 bytes".into());
        }
        let value = value.trim();
        if sound::audio_path(value).is_ok() {
            return Ok(catalog::Entry {
                value: value.into(),
                source: None,
                cue: None,
            });
        }
        self.sound_catalog()?;
        let catalog = self.sounds.catalog.as_ref().unwrap();
        catalog.verify(&self.game)?;
        let key = value.to_ascii_lowercase();
        if catalog.ambiguous.contains(&key) {
            return Err(
                "This sound-script name has multiple definitions; choose a raw sound file instead"
                    .into(),
            );
        }
        catalog.aliases.get(&key).map(|&index|catalog.entries[index].clone()).ok_or("Sound script was not found in the active manifest; choose a mounted WAV/MP3 path or listed script name".into())
    }

    fn sound_data(
        &mut self,
        value: &str,
        variant: usize,
        sources: &BuildMaterialSources,
    ) -> Result<(Value, CapturedBuildAsset), String> {
        let entry = self.sound_entry(value)?;
        let wave = entry.wave(variant)?;
        let path = sound::audio_path(wave).map_err(|error| error.to_string())?;
        let data = sources
            .read_asset(&self.game, &format!("sound/{path}"), MAX_AUDIO_BYTES)
            .map_err(|error| {
                format!("Sound source changed or is unavailable; inspect the sound again: {error}")
            })?
            .ok_or_else(|| format!("Sound file sound/{path} is missing from the content mounts"))?;
        let info = sound::inspect_audio(&path, data.bytes()).map_err(|error| error.to_string())?;
        catalog::verify_sources(sources, &self.game)?;
        let cue = entry.cue.as_ref();
        let origin = crate::materials::provenance(data.origin().clone());
        let report = json!({"value":entry.value,"kind":entry.kind(),"source":entry.source,"wave":wave,"path":path,"variant":variant,"waves":cue.map_or_else(||vec![entry.value.clone()],|cue|cue.waves.clone()),"codec":info.codec,"channels":info.channels,"sample_rate":info.sample_rate,"duration":info.duration,"encoded_bytes":data.bytes().len(),"channel":cue.and_then(|cue|cue.channel.as_deref()),"volume":cue.and_then(|cue|cue.volume.as_deref()),"pitch":cue.and_then(|cue|cue.pitch.as_deref()),"sound_level":cue.and_then(|cue|cue.sound_level.as_deref()),"audio_source_id":origin.source_id,"audio_source":origin.source,"source_checks":"Selected wave bytes and origin are captured and checked at Choose and Apply. Script choices also check the original manifest/script catalog. Inspection of an active choice uses its captured report."});
        Ok((report, data))
    }

    pub fn inspect_sound(&mut self, value: &str, variant: usize) -> Result<Value, String> {
        self.sound_data(value, variant, &catalog::source_capture(MAX_AUDIO_BYTES, 1))
            .map(|(report, _)| report)
    }

    fn checked_sound_choice(&self, token: u64) -> Result<&Draft, String> {
        let draft = self
            .sounds
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Sound choice was closed or replaced")?;
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
            || draft.definitions != self.definitions_serial
            || draft.scene != self.scene_serial
            || draft.assets != self.sounds.assets
            || draft.content != self.content_epoch
            || !std::ptr::eq(self.render_document(), &self.document)
        {
            return Err("Map, selection, definitions, sounds or geometry preview changed; reopen the sound browser".into());
        }
        Ok(draft)
    }

    fn sound_choice_fields(&self, entities: &[u32]) -> Result<Vec<Field>, String> {
        let mut fields = BTreeMap::<String, Field>::new();
        for (index, &id) in entities.iter().enumerate() {
            let target = self.document.entity(id).ok_or("Entity is missing")?;
            if target.properties.get_all("classname").count() != 1 {
                return Err(format!("entity:{id}: the classname must be unambiguous"));
            }
            let class = target
                .classname()
                .and_then(|name| self.definitions.class(name))
                .ok_or_else(|| {
                    format!("entity:{id}: load its FGD definition before choosing a sound")
                })?;
            if index == 0 {
                for property in &class.properties {
                    if property.kind == "sound" && !property.read_only {
                        fields
                            .entry(property.name.to_ascii_lowercase())
                            .or_insert_with(|| Field {
                                key: property.name.clone(),
                                label: property.label.clone(),
                            });
                    }
                }
                if fields.len() > MAX_FIELDS {
                    return Err(format!(
                        "Sound field selection exceeds {MAX_FIELDS} properties"
                    ));
                }
            }
            fields.retain(|_, field| {
                class
                    .property(&field.key)
                    .is_some_and(|property| property.kind == "sound" && !property.read_only)
            });
        }
        if fields.is_empty() {
            return Err("Captured entities have no common writable FGD sound field".into());
        }
        Ok(fields.into_values().collect())
    }

    pub fn begin_sound_choice(
        &mut self,
        entity: u32,
        key: &str,
        resume: bool,
    ) -> Result<Value, String> {
        self.begin_sound_choices(&[entity], Some(key), resume)
    }

    pub fn begin_sound_choices(
        &mut self,
        entities: &[u32],
        key: Option<&str>,
        resume: bool,
    ) -> Result<Value, String> {
        if !std::ptr::eq(self.render_document(), &self.document) {
            return Err("Apply or cancel the active geometry preview first".into());
        }
        let ids = entities
            .iter()
            .copied()
            .map(ObjectId::Entity)
            .collect::<Vec<_>>();
        let entities = self.whole_entity_targets(&ids)?;
        let fields = self.sound_choice_fields(&entities)?;
        let key = match key {
            Some(key) => Some(
                fields
                    .iter()
                    .find(|field| field.key.eq_ignore_ascii_case(key))
                    .ok_or("Choose a common writable sound field from available_fields")?
                    .key
                    .clone(),
            ),
            None if fields.len() == 1 => Some(fields[0].key.clone()),
            None => None,
        };
        if let Some(key) = &key {
            self.check_sound_field(&entities, key)?;
        }
        if resume
            && let Some(draft) = &self.sounds.draft
            && draft.entities == entities
            && (key.is_none() || draft.key == key)
            && self.checked_sound_choice(draft.token).is_ok()
        {
            return self.sound_choice(draft.token);
        }
        let token = self
            .sounds
            .serial
            .checked_add(1)
            .ok_or("Sound choice token capacity exhausted")?;
        self.sounds.serial = token;
        self.sounds.draft = Some(Draft {
            token,
            entities,
            key,
            fields,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            scene: self.scene_serial,
            assets: self.sounds.assets,
            content: self.content_epoch,
            choice: None,
        });
        self.view_serial += 1;
        self.sound_choice(token)
    }

    pub fn set_sound_choice_field(&mut self, token: u64, key: &str) -> Result<Value, String> {
        let draft = self.checked_sound_choice(token)?;
        let key = draft
            .fields
            .iter()
            .find(|field| field.key.eq_ignore_ascii_case(key))
            .ok_or("Choose a common writable sound field from available_fields")?
            .key
            .clone();
        self.check_sound_field(&draft.entities, &key)?;
        if draft.key.as_ref() == Some(&key) {
            return self.sound_choice(token);
        }
        let token = self
            .sounds
            .serial
            .checked_add(1)
            .ok_or("Sound choice token capacity exhausted")?;
        let draft = self.sounds.draft.as_mut().unwrap();
        draft.token = token;
        draft.key = Some(key);
        draft.choice = None;
        self.sounds.serial = token;
        self.view_serial += 1;
        self.sound_choice(token)
    }

    fn check_sound_field(&self, entities: &[u32], key: &str) -> Result<(), String> {
        for &id in entities {
            let target = self.document.entity(id).ok_or("Entity is missing")?;
            let property = target
                .classname()
                .and_then(|name| self.definitions.class(name))
                .and_then(|class| class.property(key));
            if !property.is_some_and(|property| property.kind == "sound" && !property.read_only) {
                return Err(format!(
                    "entity:{id}: {key} is not a writable FGD sound field"
                ));
            }
            if target.properties.get_all(key).count() > 1 {
                return Err(format!(
                    "entity:{id}: sound field {key} is duplicated; resolve the authored values first"
                ));
            }
        }
        Ok(())
    }

    fn checked_sound_changes(
        &self,
        entities: &[u32],
        key: &str,
        value: &str,
    ) -> Result<EntityChanges, String> {
        self.check_sound_field(entities, key)?;
        let changes = [(key.to_owned(), Some(value.to_owned()))];
        entities
            .iter()
            .map(|&id| {
                self.checked_entity_property_changes(id, &changes)
                    .map(|changes| (id, changes))
                    .map_err(|error| format!("entity:{id}: {error}"))
            })
            .collect()
    }

    pub fn choose_sound(
        &mut self,
        token: u64,
        value: &str,
        variant: usize,
    ) -> Result<Value, String> {
        let draft = self.checked_sound_choice(token)?;
        let (entities, key) = (
            draft.entities.clone(),
            draft.key.clone().ok_or("Choose a sound field first")?,
        );
        let sources = catalog::source_capture(MAX_AUDIO_BYTES, 1);
        let (report, _) = self.sound_data(value, variant, &sources)?;
        self.checked_sound_changes(&entities, &key, report["value"].as_str().unwrap())?;
        let sequence = self
            .sounds
            .serial
            .checked_add(1)
            .ok_or("Sound choice sequence capacity exhausted")?;
        self.sounds.serial = sequence;
        self.sounds.draft.as_mut().unwrap().choice = Some(Choice {
            value: report["value"].as_str().unwrap().into(),
            variant,
            report,
            sources,
            sequence,
        });
        self.view_serial += 1;
        self.sound_choice(token)
    }

    pub fn sound_choice(&mut self, token: u64) -> Result<Value, String> {
        let draft = self.checked_sound_choice(token)?;
        let sources = draft
            .entities
            .iter()
            .map(|&id| {
                let target = self.document.entity(id).unwrap();
                json!({"entity":format!("entity:{id}"),"classname":target.classname(),
                "value":draft.key.as_ref().and_then(|key| target.properties.get(key))})
            })
            .collect::<Vec<_>>();
        let mixed = sources
            .iter()
            .any(|source| source["value"] != sources[0]["value"]);
        let value = draft.choice.as_ref().map_or_else(
            || {
                if mixed {
                    Value::Null
                } else {
                    sources[0]["value"].clone()
                }
            },
            |choice| json!(choice.value),
        );
        let label = draft
            .fields
            .iter()
            .find(|field| Some(&field.key) == draft.key.as_ref())
            .map(|field| field.label.as_str());
        let title = if draft.entities.len() == 1 {
            self.document
                .entity(draft.entities[0])
                .and_then(|target| target.classname())
                .map(crate::inspector::title)
                .unwrap_or("Entity".into())
        } else {
            format!("{} captured entities", draft.entities.len())
        };
        Ok(json!({"token":token,"valid":true,
            "state":if draft.key.is_none() {"choose_field"} else if draft.choice.is_some() {"chosen"} else {"captured"},
            "entity":if draft.entities.len() == 1 {Some(format!("entity:{}",draft.entities[0]))} else {None},
            "entities":draft.entities.iter().map(|id|format!("entity:{id}")).collect::<Vec<_>>(),
            "count":draft.entities.len(),"key":draft.key,"available_fields":draft.fields,"label":label,"title":title,
            "value":value,"mixed":mixed,"source_fields":sources,"sound":draft.choice.as_ref().map(|choice|&choice.report),
            "choice_sequence":draft.choice.as_ref().map(|choice|choice.sequence),
            "document_changed":false,"source_checks":"Choose and Apply check captured audio bytes/origin and script catalog sources; inspecting this draft uses captured metadata."}))
    }

    pub fn current_sound_choice(&mut self) -> Value {
        let Some(draft) = &self.sounds.draft else {
            return json!({"token":null,"valid":false,"state":"closed"});
        };
        let token = draft.token;
        self.sound_choice(token).unwrap_or_else(
            |error| json!({"token":token,"valid":false,"state":"stale","error":error}),
        )
    }

    pub fn cancel_sound_choice(&mut self, token: u64) -> Result<(), String> {
        if !self
            .sounds
            .draft
            .as_ref()
            .is_some_and(|draft| draft.token == token)
        {
            return Err("Sound choice was closed or replaced".into());
        }
        self.sounds.draft = None;
        self.view_serial += 1;
        Ok(())
    }

    pub fn apply_sound_choice(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self.checked_sound_choice(token)?;
        let (entities, key, choice) = (
            draft.entities.clone(),
            draft.key.clone().ok_or("Choose a sound field first")?,
            draft.choice.clone().ok_or("Choose a sound first")?,
        );
        let (report, _) = self.sound_data(&choice.value, choice.variant, &choice.sources)?;
        if report != choice.report {
            return Err("Sound metadata changed; inspect the sound again before applying".into());
        }
        let changes = self.checked_sound_changes(&entities, &key, &choice.value)?;
        self.edit(actor, "choose sound for entities", |tx| {
            for (id, changes) in &changes {
                map_core::edit::set_properties(tx, *id, changes)?;
            }
            Ok(())
        })?;
        self.sounds.draft = None;
        self.view_serial += 1;
        Ok(
            json!({"entity":if entities.len() == 1 {Some(format!("entity:{}",entities[0]))} else {None},
            "entities":entities.iter().map(|id|format!("entity:{id}")).collect::<Vec<_>>(),
            "count":entities.len(),"key":key,"value":choice.value,"state":"applied"}),
        )
    }

    pub fn sound_preview_volume(&self) -> f32 {
        self.sounds.volume
    }

    pub fn set_sound_preview_volume(&mut self, volume: f32) -> Result<(), String> {
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            return Err("Preview volume must be between 0 and 1".into());
        }
        self.sounds.volume = volume;
        self.view_serial += 1;
        Ok(())
    }

    pub fn render_sound_preview(
        &mut self,
        value: &str,
        variant: usize,
        volume: Option<f32>,
    ) -> Result<Preview, String> {
        let volume = volume.unwrap_or(self.sounds.volume);
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            return Err("Preview volume must be between 0 and 1".into());
        }
        let (mut report, data) =
            self.sound_data(value, variant, &catalog::source_capture(MAX_AUDIO_BYTES, 1))?;
        let bytes = audio::decode(report["path"].as_str().unwrap(), data.bytes(), volume)?;
        let info =
            sound::inspect_audio("preview.wav", &bytes).map_err(|error| error.to_string())?;
        report["preview"] = json!({"volume":volume,"duration":info.duration,"channels":info.channels,"sample_rate":info.sample_rate,"mime_type":"audio/wav","bytes":bytes.len(),"maximum_duration":audio::MAX_SECONDS});
        Ok(Preview { report, bytes })
    }

    pub fn play_sound_preview(
        &mut self,
        value: &str,
        variant: usize,
        volume: Option<f32>,
    ) -> Result<Value, String> {
        if self.closing {
            return Err("Editor is closing; sound previews are unavailable".into());
        }
        let preview = self.render_sound_preview(value, variant, volume)?;
        self.sounds.playback_serial += 1;
        let playback =
            audio::Playback::start(self.sounds.playback_serial, preview.report, &preview.bytes)?;
        self.sounds.playback = Some(playback);
        self.view_serial += 1;
        Ok(self.sound_preview_status())
    }

    pub fn sound_preview_status(&mut self) -> Value {
        let mut report = self.sounds.playback.as_mut().map_or_else(
            || json!({"token":null,"state":"idle"}),
            audio::Playback::status,
        );
        report["volume"] = json!(self.sounds.volume);
        report
    }

    pub fn stop_sound_preview(&mut self, token: u64) -> Result<(), String> {
        if !self
            .sounds
            .playback
            .as_ref()
            .is_some_and(|playback| playback.token == token)
        {
            return Err("Sound preview was stopped or replaced".into());
        }
        self.sounds.playback = None;
        self.view_serial += 1;
        Ok(())
    }

    pub fn reload_sound_assets(&mut self) {
        self.sounds.catalog = None;
        self.sounds.assets += 1;
        self.sounds.playback = None;
        self.view_serial += 1;
    }

    pub(super) fn shutdown_sound_preview(&mut self) {
        self.sounds.playback = None;
    }
}
