use super::*;

pub(crate) fn placement_solid(solid: &Solid) -> std::result::Result<(), EditError> {
    let precise = solid
        .precise_vertices()
        .map_err(|error| invalid(error.to_string()))?;
    if !solid.extra_properties.0.is_empty()
        || !solid.extra.is_empty()
        || precise
            .as_ref()
            .is_some_and(|rings| rings.iter().any(|ring| ring.has_opaque_fields))
        || solid.sides.iter().any(|side| {
            !side.extra_properties.0.is_empty()
                || side
                    .extra
                    .iter()
                    .any(|node| !source_formats::vmf::is_precise_vertices_key(&node.key))
                || side.displacement.as_ref().is_some_and(|disp| {
                    !disp.extra_properties.0.is_empty() || !disp.extra.is_empty()
                })
        })
    {
        return Err(invalid(format!(
            "Brush {} has opaque metadata; rotated/scaled placement cannot transform it safely",
            solid.id
        )));
    }
    Ok(())
}

pub(crate) fn placement_entity_scale(
    entity: &Entity,
    catalog: &Catalog,
    scale: DVec3,
) -> std::result::Result<Option<f32>, EditError> {
    if !entity.extra.is_empty() {
        return Err(invalid(format!(
            "Entity {} has opaque blocks; rotated/scaled placement cannot transform them safely",
            entity.id
        )));
    }
    for key in ["origin", "angles", "angle", "pitch"] {
        if entity.properties.get_all(key).count() > 1 {
            return Err(invalid(format!(
                "Entity {} repeats spatial property {key}; resolve it before rotated/scaled placement",
                entity.id
            )));
        }
    }
    if entity.properties.get("angles").is_some() && entity.properties.get("angle").is_some() {
        return Err(invalid(format!(
            "Entity {} has both angle and angles; resolve its orientation before rotated/scaled placement",
            entity.id
        )));
    }
    if scale != DVec3::ONE && entity.solids.is_empty() {
        for key in ["model", "modelscale"] {
            if entity.properties.get_all(key).count() > 1 {
                return Err(invalid(format!(
                    "Entity {} repeats spatial property {key}; resolve it before scaled placement",
                    entity.id
                )));
            }
        }
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
                "Entity {} property {key} has unsupported spatial metadata for rotated/scaled placement",
                entity.id
            )));
        }
    }
    for solid in &entity.solids {
        placement_solid(solid)?;
    }
    scaled_model(entity, scale)
}

fn scaled_model(entity: &Entity, scale: DVec3) -> std::result::Result<Option<f32>, EditError> {
    if scale == DVec3::ONE || !entity.solids.is_empty() {
        return Ok(None);
    }
    if !source_formats::model_scale::scale_editable(entity) {
        let factor = scale
            .to_array()
            .into_iter()
            .find(|factor| *factor != 1.0)
            .unwrap_or(1.0);
        source_formats::model_scale::compose(entity, factor)
            .map_err(|error| invalid(format!("Entity {}: {error}", entity.id)))?;
    }
    if entity.properties.get("modelscale").is_some() {
        source_formats::model_scale::scale(entity)
            .map_err(|error| invalid(format!("Entity {}: {error}", entity.id)))?;
    }
    let Some(model) = entity
        .properties
        .get("model")
        .filter(|model| !model.trim().is_empty())
    else {
        return Ok(None);
    };
    let model = source_formats::gamefs::asset_relative(model.trim())
        .map_err(|error| invalid(format!("Entity {} model: {error}", entity.id)))?;
    if !model.starts_with("models/") || !model.ends_with(".mdl") {
        return Err(invalid(format!(
            "Entity {} has a non-MDL model reference; placement scaling cannot change its size",
            entity.id
        )));
    }
    if scale.x != scale.y || scale.x != scale.z {
        return Err(invalid(format!(
            "Entity {} model needs positive uniform placement scaling; nonuniform model deformation is unsupported",
            entity.id
        )));
    }
    source_formats::model_scale::compose(entity, scale.x)
        .map(Some)
        .map_err(|error| invalid(format!("Entity {}: {error}", entity.id)))
}

impl Payload {
    pub fn transformed(
        &self,
        rotation: DVec3,
        scale: DVec3,
        pivot: DVec3,
        catalog: &Catalog,
    ) -> std::result::Result<Self, EditError> {
        Ok(self
            .prepared_transform(
                edit::Transform {
                    rotation,
                    scale,
                    pivot,
                    ..Default::default()
                },
                catalog,
            )?
            .unwrap_or_else(|| self.clone()))
    }

    pub(super) fn prepared_transform(
        &self,
        transform: edit::Transform,
        catalog: &Catalog,
    ) -> std::result::Result<Option<Self>, EditError> {
        let limit = crate::primitive::MAX_COORDINATE;
        if [transform.translation, transform.rotation, transform.pivot]
            .iter()
            .any(|value| !value.is_finite() || value.abs().max_element() > limit)
            || !transform.scale.is_finite()
            || !transform.scale.cmpgt(DVec3::ZERO).all()
        {
            return Err(invalid(
                "Placement offset, rotation and pivot must be finite within ±65536; scale factors must be finite and positive",
            ));
        }
        let affine = transform.rotation.map(|angle| angle.rem_euclid(360.0)) != DVec3::ZERO
            || transform.scale != DVec3::ONE;
        if !affine && transform.translation == DVec3::ZERO {
            return Ok(None);
        }
        let mut model_scales = Vec::new();
        if affine {
            for solid in &self.document.vmf().world.solids {
                placement_solid(solid)?;
            }
            for entity in &self.document.vmf().entities {
                if let Some(value) = placement_entity_scale(entity, catalog, transform.scale)? {
                    model_scales.push((entity.id, value));
                }
            }
        }
        let solids = self
            .document
            .vmf()
            .world
            .solids
            .iter()
            .chain(
                self.document
                    .vmf()
                    .entities
                    .iter()
                    .flat_map(|entity| &entity.solids),
            )
            .map(|solid| (solid.id, solid.id))
            .collect();
        let sides = self.document.vmf().side_ids().map(|id| (id, id)).collect();
        let mut document = self.document.preview_snapshot();
        document.edit("prepare placement transform", |tx| {
            edit::transform(tx, &self.ids, transform)?;
            for &(id, value) in &model_scales {
                tx.entity_mut(id)?
                    .properties
                    .set("modelscale", value.to_string());
            }
            let partitions = partition_transfer::remap(
                &self.document,
                tx.document(),
                &solids,
                &sides,
                transform,
            )?;
            partitions.write(tx)?;
            let free_partitions = free_partition_transfer::remap(
                &self.document,
                tx.document(),
                &solids,
                &sides,
                transform,
            )?;
            free_partitions.write(tx)?;
            Ok(())
        })?;
        Self::capture(&document, &self.ids, catalog).map(Some)
    }
}
