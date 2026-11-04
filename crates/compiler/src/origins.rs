use std::collections::BTreeSet;

use source_formats::vmf::{Entity, Solid};

use super::*;

pub(crate) struct OriginPlan {
    pub position: Option<DVec3>,
    pub volumes: BTreeSet<u32>,
}

pub(crate) fn native_position(point: DVec3) -> Result<DVec3, String> {
    if !within_compile_bounds(point) {
        return Err(format!(
            "Brush-model origin requires finite coordinates within ±{MAX_COMPILE_COORD} units"
        ));
    }
    let native = point.as_vec3().as_dvec3();
    if !within_compile_bounds(native) {
        return Err("Brush-model origin cannot be represented by finite native coordinates".into());
    }
    Ok(native)
}

pub(crate) fn checked_geometry(solid: &Solid, report: &mut Report) -> Option<BrushGeometry> {
    let object = format!("solid:{}", solid.id);
    if solid.sides.iter().any(|side| {
        side.plane.0.iter().any(|point| !point.is_finite())
            || Plane::from_points(side.plane.0).is_some_and(|plane| {
                !plane.normal.is_finite()
                    || !plane.dist.is_finite()
                    || plane.dist < -MAX_COMPILE_COORD * plane.normal.abs().element_sum()
            })
    }) {
        report.diagnostics.push(bounds_error(object));
        return None;
    }
    let geometry = match BrushGeometry::from_solid(solid) {
        Ok(geometry) => geometry,
        Err(error) => {
            report.diagnostics.push(Diagnostic::error(
                format!("invalid brush: {error}"),
                Some(object),
                None,
            ));
            return None;
        }
    };
    if !within_compile_bounds(geometry.mins) || !within_compile_bounds(geometry.maxs) {
        report.diagnostics.push(bounds_error(object));
        return None;
    }
    Some(geometry)
}

pub(crate) fn origin_material(
    materials: &mut crate::materials::Materials<'_>,
    solid: &Solid,
    control: &Control,
) -> Result<bool, String> {
    let mut origin = false;
    let mut grate = false;
    for side in &solid.sides {
        control.checkpoint()?;
        let material = materials.load(&side.material)?;
        let contents = materials.get(material).contents;
        origin |= contents & contents::ORIGIN != 0;
        grate |= contents & contents::GRATE != 0;
    }
    if origin && grate {
        return Err(format!(
            "Solid {} mixes GRATE and ORIGIN materials; use separate brushes",
            solid.id
        ));
    }
    Ok(origin)
}

pub(crate) fn prepare_origins(
    materials: &mut crate::materials::Materials<'_>,
    entity: &Entity,
    report: &mut Report,
    control: &Control,
) -> Result<OriginPlan, String> {
    let mut plan = OriginPlan {
        position: entity.origin(),
        volumes: BTreeSet::new(),
    };
    for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
        control.checkpoint()?;
        if !origin_material(materials, solid, control)
            .map_err(|error| format!("Origin brush solid {}: {error}", solid.id))?
        {
            continue;
        }
        plan.volumes.insert(solid.id);
        if plan.volumes.len() > 1 {
            report.diagnostics.push(Diagnostic::error(
                "Use one visible origin brush per runtime brush entity",
                Some(format!("solid:{}", solid.id)),
                entity.origin(),
            ));
        }
        if solid.sides.iter().any(|side| side.displacement.is_some()) {
            report.diagnostics.push(Diagnostic::error(
                "Origin brushes cannot contain displacements",
                Some(format!("solid:{}", solid.id)),
                None,
            ));
            continue;
        }
        if let Some(geometry) = checked_geometry(solid, report)
            && plan.volumes.len() == 1
        {
            plan.position = Some(geometry.center());
        }
    }
    Ok(plan)
}

impl Map<'_> {
    pub(super) fn origin_material(
        &mut self,
        solid: &Solid,
        control: &Control,
    ) -> Result<bool, String> {
        origin_material(&mut self.materials, solid, control)
    }
    pub(super) fn prepare_origins(
        &mut self,
        entity: &Entity,
        report: &mut Report,
        control: &Control,
    ) -> Result<OriginPlan, String> {
        let brush_model = entity.solids.iter().any(|solid| !solid.hidden);
        if brush_model {
            if entity
                .properties
                .iter()
                .any(|(key, _)| key.eq_ignore_ascii_case("origin") && key != "origin")
            {
                return Err("Brush-model origin requires canonical property origin".into());
            }
            crate::brush_model_pose::initial_offset(&entity.properties.0)?;
            for connection in entity.connections.iter().flatten() {
                if crate::brush_model_pose::pose_key(&connection.output)?
                    .eq_ignore_ascii_case("origin")
                {
                    return Err(
                        "Brush-model origin cannot be supplied through a connection output".into(),
                    );
                }
            }
        }
        let mut plan = prepare_origins(&mut self.materials, entity, report, control)?;
        if brush_model {
            plan.position = plan.position.map(native_position).transpose()?;
        }
        Ok(plan)
    }
}
