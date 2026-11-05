use super::profile::{angles_direction, bounded, color, vector};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    SetStartDist(f32),
    SetEndDist(f32),
    SetMaxDensity(f32),
    TurnOn,
    TurnOff,
    SetColor([u8; 3]),
    SetColorSecondary([u8; 3]),
    SetFarZ(i32),
    SetAngles([f32; 3]),
    SetRadial(bool),
    SetColorLerpTo([u8; 3]),
    SetColorSecondaryLerpTo([u8; 3]),
    SetStartDistLerpTo(f32),
    SetEndDistLerpTo(f32),
    StartFogTransition,
}

#[derive(Clone, Copy, Debug)]
pub struct InputChoice {
    pub name: &'static str,
    pub kind: &'static str,
    pub help: &'static str,
    pub has_value: bool,
}

const CHOICES: [InputChoice; 15] = [
    InputChoice {
        name: "SetStartDist",
        kind: "number",
        help: "Set the current start distance",
        has_value: true,
    },
    InputChoice {
        name: "SetEndDist",
        kind: "number",
        help: "Set the current end distance",
        has_value: true,
    },
    InputChoice {
        name: "SetMaxDensity",
        kind: "number",
        help: "Set density; the renderer clamps it to 0–1",
        has_value: true,
    },
    InputChoice {
        name: "TurnOn",
        kind: "void",
        help: "Enable this controller",
        has_value: false,
    },
    InputChoice {
        name: "TurnOff",
        kind: "void",
        help: "Disable this controller; far-Z remains independent",
        has_value: false,
    },
    InputChoice {
        name: "SetColor",
        kind: "color",
        help: "Set primary RGB bytes",
        has_value: true,
    },
    InputChoice {
        name: "SetColorSecondary",
        kind: "color",
        help: "Set secondary RGB bytes",
        has_value: true,
    },
    InputChoice {
        name: "SetFarZ",
        kind: "integer",
        help: "Set the far clip; nonpositive disables its override",
        has_value: true,
    },
    InputChoice {
        name: "SetAngles",
        kind: "angles",
        help: "Set world pitch yaw roll and the negated forward fog direction",
        has_value: true,
    },
    InputChoice {
        name: "SetRadial",
        kind: "boolean",
        help: "Use radial rather than view-depth distance",
        has_value: true,
    },
    InputChoice {
        name: "SetColorLerpTo",
        kind: "color",
        help: "Stage primary RGB bytes for a transition",
        has_value: true,
    },
    InputChoice {
        name: "SetColorSecondaryLerpTo",
        kind: "color",
        help: "Stage secondary RGB bytes for a transition",
        has_value: true,
    },
    InputChoice {
        name: "SetStartDistLerpTo",
        kind: "number",
        help: "Stage a target start distance",
        has_value: true,
    },
    InputChoice {
        name: "SetEndDistLerpTo",
        kind: "number",
        help: "Stage a target end distance",
        has_value: true,
    },
    InputChoice {
        name: "StartFogTransition",
        kind: "void",
        help: "Start with authored foglerptime; zero duration is unavailable",
        has_value: false,
    },
];

pub fn input_choices() -> &'static [InputChoice] {
    &CHOICES
}

fn number(raw: &str, name: &str) -> Result<f32, String> {
    bounded(
        raw.parse::<f32>()
            .map_err(|_| format!("{name} needs a finite number"))?,
        name,
    )
}

impl Input {
    pub fn parse(name: &str, value: Option<&str>) -> Result<Self, String> {
        if name.len() > 64 || value.is_some_and(|value| value.len() > 256) {
            return Err("Fog input name or value exceeds its byte limit".into());
        }
        let choice = CHOICES
            .iter()
            .find(|choice| choice.name.eq_ignore_ascii_case(name.trim()))
            .ok_or("Unknown fog controller input")?;
        let raw = value.unwrap_or("").trim();
        if choice.has_value && raw.is_empty() {
            return Err(format!("{} needs an explicit value", choice.name));
        }
        if !choice.has_value && !raw.is_empty() {
            return Err(format!("{} takes no value", choice.name));
        }
        let input = match choice.name {
            "SetStartDist" => Self::SetStartDist(number(raw, name)?),
            "SetEndDist" => Self::SetEndDist(number(raw, name)?),
            "SetMaxDensity" => Self::SetMaxDensity(number(raw, name)?),
            "TurnOn" => Self::TurnOn,
            "TurnOff" => Self::TurnOff,
            "SetColor" => Self::SetColor(color(raw, name)?),
            "SetColorSecondary" => Self::SetColorSecondary(color(raw, name)?),
            "SetFarZ" => Self::SetFarZ(raw.parse::<i32>().map_err(|_| "SetFarZ needs an integer")?),
            "SetAngles" => Self::SetAngles(vector(raw, name)?),
            "SetRadial" => Self::SetRadial(
                raw.parse::<i32>()
                    .map_err(|_| "SetRadial needs an integer boolean")?
                    != 0,
            ),
            "SetColorLerpTo" => Self::SetColorLerpTo(color(raw, name)?),
            "SetColorSecondaryLerpTo" => Self::SetColorSecondaryLerpTo(color(raw, name)?),
            "SetStartDistLerpTo" => Self::SetStartDistLerpTo(number(raw, name)?),
            "SetEndDistLerpTo" => Self::SetEndDistLerpTo(number(raw, name)?),
            "StartFogTransition" => Self::StartFogTransition,
            _ => unreachable!("Fog input catalog and parser share fixed names"),
        };
        input.validate()?;
        Ok(input)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::SetStartDist(_) => "SetStartDist",
            Self::SetEndDist(_) => "SetEndDist",
            Self::SetMaxDensity(_) => "SetMaxDensity",
            Self::TurnOn => "TurnOn",
            Self::TurnOff => "TurnOff",
            Self::SetColor(_) => "SetColor",
            Self::SetColorSecondary(_) => "SetColorSecondary",
            Self::SetFarZ(_) => "SetFarZ",
            Self::SetAngles(_) => "SetAngles",
            Self::SetRadial(_) => "SetRadial",
            Self::SetColorLerpTo(_) => "SetColorLerpTo",
            Self::SetColorSecondaryLerpTo(_) => "SetColorSecondaryLerpTo",
            Self::SetStartDistLerpTo(_) => "SetStartDistLerpTo",
            Self::SetEndDistLerpTo(_) => "SetEndDistLerpTo",
            Self::StartFogTransition => "StartFogTransition",
        }
    }

    pub fn value(self) -> Option<String> {
        match self {
            Self::TurnOn | Self::TurnOff | Self::StartFogTransition => None,
            Self::SetStartDist(value)
            | Self::SetEndDist(value)
            | Self::SetMaxDensity(value)
            | Self::SetStartDistLerpTo(value)
            | Self::SetEndDistLerpTo(value) => Some(value.to_string()),
            Self::SetColor(value)
            | Self::SetColorSecondary(value)
            | Self::SetColorLerpTo(value)
            | Self::SetColorSecondaryLerpTo(value) => {
                Some(format!("{} {} {}", value[0], value[1], value[2]))
            }
            Self::SetFarZ(value) => Some(value.to_string()),
            Self::SetAngles(value) => Some(format!("{} {} {}", value[0], value[1], value[2])),
            Self::SetRadial(value) => Some(u8::from(value).to_string()),
        }
    }

    pub(super) fn validate(self) -> Result<(), String> {
        match self {
            Self::SetStartDist(value)
            | Self::SetEndDist(value)
            | Self::SetMaxDensity(value)
            | Self::SetStartDistLerpTo(value)
            | Self::SetEndDistLerpTo(value) => {
                bounded(value, self.name())?;
            }
            Self::SetFarZ(value) => {
                bounded(value as f32, self.name())?;
            }
            Self::SetAngles(value) => {
                for component in value {
                    bounded(component, "SetAngles")?;
                }
                if !angles_direction(value).into_iter().all(f32::is_finite) {
                    return Err("SetAngles cannot produce a finite direction".into());
                }
            }
            _ => {}
        }
        Ok(())
    }
}
