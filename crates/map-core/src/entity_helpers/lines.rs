use super::values::bounded;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
};

use glam::DVec3;
use source_formats::{
    fgd::{Catalog, Class, ClassKind, Helper},
    vmf::Entity,
};

pub const MAX_LINES_PER_ENTITY: usize = 64;
pub const MAX_KEY_BYTES: usize = 128;
pub const MAX_VALUE_BYTES: usize = 256;
pub const MAX_TARGET_ENTITIES: usize = 65_536;
pub const MAX_ANCHOR_CACHE: usize = 512;

type Anchor = Result<(DVec3, &'static str), String>;

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub lookup_property: Option<&'static str>,
    pub property: Option<String>,
    pub raw: Option<String>,
    pub raw_truncated: bool,
    pub value_source: &'static str,
    pub entity: Option<u32>,
    pub position_source: Option<&'static str>,
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
            position_source: None,
            position: None,
            matches: 0,
            state,
            error: Some(error.into()),
        }
    }

    fn set_position(&mut self, entity: &Entity, anchor: Anchor) {
        self.entity = Some(entity.id);
        self.position_source = Some(if entity.solids.is_empty() {
            "authored_origin"
        } else {
            "brush_bounds_center"
        });
        match anchor {
            Ok((position, source)) => {
                self.position = Some(position.to_array());
                self.position_source = Some(source);
                self.state = "ready";
                self.error = None;
            }
            Err(error) => {
                self.state = "invalid_origin";
                self.error = Some(error);
            }
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
    anchors: RefCell<BTreeMap<u32, Anchor>>,
    anchor_work: Cell<usize>,
}

impl<'a> Targets<'a> {
    pub fn new(entities: &'a [Entity], catalog: &Catalog) -> Self {
        let mut result = Self {
            names: BTreeMap::new(),
            node_ids: BTreeMap::new(),
            limited: entities.len() > MAX_TARGET_ENTITIES,
            anchors: Default::default(),
            anchor_work: Cell::new(0),
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

    pub(super) fn anchor(&self, entity: &Entity) -> Anchor {
        if let Some(anchor) = self.anchors.borrow().get(&entity.id) {
            return anchor.clone();
        }
        if self.anchors.borrow().len() == MAX_ANCHOR_CACHE {
            return Err("Helper anchors exceed their 512-entity cache limit".into());
        }
        let result = super::values::anchor_work(entity).and_then(|work| {
            let total = self
                .anchor_work
                .get()
                .checked_add(work)
                .filter(|total| *total <= super::values::MAX_ANCHOR_WORK)
                .ok_or("Helper anchors exceed their shared brush clipping work limit")?;
            self.anchor_work.set(total);
            super::values::anchor(entity)
        });
        self.anchors.borrow_mut().insert(entity.id, result.clone());
        result
    }

    pub(super) fn endpoint(
        &self,
        class: &Class,
        entity: &Entity,
        lookup: &str,
        key: &str,
    ) -> Endpoint {
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
        let value = super::values::Value::resolve(class, entity, &property.name, true);
        let raw = value.raw;
        result.raw = raw.map(|raw| bounded(raw, MAX_VALUE_BYTES));
        result.raw_truncated = raw.is_some_and(|raw| raw.len() > MAX_VALUE_BYTES);
        result.value_source = value.source;
        if value.duplicate {
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
        names.next();
        if names.next().is_some() {
            result.state = "duplicate_targetname";
            result.error = Some("Line endpoint has duplicate targetname keys".into());
            return result;
        }
        result.set_position(candidate, self.anchor(candidate));
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
            result.set_position(candidate, self.anchor(candidate));
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
            line.error = Some(format!(
                "Line guides support at most {MAX_LINES_PER_ENTITY} declarations per entity; remaining declarations were omitted"
            ));
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
            own.set_position(entity, targets.anchor(entity));
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
