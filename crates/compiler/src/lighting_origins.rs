use std::collections::BTreeMap;

use glam::DVec3;
use source_formats::vmf::{Entity, Vmf};

use crate::{Control, map::MAX_COMPILE_COORD};

enum Named<'a> {
    Missing,
    Unique(&'a Entity),
    Ambiguous,
}

pub(crate) struct Origins<'a> {
    named: BTreeMap<&'a str, Named<'a>>,
}

impl<'a> Origins<'a> {
    pub(crate) fn new(
        vmf: &'a Vmf,
        names: impl IntoIterator<Item = &'a str>,
        control: &Control,
    ) -> Result<Self, String> {
        let mut named: BTreeMap<_, _> = names
            .into_iter()
            .filter(|name| !name.is_empty())
            .map(|name| (name, Named::Missing))
            .collect();
        if named.is_empty() {
            return Ok(Self { named });
        }
        for helper in vmf.entities.iter().filter(|entity| {
            !entity.hidden
                && entity
                    .classname()
                    .is_some_and(|class| class.eq_ignore_ascii_case("info_lighting"))
        }) {
            control.checkpoint()?;
            if let Some(name) = helper.properties.get("targetname")
                && let Some(entry) = named.get_mut(name)
            {
                *entry = match entry {
                    Named::Missing => Named::Unique(helper),
                    _ => Named::Ambiguous,
                };
            }
        }
        Ok(Self { named })
    }

    pub(crate) fn resolve(&self, name: Option<&str>) -> Result<Option<DVec3>, String> {
        let Some(name) = name.filter(|name| !name.is_empty()) else {
            return Ok(None);
        };
        let helper = match self.named.get(name) {
            Some(Named::Unique(helper)) => helper,
            Some(Named::Ambiguous) => {
                return Err("lightingorigin resolves to multiple info_lighting helpers".into());
            }
            Some(Named::Missing) | None => {
                return Err("lightingorigin must name one visible info_lighting".into());
            }
        };
        let point = helper
            .origin()
            .filter(|point| point.is_finite() && point.abs().max_element() <= MAX_COMPILE_COORD)
            .ok_or("lighting origin is invalid")?;
        Ok(Some(point.as_vec3().as_dvec3()))
    }
}
