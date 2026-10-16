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

mod existing;
mod picking;

type NodeProperties = Vec<(String, String)>;

#[derive(Clone, Debug)]
pub struct Request {
    pub classname: String,
    pub prefix: String,
    pub points: Vec<DVec3>,
    pub closed: bool,
}

#[derive(Clone, Debug)]
pub struct ExistingNode {
    pub id: u32,
    pub origin: DVec3,
}

#[derive(Clone, Debug)]
pub struct EditRequest {
    pub nodes: Vec<ExistingNode>,
}

#[derive(Clone, Debug)]
pub struct EditNode {
    pub id: Option<u32>,
    pub origin: DVec3,
}

#[derive(Clone, Debug)]
pub struct ChangeRequest {
    pub seed: u32,
    pub prefix: String,
    pub nodes: Vec<EditNode>,
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
    existing: Option<ChangeRequest>,
    removed: Vec<u32>,
    objects: Vec<ObjectId>,
    generation: u64,
    revision: u64,
    definitions: u64,
    picking: Option<picking::Picking>,
    last_pick: Option<(usize, map_core::query::SurfaceHit)>,
}

struct Plan {
    classname: String,
    points: Vec<DVec3>,
    properties: Vec<NodeProperties>,
    existing: Option<ChangeRequest>,
    removed: Vec<u32>,
}

fn apply_nodes(tx: &mut Transaction<'_>, plan: &Plan) -> Result<Vec<ObjectId>, EditError> {
    if let Some(request) = &plan.existing {
        let mut objects = Vec::with_capacity(request.nodes.len());
        for (node, properties) in request.nodes.iter().zip(&plan.properties) {
            let origin = node.origin;
            let Some(id) = node.id else {
                objects.push(map_core::edit::create_entity(
                    tx,
                    &plan.classname,
                    Some(origin),
                    properties,
                    Vec::new(),
                )?);
                continue;
            };
            let target = properties
                .iter()
                .find(|(key, _)| key == "target")
                .map(|(_, value)| value.clone());
            let entity = tx
                .document()
                .entity(id)
                .ok_or(EditError::NotFound(ObjectId::Entity(id)))?;
            let mut changes = Vec::new();
            if entity.origin() != Some(origin) {
                changes.push((
                    "origin".into(),
                    Some(source_formats::vmf::format_vec3(origin)),
                ));
            }
            if !entity
                .properties
                .get("target")
                .unwrap_or_default()
                .eq_ignore_ascii_case(target.as_deref().unwrap_or_default())
            {
                changes.push(("target".into(), target));
            }
            if !changes.is_empty() {
                map_core::edit::set_properties(tx, id, &changes)?;
            }
            objects.push(ObjectId::Entity(id));
        }
        let removed = plan
            .removed
            .iter()
            .map(|&id| ObjectId::Entity(id))
            .collect::<Vec<_>>();
        map_core::edit::delete(tx, &removed)?;
        return Ok(objects);
    }
    plan.points
        .iter()
        .zip(&plan.properties)
        .map(|(&origin, properties)| {
            map_core::edit::create_entity(tx, &plan.classname, Some(origin), properties, Vec::new())
        })
        .collect()
}

fn validate_points(points: &[DVec3]) -> Result<(), String> {
    if !(MIN_POINTS..=MAX_POINTS).contains(&points.len()) {
        return Err(format!("Use {MIN_POINTS}–{MAX_POINTS} ordered path points"));
    }
    for (index, point) in points.iter().enumerate() {
        if !point.is_finite() || point.abs().max_element() > crate::spatial::MAX_EXTENT {
            return Err("Path points must use finite coordinates within ±65536".into());
        }
        if points[..index].contains(point) {
            return Err(
                "Path points must be distinct; use Closed loop to link the last point to the first"
                    .into(),
            );
        }
    }
    Ok(())
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
            let nodes = preview.objects.iter().enumerate().filter_map(|(index, &id)| {
                let ObjectId::Entity(entity) = id else { return None };
                let entity = preview.document.entity(entity)?;
                Some(json!({
                    "id":object_name(id),"origin":entity.origin().map(|origin|origin.to_array()),
                    "source_id":preview.existing.as_ref().and_then(|request|request.nodes[index].id).map(|id|object_name(ObjectId::Entity(id))),
                    "targetname":entity.properties.get("targetname"),
                    "target":entity.properties.get("target"),
                }))
            }).collect::<Vec<_>>();
            json!({
                "token":preview.token,"owner":preview.owner.label(),
                "valid":self.checked_linked_path(preview.token).is_ok(),
                "existing":preview.existing.is_some(),
                "seed":preview.existing.as_ref().map(|request|object_name(ObjectId::Entity(request.seed))),
                "removed":preview.removed.iter().map(|&id|json!({"id":object_name(ObjectId::Entity(id)),"targetname":self.document.entity(id).and_then(|entity|entity.properties.get("targetname"))})).collect::<Vec<_>>(),
                "classname":preview.request.classname,"prefix":preview.request.prefix,
                "points":preview.request.points.iter().map(|point|point.to_array()).collect::<Vec<_>>(),
                "closed":preview.request.closed,"nodes":nodes,
                "picking":preview.picking.as_ref().map(|pick|json!({"row":pick.row,"width":pick.size.0,"height":pick.size.1,"valid":self.checked_linked_path_pick(preview.owner,preview.token).is_ok()})),
                "last_pick":preview.last_pick.map(|(row,surface)|json!({"row":row,"target":object_name(surface.hit.object),"solid":surface.hit.solid,"side":surface.hit.side,"point":surface.hit.point.to_array(),"normal":surface.normal.to_array()})),
            })
        });
        json!({
            "snapshot":self.linked_path_stamp(),"pick_snapshot":self.linked_path_pick_stamp(),"preview":preview,"editable":!self.closing,
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
        validate_points(&request.points)?;
        let names = self.linked_path_names(&request.prefix, request.points.len())?;
        let mut properties = Vec::with_capacity(names.len());
        let mut classname = request.classname.clone();
        for (index, name) in names.iter().enumerate() {
            let target = names
                .get(index + 1)
                .or_else(|| request.closed.then(|| &names[0]));
            let (class, values) =
                self.linked_path_new_properties(&request.classname, name, target)?;
            classname = class;
            properties.push(values);
        }
        Ok(Plan {
            classname,
            points: request.points.clone(),
            properties,
            existing: None,
            removed: Vec::new(),
        })
    }

    fn linked_path_names(&self, prefix: &str, count: usize) -> Result<Vec<String>, String> {
        if prefix.is_empty()
            || prefix.len() > MAX_PREFIX_BYTES
            || !prefix.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_alphabetic()
                    || byte == b'_'
                    || (index > 0 && (byte.is_ascii_digit() || byte == b'-'))
            })
        {
            return Err(format!(
                "Name prefix must use 1–{MAX_PREFIX_BYTES} ASCII letters, digits, underscores or dashes and start with a letter or underscore"
            ));
        }
        let scan = references::scan(&self.document, &self.definitions);
        if scan.len().saturating_mul(count) > MAX_REFERENCE_WORK {
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
        let mut names = Vec::with_capacity(count);
        for index in 1..=count {
            let stem = format!("{prefix}_{index:03}");
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
            if scan
                .iter()
                .any(|reference| connections::name_matches(&name, &reference.target))
            {
                return Err(format!(
                    "Generated name {name} would acquire an existing reference; choose another prefix"
                ));
            }
            occupied.insert(name.to_ascii_lowercase());
            names.push(name);
        }
        Ok(names)
    }

    fn linked_path_new_properties(
        &self,
        classname: &str,
        name: &str,
        target: Option<&String>,
    ) -> Result<(String, NodeProperties), String> {
        let mut values = BTreeMap::from([("targetname".into(), name.to_owned())]);
        if let Some(target) = target {
            values.insert("target".into(), target.clone());
        }
        let (class, mut values) = self.entity_properties(classname, ClassKind::Point, values)?;
        if target.is_none() {
            values.retain(|(key, _)| !key.eq_ignore_ascii_case("target"));
        }
        Ok((class, values))
    }

    pub fn inspect_existing_linked_path(&self, seed: u32) -> Result<Value, String> {
        let graph = self.existing_linked_path(seed)?;
        let nodes = graph.nodes.iter().map(|&id| {
            let entity = self.document.entity(id).unwrap();
            json!({"id":object_name(ObjectId::Entity(id)),"source_id":object_name(ObjectId::Entity(id)),"origin":entity.origin().unwrap().to_array(),
                "targetname":entity.properties.get("targetname"),"target":entity.properties.get("target")})
        }).collect::<Vec<_>>();
        Ok(
            json!({"snapshot":self.linked_path_stamp(),"existing":true,"classname":graph.classname,
            "prefix":"path","seed":object_name(ObjectId::Entity(seed)),"removed":[],"closed":graph.closed,"points":nodes.iter().map(|node|node["origin"].clone()).collect::<Vec<_>>(),"nodes":nodes}),
        )
    }

    fn linked_path_edit_plan(&self, request: &EditRequest) -> Result<(Request, Plan), String> {
        let seed = request
            .nodes
            .first()
            .ok_or("Edit one complete path with 2–64 nodes")?
            .id;
        let graph = self.existing_linked_path(seed)?;
        let members = request
            .nodes
            .iter()
            .map(|node| node.id)
            .collect::<BTreeSet<_>>();
        if members.len() != request.nodes.len() || members != graph.nodes.iter().copied().collect()
        {
            return Err("Edit must contain every existing path node exactly once; use an explicit seed to insert or remove nodes".into());
        }
        self.linked_path_change_plan(&ChangeRequest {
            seed,
            prefix: "path".into(),
            nodes: request
                .nodes
                .iter()
                .map(|node| EditNode {
                    id: Some(node.id),
                    origin: node.origin,
                })
                .collect(),
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
        self.check_linked_path_owner(actor, snapshot)?;
        let plan = self.linked_path_plan(&request)?;
        self.install_linked_path(actor, request, plan)
    }

    pub fn preview_existing_linked_path(
        &mut self,
        actor: Actor,
        request: EditRequest,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.check_linked_path_owner(actor, snapshot)?;
        let (request, plan) = self.linked_path_edit_plan(&request)?;
        self.install_linked_path(actor, request, plan)
    }

    pub fn preview_linked_path_changes(
        &mut self,
        actor: Actor,
        request: ChangeRequest,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.check_linked_path_owner(actor, snapshot)?;
        let (request, plan) = self.linked_path_change_plan(&request)?;
        self.install_linked_path(actor, request, plan)
    }

    fn check_linked_path_owner(&self, actor: Actor, snapshot: &str) -> Result<(), String> {
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
        Ok(())
    }

    fn install_linked_path(
        &mut self,
        actor: Actor,
        request: Request,
        plan: Plan,
    ) -> Result<Value, String> {
        let mut document = self.document.preview_snapshot();
        let objects = document
            .edit("preview linked path", |tx| apply_nodes(tx, &plan))
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
            existing: plan.existing,
            removed: plan.removed,
            objects,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            picking: None,
            last_pick: None,
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
        preview.picking = None;
        self.view_serial += 1;
        Ok(self.linked_path())
    }

    pub fn apply_linked_path(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let preview = self.checked_linked_path(token)?;
        if preview.picking.is_some() {
            return Err("Finish or cancel position picking before applying the path".into());
        }
        if preview.owner != actor {
            return Err(
                "Another actor owns this linked path; Review preview before applying it".into(),
            );
        }
        let plan = if let Some(request) = &preview.existing {
            self.linked_path_change_plan(request)?.1
        } else {
            self.linked_path_plan(&preview.request)?
        };
        let expected = preview.document.vmf().clone();
        let expected_objects = preview.objects.clone();
        let label = if preview.existing.is_some() {
            "edit linked path"
        } else {
            "create linked path"
        };
        let objects = self.edit(actor, label, |tx| {
            let objects = apply_nodes(tx, &plan)?;
            if objects != expected_objects || tx.document().vmf() != &expected {
                return Err(EditError::Invalid(
                    "Linked path differs from its preview; preview it again".into(),
                ));
            }
            Ok(objects)
        })?;
        self.clear_linked_path_preview();
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

#[cfg(test)]
pub(crate) mod existing_tests;

#[cfg(test)]
pub(crate) mod node_changes_tests;
