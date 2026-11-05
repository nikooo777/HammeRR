use source_formats::vmf::Entity;

use super::{
    MAX_IDENTIFIER_BYTES, MAX_SLACK, MAX_WIDTH, MIN_SLACK, SpanSettings, identifier, number,
    property,
};

fn integer(entity: &Entity, key: &str, default: i32) -> Result<i32, String> {
    property(entity, key)?.map_or(Ok(default), |raw| {
        raw.parse::<i32>()
            .map_err(|_| format!("{key} must be an integer"))
    })
}

fn flag(entity: &Entity, key: &str) -> Result<bool, String> {
    Ok(integer(entity, key, 0)? == 1)
}

impl SpanSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(2..=super::MAX_PHYSICS_NODES).contains(&self.nodes)
            || self.subdiv_authored > 15
            || self.subdiv != usize::from(self.subdiv_authored.min(7))
        {
            return Err("Rope physics nodes or subdivision exceed the supported limits".into());
        }
        if !(MIN_SLACK as i32..=MAX_SLACK as i32).contains(&self.slack) {
            return Err(format!("Slack must be between {MIN_SLACK} and {MAX_SLACK}"));
        }
        if !self.width.is_finite() || !(1.0..=MAX_WIDTH).contains(&self.width) {
            return Err(format!("Width must be between 1 and {MAX_WIDTH}"));
        }
        if !self.texture_scale.is_finite() || !(0.1..=10.0).contains(&self.texture_scale) {
            return Err("TextureScale must be between 0.1 and 10".into());
        }
        if self.material.is_empty() || self.material.len() > MAX_IDENTIFIER_BYTES {
            return Err("RopeMaterial needs a bounded material name".into());
        }
        if !self.scroll_speed.is_finite() || self.scroll_speed.abs() > 1024.0 {
            return Err("ScrollSpeed must be finite within ±1024".into());
        }
        if self.inert_keys.len() > 5
            || self.inert_keys.iter().any(|key| {
                !["Stiff", "Stiffness", "LockedPoints", "RopeFlags", "Gravity"]
                    .contains(&key.as_str())
            })
        {
            return Err("Rope ignored metadata list exceeds its bounded known keys".into());
        }
        Ok(())
    }
}

pub(super) fn parse(entity: &Entity) -> Result<SpanSettings, String> {
    let nodes = match property(entity, "Type")? {
        None => 5,
        Some(_) => match integer(entity, "Type", 0)? {
            0 => 10,
            1 => 4,
            _ => 2,
        },
    };
    let subdiv = integer(entity, "Subdiv", 2)?;
    if !(0..=15).contains(&subdiv) {
        return Err("Subdiv must fit its authored 4-bit range 0–15".into());
    }
    let material = identifier(entity, "RopeMaterial")?.filter(|name| !name.is_empty());
    let legacy = integer(entity, "RopeShader", 0)?;
    let material = material.unwrap_or(match legacy {
        0 => "cable/cable",
        1 => "cable/rope",
        _ => "cable/chain",
    });
    let settings = SpanSettings {
        nodes,
        subdiv_authored: subdiv as u8,
        subdiv: subdiv.min(7) as usize,
        slack: integer(entity, "Slack", 0)?,
        width: number(entity, "Width", 2.0)?,
        texture_scale: number(entity, "TextureScale", 4.0)?,
        material: material.to_owned(),
        scroll_speed: number(entity, "ScrollSpeed", 0.0)?,
        dangling: flag(entity, "Dangling")?,
        collide: flag(entity, "Collide")?,
        no_wind: flag(entity, "NoWind")?,
        barbed: flag(entity, "Barbed")?,
        breakable: flag(entity, "Breakable")?,
        inert_keys: ["Stiff", "Stiffness", "LockedPoints", "RopeFlags", "Gravity"]
            .into_iter()
            .filter(|key| entity.properties.get(key).is_some())
            .map(str::to_owned)
            .collect(),
    };
    settings.validate()?;
    Ok(settings)
}
