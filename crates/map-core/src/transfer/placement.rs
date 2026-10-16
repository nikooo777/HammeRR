use super::*;

fn check_solid(solid: &Solid) -> std::result::Result<(), EditError> {
    if !solid.extra_properties.0.is_empty()
        || !solid.extra.is_empty()
        || solid.sides.iter().any(|side| {
            !side.extra_properties.0.is_empty()
                || !side.extra.is_empty()
                || side.displacement.as_ref().is_some_and(|disp| {
                    !disp.extra_properties.0.is_empty() || !disp.extra.is_empty()
                })
        })
    {
        return Err(invalid(format!(
            "Brush {} has opaque metadata; rotated/scaled prefab placement cannot transform it safely",
            solid.id
        )));
    }
    Ok(())
}

fn check_entity(
    entity: &Entity,
    catalog: &Catalog,
    scaled: bool,
) -> std::result::Result<(), EditError> {
    if scaled && entity.solids.is_empty() {
        return Err(invalid(format!(
            "Prefab scaling supports brushes and brush entities; entity {} has no scalable brush geometry",
            entity.id
        )));
    }
    if !entity.extra.is_empty() {
        return Err(invalid(format!(
            "Entity {} has opaque blocks; rotated/scaled prefab placement cannot transform them safely",
            entity.id
        )));
    }
    for key in ["origin", "angles", "angle", "pitch"] {
        if entity.properties.get_all(key).count() > 1 {
            return Err(invalid(format!(
                "Entity {} repeats spatial property {key}; resolve it before rotated/scaled prefab placement",
                entity.id
            )));
        }
    }
    if entity.properties.get("angles").is_some() && entity.properties.get("angle").is_some() {
        return Err(invalid(format!(
            "Entity {} has both angle and angles; resolve its orientation before rotated/scaled prefab placement",
            entity.id
        )));
    }
    let class = entity.classname().and_then(|name| catalog.class(name));
    let light_pitch = matches!(entity.classname(), Some("light_spot" | "light_environment"));
    for (key, value) in &entity.properties.0 {
        if value.trim().is_empty() || key.eq_ignore_ascii_case("origin") {
            continue;
        }
        if entity.solids.is_empty()
            && (key.eq_ignore_ascii_case("angles")
                || key.eq_ignore_ascii_case("angle")
                || (light_pitch && key.eq_ignore_ascii_case("pitch")))
        {
            continue;
        }
        let kind = class
            .and_then(|class| class.property(key))
            .map(|property| property.kind.as_str());
        let supported_color = matches!(kind, Some("color255" | "color1"))
            || ["rendercolor", "_light", "_ambient"]
                .iter()
                .any(|color| key.eq_ignore_ascii_case(color));
        let spatial = matches!(kind, Some("vector" | "origin" | "axis" | "angle"))
            || [
                "angles",
                "angle",
                "pitch",
                "movedir",
                "basisorigin",
                "basisnormal",
                "normal",
            ]
            .iter()
            .any(|name| key.eq_ignore_ascii_case(name))
            || (!supported_color && source_formats::vmf::parse_vec3(value).is_some());
        if spatial {
            return Err(invalid(format!(
                "Entity {} property {key} has unsupported spatial metadata for rotated/scaled prefab placement",
                entity.id
            )));
        }
    }
    for solid in &entity.solids {
        check_solid(solid)?;
    }
    Ok(())
}

impl Payload {
    pub fn transformed(
        &self,
        rotation: DVec3,
        scale: DVec3,
        pivot: DVec3,
        catalog: &Catalog,
    ) -> std::result::Result<Self, EditError> {
        let limit = crate::primitive::MAX_COORDINATE;
        if [rotation, pivot]
            .iter()
            .any(|value| !value.is_finite() || value.abs().max_element() > limit)
            || !scale.is_finite()
            || !scale.cmpgt(DVec3::ZERO).all()
        {
            return Err(invalid(
                "Prefab rotation and pivot must be finite within ±65536; scale factors must be finite and positive",
            ));
        }
        if rotation.map(|angle| angle.rem_euclid(360.0)) == DVec3::ZERO && scale == DVec3::ONE {
            return Ok(self.clone());
        }
        for solid in &self.document.vmf().world.solids {
            check_solid(solid)?;
        }
        for entity in &self.document.vmf().entities {
            check_entity(entity, catalog, scale != DVec3::ONE)?;
        }
        let mut document = self.document.preview_snapshot();
        document.edit("prepare prefab transform", |tx| {
            edit::transform(
                tx,
                &self.ids,
                edit::Transform {
                    rotation,
                    scale,
                    pivot,
                    ..Default::default()
                },
            )
        })?;
        Self::capture(&document, &self.ids, catalog)
    }
}
