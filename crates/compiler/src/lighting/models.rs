use std::ops::Range;

use glam::DVec3;
use source_formats::bsp::Model;

use crate::brush_model_pose::{MAX_INITIAL_POSE_BYTES, Pose, initial_offset, initial_rotation};
use crate::map::Map;
use crate::{Diagnostic, Report};

pub(super) struct Scope {
    pub entity: usize,
    pub pose: Pose,
    pub faces: Range<usize>,
    pub reflected_source: bool,
}

pub(super) fn admit_initial_poses(
    map: &Map<'_>,
    scopes: &mut [Scope],
    receivers: &[bool],
    report: &mut Report,
    budget: &mut super::bake::Budget<'_>,
) -> Result<(), String> {
    if scopes.len() != receivers.len() {
        return Err("Initial lighting pose scopes do not match receivers".into());
    }
    for (scope, &receiver) in scopes.iter_mut().zip(receivers) {
        budget.consume(1)?;
        if scope.entity == 0 {
            scope.reflected_source = true;
            continue;
        }
        let entity = &map.entities[scope.entity];
        budget.consume(entity.properties.len())?;
        let reflected_source = crate::brush_shadow_casters::enabled(&entity.properties);
        if !receiver && !reflected_source {
            continue;
        }
        for (key, value) in &entity.properties {
            budget.consume(key.len().max(1))?;
            budget.consume(value.len().min(MAX_INITIAL_POSE_BYTES))?;
        }
        let rotation = initial_rotation(&entity.classname, &entity.properties);
        let checked = match rotation {
            Ok(Some(rotation)) => match initial_offset(&entity.properties) {
                Ok(offset) => {
                    let pose = Pose {
                        offset,
                        rotation: Some(rotation),
                    };
                    let mut bounds = (DVec3::MAX, DVec3::MIN);
                    for &index in &entity.brushes {
                        budget.consume(1)?;
                        let brush = &map.brushes[index];
                        bounds = (bounds.0.min(brush.mins), bounds.1.max(brush.maxs));
                    }
                    budget.consume(16)?;
                    pose.raw_and_stored_bounds(bounds.0, bounds.1).map(|_| pose)
                }
                Err(error) => Err(error),
            },
            Ok(None) => Ok(scope.pose),
            Err(error) => Err(error),
        };
        match checked {
            Ok(pose) => {
                scope.pose = pose;
                scope.reflected_source = reflected_source;
            }
            Err(error) => report.diagnostics.push(Diagnostic::error(
                    format!("Unsupported authored initial brush-model lighting pose: {error}; use one bounded angles triple on an unparented func_brush, zero angles, or Fullbright"),
                    Some(format!("entity:{}", entity.id)),
                    Some(scope.pose.offset),
                )),
        }
    }
    Ok(())
}

pub(super) fn scopes(
    map: &Map<'_>,
    models: &[Model],
    entities: &[usize],
    face_count: usize,
) -> Result<Vec<Scope>, String> {
    if models.len() != entities.len() || entities.first() != Some(&0) {
        return Err("Lighting model bindings do not match emitted models".into());
    }
    let mut end = 0;
    let mut seen = std::collections::HashSet::new();
    let mut scopes = Vec::with_capacity(models.len());
    for (model, &entity) in models.iter().zip(entities) {
        if !seen.insert(entity) {
            return Err("Lighting model bindings contain a repeated entity".into());
        }
        let source = map
            .entities
            .get(entity)
            .ok_or("Lighting model entity is missing")?;
        let first = usize::try_from(model.first_face)
            .map_err(|_| "Lighting model has a negative first face")?;
        let count = usize::try_from(model.face_count)
            .map_err(|_| "Lighting model has a negative face count")?;
        if first != end {
            return Err("Lighting model face ranges are not consecutive".into());
        }
        end = first
            .checked_add(count)
            .filter(|&end| end <= face_count)
            .ok_or("Lighting model exceeds emitted faces")?;
        let offset = if entity == 0 {
            DVec3::ZERO
        } else {
            source.origin.unwrap_or(DVec3::ZERO)
        };
        if !offset.is_finite() {
            return Err("Lighting model pivot must be finite".into());
        }
        scopes.push(Scope {
            entity,
            pose: Pose {
                offset,
                rotation: None,
            },
            faces: first..end,
            reflected_source: false,
        });
    }
    if end != face_count {
        return Err("Lighting model bindings do not cover emitted faces".into());
    }
    Ok(scopes)
}
