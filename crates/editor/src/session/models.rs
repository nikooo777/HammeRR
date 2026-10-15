use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use map_core::{Document, ObjectId};
use render::models::{Instance, MAX_SCENE_INDICES, MAX_SCENE_VERTICES};
use serde_json::{Value, json};

use super::{Actor, Session};

#[cfg(test)]
mod tests;

pub(super) struct Models {
    pub instances: Vec<Instance>,
    pub issues: BTreeMap<u32, String>,
}

impl Session {
    pub(super) fn model_instances(&self, document: &Document) -> Models {
        let mut models = Models {
            instances: Vec::new(),
            issues: BTreeMap::new(),
        };
        let mut vertices = 0;
        let mut indices = 0;
        for entity in document
            .vmf()
            .entities
            .iter()
            .filter(|entity| !entity.hidden && entity.solids.is_empty())
        {
            let Some(path) = entity
                .properties
                .get("model")
                .filter(|path| !path.trim().is_empty())
            else {
                continue;
            };
            let result = self
                .models
                .borrow_mut()
                .get(&self.game, path)
                .and_then(|asset| Instance::new(entity, asset));
            match result {
                Ok(instance) => {
                    let count = instance
                        .asset
                        .model
                        .parts
                        .iter()
                        .map(|part| part.indices.len())
                        .sum::<usize>();
                    if vertices + instance.asset.model.vertices.len() > MAX_SCENE_VERTICES
                        || indices + count > MAX_SCENE_INDICES
                    {
                        models.issues.insert(
                            entity.id,
                            "Model scene geometry budget exceeded; this entity remains a helper"
                                .into(),
                        );
                    } else {
                        vertices += instance.asset.model.vertices.len();
                        indices += count;
                        models.instances.push(instance);
                    }
                }
                Err(error) => {
                    models.issues.insert(entity.id, error);
                }
            }
        }
        models
    }

    pub fn inspect_model(&self, path: &str, skin: usize) -> Result<Value, String> {
        let asset = self.models.borrow_mut().get(&self.game, path)?;
        let family = asset
            .model
            .skins
            .get(skin)
            .ok_or("Skin is outside the model's skin families")?;
        Ok(
            json!({"path":asset.path,"static_prop":true,"vertices":asset.model.vertices.len(),"triangles":asset.model.parts.iter().map(|part|part.indices.len()/3).sum::<usize>(),"meshes":asset.model.parts.len(),"body_parts":asset.model.body_parts,"body":0,"lod":0,"skins":asset.model.skins.len(),"skin":skin,"materials":asset.model.parts.iter().map(|part|&asset.materials[family[part.material]]).collect::<BTreeSet<_>>(),"local_bounds":asset.model.bounds,"hull_bounds":[asset.model.header.hull_mins,asset.model.header.hull_maxs],"asset_bytes":asset.memory_bytes(),"limits":{"scene_vertices":MAX_SCENE_VERTICES,"scene_indices":MAX_SCENE_INDICES}}),
        )
    }

    pub fn inspect_entity_model(&self, id: u32) -> Result<Value, String> {
        let document = self.render_document();
        let entity = document
            .entity(id)
            .ok_or("Select an existing model entity")?;
        let path = entity
            .properties
            .get("model")
            .ok_or("This entity has no model path")?;
        let skin = entity.properties.get("skin").map_or(Ok(0), |value| {
            value
                .parse::<usize>()
                .map_err(|_| "Model skin must be a nonnegative integer")
        })?;
        let asset = self.inspect_model(path, skin)?;
        let models = self.model_instances(document);
        let instance = models
            .instances
            .iter()
            .find(|instance| instance.id == ObjectId::Entity(id));
        Ok(
            json!({"entity":format!("entity:{id}"),"asset":asset,"displayed":instance.is_some(),"error":models.issues.get(&id),"hidden":entity.hidden,"world_bounds":instance.map(|instance|{let(a,b)=instance.bounds();[a.to_array(),b.to_array()]})}),
        )
    }

    pub fn refresh_model_assets(&mut self, actor: Actor) {
        self.models.get_mut().clear();
        self.scene_serial += 1;
        self.view_serial += 1;
        self.log(actor, "reloaded model assets");
    }

    pub fn object_bounds_in(
        &self,
        document: &Document,
        ids: &[ObjectId],
    ) -> Option<(DVec3, DVec3)> {
        let models = self.model_instances(document);
        map_core::query::union(ids.iter().filter_map(|&id| {
            models
                .instances
                .iter()
                .find(|instance| instance.id == id)
                .map(Instance::bounds)
                .or_else(|| map_core::query::bounds(document, id))
        }))
    }
}
