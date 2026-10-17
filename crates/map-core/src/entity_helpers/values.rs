use source_formats::{
    fgd::{Class, Property},
    vmf::Entity,
};

pub(super) fn bounded(value: &str, limit: usize) -> String {
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

pub(super) struct Value<'a> {
    pub(super) property: Option<&'a Property>,
    pub(super) raw: Option<&'a str>,
    pub(super) duplicate: bool,
    pub(super) source: &'static str,
}

impl<'a> Value<'a> {
    pub(super) fn resolve(
        class: &'a Class,
        entity: &'a Entity,
        key: &'a str,
        defaults: bool,
    ) -> Self {
        let property = class.property(key);
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
