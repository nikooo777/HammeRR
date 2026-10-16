use std::collections::BTreeMap;

use glam::DVec3;
use source_formats::{
    fgd::{Catalog, Class, ClassKind, Helper},
    vmf::Entity,
};

use super::MAX_HELPER_EXTENT;

pub const MAX_LINES_PER_ENTITY: usize = 16;
pub const MAX_KEY_BYTES: usize = 128;
pub const MAX_VALUE_BYTES: usize = 256;
pub const MAX_TARGET_ENTITIES: usize = 65_536;

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub lookup_property: Option<&'static str>,
    pub property: Option<String>,
    pub raw: Option<String>,
    pub raw_truncated: bool,
    pub value_source: &'static str,
    pub entity: Option<u32>,
    pub position: Option<[f64; 3]>,
    pub matches: usize,
    pub state: &'static str,
    pub error: Option<String>,
}

impl Endpoint {
    fn unavailable(state: &'static str, error: &str) -> Self {
        Self {
            lookup_property: None,
            property: None,
            raw: None,
            raw_truncated: false,
            value_source: "missing",
            entity: None,
            position: None,
            matches: 0,
            state,
            error: Some(error.into()),
        }
    }

    fn set_position(&mut self, entity: &Entity) {
        self.entity = Some(entity.id);
        let mut origins = entity.properties.get_all("origin");
        let position = origins
            .next()
            .filter(|raw| raw.len() <= MAX_VALUE_BYTES)
            .and_then(source_formats::vmf::parse_vec3)
            .filter(|point| {
                point.is_finite()
                    && point.as_vec3().is_finite()
                    && point.abs().max_element() <= MAX_HELPER_EXTENT
            });
        if origins.next().is_some() || position.is_none() {
            self.state = "invalid_origin";
            self.error =
                Some("Line endpoint needs one finite authored origin within ±65536 units".into());
        } else {
            self.position = position.map(|point| point.to_array());
            self.state = "ready";
            self.error = None;
        }
    }
}

#[derive(Debug, Clone)]
pub struct Line {
    pub color: Option<[u8; 3]>,
    pub endpoints: [Endpoint; 2],
    pub state: &'static str,
    pub error: Option<String>,
}

pub struct Targets<'a> {
    names: BTreeMap<String, Vec<&'a Entity>>,
    node_ids: BTreeMap<String, Vec<&'a Entity>>,
    limited: bool,
}

impl<'a> Targets<'a> {
    pub fn new(entities: &'a [Entity], catalog: &Catalog) -> Self {
        let mut result = Self {
            names: BTreeMap::new(),
            node_ids: BTreeMap::new(),
            limited: entities.len() > MAX_TARGET_ENTITIES,
        };
        if result.limited {
            return result;
        }
        let mut indexed = 0;
        for entity in entities {
            for name in entity.properties.get_all("targetname") {
                if indexed == MAX_TARGET_ENTITIES {
                    result.names.clear();
                    result.node_ids.clear();
                    result.limited = true;
                    return result;
                }
                indexed += 1;
                if name.is_empty() || name.len() > MAX_VALUE_BYTES {
                    continue;
                }
                let matches = result.names.entry(name.to_ascii_lowercase()).or_default();
                if matches.last().is_none_or(|last| last.id != entity.id) {
                    matches.push(entity);
                }
            }
            let node_class = entity.classname().and_then(|name| catalog.class(name));
            if !node_class.is_some_and(|class| {
                class.kind == ClassKind::Point
                    && class
                        .property("nodeid")
                        .is_some_and(|property| property.kind == "integer" && property.read_only)
            }) {
                continue;
            }
            for raw in entity.properties.get_all("nodeid") {
                if indexed == MAX_TARGET_ENTITIES {
                    result.names.clear();
                    result.node_ids.clear();
                    result.limited = true;
                    return result;
                }
                indexed += 1;
                let Some(id) = (raw.len() <= MAX_VALUE_BYTES)
                    .then(|| raw.trim().parse::<i32>().ok())
                    .flatten()
                    .filter(|id| *id >= 0)
                else {
                    continue;
                };
                let matches = result.node_ids.entry(id.to_string()).or_default();
                if matches.last().is_none_or(|last| last.id != entity.id) {
                    matches.push(entity);
                }
            }
        }
        result
    }

    fn endpoint(&self, class: &Class, entity: &Entity, lookup: &str, key: &str) -> Endpoint {
        let property = class.property(key).unwrap();
        let mut result = Endpoint::unavailable(
            "missing",
            "Line reference has no authored value or FGD default",
        );
        result.lookup_property = Some(if lookup.eq_ignore_ascii_case("nodeid") {
            "nodeid"
        } else {
            "targetname"
        });
        result.property = Some(property.name.clone());
        let mut authored = entity.properties.get_all(&property.name);
        let first = authored.next();
        let raw = first.or(property.default.as_deref());
        result.value_source = if first.is_some() {
            "authored"
        } else if raw.is_some() {
            "fgd_default"
        } else {
            "missing"
        };
        result.raw = raw.map(|value| bounded(value, MAX_VALUE_BYTES));
        result.raw_truncated = raw.is_some_and(|value| value.len() > MAX_VALUE_BYTES);
        if authored.next().is_some() {
            result.state = "duplicate_property";
            result.error = Some("Line reference property occurs more than once".into());
            return result;
        }
        let Some(raw) = raw.filter(|raw| !raw.is_empty()) else {
            return result;
        };
        if result.lookup_property == Some("nodeid") {
            return self.node_endpoint(result, raw);
        }
        if result.raw_truncated
            || raw.trim() != raw
            || raw.chars().any(char::is_control)
            || raw.starts_with('!')
            || raw.contains(['*', '?'])
        {
            result.state = "unsupported_reference";
            result.error = Some("Line references require literal names up to 256 bytes; whitespace padding, control characters, wildcards and !tokens are unsupported".into());
            return result;
        }
        if self.limited {
            result.state = "entity_limit";
            result.error = Some("Line lookup exceeds 65536 entities or targetname/nodeid values; no partial lookup was used".into());
            return result;
        }
        let candidates = self
            .names
            .get(&raw.to_ascii_lowercase())
            .map(Vec::as_slice)
            .unwrap_or_default();
        result.matches = candidates.len();
        if candidates.is_empty() {
            result.error = Some("Line reference names no authored targetname".into());
            return result;
        }
        if candidates.len() != 1 {
            result.state = "ambiguous";
            result.error = Some(
                "Line reference matches multiple entities; no first endpoint was chosen".into(),
            );
            return result;
        }
        let candidate = candidates[0];
        let mut names = candidate.properties.get_all("targetname");
        let name = names.next();
        if names.next().is_some() {
            result.state = "duplicate_targetname";
            result.error = Some("Line endpoint has duplicate targetname keys".into());
            return result;
        }
        if name != Some(raw) {
            result.state = "unsupported_case_match";
            result.error = Some("Line guides require exact-case literal names; native helper case matching is unconfirmed".into());
            return result;
        }
        result.set_position(candidate);
        result
    }

    fn node_endpoint(&self, mut result: Endpoint, raw: &str) -> Endpoint {
        if raw == "-1" {
            result.state = "no_association";
            result.error = Some("Node reference -1 declares no associated node".into());
            return result;
        }
        if !canonical_node_id(raw) {
            result.state = "unsupported_reference";
            result.error = Some("Node references require canonical decimal IDs from 0 to 2147483647, or -1 for no association".into());
            return result;
        }
        if self.limited {
            result.state = "entity_limit";
            result.error = Some("Line lookup exceeds 65536 entities or targetname/nodeid values; no partial lookup was used".into());
            return result;
        }
        let candidates = self
            .node_ids
            .get(raw)
            .map(Vec::as_slice)
            .unwrap_or_default();
        result.matches = candidates.len();
        let [candidate] = candidates else {
            if candidates.is_empty() {
                result.error = Some("Node reference matches no authored nodeid on a point class with a declared readonly integer nodeid".into());
            } else {
                result.state = "ambiguous";
                result.error = Some(
                    "Node reference matches multiple authored IDs; no first endpoint was chosen"
                        .into(),
                );
            }
            return result;
        };
        let mut ids = candidate.properties.get_all("nodeid");
        let id = ids.next().unwrap();
        if ids.next().is_some() {
            result.state = "duplicate_nodeid";
            result.error = Some("Line endpoint has duplicate nodeid keys".into());
        } else if !canonical_node_id(id) {
            result.state = "invalid_nodeid";
            result.error = Some(
                "Line endpoint nodeid is not a canonical nonnegative 32-bit decimal ID".into(),
            );
        } else {
            result.set_position(candidate);
        }
        result
    }
}

fn canonical_node_id(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 10
        && (raw == "0" || !raw.starts_with('0'))
        && raw.bytes().all(|byte| byte.is_ascii_digit())
        && raw.parse::<i32>().is_ok()
}

fn bounded(value: &str, maximum: usize) -> String {
    let mut end = value.len().min(maximum);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].into()
}

pub(super) fn supported(class: &Class, helper: &Helper) -> bool {
    if helper.name != "line" || ![5, 7].contains(&helper.arguments.len()) {
        return false;
    }
    helper.arguments[..3]
        .iter()
        .all(|value| value.parse::<u8>().is_ok())
        && helper.arguments[3..].as_chunks::<2>().0.iter().all(|pair| {
            !pair[1].is_empty()
                && pair[1].len() <= MAX_KEY_BYTES
                && class.property(&pair[1]).is_some_and(|property| {
                    pair[0].eq_ignore_ascii_case("targetname")
                        || (pair[0].eq_ignore_ascii_case("nodeid") && property.kind == "node_dest")
                })
        })
}

pub fn resolve(class: &Class, entity: &Entity, targets: &Targets<'_>) -> Vec<Line> {
    let mut result = Vec::new();
    for helper in class.helpers.iter().filter(|helper| helper.name == "line") {
        let unsupported = Endpoint::unavailable(
            "unsupported",
            "line() supports literal RGB and declared targetname/property or nodeid/node_dest pairs",
        );
        let mut line = Line {
            color: None,
            endpoints: [unsupported.clone(), unsupported],
            state: "unsupported",
            error: None,
        };
        if result.len() == MAX_LINES_PER_ENTITY {
            line.error = Some("Line guides support at most 16 declarations per entity; remaining declarations were omitted".into());
            result.push(line);
            break;
        }
        if !supported(class, helper) {
            line.error = line.endpoints[0].error.clone();
            result.push(line);
            continue;
        }
        let args = &helper.arguments;
        line.color = Some(std::array::from_fn(|i| args[i].parse().unwrap()));
        let first = targets.endpoint(class, entity, &args[3], &args[4]);
        line.endpoints = if args.len() == 7 {
            [first, targets.endpoint(class, entity, &args[5], &args[6])]
        } else {
            let mut own =
                Endpoint::unavailable("invalid_origin", "Line source has no authored origin");
            own.value_source = "self_origin";
            own.set_position(entity);
            [own, first]
        };
        if let Some(endpoint) = line
            .endpoints
            .iter()
            .find(|endpoint| endpoint.state != "ready")
        {
            line.state = endpoint.state;
            line.error = endpoint.error.clone();
        } else {
            let points = line
                .endpoints
                .each_ref()
                .map(|endpoint| DVec3::from_array(endpoint.position.unwrap()).as_vec3());
            if points[0] == points[1] {
                line.state = "coincident";
                line.error =
                    Some("Line endpoints coincide at native world-coordinate precision".into());
            } else {
                line.state = "ready";
            }
        }
        result.push(line);
    }
    result
}

#[cfg(test)]
mod tests;
