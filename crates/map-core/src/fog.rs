use source_formats::vmf::Entity;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    color: [u8; 3],
    start: f32,
    end: f32,
    max_density: f32,
    enabled: bool,
}

impl Fog {
    pub fn new(
        color: [u8; 3],
        start: f32,
        end: f32,
        max_density: f32,
        enabled: bool,
    ) -> Result<Self, String> {
        if !start.is_finite()
            || !end.is_finite()
            || start < 0.0
            || end <= start
            || !(1.0 / (end - start)).is_finite()
        {
            return Err("fogstart and fogend need finite values, 0 ≤ start < end and a finite inverse range".into());
        }
        if !max_density.is_finite() || !(0.0..=1.0).contains(&max_density) {
            return Err("fogmaxdensity must be finite and between 0 and 1".into());
        }
        Ok(Self {
            color,
            start,
            end,
            max_density,
            enabled,
        })
    }
    pub fn color(self) -> [u8; 3] {
        self.color
    }
    pub fn start(self) -> f32 {
        self.start
    }
    pub fn end(self) -> f32 {
        self.end
    }
    pub fn max_density(self) -> f32 {
        self.max_density
    }
    pub fn enabled(self) -> bool {
        self.enabled
    }
}

fn property<'a>(entity: &'a Entity, key: &str) -> Result<Option<&'a str>, String> {
    let mut values = entity
        .properties
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(key));
    let value = values.next().map(|(_, value)| value.trim());
    if values.next().is_some() {
        return Err(format!(
            "{key} has ambiguous case-insensitive duplicate keys"
        ));
    }
    Ok(value)
}

pub fn number_property(entity: &Entity, key: &str, default: Option<f32>) -> Result<f32, String> {
    let value = match property(entity, key)? {
        Some(value) => value
            .parse::<f32>()
            .map_err(|_| format!("{key} must be a finite number"))?,
        None => default.ok_or_else(|| format!("{key} is missing; author an explicit value"))?,
    };
    if !value.is_finite() {
        return Err(format!("{key} must be a finite number"));
    }
    Ok(value)
}

pub fn boolean_property(entity: &Entity, key: &str) -> Result<bool, String> {
    match property(entity, key)?.unwrap_or("0") {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("{key} must be 0 or 1")),
    }
}

pub fn color_property(entity: &Entity, key: &str) -> Result<[u8; 3], String> {
    let value = property(entity, key)?
        .ok_or_else(|| format!("{key} is missing; author an explicit RGB value"))?;
    let error = || format!("{key} needs exactly three integer RGB channels from 0 to 255");
    let mut channels = value.split_whitespace();
    let mut color = [0; 3];
    for channel in &mut color {
        *channel = channels
            .next()
            .ok_or_else(error)?
            .parse::<u8>()
            .map_err(|_| error())?;
    }
    if channels.next().is_some() {
        return Err(error());
    }
    Ok(color)
}

pub fn parse_static_ldr(entity: &Entity) -> Result<Fog, String> {
    if entity.hidden {
        return Err(
            "Authored-hidden fog controller is unavailable; show it before previewing".into(),
        );
    }
    if !entity.solids.is_empty() {
        return Err("env_fog_controller must be a point entity".into());
    }
    if boolean_property(entity, "fogblend")? {
        return Err("fogblend=1 directional color blending is unsupported".into());
    }
    if boolean_property(entity, "fogradial")? {
        return Err("fogradial=1 radial fog is unsupported; this preview uses view depth".into());
    }
    if number_property(entity, "farz", Some(-1.0))? > 0.0 {
        return Err("Positive farz clipping is unsupported".into());
    }
    for key in ["HDRColorScale", "ZoomFogScale"] {
        if number_property(entity, key, Some(1.0))? != 1.0 {
            return Err(format!("{key} must be 1 for this LDR preview"));
        }
    }
    if number_property(entity, "foglerptime", Some(0.0))? < 0.0 {
        return Err("foglerptime must be nonnegative".into());
    }
    Fog::new(
        color_property(entity, "fogcolor")?,
        number_property(entity, "fogstart", None)?,
        number_property(entity, "fogend", None)?,
        number_property(entity, "fogmaxdensity", Some(1.0))?,
        boolean_property(entity, "fogenable")?,
    )
}
