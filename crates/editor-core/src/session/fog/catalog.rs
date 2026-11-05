use std::collections::BTreeMap;

use map_core::fog::{ParsedController, parse_controller};
use source_formats::vmf::{Entity, Vmf};

pub const MAX_CONTROLLERS: usize = 128;
pub const MAX_ENTITIES: usize = 65_536;
pub const MAX_CONTROLLER_BYTES: usize = 64 * 1024;
pub const MAX_CONTROLLER_PROPERTIES: usize = 256;

pub(super) struct Controller {
    pub targetname: Option<String>,
    pub parsed: Result<ParsedController, String>,
}

pub(super) struct Catalog {
    pub controllers: BTreeMap<u32, Controller>,
}

pub(super) fn parse(entity: &Entity) -> Result<ParsedController, String> {
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
    parse_controller(entity).map_err(|error| format!("entity:{}: {error}", entity.id))
}

impl Catalog {
    pub fn capture(vmf: &Vmf) -> Result<Self, String> {
        if vmf.entities.len() > MAX_ENTITIES {
            return Err("Fog catalog exceeds its entity scan limit".into());
        }
        let mut controllers = BTreeMap::new();
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
                        parsed: parse(entity),
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
