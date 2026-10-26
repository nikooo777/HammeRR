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

impl PreparedInstances<'_> {
    pub fn validate_collapse_static_prop_poses(
        &self,
        game: &source_formats::gamefs::GameFs,
        control: &Control,
    ) -> Result<Option<crate::asset_sources::AssetSourceGuard>, String> {
        control.checkpoint()?;
        if self.static_prop_poses.is_empty() {
            return Ok(None);
        }
        let sources = crate::asset_sources::BuildMaterialSources::without_retained_payloads();
        let mut models = BTreeMap::new();
        for entity in &self.vmf().entities {
            control.checkpoint()?;
            let Some(poses) = self.static_prop_poses(entity.id) else {
                continue;
            };
            let path = source_formats::gamefs::asset_relative(
                entity
                    .properties
                    .get("model")
                    .ok_or("Static child prop requires model")?,
            )
            .map_err(|error| error.to_string())?;
            let result: Result<(), String> = (|| {
                let header = if let Some(header) = models.get(&path) {
                    *header
                } else {
                    if models.len() >= source_formats::bsp::MAX_STATIC_PROP_MODELS {
                        return Err("Static-prop model dictionary exceeds the native limit".into());
                    }
                    let asset = sources
                        .read_asset(game, &path, source_formats::mdl::MAX_MODEL_BYTES)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| format!("model {path} was not found"))?;
                    let header = source_formats::mdl::ModelHeader::parse(asset.bytes())
                        .map_err(|error| error.to_string())?;
                    models.insert(path.clone(), header);
                    header
                };
                for (index, pose) in poses.iter().enumerate() {
                    control.checkpoint()?;
                    pose.validate_model_bounds(header).map_err(|error| {
                        let stage = if index == 0 {
                            "source pose".to_owned()
                        } else {
                            format!("intermediate pose {index}")
                        };
                        format!("{stage}: {error}")
                    })?;
                }
                Ok(())
            })();
            result.map_err(|error| format!("Static child prop {} ({path}): {error}", entity.id))?;
        }
        sources
            .source_guard(game, control)
            .map(Some)
            .map_err(|error| error.to_string())
    }
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
        } else if matches!(entities::kind(entity), Some(entities::Kind::StaticProp)) {
            if entity
                .properties
                .get("lightingorigin")
                .is_some_and(|value| !value.is_empty())
            {
                return Err("Collapse requires empty child prop lightingorigin; remove its binding before collapse".into());
            }
            editor(entity.editor.as_ref())?;
        } else if entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("func_detail"))
        {
            if entity.solids.is_empty() {
                return Err("Collapse cannot preserve an empty child func_detail owner".into());
            }
            editor(entity.editor.as_ref())?;
        } else if cubemaps::is_helper(entity)
            || matches!(
                entities::kind(entity),
                Some(
                    entities::Kind::Point | entities::Kind::Spot | entities::Kind::SpotlightTarget
                )
            )
        {
            editor(entity.editor.as_ref())?;
        } else {
            return Err(format!(
                "Collapse supports world/detail geometry, checked static props, light assemblies and cubemaps; child entity {} ({}) requires its own editable semantics",
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
