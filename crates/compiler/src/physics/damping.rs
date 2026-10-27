use source_formats::vmf::Vmf;

use crate::{Control, PhysicsMode, map::Map};

pub const WATER_DAMPING_KEY: &str = "_hammerr_water_damping";
pub const MAX_WATER_DAMPING: f64 = 0.01;
pub const DEFAULT_WATER_DAMPING: f32 = 0.01;
pub const MAX_WATER_DAMPING_VALUE_BYTES: usize = 32;

pub fn validate_water_damping(raw: Option<&str>) -> Result<(), String> {
    raw.map(parse).transpose().map(|_| ())
}

fn invalid() -> String {
    format!(
        "{WATER_DAMPING_KEY} needs one finite dimensionless pressure coefficient from 0 to {MAX_WATER_DAMPING} and at most {MAX_WATER_DAMPING_VALUE_BYTES} ASCII bytes; nonzero values must survive native float conversion"
    )
}

fn parse(raw: &str) -> Result<f32, String> {
    if raw.len() > MAX_WATER_DAMPING_VALUE_BYTES || !raw.is_ascii() {
        return Err(invalid());
    }
    let mut words = raw.split_ascii_whitespace();
    let value = super::finite_decimal(words.next().ok_or_else(invalid)?).ok_or_else(invalid)?;
    if words.next().is_some() || !(0.0..=MAX_WATER_DAMPING).contains(&value) {
        return Err(invalid());
    }
    let canonical = if value == 0.0 { 0.0 } else { value as f32 };
    if value != 0.0 && canonical == 0.0 {
        return Err(invalid());
    }
    validate(canonical)?;
    Ok(canonical)
}

pub(super) fn validate(value: f32) -> Result<(), String> {
    if !value.is_finite() || !(0.0..=DEFAULT_WATER_DAMPING).contains(&value) {
        return Err(invalid());
    }
    Ok(())
}

pub(crate) fn from_world(vmf: &Vmf, physics: PhysicsMode) -> Result<Option<f32>, String> {
    let mut values = vmf.world.properties.get_all(WATER_DAMPING_KEY);
    let Some(raw) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(format!(
            "World {} repeats {WATER_DAMPING_KEY}",
            vmf.world.id
        ));
    }
    let value = parse(raw).map_err(|error| format!("World {}: {error}", vmf.world.id))?;
    if physics != PhysicsMode::Engine {
        return Err(format!(
            "World {} {WATER_DAMPING_KEY} requires CS:S engine physics",
            vmf.world.id
        ));
    }
    Ok(Some(value))
}

pub(crate) fn admit_map(map: &Map, control: &Control) -> Result<(), String> {
    if map.water_damping.is_none() {
        return Ok(());
    }
    super::admit_authored_liquid(map, control, WATER_DAMPING_KEY)
}

#[cfg(test)]
mod tests;
