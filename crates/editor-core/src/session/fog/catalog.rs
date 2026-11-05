use std::collections::{BTreeMap, BTreeSet};

pub use map_core::fog::MAX_PARENT_ENTITIES as MAX_ENTITIES;
use map_core::fog::{ParentContext, ParsedController, parse_controller, requires_parent_context};
use source_formats::vmf::{Entity, Vmf};

pub const MAX_CONTROLLERS: usize = 128;
pub const MAX_CONTROLLER_BYTES: usize = 64 * 1024;
pub const MAX_CONTROLLER_PROPERTIES: usize = 256;
pub const MAX_TARGET_NAMES: usize = 65_536;
pub const MAX_TARGET_NAME_BYTES: usize = 256;
pub const MAX_TOTAL_TARGET_NAME_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_NAMESPACE_PROPERTIES: usize = 4 * 1024 * 1024;
pub const MAX_NAMESPACE_MATCH_BYTES: usize = 128 * 1024 * 1024;

pub(super) struct Controller {
    pub targetname: Option<String>,
    pub parsed: Result<ParsedController, String>,
    pub switch_target: Result<String, String>,
}

pub(super) struct Catalog {
    pub controllers: BTreeMap<u32, Controller>,
}

fn switch_name(entity: &Entity) -> Result<String, String> {
    let mut names = entity
        .properties
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case("targetname"));
    let name = names
        .next()
        .map(|(_, name)| name)
        .ok_or("Player fog switches require an authored targetname")?;
    if names.next().is_some() {
        return Err("Player fog target has duplicate targetname keys".into());
    }
    if name.is_empty()
        || name != name.trim()
        || name.len() > MAX_TARGET_NAME_BYTES
        || name.starts_with('!')
        || name.contains(['*', '?', ',', '"', '[', ']'])
        || name.chars().any(char::is_control)
    {
        return Err("Player fog target needs an exact nonempty literal targetname within 256 bytes; special, wildcard and attachment forms are unsupported".into());
    }
    Ok(name.to_owned())
}

fn qualify_switch_targets(
    vmf: &Vmf,
    controllers: &mut BTreeMap<u32, Controller>,
) -> Result<(), String> {
    let mut ids = BTreeSet::from([vmf.world.id]);
    for entity in &vmf.entities {
        if !ids.insert(entity.id) {
            return Err("Player fog target namespace has duplicate entity IDs".into());
        }
    }
    let mut counts = controllers
        .values()
        .filter_map(|controller| controller.switch_target.as_ref().ok())
        .map(|name| (name.clone(), 0usize))
        .collect::<BTreeMap<_, _>>();
    if counts.is_empty() {
        return Ok(());
    }
    let mut properties = 0usize;
    let mut names = 0usize;
    let mut bytes = 0usize;
    let mut remaining = MAX_NAMESPACE_MATCH_BYTES;
    for properties_set in std::iter::once(&vmf.world.properties)
        .chain(vmf.entities.iter().map(|entity| &entity.properties))
    {
        for (key, value) in properties_set.iter() {
            properties = properties
                .checked_add(1)
                .filter(|count| *count <= MAX_NAMESPACE_PROPERTIES)
                .ok_or("Player fog target namespace exceeds its property scan limit")?;
            if !key.eq_ignore_ascii_case("targetname") {
                continue;
            }
            names = names
                .checked_add(1)
                .filter(|count| *count <= MAX_TARGET_NAMES)
                .ok_or(
                    "Player fog target namespace exceeds 65536 names; no partial lookup was used",
                )?;
            bytes = bytes
                .checked_add(value.len())
                .filter(|count| *count <= MAX_TOTAL_TARGET_NAME_BYTES)
                .ok_or("Player fog target namespace exceeds 16 MiB; no partial lookup was used")?;
            for (query, count) in &mut counts {
                if map_core::connections::bounded_event_name_matches(value, query, &mut remaining)?
                {
                    *count += 1;
                }
            }
        }
    }
    for controller in controllers.values_mut() {
        if let Ok(name) = &controller.switch_target
            && counts.get(name) != Some(&1)
        {
            controller.switch_target =
                Err("Player fog targetname is ambiguous in the complete authored namespace".into());
        }
    }
    Ok(())
}

pub(super) fn parse(vmf: &Vmf, entity: &Entity) -> Result<ParsedController, String> {
    parse_with_context(vmf, entity, &mut None)
}

fn parse_with_context<'a>(
    vmf: &'a Vmf,
    entity: &'a Entity,
    parents: &mut Option<Result<ParentContext<'a>, String>>,
) -> Result<ParsedController, String> {
    if !entity
        .classname()
        .is_some_and(|name| name.eq_ignore_ascii_case("env_fog_controller"))
    {
        return Err(format!("entity:{} is not an env_fog_controller", entity.id));
    }
    if entity.properties.0.len() > MAX_CONTROLLER_PROPERTIES {
        return Err("Fog controller has too many properties".into());
    }
    let bytes = entity
        .properties
        .iter()
        .try_fold(0usize, |total, (key, value)| {
            total.checked_add(key.len())?.checked_add(value.len())
        })
        .ok_or("Fog controller property size overflow")?;
    if bytes > MAX_CONTROLLER_BYTES {
        return Err("Fog controller exceeds its property byte limit".into());
    }
    let result = if requires_parent_context(entity)? {
        match parents.get_or_insert_with(|| ParentContext::new(vmf)) {
            Ok(context) => context.parse_controller(entity),
            Err(error) => Err(error.clone()),
        }
    } else {
        parse_controller(entity)
    };
    result.map_err(|error| format!("entity:{}: {error}", entity.id))
}

impl Catalog {
    pub fn capture(vmf: &Vmf) -> Result<Self, String> {
        if vmf.entities.len() > MAX_ENTITIES {
            return Err("Fog catalog exceeds its entity scan limit".into());
        }
        let mut controllers = BTreeMap::new();
        let mut parents = None;
        for entity in &vmf.entities {
            if !entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("env_fog_controller"))
            {
                continue;
            }
            if controllers.len() >= MAX_CONTROLLERS {
                return Err("Fog catalog exceeds 128 controllers".into());
            }
            let targetname = entity
                .properties
                .get("targetname")
                .map(|value| value.chars().take(256).collect());
            if controllers
                .insert(
                    entity.id,
                    Controller {
                        targetname,
                        parsed: parse_with_context(vmf, entity, &mut parents),
                        switch_target: switch_name(entity),
                    },
                )
                .is_some()
            {
                return Err("Fog controllers have duplicate entity IDs".into());
            }
        }
        if let Err(error) = qualify_switch_targets(vmf, &mut controllers) {
            for controller in controllers.values_mut() {
                controller.switch_target = Err(error.clone());
            }
        }
        Ok(Self { controllers })
    }
}
