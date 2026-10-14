use std::collections::HashMap;

use glam::DVec3;
use source_formats::bsp::{MAX_LIGHTING_BYTES, RgbExp32};

use super::bake::Budget;
use super::{encode_color, property};
use crate::Control;
use crate::map::{Map, MapEntity};

const FIRST_SWITCHABLE: u8 = 32;
const MAX_SWITCHABLE_GROUPS: usize = 32;
const MAX_PATTERN_BYTES: usize = 64;
const MAX_FACE_STYLES: usize = 4;

fn appearance(entity: &MapEntity, key: &str) -> Result<u8, String> {
    let value = property(entity, key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("0")
        .parse::<u8>()
        .map_err(|_| format!("Light {key} must be a whole number from 0 to 31"))?;
    if value >= FIRST_SWITCHABLE {
        return Err(format!(
            "Light {key} must be from 0 to 31; named light styles are assigned by the compiler"
        ));
    }
    Ok(value)
}

#[derive(PartialEq, Eq)]
struct State {
    pattern: String,
    appearance: u8,
    dark: bool,
}

pub(crate) fn prepare(map: &mut Map<'_>, control: &Control) -> Result<usize, String> {
    let mut groups: HashMap<String, (u8, State)> = HashMap::new();
    for entity in &mut map.entities {
        control.checkpoint()?;
        if !entity.classname.eq_ignore_ascii_case("light")
            && !entity.classname.eq_ignore_ascii_case("light_spot")
        {
            continue;
        }
        let result = prepare_entity(entity, &mut groups);
        if let Err(error) = result {
            return Err(format!("entity:{}: {error}", entity.id));
        }
    }
    Ok(groups.len())
}

fn prepare_entity(
    entity: &mut MapEntity,
    groups: &mut HashMap<String, (u8, State)>,
) -> Result<(), String> {
    let style = appearance(entity, "style")?;
    let default = appearance(entity, "defaultstyle")?;
    let pattern = property(entity, "pattern").unwrap_or_default();
    if pattern.len() > MAX_PATTERN_BYTES || !pattern.bytes().all(|value| value.is_ascii_lowercase())
    {
        return Err("Custom Appearance must contain at most 64 lowercase letters a–z".into());
    }
    let flags = property(entity, "spawnflags")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("0")
        .trim()
        .parse::<u32>()
        .map_err(|_| "Light spawnflags must be a nonnegative integer")?;
    let Some(name) = property(entity, "targetname").filter(|name| !name.is_empty()) else {
        if !pattern.is_empty() || flags & 1 != 0 {
            return Err("Custom Appearance and Initially dark need a named light".into());
        }
        return Ok(());
    };
    let name = name.to_string();
    let state = State {
        pattern: pattern.to_string(),
        appearance: if !pattern.is_empty() {
            0
        } else if style != 0 {
            style
        } else {
            default
        },
        dark: flags & 1 != 0,
    };
    let assigned = if let Some((assigned, previous)) = groups.get(&name) {
        if previous != &state {
            return Err(format!(
                "Lights named {name} must share their initial light pattern and Initially dark"
            ));
        }
        *assigned
    } else {
        if groups.len() == MAX_SWITCHABLE_GROUPS {
            return Err("Direct LDR supports at most 32 named light groups".into());
        }
        let assigned = FIRST_SWITCHABLE + groups.len() as u8;
        groups.insert(name, (assigned, state));
        assigned
    };
    if style != 0 {
        entity
            .properties
            .retain(|(key, _)| !key.eq_ignore_ascii_case("defaultstyle"));
        entity
            .properties
            .push(("defaultstyle".into(), style.to_string()));
    }
    entity
        .properties
        .retain(|(key, _)| !key.eq_ignore_ascii_case("style"));
    entity
        .properties
        .push(("style".into(), assigned.to_string()));
    Ok(())
}

struct Channel {
    style: u8,
    samples: Vec<RgbExp32>,
    total: DVec3,
}

pub(super) struct Channels {
    channels: Vec<Channel>,
    count: usize,
    completed: usize,
}

impl Channels {
    pub(super) fn new(count: usize, completed: usize) -> Result<Self, String> {
        let mut result = Self {
            channels: Vec::new(),
            count,
            completed,
        };
        result.allocate(0)?;
        Ok(result)
    }

    fn allocate(&mut self, style: u8) -> Result<usize, String> {
        if self.channels.len() == MAX_FACE_STYLES {
            return Err("A face receives more than four light styles including Normal; reduce overlapping light groups or separate their reach".into());
        }
        (self.count + 1)
            .checked_mul(self.channels.len() + 1)
            .and_then(|count| count.checked_add(self.completed))
            .filter(|&count| count <= MAX_LIGHTING_BYTES / size_of::<RgbExp32>())
            .ok_or("Direct LDR lighting exceeds Source's 16 MiB limit")?;
        let index = self.channels.len();
        self.channels.push(Channel {
            style,
            samples: vec![RgbExp32::default(); self.count],
            total: DVec3::ZERO,
        });
        Ok(index)
    }

    pub(super) fn add(
        &mut self,
        style: u8,
        color: DVec3,
        colors: &mut [DVec3; MAX_FACE_STYLES],
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        if color == DVec3::ZERO {
            return Ok(());
        }
        let index = if let Some(index) = self
            .channels
            .iter()
            .position(|channel| channel.style == style)
        {
            index
        } else {
            budget.consume(self.count + 1)?;
            self.allocate(style)?
        };
        colors[index] += color;
        Ok(())
    }

    pub(super) fn finish_sample(
        &mut self,
        index: usize,
        colors: &[DVec3; MAX_FACE_STYLES],
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        budget.consume(self.channels.len() - 1)?;
        for (channel, &color) in self.channels.iter_mut().zip(colors) {
            channel.samples[index] = encode_color(color)?;
            channel.total += color;
        }
        Ok(())
    }

    pub(super) fn emit(
        mut self,
        colors: &mut Vec<RgbExp32>,
    ) -> Result<([u8; 4], i32, usize), String> {
        self.channels.sort_unstable_by_key(|channel| channel.style);
        let mut styles = [255; MAX_FACE_STYLES];
        let offset = (colors.len() + self.channels.len()) * size_of::<RgbExp32>();
        for channel in self.channels.iter().rev() {
            colors.push(encode_color(channel.total / self.count as f64)?);
        }
        for (index, channel) in self.channels.iter().enumerate() {
            styles[index] = channel.style;
            colors.extend_from_slice(&channel.samples);
        }
        Ok((styles, offset as i32, self.channels.len()))
    }
}
