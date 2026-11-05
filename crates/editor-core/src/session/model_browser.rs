use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use glam::DVec3;
use map_core::{Document, ObjectId};
use render::{
    Camera, MaterialLibrary, SceneMesh,
    models::{Asset, Instance},
};
use serde_json::{Value, json};
use source_formats::vmf::Entity;

use super::{Actor, Session};

impl State {
    pub(super) fn has_draft(&self) -> bool {
        self.draft.is_some()
    }

    pub(super) fn clear_document_draft(&mut self) {
        self.draft = None;
    }

    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.draft.as_ref().map(|draft| {
            serde_json::json!([self.serial, draft.token, draft.entities, draft.choice])
        })
    }
}

#[derive(Default)]
pub struct State {
    pub(super) catalog: Option<Vec<String>>,
    serial: u64,
    draft: Option<Draft>,
}

const MAX_PREVIEW_BYTES: usize = 64 * 1024 * 1024;

type EntityChanges = Vec<(u32, Vec<(String, Option<String>)>)>;

struct Draft {
    token: u64,
    entities: Vec<u32>,
    generation: u64,
    revision: u64,
    definitions: u64,
    scene: u64,
    content: u64,
    asset: Option<Arc<Asset>>,
    preview: Option<Document>,
    choice: Option<(String, usize, u32)>,
}

fn folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("models", |(folder, _)| folder)
}

impl Draft {
    fn current(&self, session: &Session) -> bool {
        self.generation == session.document_generation
            && self.revision == session.document.revision()
            && self.definitions == session.definitions_serial
            && self.scene == session.scene_serial
            && self.content == session.content_epoch
            && std::ptr::eq(
                session.geometry.document(&session.document),
                &session.document,
            )
    }
}

impl State {
    pub(super) fn document(&self, session: &Session) -> Option<&Document> {
        self.draft
            .as_ref()
            .filter(|draft| draft.current(session))?
            .preview
            .as_ref()
    }

    pub(super) fn materials_reloaded(&mut self, previous: u64, current: u64) {
        self.scene_changed(previous, current);
    }

    pub(super) fn scene_changed(&mut self, previous: u64, current: u64) {
        if let Some(draft) = &mut self.draft
            && draft.scene == previous
        {
            draft.scene = current;
        }
    }
}

fn target_report(token: u64, entities: &[u32]) -> Value {
    json!({
        "token":token,
        "entity":if entities.len() == 1 { Some(format!("entity:{}", entities[0])) } else { None },
        "entities":entities.iter().map(|id| format!("entity:{id}")).collect::<Vec<_>>(),
        "count":entities.len(),
    })
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
        if !draft.current(self) {
            return Err(
                "Map, selection, definitions, content or geometry preview changed; reopen the model browser".into(),
            );
        }
        Ok(draft)
    }

    fn model_choice_targets(&self, entities: &[u32]) -> Result<Vec<u32>, String> {
        let ids = entities
            .iter()
            .copied()
            .map(ObjectId::Entity)
            .collect::<Vec<_>>();
        let entities = self.whole_entity_targets(&ids)?;
        for &id in &entities {
            let target = self.document.entity(id).ok_or("Model entity is missing")?;
            let class = target
                .classname()
                .and_then(|name| self.definitions.class(name));
            let model_property = class.and_then(|class| class.property("model"));
            if !target.solids.is_empty()
                || crate::sprite_resource::is_source(class)
                || !(target.properties.get("model").is_some()
                    || model_property.is_some_and(|property| property.kind == "studio"))
            {
                return Err(format!(
                    "entity:{id}: select point entities with a World Model property"
                ));
            }
            render::models::skin(target).map_err(|error| format!("entity:{id}: {error}"))?;
            render::models::body(target).map_err(|error| format!("entity:{id}: {error}"))?;
        }
        Ok(entities)
    }

    pub fn begin_model_choice(&mut self, entity: u32) -> Result<Value, String> {
        self.begin_model_choices(&[entity])
    }

    pub fn begin_model_choices(&mut self, entities: &[u32]) -> Result<Value, String> {
        if !std::ptr::eq(self.geometry.document(&self.document), &self.document) {
            return Err("Apply or cancel the active geometry preview first".into());
        }
        let entities = self.model_choice_targets(entities)?;
        let active_preview = self.model_choice_preview_token().is_some();
        if let Some(draft) = &self.model_browser.draft
            && draft.preview.is_some()
        {
            self.model_attachments.choice_changed(Some(draft.token));
        }
        if active_preview {
            self.scene_serial += 1;
        }
        self.model_browser.serial += 1;
        let token = self.model_browser.serial;
        self.model_browser.draft = Some(Draft {
            token,
            entities,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            scene: self.scene_serial,
            content: self.content_epoch,
            asset: None,
            preview: None,
            choice: None,
        });
        self.view_serial += 1;
        self.model_choice(token)
    }

    pub(super) fn model_choice_preview_token(&self) -> Option<u64> {
        let draft = self.model_browser.draft.as_ref()?;
        self.checked_model_choice(draft.token)
            .ok()?
            .preview
            .as_ref()?;
        Some(draft.token)
    }

    pub(super) fn model_choice_override(
        &self,
        document: &Document,
    ) -> Option<(&[u32], &Arc<Asset>)> {
        let draft = self.model_browser.draft.as_ref()?;
        let preview = draft.preview.as_ref()?;
        if !std::ptr::eq(document, preview) {
            return None;
        }
        let draft = self.checked_model_choice(draft.token).ok()?;
        Some((&draft.entities, draft.asset.as_ref()?))
    }

    fn validate_model_choice(
        &self,
        entities: &[u32],
        asset: &Arc<Asset>,
        skin: usize,
        body: u32,
    ) -> Result<(Document, EntityChanges), String> {
        asset.verify_source(&self.game)?;
        map_core::references::admit_snapshot_storage(
            self.document.vmf(),
            MAX_PREVIEW_BYTES,
            &mut || Ok(()),
        )?;
        let changes = [
            ("model".into(), Some(asset.path.clone())),
            ("skin".into(), Some(skin.to_string())),
            ("body".into(), Some(body.to_string())),
        ];
        let mut checked = Vec::with_capacity(entities.len());
        for &id in entities {
            let changes = self
                .checked_entity_property_changes(id, &changes)
                .map_err(|error| format!("entity:{id}: {error}"))?;
            checked.push((id, changes));
        }
        let mut document = self.document.preview_snapshot_uncached();
        document
            .edit("preview model choice", |tx| {
                for (id, changes) in &checked {
                    map_core::edit::set_properties(tx, *id, changes)?;
                }
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        map_core::references::admit_snapshot_storage(
            document.vmf(),
            MAX_PREVIEW_BYTES,
            &mut || Ok(()),
        )?;
        for &id in entities {
            let entity = document.entity(id).ok_or("Model entity is missing")?;
            Instance::new(entity, asset.clone())
                .map_err(|error| format!("entity:{id}: {error}"))?;
        }
        let original = self.model_instances_with_choice(&self.document, None);
        let models = self.model_instances_with_choice(&document, Some((entities, asset)));
        for &id in entities {
            let entity = self.document.entity(id).unwrap();
            if !entity.hidden
                && !models
                    .instances
                    .iter()
                    .any(|instance| instance.id == ObjectId::Entity(id))
            {
                let error = models
                    .issues
                    .get(&id)
                    .map_or("Model preview is unavailable", String::as_str);
                return Err(format!("entity:{id}: {error}"));
            }
        }
        let admitted = models
            .instances
            .iter()
            .map(|instance| instance.id)
            .collect::<BTreeSet<_>>();
        for instance in original
            .instances
            .iter()
            .filter(|instance| instance.has_geometry())
        {
            let ObjectId::Entity(id) = instance.id else {
                continue;
            };
            if !entities.contains(&id) && !admitted.contains(&instance.id) {
                let error = models
                    .issues
                    .get(&id)
                    .map_or("Model preview is unavailable", String::as_str);
                return Err(format!(
                    "entity:{id}: this choice would remove an existing model preview: {error}"
                ));
            }
        }
        Ok((document, checked))
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
        let entities = self.checked_model_choice(token)?.entities.clone();
        let asset = self.models.borrow_mut().get(&self.game, path)?;
        let (document, _) = self.validate_model_choice(&entities, &asset, skin, body)?;
        self.inspect_model_body(&asset.path, skin, body)?;
        self.model_attachments
            .choice_changed(self.model_choice_preview_token());
        self.scene_serial += 1;
        let draft = self.model_browser.draft.as_mut().unwrap();
        draft.choice = Some((asset.path.clone(), skin, body));
        draft.asset = Some(asset);
        draft.preview = Some(document);
        draft.scene = self.scene_serial;
        self.view_serial += 1;
        self.model_choice(token)
    }

    pub fn choose_model_bodygroup(
        &mut self,
        token: u64,
        group: usize,
        variant: usize,
    ) -> Result<Value, String> {
        let draft = self.checked_model_choice(token)?;
        let (path, skin, body) = draft.choice.clone().ok_or("Choose a model first")?;
        let body = draft
            .asset
            .as_ref()
            .unwrap()
            .model
            .set_bodygroup(body, group, variant)
            .map_err(|error| error.to_string())?;
        self.choose_model_body(token, &path, skin, body)
    }

    pub fn current_model_choice(&self) -> Value {
        let Some(draft) = &self.model_browser.draft else {
            return json!({"token":null,"asset":null,"valid":false,"state":"closed"});
        };
        self.model_choice(draft.token).unwrap_or_else(|error| {
            let mut report = target_report(draft.token, &draft.entities);
            report["asset"] = Value::Null;
            report["valid"] = json!(false);
            report["state"] = json!("stale");
            report["error"] = json!(error);
            report
        })
    }

    pub fn resume_model_choice(&mut self, entity: u32) -> Result<Value, String> {
        self.resume_model_choices(&[entity])
    }

    pub fn resume_model_choices(&mut self, entities: &[u32]) -> Result<Value, String> {
        let entities = self.model_choice_targets(entities)?;
        if let Some(draft) = &self.model_browser.draft
            && draft.entities == entities
            && self.checked_model_choice(draft.token).is_ok()
        {
            return self.model_choice(draft.token);
        }
        self.begin_model_choices(&entities)
    }

    pub fn model_choice(&self, token: u64) -> Result<Value, String> {
        let draft = self.checked_model_choice(token)?;
        let asset = draft
            .choice
            .as_ref()
            .map(|(path, skin, body)| self.inspect_model_body(path, *skin, *body))
            .transpose()?;
        let sources = draft
            .entities
            .iter()
            .map(|&id| {
                let entity = self.document.entity(id).unwrap();
                Ok(json!({
                    "entity":format!("entity:{id}"),
                    "classname":entity.classname(),
                    "path":entity.properties.get("model"),
                    "skin":render::models::skin(entity)?,
                    "body":render::models::body(entity)?,
                    "authored_skin":entity.properties.get("skin"),
                    "authored_body":entity.properties.get("body"),
                    "hidden":entity.hidden,
                    "editor_hidden":self.editor_object_hidden(ObjectId::Entity(id)),
                }))
            })
            .collect::<Result<Vec<Value>, String>>()?;
        let mut report = target_report(token, &draft.entities);
        let mut mixed = json!({});
        for key in ["path", "skin", "body"] {
            let common = sources.iter().all(|source| source[key] == sources[0][key]);
            mixed[key] = json!(!common);
            report[key] = asset.as_ref().map_or_else(
                || {
                    if common {
                        sources[0][key].clone()
                    } else {
                        Value::Null
                    }
                },
                |asset| asset[key].clone(),
            );
        }
        report["source_fields"] = json!(sources);
        report["mixed"] = mixed;
        report["asset"] = json!(asset);
        report["valid"] = json!(true);
        report["state"] = json!(if draft.preview.is_some() {
            "preview"
        } else {
            "captured"
        });
        report["preview"] = json!({"scope":"all captured visible entities; hidden state preserved","document_changed":false,"asset_source_checks":"MDL/VVD/VTX bytes and origins are verified by Choose and Apply; inspection uses the captured decoded asset and does not revalidate disk files"});
        Ok(report)
    }

    pub fn cancel_model_choice(&mut self, token: u64) -> Result<(), String> {
        let draft = self
            .model_browser
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Model choice was closed or replaced")?;
        let owned_preview = draft.preview.is_some();
        if self.model_choice_preview_token() == Some(token) {
            self.scene_serial += 1;
        }
        self.model_browser.draft = None;
        if owned_preview {
            self.model_attachments.choice_changed(Some(token));
        }
        self.view_serial += 1;
        Ok(())
    }

    pub fn apply_model_choice(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self.checked_model_choice(token)?;
        let entities = draft.entities.clone();
        let (path, skin, body) = draft
            .choice
            .clone()
            .ok_or("Choose a supported model first")?;
        let asset = draft.asset.as_ref().unwrap().clone();
        let (_, changes) = self.validate_model_choice(&entities, &asset, skin, body)?;
        self.edit(actor, "choose model for entities", |tx| {
            for (id, changes) in &changes {
                map_core::edit::set_properties(tx, *id, changes)?;
            }
            Ok(())
        })?;
        self.model_browser.draft = None;
        self.model_attachments.choice_changed(Some(token));
        self.scene_serial += 1;
        self.view_serial += 1;
        let mut report = target_report(token, &entities);
        report["state"] = json!("applied");
        report["path"] = json!(path);
        report["skin"] = json!(skin);
        report["body"] = json!(body);
        Ok(report)
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
