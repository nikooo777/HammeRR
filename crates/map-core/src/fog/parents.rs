use std::collections::{BTreeMap, BTreeSet};

use source_formats::vmf::{Entity, Vmf};

use super::profile::{bounded, parse_fixed_parent_controller};
use super::{ParsedController, number_property, parse_controller, property};

pub const MAX_PARENT_DEPTH: usize = 16;
pub const MAX_PARENT_ENTITIES: usize = 65_536;
const MAX_PROPERTIES: usize = 1_048_576;
const MAX_NAMES: usize = 65_536;
const MAX_NAME_BYTES: usize = 256;
const MAX_TOTAL_NAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_FRAME_PROPERTIES: usize = 256;
const MAX_FRAME_BYTES: usize = 64 * 1024;

enum Target<'a> {
    Unique(&'a Entity),
    Ambiguous,
}

pub struct ParentContext<'a> {
    names: BTreeMap<String, Target<'a>>,
    ids: BTreeMap<u32, Target<'a>>,
}

fn identifier<'a>(entity: &'a Entity, key: &str) -> Result<Option<&'a str>, String> {
    let value = property(entity, key)?;
    if entity.properties.get_all(key).any(|raw| raw != raw.trim())
        || value.is_some_and(|raw| raw.len() > MAX_NAME_BYTES || raw.chars().any(char::is_control))
    {
        return Err(format!(
            "{key} needs an exact identifier within {MAX_NAME_BYTES} bytes without control or surrounding whitespace"
        ));
    }
    Ok(value)
}

pub fn requires_parent_context(entity: &Entity) -> Result<bool, String> {
    let use_angles = property(entity, "use_angles")?.map_or(Ok(false), |raw| {
        raw.parse::<i32>()
            .map(|value| value != 0)
            .map_err(|_| "use_angles must be an integer boolean")
    })?;
    if !use_angles {
        return Ok(false);
    }
    property(entity, "parentname")?;
    Ok(entity
        .properties
        .get("parentname")
        .is_some_and(|raw| !raw.is_empty()))
}

fn pose(entity: &Entity) -> Result<(), String> {
    if entity.properties.0.len() > MAX_FRAME_PROPERTIES {
        return Err("Fixed fog-parent frame exceeds 256 properties".into());
    }
    let bytes = entity
        .properties
        .iter()
        .try_fold(0usize, |total, (key, value)| {
            total.checked_add(key.len())?.checked_add(value.len())
        })
        .ok_or("Fixed fog-parent property size overflow")?;
    if bytes > MAX_FRAME_BYTES {
        return Err("Fixed fog-parent frame exceeds 64 KiB of properties".into());
    }
    if let Some(origin) = property(entity, "origin")? {
        if origin.len() > MAX_NAME_BYTES {
            return Err("Fixed fog-parent origin exceeds 256 bytes".into());
        }
        crate::measurements::origin_from_entity(entity)?;
    }
    let angles = property(entity, "angles")?;
    let angle = property(entity, "angle")?;
    if angles
        .into_iter()
        .chain(angle)
        .any(|raw| raw.len() > MAX_NAME_BYTES)
    {
        return Err("Fixed fog-parent orientation exceeds 256 bytes".into());
    }
    if angles.is_some() && number_property(entity, "angle", Some(0.0))? != 0.0 {
        return Err("Fixed fog-parent orientation has conflicting angles and angle fields".into());
    }
    let angles = crate::query::point_entity_angles(entity)
        .ok_or("Fixed fog parent needs finite initial angles")?;
    for component in angles.as_vec3().to_array() {
        bounded(component, "Fixed fog-parent angles")?;
    }
    if property(entity, "parentattachment")?.is_some_and(|value| !value.is_empty() && value != "0")
    {
        return Err(
            "Separate parentattachment metadata is outside the fixed fog-parent profile".into(),
        );
    }
    Ok(())
}

impl<'a> ParentContext<'a> {
    pub fn new(vmf: &'a Vmf) -> Result<Self, String> {
        if vmf.entities.len() > MAX_PARENT_ENTITIES {
            return Err("Fog parent context exceeds 65536 entities".into());
        }
        let mut names = BTreeMap::new();
        let mut ids = BTreeMap::new();
        let mut properties = 0usize;
        let mut count = 0usize;
        let mut bytes = 0usize;
        for entity in &vmf.entities {
            ids.entry(entity.id)
                .and_modify(|target| *target = Target::Ambiguous)
                .or_insert(Target::Unique(entity));
            properties = properties
                .checked_add(entity.properties.0.len())
                .filter(|count| *count <= MAX_PROPERTIES)
                .ok_or("Fog parent context exceeds its property scan limit")?;
            for name in entity.properties.get_all("targetname") {
                count += 1;
                if count > MAX_NAMES {
                    return Err("Fog parent context exceeds 65536 targetname entries".into());
                }
                if name.is_empty() || name.len() > MAX_NAME_BYTES {
                    continue;
                }
                bytes = bytes
                    .checked_add(name.len())
                    .filter(|bytes| *bytes <= MAX_TOTAL_NAME_BYTES)
                    .ok_or("Fog parent name storage exceeds 16 MiB")?;
                names
                    .entry(name.to_ascii_lowercase())
                    .and_modify(|target| *target = Target::Ambiguous)
                    .or_insert(Target::Unique(entity));
            }
        }
        Ok(Self { names, ids })
    }

    fn chain(&self, entity: &Entity) -> Result<Vec<u32>, String> {
        if !matches!(self.ids.get(&entity.id), Some(Target::Unique(source)) if std::ptr::eq(*source, entity))
        {
            return Err("Fog controller is missing or duplicated in its parent context".into());
        }
        pose(entity)?;
        let mut visited = BTreeSet::from([entity.id]);
        let mut chain = Vec::new();
        let mut current = entity;
        while let Some(binding) = identifier(current, "parentname")?.filter(|name| !name.is_empty())
        {
            if chain.len() == MAX_PARENT_DEPTH {
                return Err(format!(
                    "Fog parent chain exceeds {MAX_PARENT_DEPTH} parents"
                ));
            }
            let (name, attachment) = binding
                .split_once(',')
                .map_or((binding, None), |(name, attachment)| {
                    (name, Some(attachment))
                });
            if name.is_empty() || name.contains(['*', '?', '!']) {
                return Err("Fog parentname requires one exact authored targetname".into());
            }
            if attachment.is_some() {
                return Err(
                    "Fog parent attachments need a separately qualified fixed attachment frame"
                        .into(),
                );
            }
            let parent = match self.names.get(&name.to_ascii_lowercase()) {
                Some(Target::Unique(parent)) => *parent,
                Some(Target::Ambiguous) => {
                    return Err("Fog parentname matches multiple targetname entries".into());
                }
                None => return Err("Fog parentname has no authored target".into()),
            };
            pose(parent)?;
            if !visited.insert(parent.id) {
                return Err("Fog parent chain contains a cycle".into());
            }
            if !matches!(self.ids.get(&parent.id), Some(Target::Unique(_))) {
                return Err("Fog parent entity ID is duplicated".into());
            }
            identifier(parent, "targetname")?;
            if !crate::connections::name_matches(
                parent.properties.get("targetname").unwrap_or_default(),
                name,
            ) {
                return Err("Fog parent targetname differs from its indexed binding".into());
            }
            if parent.hidden || !parent.solids.is_empty() {
                return Err("Fog parents must be visible point entities".into());
            }
            if !identifier(parent, "classname")?
                .is_some_and(|class| class.eq_ignore_ascii_case("info_target"))
            {
                return Err("Fog use_angles supports only fixed ordinary info_target parent chains; models, movers and resetting classes are unavailable".into());
            }
            let flags = property(parent, "spawnflags")?
                .unwrap_or("0")
                .parse::<u32>()
                .map_err(|_| "Fixed info_target spawnflags must be an unsigned integer")?;
            if flags & !1 != 0 {
                return Err(
                    "Fixed fog-parent info_target admits only transmission spawnflag 1".into(),
                );
            }
            chain.push(parent.id);
            current = parent;
        }
        Ok(chain)
    }

    pub fn parse_controller(&self, entity: &Entity) -> Result<ParsedController, String> {
        if !requires_parent_context(entity)? {
            return parse_controller(entity);
        }
        let chain = self.chain(entity)?;
        let mut parsed = parse_fixed_parent_controller(entity)?;
        parsed.parent_chain = chain;
        Ok(parsed)
    }
}

pub fn parse_controller_in(vmf: &Vmf, entity: &Entity) -> Result<ParsedController, String> {
    if !requires_parent_context(entity)? {
        return parse_controller(entity);
    }
    ParentContext::new(vmf)?.parse_controller(entity)
}
