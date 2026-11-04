use super::*;
use source_formats::vmf::Properties;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollapseEntityKind {
    Runtime,
    Helper,
    Cubemap,
    StaticProp,
    Detail,
}

impl CollapseEntityKind {
    pub fn emitted(self) -> bool {
        self == Self::Runtime
    }

    pub fn static_prop(self) -> bool {
        self == Self::StaticProp
    }

    pub fn cubemap(self) -> bool {
        self == Self::Cubemap
    }

    pub fn compiler_helper(self) -> bool {
        matches!(self, Self::Helper | Self::Cubemap | Self::Detail)
    }
}

pub fn collapse_entity_kind(entity: &Entity) -> Result<CollapseEntityKind, String> {
    if let Some(kind) = entities::kind(entity) {
        return Ok(match kind {
            entities::Kind::StaticProp => CollapseEntityKind::StaticProp,
            entities::Kind::LightingOrigin => CollapseEntityKind::Helper,
            _ => CollapseEntityKind::Runtime,
        });
    }
    if brush_models::is_model(entity) || occluders::is_helper(entity) {
        return Ok(CollapseEntityKind::Runtime);
    }
    if visclusters::is_helper(entity) || overlays::is_helper(entity) {
        return Ok(CollapseEntityKind::Helper);
    }
    if cubemaps::is_helper(entity) {
        return Ok(CollapseEntityKind::Cubemap);
    }
    if entity
        .classname()
        .is_some_and(|class| class.eq_ignore_ascii_case("func_detail"))
    {
        return Ok(CollapseEntityKind::Detail);
    }
    Err(format!(
        "Collapse cannot materialize child entity {} ({}) outside the shared compiler catalog",
        entity.id,
        entity.classname().unwrap_or("missing class")
    ))
}

pub fn remap_collapsed_entity_sides(
    entity: &Entity,
    sides: &BTreeMap<u32, u32>,
    work: &mut usize,
    control: &Control,
) -> Result<Option<String>, String> {
    control.checkpoint()?;
    if overlays::is_helper(entity) {
        return overlays::remap_sides(entity, sides, sides, work, control).map(Some);
    }
    cubemaps::remap_collapsed_cubemap_sides(entity, sides, work, control)
}

pub(super) fn retained_origins(
    materials: &mut crate::materials::Materials<'_>,
    entity: &Entity,
    expected_volumes: &BTreeSet<u32>,
    control: &Control,
    brush_texture_shadows: bool,
    sources: &crate::asset_sources::BuildMaterialSources,
    checked_cutouts: &mut BTreeSet<usize>,
) -> Result<BTreeSet<u32>, String> {
    crate::map::origins::native_position(brush_models::origin(entity)?)?;
    let plan = brush_models::plan_origin(
        materials,
        entity,
        control,
        brush_texture_shadows,
        sources,
        checked_cutouts,
    )?;
    if expected_volumes.is_empty() || plan.volumes != *expected_volumes {
        return Err("Collapse did not preserve its source origin volumes".into());
    }
    Ok(plan.volumes)
}

pub(crate) fn custodied_origin(
    entity: &Entity,
    plan: &crate::map::origins::OriginPlan,
    custody: &map_core::instance_order::OriginCustody,
) -> Result<DVec3, String> {
    let expected = brush_models::origin(entity)?;
    crate::map::origins::native_position(expected)?;
    if entity.id != custody.entity
        || plan.volumes.is_empty()
        || plan.volumes.len() != custody.volumes.len()
        || custody.volumes.iter().any(|id| !plan.volumes.contains(id))
    {
        return Err("Saved origin custody differs from the resolved origin volumes".into());
    }
    Ok(expected)
}

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
    pub fn collapse_material_sources(
        &self,
        game: &source_formats::gamefs::GameFs,
        control: &Control,
    ) -> Result<Option<crate::asset_sources::AssetSourceGuard>, String> {
        control.checkpoint()?;
        self.material_sources
            .as_ref()
            .map(|sources| sources.source_guard(game, control))
            .transpose()
            .map_err(|error| error.to_string())
    }

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
        if key.eq_ignore_ascii_case("classname")
            || key.eq_ignore_ascii_case("mapversion")
            || map_core::instance_order::is_storage_key(key)
        {
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
        } else {
            collapse_entity_kind(entity)?;
            editor(entity.editor.as_ref())?;
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
