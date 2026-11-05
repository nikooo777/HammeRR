use super::{Context, MAX_FIELD_BYTES, charge, source::Source};
use map_core::connections::event_name_matches as name_matches;

pub(super) struct Scenario {
    activator: Option<u32>,
    caller: Option<u32>,
    caller_unbound: bool,
}

pub(super) struct Resolution {
    pub matches: Vec<u32>,
    pub status: &'static str,
    pub reason: String,
    pub resolved: bool,
}

pub(super) fn is_resolved(status: &str) -> bool {
    matches!(status, "named" | "class" | "caller" | "activator")
}

impl Scenario {
    pub fn prepare(source: &Source<'_>, context: &Context) -> Result<Self, String> {
        if context.caller_unbound && context.caller.is_some() {
            return Err("Supply a caller ID or an unbound caller, not both".into());
        }
        let resolve = |value: &Option<String>| {
            value.as_ref().map(|value| {
                if value.len() > MAX_FIELD_BYTES { return Err("Graph scenario ID exceeds the field byte limit".into()); }
                source.resolve(value).ok_or_else(|| format!("Graph scenario entity {value} is missing, hidden or outside the selected source scope"))
            }).transpose()
        };
        Ok(Self {
            activator: resolve(&context.activator)?,
            caller: resolve(&context.caller)?,
            caller_unbound: context.caller_unbound,
        })
    }
}

pub(super) fn targets(
    source: &Source<'_>,
    owner: u32,
    target: &str,
    scenario: &Scenario,
    work: &mut usize,
) -> Result<Resolution, String> {
    if target.starts_with('!') {
        let (identity, status, reason) = if target.eq_ignore_ascii_case("!self")
            || target.eq_ignore_ascii_case("!caller")
        {
            (
                (!scenario.caller_unbound).then(|| scenario.caller.unwrap_or(owner)),
                "caller",
                "Scenario event caller is explicitly unbound",
            )
        } else if target.eq_ignore_ascii_case("!activator") {
            (
                scenario.activator,
                "activator",
                "Supply an authored scenario activator to resolve !activator",
            )
        } else {
            (
                None,
                "runtime_dependent",
                "Procedural target needs a live runtime identity; no player, picker or dynamic entity is inferred",
            )
        };
        return Ok(Resolution {
            matches: identity.into_iter().collect(),
            status: if identity.is_some() {
                status
            } else {
                "runtime_dependent"
            },
            reason: if identity.is_some() {
                String::new()
            } else {
                reason.into()
            },
            resolved: identity.is_some(),
        });
    }
    let entities = &source.document().vmf().entities;
    let mut matches = Vec::new();
    super::reserve(&mut matches, entities.len())?;
    for entity in entities.iter().filter(|entity| !entity.hidden) {
        let name = entity.properties.get("targetname").unwrap_or_default();
        charge(
            work,
            name.len().saturating_add(target.len()).saturating_add(1),
        )?;
        if !name.is_empty() && name_matches(name, target) {
            matches.push(entity.id);
        }
    }
    if !matches.is_empty() {
        return Ok(Resolution {
            matches,
            status: "named",
            reason: String::new(),
            resolved: true,
        });
    }
    for entity in entities.iter().filter(|entity| !entity.hidden) {
        let class = entity.classname().unwrap_or_default();
        charge(
            work,
            class.len().saturating_add(target.len()).saturating_add(1),
        )?;
        if !class.is_empty() && name_matches(class, target) {
            matches.push(entity.id);
        }
    }
    let resolved = !matches.is_empty();
    Ok(Resolution {
        matches,
        status: if resolved { "class" } else { "missing" },
        reason: if resolved {
            String::new()
        } else {
            "No visible authored name or fallback class matches; runtime-created entities are not inferred".into()
        },
        resolved,
    })
}
