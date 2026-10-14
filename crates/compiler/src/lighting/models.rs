use std::ops::Range;

use glam::DVec3;
use source_formats::bsp::Model;

use crate::map::Map;

pub(super) struct Scope {
    pub entity: usize,
    pub offset: DVec3,
    pub faces: Range<usize>,
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
            offset,
            faces: first..end,
        });
    }
    if end != face_count {
        return Err("Lighting model bindings do not cover emitted faces".into());
    }
    Ok(scopes)
}
