use glam::DVec3;
use map_core::{Document, EditError, ObjectId, merge, subobject};
use serde_json::{Value, json};

use super::carving::Source;
use super::{Actor, SelectMode, Session};

pub struct Preview {
    pub document: Document,
    pub result: merge::Result,
    token: u64,
    scope: Vec<u32>,
    generation: u64,
    revision: u64,
}

#[derive(Default)]
pub struct State {
    pub preview: Option<Preview>,
    discarded_scope: Option<Vec<u32>>,
    serial: u64,
}

pub fn result_json(result: &merge::Result) -> Value {
    json!({"sources": result.sources, "solid": result.solid,
        "object": object_id(result.object), "removed": result.removed,
        "faces": result.faces.iter().map(|face| json!({"side":face.side,"source_solid":face.source_solid,"source_side":face.source_side})).collect::<Vec<_>>()})
}

fn object_id(id: ObjectId) -> String {
    match id {
        ObjectId::Solid(id) => format!("solid:{id}"),
        ObjectId::Entity(id) => format!("entity:{id}"),
    }
}

impl Session {
    fn merge_source_ids(&self) -> Vec<u32> {
        self.geometry
            .merging
            .preview
            .as_ref()
            .map(|preview| &preview.scope)
            .or(self.geometry.merging.discarded_scope.as_ref())
            .cloned()
            .unwrap_or_else(|| self.component_solids())
    }

    pub fn merge_sources(&self) -> Result<Vec<Source>, String> {
        let ids = self.merge_source_ids();
        if ids.len() > merge::MAX_BRUSHES {
            return Err("Choose at most 16 source brushes to merge".into());
        }
        ids.into_iter()
            .map(|id| {
                let (owner, solid) =
                    subobject::solid(&self.document, id).map_err(|error| error.to_string())?;
                let shape = subobject::topology(solid).map_err(|error| error.to_string())?;
                let (mins, maxs) = shape
                    .vertices
                    .iter()
                    .fold((DVec3::MAX, DVec3::MIN), |(mins, maxs), vertex| {
                        (mins.min(vertex.position), maxs.max(vertex.position))
                    });
                Ok(Source {
                    solid: id,
                    owner,
                    mins,
                    maxs,
                    material: solid.sides[0].material.clone(),
                })
            })
            .collect()
    }

    pub fn merge_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.scene_serial,
            self.geometry.merging.serial
        )
    }

    pub fn merge_report(&self) -> Value {
        let mut report = json!({"stamp":self.merge_stamp(),"preview":self.geometry.merging.preview.as_ref().map(|preview|json!({"token":preview.token.to_string(),"generation":preview.generation,"revision":preview.revision,"result":result_json(&preview.result)})),
            "limits":{"brushes":merge::MAX_BRUSHES,"proof_fragments":merge::MAX_PROOF_FRAGMENTS,"sides_per_brush":subobject::MAX_SIDES},
            "inheritance_policy":"Lowest brush ID supplies solid/editor metadata. Each coplanar outer face inherits the lowest brush ID, then lowest side ID, with unchanged world texture projection.",
            "owner_policy":"World brushes together, or children of one brush entity. Mixed owners and displacements reject."});
        match self.merge_sources() {
            Ok(sources) => report["sources"] = json!(sources.iter().map(|source|json!({"solid":source.solid,"owner":object_id(source.owner),"mins":source.mins.to_array(),"maxs":source.maxs.to_array(),"material":source.material})).collect::<Vec<_>>()),
            Err(error) => report["source_error"] = json!(error),
        }
        report
    }

    pub fn clear_merge_preview(&mut self) {
        let preview = self.geometry.merging.preview.take();
        let scope = self.geometry.merging.discarded_scope.take();
        if preview.is_some() || scope.is_some() {
            self.geometry.merging.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn preview_merge(&mut self, sources: &[u32], stamp: &str) -> Result<(), String> {
        if stamp != self.merge_stamp() {
            return Err("Map, selection or preview changed; inspect the merge state again".into());
        }
        let mut document = self.document.preview_snapshot();
        let result = document
            .edit("preview merge", |tx| merge::merge(tx, sources))
            .map_err(|error| error.to_string())?;
        let selected = self.merge_source_ids();
        let scope = if selected.len() <= merge::MAX_BRUSHES
            && sources.iter().all(|id| selected.contains(id))
        {
            selected
        } else {
            result.sources.clone()
        };
        let owners = scope
            .iter()
            .map(|id| subobject::solid(&self.document, *id).map(|(owner, _)| owner))
            .collect::<Result<std::collections::BTreeSet<_>, _>>()
            .map_err(|error| error.to_string())?;
        self.select(&owners.into_iter().collect::<Vec<_>>(), SelectMode::Replace)?;
        self.cancel_block_preview();
        self.clear_geometry_previews_except(&[]);

        self.cancel_area_selection();
        self.clear_volume_selection();

        self.clip_tool = false;
        self.geometry.paint.enabled = false;
        self.spatial.draw_block = false;
        self.geometry.merging.serial += 1;
        self.geometry.merging.preview = Some(Preview {
            document,
            result,
            scope,
            token: self.geometry.merging.serial,
            generation: self.document_generation,
            revision: self.document.revision(),
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    fn checked_merge(&self, token: u64) -> Result<&Preview, String> {
        let preview = self
            .geometry
            .merging
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Merge preview was closed or replaced")?;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
        {
            return Err("Map changed; preview the merge again".into());
        }
        Ok(preview)
    }

    pub fn commit_merge(&mut self, actor: Actor, token: u64) -> Result<merge::Result, String> {
        let preview = self.checked_merge(token)?;
        let expected = preview.result.clone();
        let solid = subobject::solid(&preview.document, expected.solid)
            .map_err(|error| error.to_string())?
            .1
            .clone();
        let result = self.edit(actor, "merge brushes", |tx| {
            let result = merge::merge(tx, &expected.sources)?;
            if result != expected || subobject::solid(tx.document(), result.solid)?.1 != &solid {
                return Err(EditError::Invalid(
                    "Merged geometry or face inheritance changed; preview the merge again".into(),
                ));
            }
            Ok(result)
        })?;
        self.select(&[result.object], SelectMode::Replace)?;
        Ok(result)
    }

    pub fn cancel_merge(&mut self, token: u64) -> Result<(), String> {
        let scope = self.checked_merge(token)?.scope.clone();
        self.clear_merge_preview();
        self.geometry.merging.discarded_scope = Some(scope);
        Ok(())
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.preview
            .as_ref()
            .map(|preview| serde_json::json!([self.serial, preview.token]))
    }
}
