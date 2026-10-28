use std::collections::{BTreeMap, BTreeSet};

use glam::{DMat3, DVec3};
use map_core::{Document, ObjectId};
use render::models::{Instance, MAX_SCENE_INDICES, MAX_SCENE_VERTICES};
use serde_json::{Value, json};

use super::{Actor, Session};

pub(super) struct Models {
    pub instances: Vec<Instance>,
    pub issues: BTreeMap<u32, String>,
}

pub(super) fn combined_bounds(
    document: &Document,
    ids: &[ObjectId],
    overrides: &BTreeMap<ObjectId, (DVec3, DVec3)>,
) -> Option<(DVec3, DVec3)> {
    map_core::query::union(
        ids.iter()
            .filter_map(|&id| object_bounds(document, id, overrides)),
    )
}

pub(super) fn object_bounds(
    document: &Document,
    id: ObjectId,
    overrides: &BTreeMap<ObjectId, (DVec3, DVec3)>,
) -> Option<(DVec3, DVec3)> {
    overrides
        .get(&id)
        .copied()
        .or_else(|| map_core::query::bounds(document, id))
}

fn label(name: &str, fallback: &str) -> String {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    let name = name.strip_suffix(".smd").unwrap_or(name);
    let name = name
        .strip_suffix("_reference")
        .unwrap_or(name)
        .replace('_', " ");
    if name.is_empty() {
        return fallback.into();
    }
    let mut chars = name.chars();
    chars.next().unwrap().to_uppercase().collect::<String>() + chars.as_str()
}

fn bodygroups(model: &source_formats::model::Model, body: u32) -> Vec<Value> {
    model.body_groups.iter().enumerate().map(|(index,group)| {
        let group_label=label(&group.name,&format!("Body group {}",index+1));
        let single=group.variants.iter().filter(|variant|variant.bounds.is_some()).count()==1;
        let variants=group.variants.iter().enumerate().map(|(index,variant)| {
            let variant_label=if variant.bounds.is_none(){"None".into()}else if single && !["","body","studio"].contains(&group.name.to_ascii_lowercase().as_str()){group_label.clone()}else{label(&variant.name,&format!("Variant {}",index+1))};
            json!({"index":index,"name":variant.name,"label":variant_label,"blank":variant.bounds.is_none(),"triangles":model.parts[variant.parts.clone()].iter().map(|part|part.indices.len()/3).sum::<usize>()})
        }).collect::<Vec<_>>();
        json!({"index":index,"name":group.name,"label":group_label,"base":group.base,"selected":group.selected(body),"variants":variants})
    }).collect()
}

fn appearance_report(entity: &source_formats::vmf::Entity) -> Value {
    let appearance = render::models::appearance(entity);
    json!({
        "supported": appearance.is_ok(),
        "state": match &appearance {
            Ok(appearance) if appearance.mode == source_formats::render_color::TEXTURE => "texture",
            Ok(_) => "normal",
            Err(_) => "unsupported",
        },
        "render_mode": appearance.as_ref().ok().map(|appearance| appearance.mode),
        "render_amount": appearance.as_ref().ok().map(|appearance| appearance.amount),
        "opacity": appearance.as_ref().ok().map(|appearance| appearance.opacity()),
        "authored": {
            "rendermode": entity.properties.get("rendermode"),
            "renderamt": entity.properties.get("renderamt"),
            "renderfx": entity.properties.get("renderfx"),
        },
        "error": appearance.err(),
        "limits": "Textured model preview only: Normal (0) ignores renderamt; Texture (2) multiplies material alpha by renderamt/255, depth-tests without depth writes and sorts material batches by camera depth. Render FX must be None (0). No animation, runtime fading, material proxies, intersecting-transparency or native game-parity claim. rendercolor's fourth component is preserved and does not set opacity. Static-prop output requires Normal (0).",
    })
}

fn scale_report(entity: &source_formats::vmf::Entity) -> Value {
    let scale = render::models::scale(entity);
    json!({
        "supported": scale.is_ok(),
        "authored": entity.properties.get("modelscale"),
        "effective": scale.as_ref().ok(),
        "range": [render::models::MIN_MODEL_SCALE, render::models::MAX_MODEL_SCALE],
        "token_bytes": render::models::MAX_MODEL_SCALE_TEXT_BYTES,
        "error": scale.err(),
        "limits": "Authored MDL mesh preview: positive uniform native-float scale about the entity origin, with editor numeric range 1/64–64 and finite transformed mesh bounds within ±65536. Missing means 1; unsupported values retain a labeled helper. Mesh positions, picking, bounds and default-pose attachments share the transform. Static-prop and native detail writers require unit scale. Animation, runtime scale inputs, collision/physics, sequence culling bounds and native appearance parity are not evaluated. Asset-only browser previews remain unit scale; literal FGD guides retain their unit-scale admission.",
    })
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
            if crate::sprite_resource::is_source(
                entity
                    .classname()
                    .and_then(|name| self.definitions().class(name)),
            ) {
                continue;
            }
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
                        .parts_for_body(instance.body)
                        .map(|part| part.indices.len())
                        .sum::<usize>();
                    let vertex_count = if instance.has_geometry() {
                        instance.asset.model.vertices.len()
                    } else {
                        0
                    };
                    if vertices + vertex_count > MAX_SCENE_VERTICES
                        || indices + count > MAX_SCENE_INDICES
                    {
                        models.issues.insert(
                            entity.id,
                            "Model scene geometry budget exceeded; this entity remains a helper"
                                .into(),
                        );
                    } else {
                        vertices += vertex_count;
                        indices += count;
                        if let Err(error) = instance.asset.materials() {
                            models.issues.insert(entity.id, error);
                        }
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
        self.inspect_model_body(path, skin, 0)
    }

    pub fn inspect_model_body(&self, path: &str, skin: usize, body: u32) -> Result<Value, String> {
        if body > i32::MAX as u32 {
            return Err("Model body must fit a nonnegative native integer".into());
        }
        let asset = self.models.borrow_mut().get(&self.game, path)?;
        let family = asset
            .model
            .skins
            .get(skin)
            .ok_or("Skin is outside the model's skin families")?;
        let parts = asset.model.parts_for_body(body).collect::<Vec<_>>();
        let bounds = asset.model.bounds_for_body(body);
        let attachments = asset
            .model
            .attachments
            .iter()
            .enumerate()
            .map(|(index, attachment)| {
                super::model_attachments::report(attachment, index + 1, attachment.transform)
            })
            .collect::<Vec<_>>();
        let bindings = asset.materials();
        let materials = bindings
            .as_ref()
            .map(|materials| {
                parts
                    .iter()
                    .map(|part| &materials[family[part.material]])
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let stem = asset.path.strip_suffix(".mdl").unwrap();
        let source_paths = [
            asset.path.clone(),
            format!("{stem}.vvd"),
            format!("{stem}.dx90.vtx"),
        ];
        let sources = asset.source_origins.as_ref().map(|origins| {
            source_paths
                .iter()
                .zip(origins)
                .map(|(path, origin)| {
                    let origin = crate::materials::provenance(origin.clone());
                    json!({"path":path,"source_id":origin.source_id,"source":origin.source})
                })
                .collect::<Vec<_>>()
        });
        let mut report = json!({"path":asset.path,"static_prop":asset.model.header.is_static_prop(),"pose":if asset.model.header.is_static_prop(){"static"}else{"reference"},"reference_bones":asset.model.reference_bones,"vertices":asset.model.vertices.len(),"triangles":parts.iter().map(|part|part.indices.len()/3).sum::<usize>(),"meshes":parts.len(),"body_parts":asset.model.body_parts,"body":body,"bodygroups":bodygroups(&asset.model,body),"attachments":attachments,"blank":bounds.is_none(),"lod":0,"skins":asset.model.skins.len(),"skin":skin,"materials":materials,"material_error":bindings.as_ref().err(),"local_bounds":bounds,"hull_bounds":[asset.model.header.hull_mins,asset.model.header.hull_maxs],"asset_bytes":asset.memory_bytes(),"limits":{"scene_vertices":MAX_SCENE_VERTICES,"scene_indices":MAX_SCENE_INDICES}});
        report["sources"] = json!(sources);
        report["provenance_scope"] =
            json!("captured_at_model_load; current filesystem has not been revalidated");
        Ok(report)
    }

    pub fn inspect_entity_model(&self, id: u32) -> Result<Value, String> {
        let document = self.render_document();
        let entity = document
            .entity(id)
            .ok_or("Select an existing model entity")?;
        if crate::sprite_resource::is_source(
            entity
                .classname()
                .and_then(|name| self.definitions().class(name)),
        ) {
            return Err("Sprite resource source; inspect get_entity_inspector.helpers.sprite_resource for the shared image policy and resource status".into());
        }
        let path = entity
            .properties
            .get("model")
            .ok_or("This entity has no model path")?;
        let skin = entity.properties.get("skin").map_or(Ok(0), |value| {
            value
                .parse::<usize>()
                .map_err(|_| "Model skin must be a nonnegative integer")
        })?;
        let body = entity.properties.get("body").map_or(Ok(0), |value| {
            value
                .parse::<u32>()
                .map_err(|_| "Model body must be a nonnegative native integer")
        })?;
        let asset = self.inspect_model_body(path, skin, body)?;
        let appearance = appearance_report(entity);
        let mut scale = scale_report(entity);
        let models = self.model_instances(document);
        let instance = models
            .instances
            .iter()
            .find(|instance| instance.id == ObjectId::Entity(id));
        let error = models
            .issues
            .get(&id)
            .map(String::as_str)
            .or_else(|| asset["material_error"].as_str());
        let visible = self.working_object_visible(document, ObjectId::Entity(id));
        scale["applied"] = json!(instance.is_some_and(Instance::has_geometry));
        scale["instance_error"] = json!(error);
        Ok(
            json!({"entity":format!("entity:{id}"),"asset":asset,"displayed":visible && instance.is_some_and(Instance::has_geometry),"helper":visible && !entity.hidden && instance.is_none_or(|instance|!instance.has_geometry()),"editor_hidden":self.editor_object_hidden(ObjectId::Entity(id)),"error":error,"hidden":entity.hidden,"tint":instance.map(|instance|instance.tint),"opacity_preview":instance.is_some_and(|instance|instance.appearance.mode==source_formats::render_color::TEXTURE),"appearance":appearance,"model_scale":scale,"attachments":instance.map(|instance|instance.asset.model.attachments.iter().enumerate().map(|(index,attachment)|super::model_attachments::report(attachment,index+1,instance.attachment(index+1).unwrap())).collect::<Vec<_>>()).unwrap_or_default(),"world_bounds":instance.map(|instance|{let(a,b)=instance.bounds();[a.to_array(),b.to_array()]})}),
        )
    }

    pub fn refresh_model_assets(&mut self, actor: Actor) -> bool {
        self.cancel_selection_previews();
        self.advance_content_epoch();
        let snap_preview_cancelled = self
            .transform_preview()
            .is_some_and(|preview| preview.grid_snap.is_some());
        if snap_preview_cancelled {
            self.cancel_transform_preview();
        }
        self.models.get_mut().clear();
        self.invalidate_instance_model_preview();
        self.model_browser.catalog = None;
        self.scene_serial += 1;
        self.view_serial += 1;
        self.log(
            actor,
            if snap_preview_cancelled {
                "reloaded model assets; preview snap to grid again"
            } else {
                "reloaded model assets"
            },
        );
        snap_preview_cancelled
    }

    pub fn object_bounds_in(
        &self,
        document: &Document,
        ids: &[ObjectId],
    ) -> Option<(DVec3, DVec3)> {
        let bounds = self.display_bounds_in(document);
        combined_bounds(document, ids, &bounds)
    }

    pub fn display_bounds_in(&self, document: &Document) -> BTreeMap<ObjectId, (DVec3, DVec3)> {
        let models = self.model_instances(document);
        self.display_bounds_with_models(document, &models)
    }

    pub(super) fn display_bounds_with_models(
        &self,
        document: &Document,
        models: &Models,
    ) -> BTreeMap<ObjectId, (DVec3, DVec3)> {
        let helpers = self.fgd_helper_appearances(document, models);
        let mut bounds = models
            .instances
            .iter()
            .map(|instance| (instance.id, instance.bounds()))
            .collect::<BTreeMap<_, _>>();
        for (id, helper) in helpers {
            let ObjectId::Entity(entity) = id else {
                continue;
            };
            if let Some(bounds_value) = document
                .entity(entity)
                .and_then(|entity| helper.world_bounds(entity))
            {
                bounds.insert(id, bounds_value);
            }
        }
        bounds
    }

    pub fn object_bounds_in_basis(
        &self,
        document: &Document,
        ids: &[ObjectId],
        basis: DMat3,
    ) -> Option<(DVec3, DVec3)> {
        let models = self.model_instances(document);
        let mut bounds = models
            .instances
            .iter()
            .map(|instance| (instance.id, instance.bounds_in_basis(basis)))
            .collect::<BTreeMap<_, _>>();
        let inverse = basis.transpose();
        for (id, helper) in self.fgd_helper_appearances(document, &models) {
            let ObjectId::Entity(entity) = id else {
                continue;
            };
            if let Some((mins, maxs)) = document
                .entity(entity)
                .and_then(|entity| helper.world_bounds(entity))
            {
                let corners = (0..8).map(|index| {
                    let point = DVec3::new(
                        if index & 1 == 0 { mins.x } else { maxs.x },
                        if index & 2 == 0 { mins.y } else { maxs.y },
                        if index & 4 == 0 { mins.z } else { maxs.z },
                    );
                    let point = inverse * point;
                    (point, point)
                });
                if let Some(value) = map_core::query::union(corners) {
                    bounds.insert(id, value);
                }
            }
        }
        let ordinary = ids
            .iter()
            .copied()
            .filter(|id| !bounds.contains_key(id))
            .collect::<Vec<_>>();
        map_core::query::union(
            map_core::query::bounds_in_basis(document, &ordinary, basis)
                .into_iter()
                .chain(ids.iter().filter_map(|id| bounds.get(id).copied())),
        )
    }

    pub(super) fn select_region(
        &mut self,
        region: &map_core::region::Region,
        rule: map_core::region::Match,
        faces: bool,
    ) -> map_core::region::Selection {
        let hidden = self.hidden_brushes_for(false);
        let evaluation = self.working_evaluation(&self.document);
        let bounds = if faces {
            BTreeMap::new()
        } else {
            self.presentation_bounds_in(&self.document, None)
        };
        let mut matches = region.select_with_entity_bounds(
            &self.document,
            rule,
            faces,
            |solid| !hidden.contains(&solid.id),
            |entity| {
                if !evaluation.objects.contains(&ObjectId::Entity(entity.id)) {
                    return None;
                }
                bounds
                    .get(&ObjectId::Entity(entity.id))
                    .copied()
                    .or_else(|| map_core::query::entity_bounds(entity))
            },
        );
        if !faces {
            matches.objects = self.group_region_matches(&matches.objects, rule);
        }
        matches
    }
}
