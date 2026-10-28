use source_formats::vmf::Vmf;

use crate::{Control, PhysicsMode, map::Map};

pub const WATER_CURRENT_VELOCITY_KEY: &str = "_hammerr_water_current_velocity";
pub const MAX_WATER_CURRENT_SPEED: f64 = 128.0;
pub const MAX_WATER_CURRENT_VALUE_BYTES: usize = 128;
pub const WATER_CURRENT_COMPONENTS: usize = 3;
const METERS_PER_HAMMER_UNIT: f32 = 0.0254;

pub fn validate_water_current_velocity(raw: Option<&str>) -> Result<(), String> {
    raw.map(parse).transpose().map(|_| ())
}

fn invalid() -> String {
    format!(
        "{WATER_CURRENT_VELOCITY_KEY} needs exactly three finite world XYZ velocities in HU/s, magnitude at most {MAX_WATER_CURRENT_SPEED}, and at most {MAX_WATER_CURRENT_VALUE_BYTES} bytes; nonzero values must survive native float and unit conversion"
    )
}

fn magnitude(value: [f64; 3]) -> f64 {
    value[0].hypot(value[1]).hypot(value[2])
}

fn parse(raw: &str) -> Result<[f32; 3], String> {
    if raw.len() > MAX_WATER_CURRENT_VALUE_BYTES {
        return Err(invalid());
    }
    let mut words = raw.split_ascii_whitespace();
    let mut values = [0.0; WATER_CURRENT_COMPONENTS];
    for value in &mut values {
        let word = words.next().ok_or_else(invalid)?;
        *value = super::finite_decimal(word).ok_or_else(invalid)?;
    }
    if words.next().is_some() || magnitude(values) > MAX_WATER_CURRENT_SPEED {
        return Err(invalid());
    }
    let canonical = values.map(|value| if value == 0.0 { 0.0 } else { value as f32 });
    if values
        .iter()
        .zip(canonical)
        .any(|(&source, value)| source != 0.0 && value == 0.0)
    {
        return Err(invalid());
    }
    validate(canonical)?;
    Ok(canonical)
}

pub(super) fn validate(value: [f32; 3]) -> Result<(), String> {
    if value.iter().any(|value| !value.is_finite())
        || magnitude(value.map(f64::from)) > MAX_WATER_CURRENT_SPEED
        || value
            .iter()
            .any(|&component| component != 0.0 && component * METERS_PER_HAMMER_UNIT == 0.0)
    {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn native(value: Option<[f32; 3]>) -> [f32; 3] {
    value.unwrap_or([0.0; 3]).map(|value| {
        if value == 0.0 {
            0.0
        } else {
            value * METERS_PER_HAMMER_UNIT
        }
    })
}

pub(crate) fn from_world(vmf: &Vmf, physics: PhysicsMode) -> Result<Option<[f32; 3]>, String> {
    let mut values = vmf.world.properties.get_all(WATER_CURRENT_VELOCITY_KEY);
    let Some(raw) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(format!(
            "World {} repeats {WATER_CURRENT_VELOCITY_KEY}",
            vmf.world.id
        ));
    }
    let value = parse(raw).map_err(|error| format!("World {}: {error}", vmf.world.id))?;
    if physics != PhysicsMode::Engine {
        return Err(format!(
            "World {} {WATER_CURRENT_VELOCITY_KEY} requires CS:S engine physics",
            vmf.world.id
        ));
    }
    Ok(Some(value))
}

pub(crate) fn admit_map(map: &Map, control: &Control) -> Result<(), String> {
    if map.water_current.is_none() {
        return Ok(());
    }
    super::admit_authored_liquid(map, control, WATER_CURRENT_VELOCITY_KEY)
}
