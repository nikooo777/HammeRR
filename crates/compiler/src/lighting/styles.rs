use std::collections::HashMap;

use glam::DVec3;
use source_formats::bsp::{FaceLighting, MAX_LIGHTING_BYTES, RgbExp32};

use super::bake::Budget;
use super::{encode_color, property};
use crate::Control;
use crate::map::{Map, MapEntity};

const FIRST_SWITCHABLE: u8 = 32;
const MAX_SWITCHABLE_GROUPS: usize = 32;
const MAX_PATTERN_BYTES: usize = 64;
const MAX_FACE_STYLES: usize = 4;
pub(super) type SampleColors = [[DVec3; super::bump::MAPS]; MAX_FACE_STYLES];

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
    average: Option<DVec3>,
}

pub(super) struct Channels {
    channels: Vec<Channel>,
    count: usize,
    completed: usize,
    maps: usize,
}

impl Channels {
    pub(super) fn new(count: usize, completed: usize, maps: usize) -> Result<Self, String> {
        if ![1, super::bump::MAPS].contains(&maps) {
            return Err("Lightmap direction count must be one or four".into());
        }
        let mut result = Self {
            channels: Vec::new(),
            count,
            completed,
            maps,
        };
        result.allocate(0)?;
        Ok(result)
    }

    fn allocate(&mut self, style: u8) -> Result<usize, String> {
        if self.channels.len() == MAX_FACE_STYLES {
            return Err("A face receives more than four light styles including Normal; reduce overlapping light groups or separate their reach".into());
        }
        let samples = self
            .count
            .checked_mul(self.maps)
            .ok_or("Lightmap sample count overflow")?;
        samples
            .checked_add(1)
            .and_then(|count| count.checked_mul(self.channels.len() + 1))
            .and_then(|count| count.checked_add(self.completed))
            .filter(|&count| count <= MAX_LIGHTING_BYTES / size_of::<RgbExp32>())
            .ok_or("Direct LDR lighting exceeds Source's 16 MiB limit")?;
        let index = self.channels.len();
        self.channels.push(Channel {
            style,
            samples: vec![RgbExp32::default(); samples],
            total: DVec3::ZERO,
            average: None,
        });
        Ok(index)
    }

    pub(super) fn add(
        &mut self,
        style: u8,
        color: &[DVec3; super::bump::MAPS],
        colors: &mut SampleColors,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        if color[..self.maps].iter().all(|&color| color == DVec3::ZERO) {
            return Ok(());
        }
        let index = if let Some(index) = self
            .channels
            .iter()
            .position(|channel| channel.style == style)
        {
            index
        } else {
            budget.consume(self.count * self.maps + 1)?;
            self.allocate(style)?
        };
        for (target, &contribution) in colors[index].iter_mut().zip(color).take(self.maps) {
            *target += contribution;
        }
        Ok(())
    }

    pub(super) fn restore(
        light: &FaceLighting<'_>,
        completed: usize,
        budget: &mut Budget<'_>,
    ) -> Result<Self, String> {
        let [width, height] = light.dimensions();
        let mut result = Self::new(width * height, completed, light.maps())?;
        for (style_index, &style) in light.styles().iter().enumerate() {
            let index = if style == 0 {
                0
            } else {
                result.allocate(style)?
            };
            budget.consume(result.count * result.maps + 1)?;
            let channel = &mut result.channels[index];
            channel.average = Some(DVec3::from_array(
                light
                    .average(style_index)
                    .ok_or("Bounce source average is missing")?
                    .linear(),
            ));
            for map in 0..result.maps {
                channel.samples[map * result.count..(map + 1) * result.count].copy_from_slice(
                    light
                        .samples(style_index, map)
                        .ok_or("Bounce source directional map is missing")?,
                );
            }
        }
        Ok(result)
    }

    pub(super) fn finish_bounce_sample(
        &mut self,
        index: usize,
        colors: &SampleColors,
        budget: &mut Budget<'_>,
    ) -> Result<bool, String> {
        budget.consume(self.channels.len() * self.maps - 1)?;
        let mut positive = false;
        for (channel, color) in self.channels.iter_mut().zip(colors) {
            for (map, &color) in color.iter().enumerate().take(self.maps) {
                if color != DVec3::ZERO {
                    let slot = &mut channel.samples[map * self.count + index];
                    *slot = encode_color(DVec3::from_array(slot.linear()) + color)?;
                    positive = true;
                }
            }
            channel.total += color[0];
        }
        Ok(positive)
    }

    pub(super) fn finish_sample(
        &mut self,
        index: usize,
        colors: &SampleColors,
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        budget.consume(self.channels.len() * self.maps - 1)?;
        for (channel, &color) in self.channels.iter_mut().zip(colors) {
            for (map, &color) in color.iter().enumerate().take(self.maps) {
                channel.samples[map * self.count + index] = encode_color(color)?;
            }
            channel.total += color[0];
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
            colors.push(encode_color(
                channel.average.unwrap_or(DVec3::ZERO) + channel.total / self.count as f64,
            )?);
        }
        for (index, channel) in self.channels.iter().enumerate() {
            styles[index] = channel.style;
            colors.extend_from_slice(&channel.samples);
        }
        Ok((styles, offset as i32, self.channels.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auxiliary_maps_count_toward_the_shared_limit_before_allocation() {
        let limit = MAX_LIGHTING_BYTES / size_of::<RgbExp32>();
        assert!(Channels::new(usize::MAX, 0, super::super::bump::MAPS).is_err());
        assert!(Channels::new(1, limit - 2, super::super::bump::MAPS).is_err());
        let mut channels = Channels::new(1, limit - 8, super::super::bump::MAPS).unwrap();
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let mut colors = [[DVec3::ZERO; super::super::bump::MAPS]; MAX_FACE_STYLES];
        assert!(
            channels
                .add(
                    32,
                    &[DVec3::X; super::super::bump::MAPS],
                    &mut colors,
                    &mut budget
                )
                .is_err()
        );
        assert_eq!(channels.channels.len(), 1);
    }
}
