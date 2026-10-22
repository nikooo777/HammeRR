use super::*;
use map_core::edit;

pub(super) const POSE_WORK: usize = source_formats::bsp::MAX_OVERLAY_FACES.pow(2);

pub(super) fn is_helper(entity: &Entity) -> bool {
    entity
        .classname()
        .is_some_and(|class| class.eq_ignore_ascii_case("info_overlay"))
}

pub(super) fn admit(entity: &Entity) -> Result<(), String> {
    if !entity.solids.is_empty()
        || !entity.extra.is_empty()
        || entity
            .connections
            .as_ref()
            .is_some_and(|pairs| !pairs.is_empty())
    {
        return Err("Child overlays cannot have brushes, opaque blocks or connections".into());
    }
    let fields = [
        "classname",
        "material",
        "sides",
        "BasisOrigin",
        "BasisNormal",
        "BasisU",
        "BasisV",
        "uv0",
        "uv1",
        "uv2",
        "uv3",
        "RenderOrder",
        "StartU",
        "EndU",
        "StartV",
        "EndV",
        "fademindist",
        "fademaxdist",
        "origin",
        "angles",
        "angle",
        "spawnflags",
        "mindxlevel",
        "maxdxlevel",
        "targetname",
        "target",
        "parentname",
        "name",
        "globalname",
        "killtarget",
        "lightingorigin",
    ];
    let mut seen = 0_u32;
    for (key, value) in entity.properties.iter() {
        field(key)?;
        field(value)?;
        let index = fields
            .iter()
            .position(|field| field.eq_ignore_ascii_case(key))
            .ok_or_else(|| format!("Child overlay property {key} is unsupported"))?;
        let bit = 1 << index;
        if seen & bit != 0 {
            return Err(format!("Child overlay repeats property {key}"));
        }
        seen |= bit;
        if key != fields[index] {
            return Err(format!(
                "Child overlay requires canonical property {}",
                fields[index]
            ));
        }
        match key {
            "classname" | "material" | "sides" | "BasisOrigin" | "BasisNormal" | "BasisU"
            | "BasisV" | "uv0" | "uv1" | "uv2" | "uv3" | "RenderOrder" | "StartU" | "EndU"
            | "StartV" | "EndV" | "fademindist" | "fademaxdist" | "origin" => {}
            "angles" if vector(entity, "angles")? == DVec3::ZERO => {}
            "angle" if value.trim().parse::<f64>().ok() == Some(0.0) => {}
            "spawnflags" | "mindxlevel" | "maxdxlevel"
                if value.trim().parse::<u32>().ok() == Some(0) => {}
            "targetname" | "target" | "parentname" | "name" | "globalname" | "killtarget"
            | "lightingorigin" => {
                if !value.is_empty() {
                    return Err(format!("Child overlay requires exactly empty {key}"));
                }
            }
            _ => return Err(format!("Child overlay property {key} must be neutral")),
        }
    }
    if entity.properties.get("classname").is_none() {
        return Err("Child overlay requires canonical property classname".into());
    }
    check_pose(entity).map(|_| ())
}

pub(super) fn check_pose(entity: &Entity) -> Result<map_core::overlay::Definition, String> {
    let sides = entity.properties.get("sides").unwrap_or("");
    field(sides)?;
    let projection = map_core::overlay::parse(entity)?;
    if projection.sides.first() == Some(&0)
        || projection.sides.len() != sides.split_whitespace().count()
    {
        return Err("Child overlay sides require 1–64 positive distinct side IDs".into());
    }
    if entity.properties.get("origin").is_some()
        && vector(entity, "origin")?.abs().max_element() > crate::map::MAX_COMPILE_COORD
    {
        return Err("Child overlay origin exceeds finite Source compile bounds".into());
    }
    Ok(projection)
}

pub(super) fn transform(
    entity: &mut Entity,
    pose: edit::Transform,
) -> Result<(), map_core::EditError> {
    let rotation = edit::Transform {
        translation: DVec3::ZERO,
        ..pose
    };
    for (key, transform) in [
        ("BasisOrigin", pose),
        ("BasisU", rotation),
        ("BasisV", rotation),
        ("BasisNormal", rotation),
    ] {
        let point = vector(entity, key).map_err(map_core::EditError::Invalid)?;
        let point = map_core::transform::transform_point(point, transform)?;
        entity
            .properties
            .set(key, source_formats::vmf::format_vec3(point));
    }
    if entity.properties.get("origin").is_some() {
        let point = vector(entity, "origin").map_err(map_core::EditError::Invalid)?;
        let point = map_core::transform::transform_point(point, pose)?;
        entity
            .properties
            .set("origin", source_formats::vmf::format_vec3(point));
    }
    check_pose(entity).map_err(map_core::EditError::Invalid)?;
    Ok(())
}

pub(super) fn remap_sides(
    original: &Entity,
    local_sides: &BTreeMap<u32, u32>,
    work: &mut usize,
    control: &Control,
) -> Result<String, String> {
    cubemaps::charge_side_bindings(
        work,
        original.properties.get("sides").map_or(0, str::len),
        control,
        "overlay",
    )?;
    check_pose(original)?;
    let sides =
        crate::cubemap_assets::binding::manual_side_ids(original).collect::<Result<Vec<_>, _>>()?;
    cubemaps::remap_side_ids(sides, local_sides, work, control, "overlay")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn helper() -> Entity {
        let mut entity = Entity::new(1000, "info_overlay");
        for (key, value) in [
            ("material", "test/overlay"),
            ("sides", "1"),
            ("BasisOrigin", "0 0 32"),
            ("BasisU", "1 0 0"),
            ("BasisV", "0 1 0"),
            ("BasisNormal", "0 0 1"),
            ("uv0", "-16 -16 5"),
            ("uv1", "-16 16 6"),
            ("uv2", "16 16 7"),
            ("uv3", "16 -16 8"),
        ] {
            entity.properties.set(key, value);
        }
        entity
    }

    #[test]
    fn rewritten_lists_preserve_authored_order_and_enforce_512_before_formatting() {
        let mut helper = helper();
        helper.properties.set(
            "sides",
            (1..=57)
                .rev()
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
            .rev()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(expected.len(), 512);
        assert_eq!(rewritten, expected);
        local_sides.insert(1, 100_000_000);
        assert!(
            remap_sides(&helper, &local_sides, &mut 0, &Control::default())
                .unwrap_err()
                .contains("Child overlay rewritten sides exceed 512 bytes")
        );
        assert_eq!(helper, before);
    }

    #[test]
    fn remapping_keeps_positive_distinct_source_checks_and_shared_work_and_cancellation() {
        let mut helper = helper();
        for sides in ["0", "1 1", "+1 01"] {
            helper.properties.set("sides", sides);
            assert!(
                remap_sides(
                    &helper,
                    &BTreeMap::from([(1, 99)]),
                    &mut 0,
                    &Control::default()
                )
                .unwrap_err()
                .contains("positive distinct"),
                "{sides}"
            );
        }
        helper.properties.set("sides", "1");
        assert!(
            remap_sides(
                &helper,
                &BTreeMap::from([(2, 1)]),
                &mut 0,
                &Control::default()
            )
            .unwrap_err()
            .contains(
                "side:1 must name a visible ordinary world or func_detail side in the same child"
            )
        );
        assert!(
            remap_sides(
                &helper,
                &BTreeMap::from([(1, 99)]),
                &mut (MAX_GEOMETRY_WORK - 1),
                &Control::default()
            )
            .unwrap_err()
            .contains("Instance overlay bindings exceed the shared geometry-work limit")
        );
        let control = Control::default();
        control.cancel().unwrap();
        assert!(
            remap_sides(&helper, &BTreeMap::from([(1, 99)]), &mut 0, &control)
                .unwrap_err()
                .contains("cancelled")
        );
    }
}
