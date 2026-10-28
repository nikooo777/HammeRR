use std::collections::{BTreeMap, BTreeSet, HashSet};

use glam::DVec3;
use render::{Camera, MaterialLibrary, SceneMesh, models::Instance};
use serde_json::{Value, json};
use source_formats::vmf::Entity;

use super::{Actor, Session};

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.draft
            .as_ref()
            .map(|draft| serde_json::json!([self.serial, draft.token, draft.choice]))
    }
}

#[derive(Default)]
pub struct State {
    pub(super) catalog: Option<Vec<String>>,
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
    choice: Option<(String, usize, u32)>,
}

fn folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("models", |(folder, _)| folder)
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
    pub fn browse_models(
        &mut self,
        query: &str,
        group: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        self.browse_models_filtered(query, group, offset, limit, None)
    }

    pub fn browse_models_filtered(
        &mut self,
        query: &str,
        group: &str,
        offset: usize,
        limit: usize,
        source: Option<&str>,
    ) -> Result<Value, String> {
        if query.len() > 256 || group.len() > 1024 || !(1..=100).contains(&limit) {
            return Err("Use a query up to 256 bytes and a page of 1 to 100 models".into());
        }
        let sources = crate::materials::catalog_sources(&self.game)?;
        crate::materials::validate_source_filter(source, &sources)?;
        if self.model_browser.catalog.is_none() {
            self.model_browser.catalog = Some(
                self.game
                    .list("models", ".mdl")
                    .map_err(|error| format!("Model catalog: {error}"))?,
            );
        }
        let names = self.model_browser.catalog.as_ref().unwrap();
        if names.len() > crate::materials::MAX_CATALOG_ENTRIES {
            return Err(format!(
                "Model catalog exceeds {} entries",
                crate::materials::MAX_CATALOG_ENTRIES
            ));
        }
        let used = self
            .document
            .vmf()
            .entities
            .iter()
            .filter_map(|entity| entity.properties.get("model"))
            .filter_map(|path| render::models::model_path(path).ok())
            .collect::<BTreeSet<_>>();
        let query = query.to_ascii_lowercase();
        let words = query.split_whitespace().collect::<Vec<_>>();
        let candidates = names
            .iter()
            .filter(|name| words.iter().all(|word| name.contains(word)))
            .collect::<Vec<_>>();
        let mut matching = Vec::new();
        for name in candidates {
            if !matches!(source, None | Some("all")) {
                let origin = self
                    .game
                    .asset_origin(name)
                    .map_err(|error| format!("Model catalog source {name}: {error}"))?
                    .map(crate::materials::provenance)
                    .unwrap_or_default();
                if !crate::materials::source_matches(source, &origin) {
                    continue;
                }
            }
            matching.push(name);
        }
        let mut folders = BTreeMap::<&str, usize>::new();
        for name in &matching {
            *folders.entry(folder(name)).or_default() += 1;
        }
        let mut groups = vec![
            json!({"id":"all","label":"All models","count":matching.len()}),
            json!({"id":"used","label":"Used in this map","count":matching.iter().filter(|name| used.contains(name.as_str())).count()}),
        ];
        groups.extend(folders.iter().map(|(path, count)| {
            json!({"id":format!("folder:{path}"),"label":path.strip_prefix("models/").unwrap_or(path),"count":count})
        }));
        if group != "all"
            && group != "used"
            && !group.starts_with("folder:models/")
            && group != "folder:models"
        {
            return Err("Choose all, used or a folder:models/... group".into());
        }
        let rows = matching
            .into_iter()
            .filter(|path| match group {
                "all" => true,
                "used" => used.contains(path.as_str()),
                _ => group.strip_prefix("folder:") == Some(folder(path)),
            })
            .collect::<Vec<_>>();
        let mut entries = Vec::new();
        for path in rows.iter().skip(offset).take(limit) {
            let origin = self
                .game
                .asset_origin(path)
                .map_err(|error| format!("Model catalog source {path}: {error}"))?
                .map(crate::materials::provenance)
                .unwrap_or_default();
            if !crate::materials::source_matches(source, &origin) {
                return Err("Model source changed during catalog lookup; retry the request".into());
            }
            entries.push(json!({"path":path,"name":path.rsplit('/').next().unwrap_or(path),"folder":folder(path),"used":used.contains(path.as_str()),"source_id":origin.source_id,"source":origin.source}));
        }
        Ok(
            json!({"groups":groups,"entries":entries,"total":rows.len(),"offset":offset,"limit":limit,"sources":sources,"filter":{"source":source},"provenance_scope":"current_winning_mdl; inspect_model reports separately captured model-load origins"}),
        )
    }

    fn checked_model_choice(&self, token: u64) -> Result<&Draft, String> {
        let draft = self
            .model_browser
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Model choice was closed or replaced")?;
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
            || draft.definitions != self.definitions_serial
            || draft.scene != self.scene_serial
            || !std::ptr::eq(self.render_document(), &self.document)
        {
            return Err(
                "Map, selection or geometry preview changed; reopen the model browser".into(),
            );
        }
        Ok(draft)
    }

    pub fn begin_model_choice(&mut self, entity: u32) -> Result<Value, String> {
        if !std::ptr::eq(self.render_document(), &self.document) {
            return Err("Apply or cancel the active geometry preview first".into());
        }
        let target = self
            .document
            .entity(entity)
            .ok_or("Model entity is missing")?;
        let model_property = target
            .classname()
            .and_then(|name| self.definitions.class(name))
            .and_then(|class| class.property("model"));
        if !target.solids.is_empty()
            || !(target.properties.get("model").is_some()
                || model_property.is_some_and(|property| property.kind == "studio"))
        {
            return Err("Select a point entity with a World Model property".into());
        }
        self.model_browser.serial += 1;
        let token = self.model_browser.serial;
        self.model_browser.draft = Some(Draft {
            token,
            entity,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            scene: self.scene_serial,
            choice: None,
        });
        self.view_serial += 1;
        Ok(
            json!({"token":token,"entity":format!("entity:{entity}"),"path":target.properties.get("model"),"skin":target.properties.get("skin").and_then(|value|value.parse::<usize>().ok()).unwrap_or(0),"body":target.properties.get("body").and_then(|value|value.parse::<u32>().ok()).unwrap_or(0),"asset":null,"valid":true}),
        )
    }

    pub fn choose_model(&mut self, token: u64, path: &str, skin: usize) -> Result<Value, String> {
        self.choose_model_body(token, path, skin, 0)
    }

    pub fn choose_model_body(
        &mut self,
        token: u64,
        path: &str,
        skin: usize,
        body: u32,
    ) -> Result<Value, String> {
        let id = self.checked_model_choice(token)?.entity;
        let asset = self.models.borrow_mut().get(&self.game, path)?;
        let mut entity = self
            .document
            .entity(id)
            .ok_or("Model entity is missing")?
            .clone();
        entity.properties.set("model", &asset.path);
        entity.properties.set("skin", skin.to_string());
        entity.properties.set("body", body.to_string());
        Instance::new(&entity, asset.clone())?;
        let report = self.inspect_model_body(&asset.path, skin, body)?;
        self.model_browser.draft.as_mut().unwrap().choice = Some((asset.path.clone(), skin, body));
        self.view_serial += 1;
        Ok(json!({"token":token,"entity":format!("entity:{id}"),"asset":report,"valid":true}))
    }

    pub fn choose_model_bodygroup(
        &mut self,
        token: u64,
        group: usize,
        variant: usize,
    ) -> Result<Value, String> {
        let (path, skin, body) = self
            .checked_model_choice(token)?
            .choice
            .clone()
            .ok_or("Choose a model first")?;
        let asset = self.models.borrow_mut().get(&self.game, &path)?;
        let body = asset
            .model
            .set_bodygroup(body, group, variant)
            .map_err(|error| error.to_string())?;
        self.choose_model_body(token, &path, skin, body)
    }

    pub fn current_model_choice(&self) -> Value {
        let Some(draft) = &self.model_browser.draft else {
            return json!({"token":null,"asset":null,"valid":false});
        };
        self.model_choice(draft.token).unwrap_or_else(|error|json!({"token":draft.token,"entity":format!("entity:{}",draft.entity),"asset":null,"valid":false,"error":error}))
    }

    pub fn resume_model_choice(&mut self, entity: u32) -> Result<Value, String> {
        if let Some(draft) = &self.model_browser.draft
            && draft.entity == entity
            && self.checked_model_choice(draft.token).is_ok()
        {
            return self.model_choice(draft.token);
        }
        self.begin_model_choice(entity)
    }

    pub fn model_choice(&self, token: u64) -> Result<Value, String> {
        let draft = self.checked_model_choice(token)?;
        let asset = draft
            .choice
            .as_ref()
            .map(|(path, skin, body)| self.inspect_model_body(path, *skin, *body))
            .transpose()?;
        let source = self.document.entity(draft.entity).unwrap();
        let path = asset
            .as_ref()
            .and_then(|asset| asset["path"].as_str())
            .or_else(|| source.properties.get("model"));
        let skin = asset
            .as_ref()
            .and_then(|asset| asset["skin"].as_u64())
            .or_else(|| {
                source
                    .properties
                    .get("skin")
                    .and_then(|value| value.parse().ok())
            })
            .unwrap_or(0);
        let body = asset
            .as_ref()
            .and_then(|asset| asset["body"].as_u64())
            .or_else(|| {
                source
                    .properties
                    .get("body")
                    .and_then(|value| value.parse().ok())
            })
            .unwrap_or(0);
        Ok(
            json!({"token":token,"entity":format!("entity:{}",draft.entity),"path":path,"skin":skin,"body":body,"asset":asset,"valid":true}),
        )
    }

    pub fn cancel_model_choice(&mut self, token: u64) -> Result<(), String> {
        if self
            .model_browser
            .draft
            .as_ref()
            .is_none_or(|draft| draft.token != token)
        {
            return Err("Model choice was closed or replaced".into());
        }
        self.model_browser.draft = None;
        self.view_serial += 1;
        Ok(())
    }

    pub fn apply_model_choice(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self.checked_model_choice(token)?;
        let id = draft.entity;
        let (path, skin, body) = draft
            .choice
            .clone()
            .ok_or("Choose a supported model first")?;
        self.choose_model_body(token, &path, skin, body)?;
        self.set_entity_properties(
            actor,
            id,
            &[
                ("model".into(), Some(path.clone())),
                ("skin".into(), Some(skin.to_string())),
                ("body".into(), Some(body.to_string())),
            ],
        )?;
        self.model_browser.draft = None;
        Ok(json!({"entity":format!("entity:{id}"),"path":path,"skin":skin,"body":body}))
    }

    pub fn model_preview_scene(
        &self,
        path: &str,
        skin: usize,
        body: u32,
        aspect: f32,
    ) -> Result<(SceneMesh, MaterialLibrary, Camera), String> {
        let asset = self.models.borrow_mut().get(&self.game, path)?;
        let mut entity = Entity::new(2, "prop_dynamic");
        entity.properties.set("origin", "0 0 0");
        entity.properties.set("skin", skin.to_string());
        entity.properties.set("body", body.to_string());
        let instance = Instance::new(&entity, asset)?;
        let (low, high) = instance.bounds();
        let camera = Camera::looking_at(
            ((low + high) * 0.5 + DVec3::new(-1.0, -1.0, 0.65)).as_vec3(),
            ((low + high) * 0.5).as_vec3(),
        )
        .frame_bounds(low.as_vec3(), high.as_vec3(), aspect);
        let mut vmf = map_core::Document::empty().vmf().clone();
        vmf.entities.push(entity);
        let mut materials = MaterialLibrary::default();
        let mesh = SceneMesh::build_with_models(
            &vmf,
            &self.game,
            &mut materials,
            &HashSet::new(),
            &HashSet::new(),
            &[],
            &[instance],
        );
        Ok((mesh, materials, camera))
    }
}
