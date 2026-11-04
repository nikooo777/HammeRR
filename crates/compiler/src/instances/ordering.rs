use std::sync::Arc;

use super::*;
use map_core::instance_order::{Content, Ordering, OriginCustody, Slot};

#[derive(Clone)]
pub(super) struct ScopePlan {
    pub order: Option<Ordering>,
    world: BTreeMap<u32, usize>,
    entities: BTreeMap<u32, usize>,
    targets: BTreeSet<u32>,
    live: BTreeMap<u32, usize>,
    tokens: BTreeMap<u32, usize>,
    materialized_world: BTreeMap<u32, (usize, usize)>,
    materialized_entities: BTreeMap<u32, (usize, usize)>,
    custody_positions: BTreeMap<u32, usize>,
}

impl ScopePlan {
    pub fn capture(
        vmf: &Vmf,
        persist: bool,
        work: &mut usize,
        control: &Control,
    ) -> Result<Self, String> {
        names::charge(
            work,
            vmf.entities.len().saturating_add(vmf.world.solids.len()),
            control,
        )?;
        let mut order = map_core::instance_order::read(vmf)?;
        if order.is_none() && persist {
            order = Some(Ordering {
                slots: Vec::new(),
                nested_entities: vmf.entities.iter().map(|entity| entity.id).collect(),
                origin_custody: Vec::new(),
            });
        }
        if let Some(order) = &mut order {
            let roots = order
                .slots
                .iter()
                .map(|slot| slot.original_root)
                .collect::<BTreeSet<_>>();
            let materialized = order
                .slots
                .iter()
                .flat_map(|slot| match &slot.content {
                    Content::Materialized { entities, .. } => entities.as_slice(),
                    Content::LiveReference => &[],
                })
                .copied()
                .collect::<BTreeSet<_>>();
            let mut tokens = order
                .nested_entities
                .iter()
                .copied()
                .collect::<BTreeSet<_>>();
            for entity in &vmf.entities {
                control.checkpoint()?;
                if !materialized.contains(&entity.id) && tokens.insert(entity.id) {
                    order.nested_entities.push(entity.id);
                }
                if is_instance(entity) && !roots.contains(&entity.id) {
                    if order
                        .slots
                        .iter()
                        .any(|slot| slot.original_root == entity.id)
                    {
                        return Err(
                            "New reference reuses a historical collapse slot identity".into()
                        );
                    }
                    order.slots.push(Slot {
                        original_root: entity.id,
                        content: Content::LiveReference,
                    });
                }
            }
            if order.slots.len() > MAX_INSTANCES {
                return Err(format!(
                    "Collapse ordering exceeds {MAX_INSTANCES} root slots"
                ));
            }
            let positions = order
                .nested_entities
                .iter()
                .enumerate()
                .map(|(index, &id)| (id, index))
                .collect::<BTreeMap<_, _>>();
            order
                .slots
                .sort_unstable_by_key(|slot| positions[&slot.original_root]);
            names::charge(work, order.allocation_bytes()?, control)?;
        }
        let world = vmf
            .world
            .solids
            .iter()
            .enumerate()
            .map(|(index, solid)| (solid.id, index))
            .collect::<BTreeMap<_, _>>();
        let entities = vmf
            .entities
            .iter()
            .enumerate()
            .map(|(index, entity)| (entity.id, index))
            .collect::<BTreeMap<_, _>>();
        if world.len() != vmf.world.solids.len() || entities.len() != vmf.entities.len() {
            return Err("Instance ordering requires unique authored identities".into());
        }
        let targets = vmf
            .entities
            .iter()
            .filter(|entity| matches!(entity.classname(), Some("info_lighting" | "info_target")))
            .map(|entity| entity.id)
            .collect();
        let mut result = Self {
            order,
            world,
            entities,
            targets,
            live: BTreeMap::new(),
            tokens: BTreeMap::new(),
            materialized_world: BTreeMap::new(),
            materialized_entities: BTreeMap::new(),
            custody_positions: BTreeMap::new(),
        };
        if let Some(order) = &result.order {
            result.custody_positions = order
                .origin_custody
                .iter()
                .enumerate()
                .map(|(index, custody)| (custody.entity, index))
                .collect();
            result.tokens = order
                .nested_entities
                .iter()
                .enumerate()
                .map(|(index, &id)| (id, index))
                .collect();
            for (index, slot) in order.slots.iter().enumerate() {
                match &slot.content {
                    Content::LiveReference => {
                        result.live.insert(slot.original_root, index);
                    }
                    Content::Materialized { world, entities } => {
                        result.materialized_world.extend(
                            world
                                .iter()
                                .enumerate()
                                .map(|(item, &id)| (id, (index, item))),
                        );
                        result.materialized_entities.extend(
                            entities
                                .iter()
                                .enumerate()
                                .map(|(item, &id)| (id, (index, item))),
                        );
                    }
                }
            }
        } else {
            result.live = vmf
                .entities
                .iter()
                .filter(|entity| !entity.hidden && is_instance(entity))
                .enumerate()
                .map(|(index, entity)| (entity.id, index))
                .collect();
        }
        Ok(result)
    }

    pub fn origin_custody(&self, entity: u32) -> Option<&OriginCustody> {
        self.order
            .as_ref()?
            .origin_custody
            .get(*self.custody_positions.get(&entity)?)
    }

    pub fn roots<'a>(&self, vmf: &'a Vmf) -> Result<Vec<&'a Entity>, String> {
        let mut roots = self
            .live
            .iter()
            .map(|(&id, &rank)| (rank, id))
            .collect::<Vec<_>>();
        roots.sort_unstable();
        roots
            .into_iter()
            .map(|(_, id)| {
                vmf.entities
                    .get(
                        *self
                            .entities
                            .get(&id)
                            .ok_or("Ordered root reference is missing")?,
                    )
                    .ok_or_else(|| "Ordered root reference index is invalid".into())
            })
            .collect()
    }

    fn descend(&self, helper: u32, world: bool) -> Result<Vec<usize>, String> {
        if self.order.is_some() {
            let index = if world {
                *self
                    .live
                    .get(&helper)
                    .ok_or("Nested ordering has no live reference slot")?
            } else {
                *self
                    .tokens
                    .get(&helper)
                    .ok_or("Nested reference has no ordering token")?
            };
            Ok(vec![1, index])
        } else {
            Ok(vec![
                1,
                *self
                    .entities
                    .get(&helper)
                    .ok_or("Nested helper has no source order")?,
            ])
        }
    }

    fn direct(&self, id: u32, world: bool) -> Result<Vec<usize>, String> {
        if world {
            if let Some(&(slot, item)) = self.materialized_world.get(&id) {
                return Ok(vec![1, slot, item]);
            }
            Ok(vec![
                0,
                *self
                    .world
                    .get(&id)
                    .ok_or("World brush has no original order")?,
            ])
        } else {
            if let Some(&(slot, item)) = self.materialized_entities.get(&id) {
                let root = self.order.as_ref().unwrap().slots[slot].original_root;
                return Ok(vec![1, self.tokens[&root], item]);
            }
            let original = *self
                .entities
                .get(&id)
                .ok_or("Entity has no original source order")?;
            if self.targets.contains(&id) {
                Ok(vec![0, original])
            } else {
                Ok(vec![1, self.tokens.get(&id).copied().unwrap_or(original)])
            }
        }
    }

    fn root_key(&self, id: u32, world: bool) -> Result<Vec<usize>, String> {
        let materialized = if world {
            &self.materialized_world
        } else {
            &self.materialized_entities
        };
        if let Some(&(slot, item)) = materialized.get(&id) {
            return Ok(vec![1, slot, item]);
        }
        let positions = if world { &self.world } else { &self.entities };
        Ok(vec![
            0,
            *positions
                .get(&id)
                .ok_or("Authored object has no original order")?,
        ])
    }
}

fn generated_key(
    root: &ScopePlan,
    source: &SourceOrigin,
    world: bool,
    scopes: &BTreeMap<String, Arc<ScopePlan>>,
) -> Result<Vec<usize>, String> {
    let mut key = vec![
        1,
        *root
            .live
            .get(&source.legacy.0)
            .ok_or("Expanded object has no ordered root reference")?,
    ];
    let steps = source.occurrence.steps().collect::<Vec<_>>();
    for (index, step) in steps.iter().enumerate() {
        let scope = scopes
            .get(&step.file)
            .ok_or("Expanded occurrence has no captured ordering scope")?;
        if let Some(next) = steps.get(index + 1) {
            key.extend(scope.descend(next.helper, world)?);
        } else {
            key.extend(scope.direct(source.legacy.2, world)?);
        }
    }
    Ok(key)
}

fn reorder<T>(
    values: &mut [T],
    id: fn(&T) -> ObjectId,
    root: &ScopePlan,
    provenance: &Provenance,
    scopes: &BTreeMap<String, Arc<ScopePlan>>,
    work: &mut usize,
    control: &Control,
) -> Result<(), String> {
    let mut keys = BTreeMap::new();
    let mut unique = BTreeSet::new();
    for value in values.iter() {
        names::charge(work, MAX_EXPANSION_DEPTH * 8 + 1, control)?;
        let object = id(value);
        let (identity, world) = match object {
            ObjectId::Solid(id) => (id, true),
            ObjectId::Entity(id) => (id, false),
        };
        let provenance_key = format!("{}:{identity}", if world { "solid" } else { "entity" });
        let key = if let Some(source) = provenance.get(&provenance_key) {
            generated_key(root, source, world, scopes)?
        } else {
            root.root_key(identity, world)?
        };
        if !unique.insert(key.clone()) {
            return Err("Prepared instance order contains ambiguous source positions".into());
        }
        keys.insert(object, key);
    }
    names::charge(
        work,
        values
            .len()
            .saturating_mul(values.len().max(1).ilog2() as usize + 1)
            .saturating_mul(MAX_EXPANSION_DEPTH * 4),
        control,
    )?;
    values.sort_unstable_by(|a, b| keys[&id(a)].cmp(&keys[&id(b)]));
    control.checkpoint()
}

pub(super) fn apply(
    vmf: &mut Vmf,
    root: &ScopePlan,
    provenance: &Provenance,
    scopes: &BTreeMap<String, Arc<ScopePlan>>,
    work: &mut usize,
    control: &Control,
) -> Result<(), String> {
    reorder(
        &mut vmf.world.solids,
        |solid| ObjectId::Solid(solid.id),
        root,
        provenance,
        scopes,
        work,
        control,
    )?;
    reorder(
        &mut vmf.entities,
        |entity| ObjectId::Entity(entity.id),
        root,
        provenance,
        scopes,
        work,
        control,
    )
}

pub(super) fn materialized(
    root: &ScopePlan,
    vmf: &Vmf,
    provenance: &Provenance,
    selected: &BTreeSet<u32>,
    origin_custody: Vec<OriginCustody>,
    work: &mut usize,
    control: &Control,
) -> Result<Ordering, String> {
    let mut order = root
        .order
        .clone()
        .ok_or("Collapse has no captured persistent ordering plan")?;
    for slot in &mut order.slots {
        control.checkpoint()?;
        if matches!(slot.content, Content::LiveReference) && selected.contains(&slot.original_root)
        {
            names::charge(
                work,
                vmf.world.solids.len().saturating_add(vmf.entities.len()),
                control,
            )?;
            let owned = |key: String| {
                provenance
                    .get(&key)
                    .is_some_and(|source| source.legacy.0 == slot.original_root)
            };
            slot.content = Content::Materialized {
                world: vmf
                    .world
                    .solids
                    .iter()
                    .filter(|solid| owned(format!("solid:{}", solid.id)))
                    .map(|solid| solid.id)
                    .collect(),
                entities: vmf
                    .entities
                    .iter()
                    .filter(|entity| owned(format!("entity:{}", entity.id)))
                    .map(|entity| entity.id)
                    .collect(),
            };
        }
    }
    order.origin_custody.extend(origin_custody);
    order.allocation_bytes()?;
    Ok(order)
}

pub fn collapse_root_order(vmf: &Vmf) -> Result<Vec<ObjectId>, String> {
    let control = Control::default();
    let mut work = 0;
    let plan = ScopePlan::capture(vmf, false, &mut work, &control)?;
    Ok(plan
        .roots(vmf)?
        .into_iter()
        .filter(|entity| !entity.hidden)
        .map(|entity| ObjectId::Entity(entity.id))
        .collect())
}

pub fn remap_collapsed_ordering(
    order: &mut Ordering,
    removed: &[ObjectId],
    world: &BTreeMap<u32, u32>,
    entities: &BTreeMap<u32, u32>,
    control: &Control,
) -> Result<(), String> {
    let selected = removed
        .iter()
        .map(|id| match id {
            ObjectId::Entity(id) => Ok(*id),
            _ => Err("Collapse ordering requires entity roots"),
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut materialized_entities = BTreeSet::new();
    for slot in &mut order.slots {
        control.checkpoint()?;
        if !selected.contains(&slot.original_root) {
            continue;
        }
        let Content::Materialized {
            world: world_ids,
            entities: entity_ids,
        } = &mut slot.content
        else {
            return Err("Selected collapse slot was not materialized".into());
        };
        for id in world_ids {
            *id = *world
                .get(id)
                .ok_or("Collapsed world ordering remap is incomplete")?;
        }
        for id in entity_ids {
            materialized_entities.insert(*id);
            *id = *entities
                .get(id)
                .ok_or("Collapsed entity ordering remap is incomplete")?;
        }
    }
    for custody in &mut order.origin_custody {
        control.checkpoint()?;
        if !materialized_entities.contains(&custody.entity) {
            continue;
        }
        custody.entity = *entities
            .get(&custody.entity)
            .ok_or("Collapsed origin-custody entity remap is incomplete")?;
        for volume in &mut custody.volumes {
            *volume = *world
                .get(volume)
                .ok_or("Collapsed origin-custody volume remap is incomplete")?;
        }
    }
    Ok(())
}
