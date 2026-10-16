use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use map_core::{Document, EditError, ObjectId, Transaction, connections, references};
use serde_json::{Value, json};
use source_formats::fgd::ClassKind;

use super::{Actor, SelectMode, Session};

pub const MIN_POINTS: usize = 2;
pub const MAX_POINTS: usize = 64;
pub const MAX_PREFIX_BYTES: usize = 96;
const MAX_NAME_ATTEMPTS: usize = 4096;
const MAX_REFERENCE_WORK: usize = 4_000_000;

#[derive(Clone, Debug)]
pub struct Request {
    pub classname: String,
    pub prefix: String,
    pub points: Vec<DVec3>,
    pub closed: bool,
}

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    pub(super) preview: Option<Preview>,
}

pub(super) struct Preview {
    pub(super) document: Document,
    token: u64,
    owner: Actor,
    request: Request,
    objects: Vec<ObjectId>,
    generation: u64,
    revision: u64,
    definitions: u64,
}

struct Plan {
    classname: String,
    points: Vec<DVec3>,
    properties: Vec<Vec<(String, String)>>,
}

fn create_nodes(tx: &mut Transaction<'_>, plan: &Plan) -> Result<Vec<ObjectId>, EditError> {
    plan.points
        .iter()
        .zip(&plan.properties)
        .map(|(&origin, properties)| {
            map_core::edit::create_entity(tx, &plan.classname, Some(origin), properties, Vec::new())
        })
        .collect()
}

fn object_name(id: ObjectId) -> String {
    match id {
        ObjectId::Entity(id) => format!("entity:{id}"),
        ObjectId::Solid(id) => format!("solid:{id}"),
    }
}

impl Session {
    pub fn linked_path_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.definitions_serial,
            self.geometry.linked_path.serial,
        )
    }

    pub fn linked_path(&self) -> Value {
        let preview = self.geometry.linked_path.preview.as_ref().map(|preview| {
            let nodes = preview.objects.iter().filter_map(|&id| {
                let ObjectId::Entity(entity) = id else { return None };
                let entity = preview.document.entity(entity)?;
                Some(json!({
                    "id":object_name(id),"origin":entity.origin().map(|origin|origin.to_array()),
                    "targetname":entity.properties.get("targetname"),
                    "target":entity.properties.get("target"),
                }))
            }).collect::<Vec<_>>();
            json!({
                "token":preview.token,"owner":preview.owner.label(),
                "valid":self.checked_linked_path(preview.token).is_ok(),
                "classname":preview.request.classname,"prefix":preview.request.prefix,
                "points":preview.request.points.iter().map(|point|point.to_array()).collect::<Vec<_>>(),
                "closed":preview.request.closed,"nodes":nodes,
            })
        });
        json!({
            "snapshot":self.linked_path_stamp(),"preview":preview,"editable":!self.closing,
            "classes":["path_corner","path_track"],
            "limits":{"min_points":MIN_POINTS,"max_points":MAX_POINTS,"prefix_bytes":MAX_PREFIX_BYTES,"coordinate_extent":crate::spatial::MAX_EXTENT},
        })
    }

    fn linked_path_plan(&self, request: &Request) -> Result<Plan, String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        self.ensure_partitions_resolved()?;
        if !matches!(request.classname.as_str(), "path_corner" | "path_track") {
            return Err("Choose path_corner or path_track".into());
        }
        if !(MIN_POINTS..=MAX_POINTS).contains(&request.points.len()) {
            return Err(format!(
                "Create a path with {MIN_POINTS}–{MAX_POINTS} ordered points"
            ));
        }
        if request.prefix.is_empty()
            || request.prefix.len() > MAX_PREFIX_BYTES
            || !request.prefix.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_alphabetic()
                    || byte == b'_'
                    || (index > 0 && (byte.is_ascii_digit() || byte == b'-'))
            })
        {
            return Err(format!(
                "Name prefix must use 1–{MAX_PREFIX_BYTES} ASCII letters, digits, underscores or dashes and start with a letter or underscore"
            ));
        }
        for (index, point) in request.points.iter().enumerate() {
            if !point.is_finite() || point.abs().max_element() > crate::spatial::MAX_EXTENT {
                return Err("Path points must use finite coordinates within ±65536".into());
            }
            if request.points[..index].contains(point) {
                return Err("Path points must be distinct; use Closed loop to link the last point to the first".into());
            }
        }
        let scan = references::scan(&self.document, &self.definitions);
        if scan.len().saturating_mul(request.points.len()) > MAX_REFERENCE_WORK {
            return Err("Linked path reference inspection exceeds four million checks".into());
        }
        let mut occupied = self
            .document
            .vmf()
            .entities
            .iter()
            .flat_map(|entity| &entity.properties.0)
            .filter(|(key, _)| key.eq_ignore_ascii_case("targetname"))
            .map(|(_, name)| name.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        let mut names = Vec::with_capacity(request.points.len());
        for index in 1..=request.points.len() {
            let stem = format!("{}_{index:03}", request.prefix);
            let name = (1..=MAX_NAME_ATTEMPTS)
                .map(|attempt| {
                    if attempt == 1 {
                        stem.clone()
                    } else {
                        format!("{stem}_{attempt}")
                    }
                })
                .find(|candidate| !occupied.contains(&candidate.to_ascii_lowercase()))
                .ok_or("Too many existing names use this path prefix; choose another prefix")?;
            if scan.iter().any(|reference| {
                (reference.declared && connections::name_matches(&name, &reference.target))
                    || reference.target.eq_ignore_ascii_case(&name)
            }) {
                return Err(format!(
                    "Generated name {name} would acquire an existing reference; choose another prefix"
                ));
            }
            occupied.insert(name.to_ascii_lowercase());
            names.push(name);
        }
        let mut properties = Vec::with_capacity(names.len());
        let mut classname = request.classname.clone();
        for (index, name) in names.iter().enumerate() {
            let target = names
                .get(index + 1)
                .or_else(|| request.closed.then(|| &names[0]));
            let mut values = BTreeMap::from([("targetname".into(), name.clone())]);
            if let Some(target) = target {
                values.insert("target".into(), target.clone());
            }
            let (class, mut values) =
                self.entity_properties(&request.classname, ClassKind::Point, values)?;
            if target.is_none() {
                values.retain(|(key, _)| !key.eq_ignore_ascii_case("target"));
            }
            classname = class;
            properties.push(values);
        }
        Ok(Plan {
            classname,
            points: request.points.clone(),
            properties,
        })
    }

    fn checked_linked_path(&self, token: u64) -> Result<&Preview, String> {
        let preview = self
            .geometry
            .linked_path
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Linked path preview was closed or replaced")?;
        if self.closing
            || preview.generation != self.document_generation
            || preview.revision != self.document.revision()
            || preview.definitions != self.definitions_serial
        {
            return Err("Map or entity definitions changed; preview the linked path again".into());
        }
        Ok(preview)
    }

    pub(super) fn clear_linked_path_preview(&mut self) {
        if self.geometry.linked_path.preview.take().is_some() {
            self.geometry.linked_path.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn preview_linked_path(
        &mut self,
        actor: Actor,
        request: Request,
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.linked_path_stamp() {
            return Err(
                "Map, entity definitions or path preview changed; inspect the linked path again"
                    .into(),
            );
        }
        if self
            .geometry
            .linked_path
            .preview
            .as_ref()
            .is_some_and(|preview| preview.owner != actor)
        {
            return Err(
                "Another actor owns this linked path; Review preview before changing it".into(),
            );
        }
        let plan = self.linked_path_plan(&request)?;
        let mut document = self.document.preview_snapshot();
        let objects = document
            .edit("preview linked path", |tx| create_nodes(tx, &plan))
            .map_err(|error| error.to_string())?;
        self.clear_geometry_previews_except(&[]);
        self.cancel_block_preview();
        self.cancel_area_selection();
        self.clear_volume_selection();
        self.clip_tool = false;
        self.geometry.paint.enabled = false;
        self.spatial.draw_block = false;
        self.geometry.linked_path.serial += 1;
        self.geometry.linked_path.preview = Some(Preview {
            document,
            token: self.geometry.linked_path.serial,
            owner: actor,
            request,
            objects,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(self.linked_path())
    }

    pub fn review_linked_path(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.checked_linked_path(token)?;
        self.geometry.linked_path.serial += 1;
        let preview = self.geometry.linked_path.preview.as_mut().unwrap();
        preview.token = self.geometry.linked_path.serial;
        preview.owner = actor;
        self.view_serial += 1;
        Ok(self.linked_path())
    }

    pub fn apply_linked_path(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let preview = self.checked_linked_path(token)?;
        if preview.owner != actor {
            return Err(
                "Another actor owns this linked path; Review preview before applying it".into(),
            );
        }
        let plan = self.linked_path_plan(&preview.request)?;
        let expected = preview.document.vmf().clone();
        let expected_objects = preview.objects.clone();
        let objects = self.edit(actor, "create linked path", |tx| {
            let objects = create_nodes(tx, &plan)?;
            if objects != expected_objects || tx.document().vmf() != &expected {
                return Err(EditError::Invalid(
                    "Linked path differs from its preview; preview it again".into(),
                ));
            }
            Ok(objects)
        })?;
        self.select(&objects, SelectMode::Replace)?;
        Ok(
            json!({"objects":objects.into_iter().map(object_name).collect::<Vec<_>>(),"state":self.linked_path()}),
        )
    }

    pub fn cancel_linked_path(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let preview = self
            .geometry
            .linked_path
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Linked path preview was closed or replaced")?;
        if preview.owner != actor {
            return Err(
                "Another actor owns this linked path; Review preview before cancelling it".into(),
            );
        }
        self.clear_linked_path_preview();
        Ok(self.linked_path())
    }
}

#[cfg(test)]
pub(crate) mod tests;
