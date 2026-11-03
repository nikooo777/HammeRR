use glam::DVec3;
use source_formats::{
    fgd::{Class, Property},
    vmf::Entity,
};

pub(super) const MAX_ANCHOR_SOLIDS: usize = 256;
pub(super) const MAX_ANCHOR_SIDES: usize = 8192;
pub(super) const MAX_ANCHOR_BRUSH_SIDES: usize = 256;
pub(super) const MAX_ANCHOR_WORK: usize = 4_000_000;

pub(super) fn anchor_work(entity: &Entity) -> Result<usize, String> {
    if entity.solids.len() > MAX_ANCHOR_SOLIDS {
        return Err("Helper anchor exceeds 256 owned brushes".into());
    }
    let mut sides = 0usize;
    let mut work = 1usize;
    for solid in &entity.solids {
        let count = solid.sides.len();
        if count > MAX_ANCHOR_BRUSH_SIDES {
            return Err("Helper anchor brush exceeds 256 sides".into());
        }
        sides = sides
            .checked_add(count)
            .filter(|sides| *sides <= MAX_ANCHOR_SIDES)
            .ok_or("Helper anchor exceeds 8192 owned brush sides")?;
        let clips = count
            .checked_mul(count)
            .and_then(|work| {
                count
                    .checked_add(4)
                    .and_then(|vertices| work.checked_mul(vertices))
            })
            .ok_or("Helper anchor clipping work overflow")?;
        work = work
            .checked_add(clips)
            .filter(|work| *work <= MAX_ANCHOR_WORK)
            .ok_or("Helper anchor exceeds its bounded brush clipping work")?;
    }
    Ok(work)
}

pub(super) fn anchor(entity: &Entity) -> Result<(DVec3, &'static str), String> {
    anchor_work(entity)?;
    let point = if entity.solids.is_empty() {
        let mut origins = entity.properties.get_all("origin");
        let origin = origins
            .next()
            .filter(|raw| raw.len() <= 256)
            .and_then(source_formats::vmf::parse_vec3);
        if origins.next().is_some() {
            return Err("Helper anchor has duplicate origin keys".into());
        }
        (
            origin.ok_or("Helper anchor needs one finite authored origin")?,
            "authored_origin",
        )
    } else {
        let mut mins = DVec3::splat(f64::INFINITY);
        let mut maxs = DVec3::splat(f64::NEG_INFINITY);
        for solid in &entity.solids {
            let geometry =
                crate::BrushGeometry::from_solid(solid).map_err(|error| error.to_string())?;
            mins = mins.min(geometry.mins);
            maxs = maxs.max(geometry.maxs);
        }
        if !mins.is_finite()
            || !maxs.is_finite()
            || !mins.cmplt(maxs).all()
            || mins.abs().max(maxs.abs()).max_element() > super::MAX_HELPER_EXTENT
        {
            return Err("Helper brush bounds must be finite within ±65536".into());
        }
        ((mins + maxs) * 0.5, "brush_bounds_center")
    };
    if !point.0.is_finite()
        || !point.0.as_vec3().is_finite()
        || point.0.abs().max_element() > super::MAX_HELPER_EXTENT
    {
        return Err("Helper anchor must be finite within ±65536".into());
    }
    Ok(point)
}

pub fn bounded(value: &str, limit: usize) -> String {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

pub(super) fn numbers(raw: &str, counts: &[usize]) -> Option<Vec<f64>> {
    raw.split_whitespace()
        .map(|part| {
            part.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && (*v as f32).is_finite())
        })
        .collect::<Option<Vec<_>>>()
        .filter(|values| counts.contains(&values.len()))
}

pub(super) enum Error {
    Duplicate,
    Oversized,
    Kind,
    Invalid(String),
}

pub struct Value<'a> {
    pub property: Option<&'a Property>,
    pub raw: Option<&'a str>,
    pub duplicate: bool,
    pub source: &'static str,
}

impl<'a> Value<'a> {
    pub fn resolve(class: &'a Class, entity: &'a Entity, key: &'a str, defaults: bool) -> Self {
        Self::resolve_optional(Some(class), entity, key, defaults)
    }

    pub(super) fn resolve_optional(
        class: Option<&'a Class>,
        entity: &'a Entity,
        key: &'a str,
        defaults: bool,
    ) -> Self {
        let property = class.and_then(|class| class.property(key));
        let mut authored = entity.properties.get_all(key);
        let first = authored.next();
        let raw = first.or_else(|| {
            defaults
                .then(|| property.and_then(|p| p.default.as_deref()))
                .flatten()
        });
        Self {
            property,
            raw,
            duplicate: authored.next().is_some(),
            source: if first.is_some() {
                "authored"
            } else if raw.is_some() {
                "fgd_default"
            } else {
                "missing"
            },
        }
    }

    pub(super) fn validate(&self, limit: usize, kinds: Option<&[&str]>) -> Result<(), Error> {
        if self.duplicate {
            return Err(Error::Duplicate);
        }
        if self.raw.is_some_and(|raw| raw.len() > limit) {
            return Err(Error::Oversized);
        }
        if kinds.is_some_and(|kinds| {
            !self
                .property
                .is_some_and(|p| kinds.contains(&p.kind.as_str()))
        }) {
            return Err(Error::Kind);
        }
        if let (Some(raw), Some(property)) = (self.raw, self.property) {
            property.validate(raw.trim()).map_err(Error::Invalid)?;
        }
        Ok(())
    }
}
