use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use source_formats::sound;

use super::{Actor, Session};

mod audio;
mod catalog;
#[cfg(any(test, feature = "test-support"))]
pub(crate) mod fixtures;

#[cfg(test)]
pub(crate) mod tests;

const MAX_AUDIO_BYTES: usize = 16 * 1024 * 1024;

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
    entity: u32,
    key: String,
    generation: u64,
    revision: u64,
    definitions: u64,
    scene: u64,
    assets: u64,
    choice: Option<Choice>,
}

#[derive(Clone)]
struct Choice {
    value: String,
    variant: usize,
    report: Value,
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
        let catalog = self.sound_catalog()?;
        let key = value.to_ascii_lowercase();
        if catalog.ambiguous.contains(&key) {
            return Err(
                "This sound-script name has multiple definitions; choose a raw sound file instead"
                    .into(),
            );
        }
        catalog.aliases.get(&key).map(|&index|catalog.entries[index].clone()).ok_or("Sound script was not found in the active manifest; choose a mounted WAV/MP3 path or listed script name".into())
    }

    fn sound_data(&mut self, value: &str, variant: usize) -> Result<(Value, Vec<u8>), String> {
        let entry = self.sound_entry(value)?;
        let wave = entry.wave(variant)?;
        let path = sound::audio_path(wave).map_err(|error| error.to_string())?;
        let data = self
            .game
            .read_limited(&format!("sound/{path}"), MAX_AUDIO_BYTES)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Sound file sound/{path} is missing from the content mounts"))?;
        let info = sound::inspect_audio(&path, &data).map_err(|error| error.to_string())?;
        let cue = entry.cue.as_ref();
        let report = json!({"value":entry.value,"kind":entry.kind(),"source":entry.source,"wave":wave,"path":path,"variant":variant,"waves":cue.map_or_else(||vec![entry.value.clone()],|cue|cue.waves.clone()),"codec":info.codec,"channels":info.channels,"sample_rate":info.sample_rate,"duration":info.duration,"encoded_bytes":data.len(),"channel":cue.and_then(|cue|cue.channel.as_deref()),"volume":cue.and_then(|cue|cue.volume.as_deref()),"pitch":cue.and_then(|cue|cue.pitch.as_deref()),"sound_level":cue.and_then(|cue|cue.sound_level.as_deref())});
        Ok((report, data))
    }

    pub fn inspect_sound(&mut self, value: &str, variant: usize) -> Result<Value, String> {
        self.sound_data(value, variant).map(|(report, _)| report)
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
            || !std::ptr::eq(self.render_document(), &self.document)
        {
            return Err("Map, selection, definitions, sounds or geometry preview changed; reopen the sound browser".into());
        }
        Ok(draft)
    }

    pub fn begin_sound_choice(
        &mut self,
        entity: u32,
        key: &str,
        resume: bool,
    ) -> Result<Value, String> {
        if resume
            && let Some(draft) = &self.sounds.draft
            && draft.entity == entity
            && draft.key.eq_ignore_ascii_case(key)
            && self.checked_sound_choice(draft.token).is_ok()
        {
            return self.sound_choice(draft.token);
        }
        if !std::ptr::eq(self.render_document(), &self.document) {
            return Err("Apply or cancel the active geometry preview first".into());
        }
        let target = self.document.entity(entity).ok_or("Entity is missing")?;
        let property = target
            .classname()
            .and_then(|name| self.definitions.class(name))
            .and_then(|class| class.property(key))
            .filter(|property| property.kind == "sound" && !property.read_only)
            .ok_or("Choose a writable sound field from the entity inspector")?;
        let key = property.name.clone();
        self.sounds.serial += 1;
        let token = self.sounds.serial;
        self.sounds.draft = Some(Draft {
            token,
            entity,
            key,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            scene: self.scene_serial,
            assets: self.sounds.assets,
            choice: None,
        });
        self.view_serial += 1;
        self.sound_choice(token)
    }

    pub fn choose_sound(
        &mut self,
        token: u64,
        value: &str,
        variant: usize,
    ) -> Result<Value, String> {
        self.checked_sound_choice(token)?;
        let report = self.inspect_sound(value, variant)?;
        self.sounds.draft.as_mut().unwrap().choice = Some(Choice {
            value: report["value"].as_str().unwrap().into(),
            variant,
            report: report.clone(),
        });
        self.view_serial += 1;
        Ok(json!({"token":token,"valid":true,"sound":report}))
    }

    pub fn sound_choice(&mut self, token: u64) -> Result<Value, String> {
        let draft = self.checked_sound_choice(token)?;
        let (entity, key, choice) = (draft.entity, draft.key.clone(), draft.choice.clone());
        let target = self.document.entity(entity).unwrap();
        let label = target
            .classname()
            .and_then(|name| self.definitions.class(name))
            .and_then(|class| class.property(&key))
            .map_or(key.as_str(), |property| property.label.as_str());
        let title = target
            .classname()
            .map(crate::inspector::title)
            .unwrap_or("Entity".into());
        let source = target.properties.get(&key).unwrap_or("").to_owned();
        let sound = choice.as_ref().map(|choice| &choice.report);
        Ok(
            json!({"token":token,"valid":true,"entity":format!("entity:{entity}"),"key":key,"label":label,"title":title,"value":choice.as_ref().map_or(source.as_str(),|choice|&choice.value),"sound":sound}),
        )
    }

    pub fn current_sound_choice(&mut self) -> Value {
        let Some(draft) = &self.sounds.draft else {
            return json!({"token":null,"valid":false});
        };
        let token = draft.token;
        self.sound_choice(token)
            .unwrap_or_else(|error| json!({"token":token,"valid":false,"error":error}))
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
        let (entity, key, choice) = (
            draft.entity,
            draft.key.clone(),
            draft.choice.clone().ok_or("Choose a sound first")?,
        );
        self.inspect_sound(&choice.value, choice.variant)?;
        self.set_entity_properties(actor, entity, &[(key.clone(), Some(choice.value.clone()))])?;
        self.sounds.draft = None;
        self.view_serial += 1;
        Ok(json!({"entity":format!("entity:{entity}"),"key":key,"value":choice.value}))
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
        let (mut report, data) = self.sound_data(value, variant)?;
        let bytes = audio::decode(report["path"].as_str().unwrap(), &data, volume)?;
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
