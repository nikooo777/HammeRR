use std::collections::BTreeMap;

use source_formats::vmf::{Entity, Solid, Vmf};

use crate::metadata::InputBudget;
use crate::{ObjectId, references::admit_snapshot_storage};

pub const MAX_OBJECTS: usize = 8192;
pub const MAX_SIDES: usize = 32768;
pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_WORK: usize = 1_000_000;
pub const MAX_LABEL: usize = 512;
pub const MAX_VALUE_TOKENS: usize = crate::metadata::MAX_NODES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Identical,
    Changed,
    CurrentOnly,
    ExternalOnly,
    Ambiguous,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Self::Identical => "identical",
            Self::Changed => "changed",
            Self::CurrentOnly => "current_only",
            Self::ExternalOnly => "external_only",
            Self::Ambiguous => "ambiguous",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub object: ObjectId,
    pub status: Status,
    pub label: String,
    pub external_label: String,
    pub current_count: usize,
    pub external_count: usize,
}

#[derive(Default)]
pub struct Work(usize);

impl Work {
    pub fn checkpoint(
        &mut self,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        self.0 = self
            .0
            .checked_add(1)
            .filter(|work| *work <= MAX_WORK)
            .ok_or("Map comparison exceeds its cumulative work limit")?;
        check()
    }
}

pub fn admit_parsed(
    nodes: &[source_formats::keyvalues::Node],
    work: &mut Work,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    InputBudget::default()
        .nodes(nodes)
        .map_err(|error| error.to_string())?;
    let mut stack = nodes.iter().collect::<Vec<_>>();
    let mut tokens = 0usize;
    while let Some(node) = stack.pop() {
        work.checkpoint(check)?;
        if let Some(value) = node.as_str() {
            for _ in source_formats::vmf::numeric_tokens(value) {
                work.checkpoint(check)?;
                tokens = tokens
                    .checked_add(1)
                    .filter(|count| *count <= MAX_VALUE_TOKENS)
                    .ok_or(
                        "Map comparison exceeds 131072 stored value tokens before typed parsing",
                    )?;
            }
        } else if let Some(children) = node.children() {
            stack.extend(children);
        }
    }
    Ok(())
}

pub fn admit(
    vmf: &Vmf,
    work: &mut Work,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), String> {
    if vmf
        .world
        .solids
        .len()
        .checked_add(vmf.entities.len())
        .is_none_or(|count| count > MAX_OBJECTS)
    {
        return Err("Map comparison supports at most 8192 root objects per map".into());
    }
    let storage = admit_snapshot_storage(vmf, MAX_INPUT_BYTES, &mut || work.checkpoint(check))?;
    let mut numbers = InputBudget::default();
    let mut numeric_bytes = 0usize;
    let mut number = |value: f64| -> Result<(), String> {
        work.checkpoint(check)?;
        if !value.is_finite() {
            return Err("Map comparison requires finite stored geometry numbers".into());
        }
        let text = source_formats::vmf::format_number(value);
        numeric_bytes = numeric_bytes
            .checked_add(text.len().saturating_add(1))
            .ok_or("Comparison numeric size overflow")?;
        numbers.text("", &text).map_err(|error| error.to_string())
    };
    let mut sides = 0usize;
    for solid in vmf.solids() {
        sides = sides
            .checked_add(solid.sides.len())
            .filter(|count| *count <= MAX_SIDES)
            .ok_or("Map comparison exceeds 32768 sides")?;
        for side in &solid.sides {
            for point in side.plane.0 {
                for value in point.to_array() {
                    number(value)?;
                }
            }
            for axis in [side.u_axis, side.v_axis] {
                for value in axis.axis.to_array() {
                    number(value)?;
                }
                number(axis.offset)?;
                number(axis.scale)?;
            }
            number(side.rotation)?;
            if let Some(disp) = &side.displacement {
                for value in disp.start_position.to_array() {
                    number(value)?;
                }
                number(disp.elevation)?;
                for rows in [&disp.normals, &disp.offsets, &disp.offset_normals]
                    .into_iter()
                    .flatten()
                {
                    for row in rows {
                        for point in row {
                            for value in point.to_array() {
                                number(value)?;
                            }
                        }
                    }
                }
                for rows in [&disp.distances, &disp.alphas].into_iter().flatten() {
                    for row in rows {
                        for value in row {
                            number(*value)?;
                        }
                    }
                }
                if let Some(rows) = &disp.triangle_tags {
                    for row in rows {
                        for value in row {
                            number(f64::from(*value))?;
                        }
                    }
                }
                if let Some(values) = &disp.allowed_verts {
                    for value in values {
                        number(f64::from(*value))?;
                    }
                }
            }
        }
    }
    let allocation = storage
        .checked_mul(4)
        .and_then(|bytes| {
            numeric_bytes
                .checked_mul(4)
                .and_then(|numeric| bytes.checked_add(numeric))
        })
        .filter(|bytes| *bytes <= MAX_INPUT_BYTES)
        .ok_or("Map comparison exceeds its 8 MiB conservative typed/node storage admission")?;
    for _ in 0..allocation.div_ceil(64) {
        work.checkpoint(check)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Object<'a> {
    Solid(&'a Solid),
    Entity(&'a Entity),
}

impl Object<'_> {
    fn label(self) -> String {
        let text = match self {
            Self::Solid(solid) => format!(
                "World brush · {}",
                solid
                    .sides
                    .first()
                    .map(|side| side.material.as_str())
                    .unwrap_or("No material")
            ),
            Self::Entity(entity) => format!(
                "{} · {}",
                entity
                    .properties
                    .get("targetname")
                    .filter(|name| !name.is_empty())
                    .unwrap_or("Unnamed"),
                entity.classname().unwrap_or("Unknown entity")
            ),
        };
        text.chars().take(MAX_LABEL).collect()
    }
    fn node(self) -> source_formats::keyvalues::Node {
        match self {
            Self::Solid(solid) => source_formats::keyvalues::Node::block(
                if solid.hidden { "hidden" } else { "visible" },
                vec![solid.to_node()],
            ),
            Self::Entity(entity) => entity.to_node(),
        }
    }
}

fn objects(vmf: &Vmf) -> BTreeMap<ObjectId, Vec<Object<'_>>> {
    let mut objects = BTreeMap::<ObjectId, Vec<Object<'_>>>::new();
    for solid in &vmf.world.solids {
        objects
            .entry(ObjectId::Solid(solid.id))
            .or_default()
            .push(Object::Solid(solid));
    }
    for entity in &vmf.entities {
        objects
            .entry(ObjectId::Entity(entity.id))
            .or_default()
            .push(Object::Entity(entity));
    }
    objects
}

pub fn compare(
    current: &Vmf,
    external: &Vmf,
    work: &mut Work,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Vec<Row>, String> {
    admit(current, work, check)?;
    admit(external, work, check)?;
    let current = objects(current);
    let external = objects(external);
    let mut rows = Vec::new();
    for object in current
        .keys()
        .chain(external.keys())
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
    {
        work.checkpoint(check)?;
        let left = current.get(&object).map(Vec::as_slice).unwrap_or_default();
        let right = external.get(&object).map(Vec::as_slice).unwrap_or_default();
        let status = match (left.len(), right.len()) {
            (1, 1) => {
                if left[0].node() == right[0].node() {
                    Status::Identical
                } else {
                    Status::Changed
                }
            }
            (1, 0) => Status::CurrentOnly,
            (0, 1) => Status::ExternalOnly,
            _ => Status::Ambiguous,
        };
        rows.push(Row {
            object,
            status,
            label: left
                .first()
                .copied()
                .or_else(|| right.first().copied())
                .map(Object::label)
                .unwrap_or_default(),
            external_label: right
                .first()
                .copied()
                .map(Object::label)
                .unwrap_or_default(),
            current_count: left.len(),
            external_count: right.len(),
        });
    }
    Ok(rows)
}
