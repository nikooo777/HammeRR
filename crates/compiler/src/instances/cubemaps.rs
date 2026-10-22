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
            "classname" | "origin" | "cubemapsize" => {}
            "sides" if value.trim().is_empty() => {}
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
    point(entity).map(|_| ())
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
    fn manual_runtime_ambiguous_and_nonfinite_child_properties_reject() {
        for (key, value) in [
            ("origin", "NaN 0 0"),
            ("origin", "16385 0 0"),
            ("origin", "0 0"),
            ("cubemapsize", "10"),
            ("cubemapsize", "-1"),
            ("cubemapsize", "1.0"),
            ("sides", "1"),
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
