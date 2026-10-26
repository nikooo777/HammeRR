use super::*;
use source_formats::vmf::Properties;

fn empty_settings(node: &source_formats::keyvalues::Node, key: &str, value: &str) -> bool {
    node.children().is_some_and(|children| {
        children
            .iter()
            .all(|child| child.key.eq_ignore_ascii_case(key) && child.as_str() == Some(value))
    })
}

pub(super) fn editor(editor: Option<&Properties>) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for (key, value) in editor.into_iter().flat_map(Properties::iter) {
        if !seen.insert(key.to_ascii_lowercase())
            || key.len() > MAX_FIELD_BYTES
            || value.len() > MAX_FIELD_BYTES
        {
            return Err("Collapse cannot preserve ambiguous or oversized editor metadata".into());
        }
        match key.to_ascii_lowercase().as_str() {
            "color" if source_formats::vmf::Color::parse(value).is_some() => {}
            "logicalpos"
                if source_formats::vmf::parse_numbers(value).is_some_and(|values| {
                    values.len() == 2
                        && values
                            .iter()
                            .all(|value| value.is_finite() && (*value as f32).is_finite())
                }) => {}
            "visgroupshown" | "visgroupautoshown" if value == "1" => {}
            _ => {
                return Err(format!(
                    "Collapse cannot preserve editor metadata {key}; remove its group, layer or extension first"
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn helper(helper: &Entity) -> Result<(), String> {
    editor(helper.editor.as_ref())
}

pub(super) fn child(child: &Vmf) -> Result<(), String> {
    if !child.world.groups.is_empty()
        || child
            .visgroups
            .as_ref()
            .is_some_and(|groups| !groups.is_empty())
        || !child.visgroups_extra.is_empty()
        || child.extra.iter().any(|node| {
            !node.key.eq_ignore_ascii_case("cordons") || !empty_settings(node, "active", "0")
        })
        || child
            .cameras
            .as_ref()
            .is_some_and(|node| !empty_settings(node, "activecamera", "-1"))
        || child
            .cordon
            .as_ref()
            .is_some_and(|node| !empty_settings(node, "active", "0"))
    {
        return Err("Collapse cannot preserve child groups, layers, saved cameras, cordons or map extension metadata".into());
    }
    let defaults = Document::empty().into_vmf();
    for (key, value) in child.world.properties.iter() {
        if key.eq_ignore_ascii_case("classname") || key.eq_ignore_ascii_case("mapversion") {
            continue;
        }
        if defaults.world.properties.get(key) != Some(value) {
            return Err(format!(
                "Collapse cannot preserve nondefault child world setting {key}"
            ));
        }
    }
    for (properties, allowed) in [
        (
            child.version_info.as_ref(),
            &[
                "editorversion",
                "editorbuild",
                "mapversion",
                "formatversion",
                "prefab",
            ][..],
        ),
        (
            child.view_settings.as_ref(),
            &[
                "bSnapToGrid",
                "bShowGrid",
                "bShowLogicalGrid",
                "nGridSpacing",
                "bShow3DGrid",
            ][..],
        ),
    ] {
        let Some(properties) = properties else {
            continue;
        };
        for (key, value) in properties.iter() {
            if key.len() > MAX_FIELD_BYTES
                || value.len() > MAX_FIELD_BYTES
                || !allowed
                    .iter()
                    .any(|allowed| key.eq_ignore_ascii_case(allowed))
            {
                return Err("Collapse cannot preserve custom child file settings".into());
            }
        }
    }
    for entity in &child.entities {
        if entity.hidden {
            return Err("Show or remove hidden child entities before collapse".into());
        }
        if is_instance(entity) {
            helper(entity)?;
        } else if entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("func_detail"))
        {
            if entity.solids.is_empty() {
                return Err("Collapse cannot preserve an empty child func_detail owner".into());
            }
            editor(entity.editor.as_ref())?;
        } else {
            return Err(format!(
                "Collapse supports world and ordinary func_detail geometry only; child entity {} ({}) requires its own editable semantics",
                entity.id,
                entity.classname().unwrap_or("missing class")
            ));
        }
    }
    for solid in child.solids() {
        if solid.hidden {
            return Err("Show or remove hidden child brushes before collapse".into());
        }
        editor(solid.editor.as_ref())?;
    }
    Ok(())
}
