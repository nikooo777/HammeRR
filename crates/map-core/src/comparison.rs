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

#[cfg(test)]
mod tests {
    use super::*;
    use source_formats::{
        keyvalues::Node,
        vmf::{Connection, Entity, Properties},
    };

    fn map() -> Vmf {
        Vmf::parse(r#"world { "id" "1" "classname" "worldspawn" } entity { "id" "10" "classname" "logic_relay" "targetname" "Lamp" "origin" "0 0 64" }"#).unwrap()
    }
    fn rows(left: &Vmf, right: &Vmf) -> Vec<Row> {
        compare(left, right, &mut Work::default(), &mut || Ok(())).unwrap()
    }

    #[test]
    fn scoped_ids_report_all_five_categories_and_duplicate_counts() {
        let mut current = map();
        for id in 11..=13 {
            let mut entity = current.entities[0].clone();
            entity.id = id;
            current.entities.push(entity);
        }
        let mut external = current.clone();
        external.entities[1].properties.set("origin", "128 0 64");
        external.entities.remove(2);
        external.entities.push(external.entities[2].clone());
        external.entities.push(Entity::new(99, "logic_relay"));
        let result = rows(&current, &external);
        assert_eq!(
            result.iter().map(|row| row.status).collect::<Vec<_>>(),
            [
                Status::Identical,
                Status::Changed,
                Status::CurrentOnly,
                Status::Ambiguous,
                Status::ExternalOnly
            ]
        );
        assert_eq!((result[3].current_count, result[3].external_count), (1, 2));
        external.entities.reverse();
        assert_eq!(
            rows(&current, &external)
                .iter()
                .map(|row| row.status)
                .collect::<Vec<_>>(),
            result.iter().map(|row| row.status).collect::<Vec<_>>()
        );
    }

    #[test]
    fn canonical_authored_editor_hidden_opaque_and_order_data_count() {
        let current = map();
        for mutation in 0..6 {
            let mut external = current.clone();
            let entity = &mut external.entities[0];
            match mutation {
                0 => entity.hidden = true,
                1 => entity.editor = Some(Properties(vec![("color".into(), "1 2 3".into())])),
                2 => entity
                    .extra
                    .push(Node::block("opaque", vec![Node::string("saved", "yes")])),
                3 => entity.properties.0.swap(0, 1),
                4 => {
                    entity.connections = Some(vec![Connection {
                        output: "OnTrigger".into(),
                        value: "Lamp,Trigger,,0,-1".into(),
                    }])
                }
                _ => entity.properties.set("origin", "0.0 0 64"),
            }
            assert_eq!(
                rows(&current, &external)[0].status,
                Status::Changed,
                "mutation {mutation}"
            );
        }
        let mut external = current.clone();
        external.world.properties.set("skyname", "different");
        assert_eq!(rows(&current, &external)[0].status, Status::Identical);
        external.entities[0].id = 20;
        assert_eq!(
            rows(&current, &external)
                .iter()
                .map(|row| row.status)
                .collect::<Vec<_>>(),
            [Status::CurrentOnly, Status::ExternalOnly]
        );
    }

    #[test]
    fn geometry_storage_and_cumulative_cancellation_are_admitted_before_nodes() {
        let amplified = [Node::string(
            "plane",
            "0 ".repeat(crate::metadata::MAX_NODES + 1),
        )];
        assert!(
            admit_parsed(&amplified, &mut Work::default(), &mut || Ok(()))
                .unwrap_err()
                .contains("value tokens")
        );

        let map = map();
        let mut work = Work(MAX_WORK);
        assert!(
            compare(&map, &map, &mut work, &mut || Ok(()))
                .unwrap_err()
                .contains("work limit")
        );
        assert!(
            compare(&map, &map, &mut Work::default(), &mut || Err(
                "cancelled".into()
            ))
            .unwrap_err()
            .contains("cancelled")
        );
        let mut map = map;
        map.entities[0]
            .extra
            .push(Node::string("large", "x".repeat(MAX_INPUT_BYTES)));
        assert!(admit(&map, &mut Work::default(), &mut || Ok(())).is_err());
        let mut document = crate::Document::empty();
        document
            .edit("finite brush", |tx| {
                crate::edit::create_block(
                    tx,
                    glam::DVec3::ZERO,
                    glam::DVec3::splat(64.0),
                    "brick/test",
                )
            })
            .unwrap();
        let mut geometry = document.vmf().clone();
        geometry.world.solids[0].sides[0].plane.0[0].x = f64::NAN;
        assert!(
            admit(&geometry, &mut Work::default(), &mut || Ok(()))
                .unwrap_err()
                .contains("finite")
        );
    }

    #[test]
    fn world_brush_and_whole_entity_geometry_keep_hidden_and_nested_ids() {
        let mut document = crate::Document::empty();
        document
            .edit("brushes", |tx| {
                crate::edit::create_block(
                    tx,
                    glam::DVec3::ZERO,
                    glam::DVec3::splat(64.0),
                    "brick/test",
                )?;
                crate::edit::create_block(
                    tx,
                    glam::DVec3::splat(128.0),
                    glam::DVec3::splat(192.0),
                    "brick/test",
                )?;
                Ok(())
            })
            .unwrap();
        let current = document.vmf().clone();
        let mut external = current.clone();
        external.world.solids.reverse();
        assert!(
            rows(&current, &external)
                .iter()
                .all(|row| row.status == Status::Identical)
        );
        external.world.solids[0].hidden = true;
        assert_eq!(
            rows(&current, &external)
                .iter()
                .filter(|row| row.status == Status::Changed)
                .count(),
            1
        );
        let mut current = current;
        let mut owner = Entity::new(100, "func_detail");
        owner.solids = std::mem::take(&mut current.world.solids);
        current.entities.push(owner);
        let mut external = current.clone();
        external.entities[0].solids[0].sides[0].id += 1000;
        let result = rows(&current, &external);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].object, ObjectId::Entity(100));
        assert_eq!(result[0].status, Status::Changed);
    }
}
