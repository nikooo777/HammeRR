use std::collections::BTreeMap;

pub use map_core::fog::MAX_PARENT_ENTITIES as MAX_ENTITIES;
use map_core::fog::{ParentContext, ParsedController, parse_controller, requires_parent_context};
use source_formats::vmf::{Entity, Vmf};

pub const MAX_CONTROLLERS: usize = 128;
pub const MAX_CONTROLLER_BYTES: usize = 64 * 1024;
pub const MAX_CONTROLLER_PROPERTIES: usize = 256;

pub(super) struct Controller {
    pub targetname: Option<String>,
    pub parsed: Result<ParsedController, String>,
}

pub(super) struct Catalog {
    pub controllers: BTreeMap<u32, Controller>,
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
                    },
                )
                .is_some()
            {
                return Err("Fog controllers have duplicate entity IDs".into());
            }
        }
        Ok(Self { controllers })
    }
}
