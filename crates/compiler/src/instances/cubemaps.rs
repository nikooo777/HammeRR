use std::fmt::Write;

use super::*;

pub(super) fn is_helper(entity: &Entity) -> bool {
    entity
        .classname()
        .is_some_and(|class| class.eq_ignore_ascii_case("env_cubemap"))
}

pub(super) fn admit(entity: &Entity) -> Result<(), String> {
    if !entity.solids.is_empty()
        || !entity.extra.is_empty()
        || entity
            .connections
            .as_ref()
            .is_some_and(|pairs| !pairs.is_empty())
    {
        return Err("Child cubemap cannot have brushes, connections or opaque blocks".into());
    }
    let mut seen = BTreeSet::new();
    for (key, value) in entity.properties.iter() {
        field(key)?;
        field(value)?;
        let lower = key.to_ascii_lowercase();
        if !seen.insert(lower.clone()) {
            return Err(format!("Child cubemap repeats property {key}"));
        }
        if key != lower {
            return Err(format!("Child cubemap requires canonical property {lower}"));
        }
        match key {
            "classname" | "origin" | "cubemapsize" | "sides" => {}
            "angles" if vector(entity, key)? == DVec3::ZERO => {}
            "angle" if value.trim().parse::<f64>().ok() == Some(0.0) => {}
            "spawnflags" | "mindxlevel" | "maxdxlevel"
                if value.trim().parse::<u32>().ok() == Some(0) => {}
            "targetname" | "target" | "parentname" | "name" | "globalname" | "killtarget"
            | "lightingorigin"
                if value.trim().is_empty() => {}
            _ => return Err(format!("Child cubemap property {key} is unsupported")),
        }
    }
    if entity.properties.get("classname").is_none() {
        return Err("Child cubemap requires canonical property classname".into());
    }
    crate::map::Map::cubemap_size(entity)?;
    side_ids(entity)?;
    point(entity).map(|_| ())
}

pub(super) fn has_manual_bindings(entity: &Entity) -> bool {
    is_helper(entity)
        && entity
            .properties
            .get("sides")
            .is_some_and(|value| !value.trim().is_empty())
}

fn side_ids(entity: &Entity) -> Result<Vec<u32>, String> {
    field(entity.properties.get("sides").unwrap_or(""))?;
    let mut seen = BTreeSet::new();
    let mut sides = Vec::new();
    for id in crate::cubemap_assets::binding::manual_side_ids(entity) {
        let id = id?;
        if id == 0 || !seen.insert(id) {
            return Err("Child cubemap sides require positive distinct side IDs".into());
        }
        sides.push(id);
    }
    Ok(sides)
}

pub(super) fn charge_bindings(
    work: &mut usize,
    amount: usize,
    control: &Control,
) -> Result<(), String> {
    charge_side_bindings(work, amount, control, "cubemap")
}

pub(super) fn charge_side_bindings(
    work: &mut usize,
    amount: usize,
    control: &Control,
    helper: &str,
) -> Result<(), String> {
    control.checkpoint()?;
    *work = work
        .checked_add(amount)
        .filter(|value| *value <= MAX_GEOMETRY_WORK)
        .ok_or_else(|| {
            format!("Instance {helper} bindings exceed the shared geometry-work limit")
        })?;
    Ok(())
}

pub(super) fn remap_sides(
    original: &Entity,
    local_sides: &BTreeMap<u32, u32>,
    work: &mut usize,
    control: &Control,
) -> Result<String, String> {
    charge_bindings(
        work,
        original.properties.get("sides").map_or(0, str::len),
        control,
    )?;
    remap_side_ids(
        side_ids(original)?,
        local_sides,
        None,
        work,
        control,
        "cubemap",
    )
}

pub fn remap_collapsed_cubemap_sides(
    entity: &Entity,
    sides: &BTreeMap<u32, u32>,
    work: &mut usize,
    control: &Control,
) -> Result<Option<String>, String> {
    control.checkpoint()?;
    if !has_manual_bindings(entity) {
        return Ok(None);
    }
    remap_sides(entity, sides, work, control).map(Some)
}

pub(super) fn remap_side_ids(
    mut sides: Vec<u32>,
    local_sides: &BTreeMap<u32, u32>,
    displacement_sides: Option<&BTreeMap<u32, u32>>,
    work: &mut usize,
    control: &Control,
    helper: &str,
) -> Result<String, String> {
    let mut bytes = 0usize;
    for (index, id) in sides.iter_mut().enumerate() {
        charge_side_bindings(work, 1, control, helper)?;
        *id = local_sides
            .get(id)
            .or_else(|| displacement_sides.and_then(|sides| sides.get(id)))
            .copied()
            .filter(|id| *id != 0)
            .ok_or_else(|| {
                let target = if displacement_sides.is_some() {
                    "ordinary or displacement"
                } else {
                    "ordinary"
                };
                format!(
                    "{helper} sides target side:{id} must name a visible {target} world or func_detail side in the same child"
                )
            })?;
        bytes = bytes
            .checked_add(id.ilog10() as usize + 1 + usize::from(index != 0))
            .filter(|bytes| *bytes <= MAX_FIELD_BYTES)
            .ok_or_else(|| {
                format!("Child {helper} rewritten sides exceed {MAX_FIELD_BYTES} bytes")
            })?;
    }
    charge_side_bindings(work, bytes, control, helper)?;
    let mut rewritten = String::with_capacity(bytes);
    for (index, id) in sides.into_iter().enumerate() {
        control.checkpoint()?;
        if index != 0 {
            rewritten.push(' ');
        }
        write!(rewritten, "{id}").unwrap();
    }
    Ok(rewritten)
}

pub(super) fn point(entity: &Entity) -> Result<DVec3, String> {
    if entity.properties.get("origin").is_none() {
        return Err("Child cubemap requires canonical property origin".into());
    }
    let origin = vector(entity, "origin")?;
    if origin.abs().max_element() > crate::map::MAX_COMPILE_COORD {
        return Err("Child cubemap origin exceeds finite Source compile bounds".into());
    }
    Ok(origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Entity {
        let mut helper = Entity::new(1000, "env_cubemap");
        helper.properties.set("origin", "125 -250 16");
        helper.properties.set("cubemapsize", "0");
        helper
    }

    #[test]
    fn default_sizes_and_empty_manual_bindings_preserve_authored_orientation_fields() {
        let mut helper = sample();
        admit(&helper).unwrap();
        assert!(helper.properties.get("angles").is_none());
        for size in [None, Some(""), Some(" \t"), Some("0"), Some("8"), Some("9")] {
            if let Some(size) = size {
                helper.properties.set("cubemapsize", size);
            } else {
                helper.properties.remove("cubemapsize");
            }
            helper.properties.set("sides", " \t\n");
            helper.properties.set("angles", "-0 +0 0e10");
            helper.properties.set("angle", "-0");
            for key in ["spawnflags", "mindxlevel", "maxdxlevel"] {
                helper.properties.set(key, "0");
            }
            for key in [
                "targetname",
                "target",
                "parentname",
                "name",
                "globalname",
                "killtarget",
                "lightingorigin",
            ] {
                helper.properties.set(key, "");
            }
            let before = helper.clone();
            admit(&helper).unwrap();
            assert_eq!(helper, before);
        }
    }

    #[test]
    fn manual_ids_use_the_shared_positive_distinct_grammar_without_changing_sources() {
        for (text, expected) in [
            ("1 2", vec![1, 2]),
            (" \t1\n2 ", vec![1, 2]),
            ("+1 02", vec![1, 2]),
            ("4294967295", vec![u32::MAX]),
        ] {
            let mut helper = sample();
            helper.properties.set("sides", text);
            let before = helper.clone();
            admit(&helper).unwrap();
            assert_eq!(side_ids(&helper).unwrap(), expected);
            assert_eq!(helper, before);
        }
        for text in [
            "0",
            "1 1",
            "01 +1",
            "-1",
            "1.0",
            "4294967296",
            "1,2",
            "side",
        ] {
            let mut helper = sample();
            helper.properties.set("sides", text);
            assert!(admit(&helper).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn collapsed_manual_remap_keeps_automatic_absence_and_bounds_shared_work() {
        let mut helper = sample();
        let control = Control::default();
        let sides = BTreeMap::from([(1, 101), (2, 102)]);
        let mut work = 0;
        assert_eq!(
            remap_collapsed_cubemap_sides(&helper, &sides, &mut work, &control).unwrap(),
            None
        );
        assert_eq!(helper.properties.get("sides"), None);
        helper.properties.set("sides", "+1 02");
        let before = helper.clone();
        assert_eq!(
            remap_collapsed_cubemap_sides(&helper, &sides, &mut work, &control)
                .unwrap()
                .as_deref(),
            Some("101 102")
        );
        assert!(
            remap_collapsed_cubemap_sides(&helper, &BTreeMap::new(), &mut work, &control).is_err()
        );
        work = MAX_GEOMETRY_WORK;
        assert!(
            remap_collapsed_cubemap_sides(&helper, &sides, &mut work, &control)
                .unwrap_err()
                .contains("work limit")
        );
        control.cancel().unwrap();
        assert!(
            remap_collapsed_cubemap_sides(&helper, &sides, &mut 0, &control)
                .unwrap_err()
                .contains("cancelled")
        );
        assert_eq!(helper, before);
    }

    #[test]
    fn rewritten_manual_lists_check_the_exact_byte_limit_before_formatting() {
        let mut helper = sample();
        helper.properties.set(
            "sides",
            (1..=57)
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(" "),
        );
        let before = helper.clone();
        let mut local_sides = (1..=57)
            .map(|id| (id, 10_000_000 + id))
            .collect::<BTreeMap<_, _>>();
        let rewritten = remap_sides(&helper, &local_sides, &mut 0, &Control::default()).unwrap();
        let expected = (10_000_001..=10_000_057)
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(expected.len(), 512);
        assert_eq!(rewritten, expected);
        local_sides.insert(1, 100_000_000);
        assert!(
            remap_sides(&helper, &local_sides, &mut 0, &Control::default())
                .unwrap_err()
                .contains("rewritten sides exceed 512 bytes")
        );
        assert_eq!(helper, before);
    }

    #[test]
    fn manual_remapping_requires_local_keys_and_shared_work_and_cancellation() {
        let mut helper = sample();
        helper.properties.set("sides", "1");
        let missing = remap_sides(
            &helper,
            &BTreeMap::from([(2, 1)]),
            &mut 0,
            &Control::default(),
        )
        .unwrap_err();
        assert!(missing.contains("side:1") && missing.contains("same child"));
        let local_sides = BTreeMap::from([(1, 10_000_000)]);
        assert!(
            remap_sides(
                &helper,
                &local_sides,
                &mut (MAX_GEOMETRY_WORK - 2),
                &Control::default(),
            )
            .unwrap_err()
            .contains("shared geometry-work limit")
        );
        let control = Control::default();
        control.cancel().unwrap();
        assert!(
            remap_sides(&helper, &local_sides, &mut 0, &control)
                .unwrap_err()
                .contains("cancelled")
        );
    }

    #[test]
    fn runtime_ambiguous_and_nonfinite_child_properties_reject() {
        for (key, value) in [
            ("origin", "NaN 0 0"),
            ("origin", "16385 0 0"),
            ("origin", "0 0"),
            ("cubemapsize", "10"),
            ("cubemapsize", "-1"),
            ("cubemapsize", "1.0"),
            ("sides", "1 1"),
            ("angles", "0 1 0"),
            ("angles", "NaN 0 0"),
            ("angle", "1"),
            ("targetname", "named"),
            ("parentname", "parent"),
            ("spawnflags", "1"),
            ("mindxlevel", "1"),
            ("Origin", "0 0 0"),
            ("vscripts", "unsafe.nut"),
            ("opaque", ""),
        ] {
            let mut helper = sample();
            helper.properties.remove(key);
            helper.properties.set(key, value);
            assert!(admit(&helper).is_err(), "{key}={value}");
        }
        let mut helper = sample();
        helper.properties.remove("origin");
        assert!(admit(&helper).unwrap_err().contains("origin"));
        helper = sample();
        helper.properties.0.push(("origin".into(), "0 0 0".into()));
        assert!(admit(&helper).unwrap_err().contains("repeats"));
        helper = sample();
        helper
            .extra
            .push(source_formats::keyvalues::Node::block("opaque", Vec::new()));
        assert!(admit(&helper).unwrap_err().contains("opaque"));
        helper = sample();
        helper.connections = Some(vec![source_formats::vmf::Connection {
            output: "OnUser1".into(),
            value: "target,Kill,,0,-1".into(),
        }]);
        assert!(admit(&helper).unwrap_err().contains("connections"));
        helper = sample();
        helper
            .solids
            .push(crate::test_common::room().world.solids.remove(0));
        assert!(admit(&helper).unwrap_err().contains("brushes"));
    }
}
