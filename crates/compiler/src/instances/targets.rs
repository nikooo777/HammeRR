use super::*;

pub(super) fn admit(entity: &Entity) -> Result<(), String> {
    if !entity.solids.is_empty()
        || !entity.extra.is_empty()
        || entity
            .connections
            .as_ref()
            .is_some_and(|pairs| !pairs.is_empty())
    {
        return Err(
            "Child spotlight targets cannot have brushes, connections or opaque blocks".into(),
        );
    }
    let mut seen = BTreeSet::new();
    for (key, value) in entity.properties.iter() {
        field(key)?;
        field(value)?;
        let lower = key.to_ascii_lowercase();
        if !seen.insert(lower.clone()) {
            return Err(format!("Child spotlight target repeats property {key}"));
        }
        if key != lower {
            return Err(format!(
                "Child spotlight target requires canonical property {lower}"
            ));
        }
        match key {
            "classname" | "origin" => {}
            "targetname" => names::component(value)?,
            "angles" => {
                vector(entity, key)?;
            }
            "angle" if value.trim().parse::<f64>().ok() == Some(0.0) => {}
            "spawnflags" if value.trim().parse::<u32>().ok() == Some(0) => {}
            "target" | "parentname" | "name" | "globalname" | "killtarget" if value.is_empty() => {}
            _ => {
                return Err(format!(
                    "Child spotlight target property {key} is unsupported"
                ));
            }
        }
    }
    if entity.properties.get("classname") != Some("info_target") {
        return Err("Child spotlight target requires canonical classname info_target".into());
    }
    names::component(entity.properties.get("targetname").unwrap_or(""))?;
    check_pose(entity)
}

pub(super) fn check_pose(entity: &Entity) -> Result<(), String> {
    if entity.properties.get("origin").is_none() {
        return Err("Child spotlight target requires canonical property origin".into());
    }
    if vector(entity, "origin")?.abs().max_element() > crate::map::MAX_COMPILE_COORD {
        return Err("Child spotlight target origin exceeds finite Source compile bounds".into());
    }
    vector(entity, "angles")?;
    Ok(())
}

#[derive(Default)]
pub(super) struct LocalIndex {
    positions: BTreeMap<String, DVec3>,
}

impl LocalIndex {
    pub fn new(vmf: &Vmf, work: &mut usize, control: &Control) -> Result<Self, String> {
        let mut result = Self::default();
        if !vmf
            .entities
            .iter()
            .any(|entity| !entity.hidden && entity.classname() == Some("info_target"))
        {
            return Ok(result);
        }
        let mut folded = BTreeSet::new();
        for entity in vmf.entities.iter().filter(|entity| !entity.hidden) {
            names::charge(work, 1, control)?;
            if entity.classname() != Some("info_target") {
                continue;
            }
            if result.positions.len() >= MAX_OBJECTS {
                return Err("Instance spotlight-target index exceeds the object limit".into());
            }
            admit(entity)
                .map_err(|error| format!("Child spotlight target {}: {error}", entity.id))?;
            let name = entity.properties.get("targetname").unwrap();
            names::charge(work, name.len().saturating_mul(2), control)?;
            if !folded.insert(name.to_ascii_lowercase()) {
                return Err(format!(
                    "Child spotlight target {} has a duplicate or case-ambiguous name: {name}",
                    entity.id
                ));
            }
            result
                .positions
                .insert(name.to_owned(), entity.origin().unwrap());
        }
        Ok(result)
    }

    pub fn positions(
        &self,
        work: &mut usize,
        control: &Control,
    ) -> Result<map_core::light::Targets<'_>, String> {
        if self.positions.is_empty() {
            return Ok(map_core::light::Targets::new());
        }
        names::charge(work, self.positions.len(), control)?;
        Ok(self
            .positions
            .iter()
            .map(|(name, point)| (name.as_str(), vec![*point]))
            .collect())
    }
}

pub(super) fn check_poses(
    entity: &Entity,
    targets: &map_core::light::Targets<'_>,
    transforms: &[map_core::edit::Transform],
    work: &mut usize,
    control: &Control,
) -> Result<(), String> {
    let Some(mut target) = map_core::light::target(entity.properties.get("target"), targets)?
    else {
        return Ok(());
    };
    names::charge(work, transforms.len().saturating_mul(2), control)?;
    let mut origin = vector(entity, "origin")?;
    map_core::light::direction(None, None, None, origin, Some(target))?;
    for transform in transforms.iter().rev() {
        control.checkpoint()?;
        origin = map_core::transform::transform_point(origin, *transform)
            .map_err(|error| error.to_string())?;
        target = map_core::transform::transform_point(target, *transform)
            .map_err(|error| error.to_string())?;
        for point in [origin, target] {
            if !point.is_finite()
                || !point.as_vec3().is_finite()
                || point.abs().max_element() > crate::map::MAX_COMPILE_COORD
            {
                return Err(
                    "Child spotlight target pose exceeds finite Source compile bounds".into(),
                );
            }
        }
        map_core::light::direction(None, None, None, origin, Some(target))?;
    }
    Ok(())
}
