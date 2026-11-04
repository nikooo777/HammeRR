use std::collections::{HashMap, HashSet};

use glam::DVec3;
use source_formats::keyvalues::{self, LineEnding, Node, WriteOptions};
use source_formats::vmf::{Entity, Properties, VisGroup, Vmf};

use crate::metadata::InputBudget;
use crate::{EditError, ObjectId, Transaction, retained_metadata};

pub const MAX_SLOTS: usize = 256;
pub const MAX_IDS: usize = 16_384;
pub const MAX_SERIALIZED_BYTES: usize = 1024 * 1024;
const PREFIX: &str = "_hammerr_instance_order";
const METADATA_KEY: &str = "hammerr_instance_order";
const VERSION: &str = "1";
const MAX_DEPTH: usize = 3;
const MAX_NODES: usize = 3 + MAX_SLOTS * 6 + MAX_IDS * 2;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ordering {
    pub slots: Vec<Slot>,
    pub nested_entities: Vec<u32>,
    pub origin_custody: Vec<OriginCustody>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginCustody {
    pub entity: u32,
    pub volumes: Vec<u32>,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub original_root: u32,
    pub content: Content,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    LiveReference,
    Materialized { world: Vec<u32>, entities: Vec<u32> },
}

pub fn is_storage_key(key: &str) -> bool {
    retained_metadata::is_key(key, PREFIX)
}

fn properties_misplaced(properties: &Properties) -> bool {
    properties.iter().any(|(key, _)| is_storage_key(key))
}

fn nodes_misplaced(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| {
        is_storage_key(&node.key)
            || node.children().is_some_and(nodes_misplaced)
            || node.key.trim().eq_ignore_ascii_case(METADATA_KEY)
    })
}

fn visgroups_misplaced(groups: &[VisGroup]) -> bool {
    groups.iter().any(|group| {
        properties_misplaced(&group.extra_properties)
            || nodes_misplaced(&group.extra)
            || visgroups_misplaced(&group.children)
    })
}

fn misplaced(vmf: &Vmf) -> bool {
    retained_metadata::misplaced(vmf, PREFIX)
        || nodes_misplaced(&vmf.extra)
        || nodes_misplaced(&vmf.world.extra)
        || nodes_misplaced(&vmf.visgroups_extra)
        || vmf.version_info.as_ref().is_some_and(properties_misplaced)
        || vmf.view_settings.as_ref().is_some_and(properties_misplaced)
        || vmf
            .visgroups
            .as_ref()
            .is_some_and(|groups| visgroups_misplaced(groups))
        || vmf
            .cameras
            .iter()
            .chain(vmf.cordon.iter())
            .any(|node| nodes_misplaced(std::slice::from_ref(node)))
        || vmf.world.groups.iter().any(|group| {
            properties_misplaced(&group.extra_properties)
                || group.editor.as_ref().is_some_and(properties_misplaced)
                || nodes_misplaced(&group.extra)
        })
        || vmf.entities.iter().any(|entity| {
            nodes_misplaced(&entity.extra)
                || entity
                    .connections
                    .iter()
                    .flatten()
                    .any(|connection| is_storage_key(&connection.output))
        })
        || vmf.solids().any(|solid| {
            properties_misplaced(&solid.extra_properties)
                || solid.editor.as_ref().is_some_and(properties_misplaced)
                || nodes_misplaced(&solid.extra)
                || solid.sides.iter().any(|side| {
                    properties_misplaced(&side.extra_properties)
                        || nodes_misplaced(&side.extra)
                        || side.displacement.as_ref().is_some_and(|displacement| {
                            properties_misplaced(&displacement.extra_properties)
                                || nodes_misplaced(&displacement.extra)
                        })
                })
        })
}

fn is_reference(entity: &Entity) -> bool {
    entity
        .classname()
        .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
}

fn coordinates(point: DVec3) -> Result<[u64; 3], String> {
    if !point.is_finite() {
        return Err("Origin custody requires finite coordinates".into());
    }
    Ok(point
        .to_array()
        .map(|value| if value == 0.0 { 0 } else { value.to_bits() }))
}

pub fn origin_fingerprint(entity: &Entity, volumes: &[u32]) -> Result<String, String> {
    let class = entity
        .classname()
        .filter(|class| {
            class.eq_ignore_ascii_case("func_brush") || class.eq_ignore_ascii_case("func_wall")
        })
        .ok_or("Origin custody requires a brush-model entity")?;
    if volumes.is_empty() || volumes.len() > MAX_IDS || entity.solids.len() > MAX_IDS {
        return Err("Origin custody requires bounded nonempty origin volumes".into());
    }
    let origins = entity
        .properties
        .iter()
        .filter(|(key, _)| key.trim().eq_ignore_ascii_case("origin"))
        .collect::<Vec<_>>();
    let [("origin", value)] = origins.as_slice() else {
        return Err("Origin custody requires one canonical explicit origin".into());
    };
    let origin = source_formats::vmf::parse_vec3(value)
        .ok_or("Origin custody requires a finite explicit origin")?;
    let mut fingerprint = crate::context::text_fingerprint(&format!(
        "{} {:?} {}",
        class.to_ascii_lowercase(),
        coordinates(origin)?,
        volumes.len()
    ));
    let mut budget = InputBudget::default();
    let mut seen = HashSet::new();
    let solids = entity
        .solids
        .iter()
        .map(|solid| (solid.id, solid))
        .collect::<HashMap<_, _>>();
    if solids.len() != entity.solids.len() {
        return Err("Origin custody volume ID is ambiguous".into());
    }
    for id in volumes {
        if *id == 0 || !seen.insert(*id) {
            return Err("Origin custody repeats or has a zero volume ID".into());
        }
        let solid = solids.get(id).ok_or("Origin custody volume is missing")?;
        let rings = solid
            .precise_vertices()
            .map_err(|error| error.to_string())?;
        fingerprint = crate::context::text_fingerprint(&format!(
            "{fingerprint} {} {}",
            solid.hidden,
            solid.sides.len()
        ));
        for (index, side) in solid.sides.iter().enumerate() {
            if side.displacement.is_some() {
                return Err("Origin custody volumes cannot contain terrain".into());
            }
            budget
                .text("material", &side.material)
                .map_err(|error| error.to_string())?;
            let material = source_formats::gamefs::asset_relative(&side.material)
                .map_err(|error| error.to_string())?;
            let points = if let Some(rings) = &rings {
                rings[index].vertices.as_slice()
            } else {
                side.plane.0.as_slice()
            };
            let points = points
                .iter()
                .copied()
                .map(coordinates)
                .collect::<Result<Vec<_>, _>>()?;
            let geometry = format!(
                "{}:{material} {} {points:?}",
                material.len(),
                rings.is_some()
            );
            budget
                .text("geometry", &geometry)
                .map_err(|error| error.to_string())?;
            fingerprint = crate::context::text_fingerprint(&format!("{fingerprint} {geometry}"));
        }
    }
    Ok(fingerprint)
}

impl Ordering {
    pub fn allocation_bytes(&self) -> Result<usize, String> {
        let mut bytes = self
            .slots
            .len()
            .checked_mul(std::mem::size_of::<Slot>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>()))
            .ok_or("Instance ordering allocation size overflow")?;
        bytes = self
            .nested_entities
            .len()
            .checked_mul(std::mem::size_of::<u32>())
            .and_then(|ids| bytes.checked_add(ids))
            .ok_or("Instance ordering allocation size overflow")?;
        for slot in &self.slots {
            if let Content::Materialized { world, entities } = &slot.content {
                bytes = world
                    .len()
                    .checked_add(entities.len())
                    .and_then(|ids| ids.checked_mul(std::mem::size_of::<u32>()))
                    .and_then(|ids| bytes.checked_add(ids))
                    .ok_or("Instance ordering allocation size overflow")?;
            }
        }
        bytes = self
            .origin_custody
            .len()
            .checked_mul(std::mem::size_of::<OriginCustody>())
            .and_then(|records| bytes.checked_add(records))
            .ok_or("Instance ordering allocation size overflow")?;
        for origin in &self.origin_custody {
            bytes = origin
                .volumes
                .len()
                .checked_mul(std::mem::size_of::<u32>())
                .and_then(|ids| ids.checked_add(origin.fingerprint.len()))
                .and_then(|record| bytes.checked_add(record))
                .ok_or("Instance ordering allocation size overflow")?;
        }
        Ok(bytes)
    }

    fn validate_structure(&self) -> Result<(), String> {
        if self.slots.len() > MAX_SLOTS {
            return Err(format!("Instance ordering exceeds {MAX_SLOTS} root slots"));
        }
        if self.nested_entities.len() > MAX_IDS {
            return Err(format!("Instance ordering exceeds {MAX_IDS} nested tokens"));
        }
        let mut roots = HashSet::new();
        let mut ids = HashSet::new();
        for slot in &self.slots {
            if slot.original_root == 0 || !roots.insert(slot.original_root) {
                return Err("Instance ordering has a zero or duplicate original root ID".into());
            }
            let (world, entities): (&[u32], &[u32]) = match &slot.content {
                Content::LiveReference => (std::slice::from_ref(&slot.original_root), &[]),
                Content::Materialized { world, entities } => (world, entities),
            };
            for id in world.iter().chain(entities) {
                if *id == 0 || !ids.insert(*id) {
                    return Err("Instance ordering has a zero or duplicate content ID".into());
                }
                if ids.len() > MAX_IDS {
                    return Err(format!("Instance ordering exceeds {MAX_IDS} content IDs"));
                }
            }
        }
        let mut tokens = HashSet::new();
        for id in &self.nested_entities {
            if *id == 0 || !tokens.insert(*id) {
                return Err("Instance ordering has a zero or duplicate nested entity token".into());
            }
        }
        if ids.len().saturating_add(tokens.len()) > MAX_IDS {
            return Err(format!(
                "Instance ordering exceeds {MAX_IDS} content IDs and nested tokens"
            ));
        }
        if roots.iter().any(|root| !tokens.contains(root)) {
            return Err("Instance ordering is missing a root slot's nested token".into());
        }
        if !self
            .nested_entities
            .iter()
            .filter(|id| roots.contains(*id))
            .copied()
            .eq(self.slots.iter().map(|slot| slot.original_root))
        {
            return Err("Instance ordering root slots contradict their nested token order".into());
        }
        let mut count = ids.len().saturating_add(tokens.len());
        let mut custody_entities = HashSet::new();
        let mut custody_volumes = HashSet::new();
        for origin in &self.origin_custody {
            if origin.entity == 0 || !custody_entities.insert(origin.entity) {
                return Err("Origin custody has a zero or duplicate entity ID".into());
            }
            if origin.volumes.is_empty()
                || origin.fingerprint.len() != 16
                || !origin
                    .fingerprint
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(
                    "Origin custody has empty volumes or a noncanonical fingerprint".into(),
                );
            }
            count = count.saturating_add(origin.volumes.len()).saturating_add(1);
            if count > MAX_IDS {
                return Err(format!(
                    "Instance ordering exceeds {MAX_IDS} referenced IDs"
                ));
            }
            for id in &origin.volumes {
                if *id == 0 || ids.contains(id) || !custody_volumes.insert(*id) {
                    return Err("Origin custody has a zero, duplicate or mistyped volume ID".into());
                }
            }
        }
        Ok(())
    }

    fn validate_membership(&self, vmf: &Vmf) -> Result<(), String> {
        vmf.validate_ids().map_err(|error| error.to_string())?;
        let objects: HashSet<_> = vmf.object_ids().collect();
        let world: HashSet<_> = vmf.world.solids.iter().map(|solid| solid.id).collect();
        let entities = vmf
            .entities
            .iter()
            .map(|entity| (entity.id, entity))
            .collect::<std::collections::HashMap<_, _>>();
        for slot in &self.slots {
            match &slot.content {
                Content::LiveReference => {
                    if !entities
                        .get(&slot.original_root)
                        .is_some_and(|entity| is_reference(entity))
                    {
                        return Err(format!(
                            "Instance ordering root {} is not a current func_instance entity",
                            slot.original_root
                        ));
                    }
                }
                Content::Materialized {
                    world: held_world,
                    entities: held_entities,
                } => {
                    if objects.contains(&slot.original_root) {
                        return Err(format!(
                            "Instance ordering historical root {} is still a current map object",
                            slot.original_root
                        ));
                    }
                    for id in held_world {
                        if !world.contains(id) {
                            return Err(format!(
                                "Instance ordering content {id} is not a current world brush"
                            ));
                        }
                    }
                    for id in held_entities {
                        if !entities.get(id).is_some_and(|entity| !is_reference(entity)) {
                            return Err(format!(
                                "Instance ordering content {id} is not a current materialized entity"
                            ));
                        }
                    }
                }
            }
        }
        let roots: HashSet<_> = self.slots.iter().map(|slot| slot.original_root).collect();
        let materialized: HashSet<_> = self
            .slots
            .iter()
            .filter_map(|slot| match &slot.content {
                Content::Materialized { entities, .. } => Some(entities),
                Content::LiveReference => None,
            })
            .flatten()
            .copied()
            .collect();
        for id in &self.nested_entities {
            if !roots.contains(id) && (!entities.contains_key(id) || materialized.contains(id)) {
                return Err(format!(
                    "Instance ordering nested token {id} is not a root slot or current ordinary entity"
                ));
            }
        }
        for origin in &self.origin_custody {
            if !materialized.contains(&origin.entity) {
                return Err("Origin custody must belong to a materialized entity".into());
            }
            let entity = entities
                .get(&origin.entity)
                .ok_or("Origin custody entity is missing")?;
            if origin_fingerprint(entity, &origin.volumes)? != origin.fingerprint {
                return Err(format!(
                    "Origin custody for entity {} does not match its current origin volumes and pivot",
                    origin.entity
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn reconcile(
        &mut self,
        vmf: &Vmf,
        removed: &HashSet<ObjectId>,
        touched: &HashSet<ObjectId>,
    ) {
        let previous_roots: HashSet<_> = self.slots.iter().map(|slot| slot.original_root).collect();
        let world: HashSet<_> = vmf.world.solids.iter().map(|solid| solid.id).collect();
        let entities = vmf
            .entities
            .iter()
            .map(|entity| (entity.id, entity))
            .collect::<std::collections::HashMap<_, _>>();
        self.slots.retain_mut(|slot| match &mut slot.content {
            Content::LiveReference => {
                !removed.contains(&ObjectId::Entity(slot.original_root))
                    && entities
                        .get(&slot.original_root)
                        .is_some_and(|entity| is_reference(entity))
            }
            Content::Materialized {
                world: held_world,
                entities: held_entities,
            } => {
                held_world
                    .retain(|id| !removed.contains(&ObjectId::Solid(*id)) && world.contains(id));
                held_entities.retain(|id| {
                    !removed.contains(&ObjectId::Entity(*id))
                        && entities.get(id).is_some_and(|entity| !is_reference(entity))
                });
                !held_world.is_empty() || !held_entities.is_empty()
            }
        });
        let roots: HashSet<_> = self.slots.iter().map(|slot| slot.original_root).collect();
        self.nested_entities.retain(|id| {
            if previous_roots.contains(id) {
                roots.contains(id)
            } else {
                !removed.contains(&ObjectId::Entity(*id)) && entities.contains_key(id)
            }
        });
        let materialized: HashSet<_> = self
            .slots
            .iter()
            .filter_map(|slot| match &slot.content {
                Content::Materialized { entities, .. } => Some(entities),
                Content::LiveReference => None,
            })
            .flatten()
            .copied()
            .collect();
        let mut tokens: HashSet<_> = self.nested_entities.iter().copied().collect();
        for entity in &vmf.entities {
            if touched.contains(&ObjectId::Entity(entity.id))
                && !materialized.contains(&entity.id)
                && tokens.insert(entity.id)
            {
                self.nested_entities.push(entity.id);
            }
        }
        self.origin_custody.retain(|origin| {
            materialized.contains(&origin.entity)
                && !removed.contains(&ObjectId::Entity(origin.entity))
                && !origin
                    .volumes
                    .iter()
                    .any(|id| removed.contains(&ObjectId::Solid(*id)))
                && entities.get(&origin.entity).is_some_and(|entity| {
                    origin_fingerprint(entity, &origin.volumes)
                        .is_ok_and(|fingerprint| fingerprint == origin.fingerprint)
                })
        });
    }

    fn serialized(&self) -> Result<String, String> {
        let mut fields = vec![
            Node::string("version", VERSION),
            Node::string("nested_entities", id_list(&self.nested_entities)),
        ];
        for slot in &self.slots {
            let mut content = vec![Node::string(
                "original_root",
                slot.original_root.to_string(),
            )];
            match &slot.content {
                Content::LiveReference => content.push(Node::string("kind", "live")),
                Content::Materialized { world, entities } => {
                    content.push(Node::string("kind", "materialized"));
                    content.push(Node::string("world", id_list(world)));
                    content.push(Node::string("entities", id_list(entities)));
                }
            }
            fields.push(Node::block("slot", content));
        }
        for origin in &self.origin_custody {
            fields.push(Node::block(
                "origin_custody",
                vec![
                    Node::string("entity", origin.entity.to_string()),
                    Node::string("volumes", id_list(&origin.volumes)),
                    Node::string("fingerprint", &origin.fingerprint),
                ],
            ));
        }
        keyvalues::try_write(
            &[Node::block(METADATA_KEY, fields)],
            &WriteOptions {
                line_ending: LineEnding::Lf,
            },
        )
        .map_err(|error| error.to_string())
    }
}

fn id_list(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(" ")
}

fn id(value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .ok()
        .filter(|id| *id != 0 && value == id.to_string())
        .ok_or_else(|| "Instance ordering requires canonical positive object IDs".into())
}

fn ids(value: &str, remaining: &mut usize) -> Result<Vec<u32>, String> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let ids = value
        .split(' ')
        .take(*remaining + 1)
        .map(id)
        .collect::<Result<Vec<_>, _>>()?;
    if ids.len() > *remaining {
        return Err(format!("Instance ordering exceeds {MAX_IDS} content IDs"));
    }
    *remaining -= ids.len();
    Ok(ids)
}

fn scalar<'a>(node: &'a Node, key: &str) -> Result<&'a str, String> {
    if node.key != key {
        return Err(format!("Instance ordering expected field {key}"));
    }
    node.as_str()
        .ok_or_else(|| format!("Instance ordering field {key} must be scalar"))
}

pub(crate) fn read_structure(vmf: &Vmf) -> Result<Option<Ordering>, String> {
    if misplaced(vmf) {
        return Err("HammeRR instance ordering must use scalar world properties".into());
    }
    let Some(value) =
        retained_metadata::read(vmf, PREFIX, "Instance ordering", MAX_SERIALIZED_BYTES)?
    else {
        return Ok(None);
    };
    let nodes = keyvalues::parse_bounded(&value, MAX_DEPTH, MAX_NODES)
        .map_err(|error| format!("Invalid instance ordering metadata: {error}"))?;
    let [block] = nodes.as_slice() else {
        return Err("Instance ordering must contain exactly one metadata block".into());
    };
    if block.key != METADATA_KEY {
        return Err("Instance ordering requires its canonical metadata block".into());
    }
    let fields = block
        .children()
        .ok_or("Instance ordering metadata must be a block")?;
    let [version, nested_entities, slots @ ..] = fields else {
        return Err("Instance ordering is missing its version or nested entity sequence".into());
    };
    if scalar(version, "version")? != VERSION {
        return Err("Unsupported instance ordering version".into());
    }
    let mut remaining = MAX_IDS;
    let mut ordering = Ordering {
        slots: Vec::new(),
        nested_entities: ids(scalar(nested_entities, "nested_entities")?, &mut remaining)?,
        origin_custody: Vec::new(),
    };
    for slot in slots {
        if slot.key == "origin_custody" {
            let fields = slot.children().ok_or("Origin custody must be a block")?;
            let [entity, volumes, fingerprint] = fields else {
                return Err("Origin custody has incomplete or unknown fields".into());
            };
            let origin = OriginCustody {
                entity: id(scalar(entity, "entity")?)?,
                volumes: ids(scalar(volumes, "volumes")?, &mut remaining)?,
                fingerprint: scalar(fingerprint, "fingerprint")?.to_owned(),
            };
            remaining = remaining
                .checked_sub(1)
                .ok_or_else(|| format!("Instance ordering exceeds {MAX_IDS} referenced IDs"))?;
            ordering.origin_custody.push(origin);
            continue;
        }
        if slot.key != "slot" {
            return Err("Instance ordering contains an unknown field".into());
        }
        let fields = slot
            .children()
            .ok_or("Instance ordering slot must be a block")?;
        let content = match fields {
            [root, kind] if scalar(kind, "kind")? == "live" => Slot {
                original_root: id(scalar(root, "original_root")?)?,
                content: Content::LiveReference,
            },
            [root, kind, world, entities] if scalar(kind, "kind")? == "materialized" => Slot {
                original_root: id(scalar(root, "original_root")?)?,
                content: Content::Materialized {
                    world: ids(scalar(world, "world")?, &mut remaining)?,
                    entities: ids(scalar(entities, "entities")?, &mut remaining)?,
                },
            },
            _ => return Err("Instance ordering slot has incomplete or unknown fields".into()),
        };
        if matches!(&content.content, Content::LiveReference) {
            remaining = remaining
                .checked_sub(1)
                .ok_or_else(|| format!("Instance ordering exceeds {MAX_IDS} content IDs"))?;
        }
        ordering.slots.push(content);
    }
    ordering.validate_structure()?;
    if ordering.slots.is_empty() || ordering.serialized()? != value {
        return Err("Instance ordering payload is empty or noncanonical".into());
    }
    Ok(Some(ordering))
}

pub fn read(vmf: &Vmf) -> Result<Option<Ordering>, String> {
    let ordering = read_structure(vmf)?;
    if let Some(ordering) = &ordering {
        ordering.validate_membership(vmf)?;
    }
    Ok(ordering)
}

pub fn store(vmf: &mut Vmf, ordering: &Ordering) -> Result<(), String> {
    read_structure(vmf)?;
    ordering.validate_structure()?;
    ordering.validate_membership(vmf)?;
    let carrier = if ordering.slots.is_empty() {
        Vec::new()
    } else {
        retained_metadata::encode(
            PREFIX,
            "Instance ordering",
            &ordering.serialized()?,
            MAX_SERIALIZED_BYTES,
        )?
    };
    retained_metadata::store(vmf, PREFIX, "Instance ordering", carrier)
}

pub fn install(tx: &mut Transaction, ordering: &Ordering) -> Result<(), EditError> {
    tx.set_instance_order(ordering)
}
