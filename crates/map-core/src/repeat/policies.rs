use super::*;

pub const MAX_AFFIX_BYTES: usize = 32;
pub const MAX_NAME_INDEX: u32 = 1_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NameStyle {
    #[default]
    SourceSuffix,
    IndexedPrefix,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Naming {
    pub style: NameStyle,
    pub prefix: String,
    pub suffix: String,
    pub start_index: u32,
}

impl Default for Naming {
    fn default() -> Self {
        Self {
            style: NameStyle::SourceSuffix,
            prefix: String::new(),
            suffix: "_copy".into(),
            start_index: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InternalReferences {
    #[default]
    Remap,
    KeepOriginal,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExternalReferences {
    #[default]
    Keep,
    Reject,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policies {
    pub naming: Naming,
    pub internal: InternalReferences,
    pub external: ExternalReferences,
}

impl Policies {
    pub(super) fn validate(&self, count: u32) -> std::result::Result<(), EditError> {
        for (label, value) in [
            ("prefix", &self.naming.prefix),
            ("suffix", &self.naming.suffix),
        ] {
            if value.len() > MAX_AFFIX_BYTES
                || value
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '*' | '?' | ',' | '"' | '!'))
            {
                return Err(invalid(format!(
                    "Naming {label} must fit {MAX_AFFIX_BYTES} UTF-8 bytes without control, wildcard, comma, quote or ! characters"
                )));
            }
        }
        if self.naming.start_index == 0
            || self
                .naming
                .start_index
                .checked_add(count.saturating_sub(1))
                .is_none_or(|last| last > MAX_NAME_INDEX)
        {
            return Err(invalid(format!(
                "Naming indices must stay between 1 and {MAX_NAME_INDEX}"
            )));
        }
        Ok(())
    }
}

pub(super) fn planned_names_with(
    document: &Document,
    names: &BTreeMap<String, (u32, String)>,
    scan: &[references::Reference],
    count: u32,
    policies: &Policies,
) -> std::result::Result<Vec<BTreeMap<String, String>>, EditError> {
    policies.validate(count)?;
    let mut occupied = document
        .vmf()
        .entities
        .iter()
        .flat_map(|entity| &entity.properties.0)
        .filter(|(key, _)| key.eq_ignore_ascii_case("targetname"))
        .map(|(_, name)| name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let mut batches = Vec::with_capacity(count as usize);
    for offset in 0..count {
        let index = policies.naming.start_index + offset;
        let naming = &policies.naming;
        let mut batch = BTreeMap::new();
        for (key, (_, name)) in names {
            let stem = match naming.style {
                NameStyle::SourceSuffix => {
                    format!("{}{name}{}{index}", naming.prefix, naming.suffix)
                }
                NameStyle::IndexedPrefix => {
                    format!("{}{index}_{name}{}", naming.prefix, naming.suffix)
                }
            };
            let mut candidate = stem.clone();
            let mut collision = 2_u64;
            while occupied.contains(&candidate.to_ascii_lowercase()) {
                candidate = format!("{stem}_{collision}");
                collision += 1;
            }
            if candidate.trim() != candidate || candidate.len() > MAX_NAME_BYTES {
                return Err(invalid(format!(
                    "Generated name for {name} must be trimmed and fit {MAX_NAME_BYTES} UTF-8 bytes"
                )));
            }
            if scan
                .iter()
                .any(|reference| connections::name_matches(&candidate, &reference.target))
            {
                return Err(invalid(format!(
                    "Generated name {candidate} would acquire an existing reference; change the naming policy before repeating"
                )));
            }
            occupied.insert(candidate.to_ascii_lowercase());
            batch.insert(key.clone(), candidate);
        }
        batches.push(batch);
    }
    Ok(batches)
}
