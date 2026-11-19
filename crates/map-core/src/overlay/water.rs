use std::collections::BTreeSet;

use source_formats::{
    bsp,
    keyvalues::{Node, Value},
    vmf::Entity,
};

use super::{Definition, Options};

pub const CLASSNAME: &str = "info_overlay_transition";
const TRANSITION: &str = "overlaytransition";
const DATA: &str = "overlaydata";
const MAX_FIELDS: usize = 32;
const MAX_FIELD_BYTES: usize = 4096;
const REGULAR_FIELDS: &[&str] = &[
    "RenderOrder",
    "fademindist",
    "fademaxdist",
    "targetname",
    "classname",
];

#[derive(Clone, Copy)]
pub(crate) struct Location {
    transition: usize,
    projection: usize,
}

pub fn is_transition(entity: &Entity) -> bool {
    entity
        .classname()
        .is_some_and(|class| class.eq_ignore_ascii_case(CLASSNAME))
}

pub fn has_transition_data(entity: &Entity) -> bool {
    entity
        .extra
        .iter()
        .any(|node| node.key.eq_ignore_ascii_case(TRANSITION))
}

pub fn parse(entity: &Entity) -> Result<Vec<Definition>, String> {
    Ok(projections(entity)?
        .into_iter()
        .map(|(_, projection)| projection)
        .collect())
}

pub fn topology_fields_known(entity: &Entity) -> bool {
    const FIELDS: &[&str] = &[
        "material",
        "sides",
        "StartU",
        "EndU",
        "StartV",
        "EndV",
        "BasisOrigin",
        "BasisU",
        "BasisV",
        "BasisNormal",
        "uv0",
        "uv1",
        "uv2",
        "uv3",
    ];
    is_transition(entity)
        && entity.extra.iter().all(|transition| {
            transition.key.eq_ignore_ascii_case(TRANSITION)
                && transition.children().is_some_and(|projections| {
                    projections.iter().all(|projection| {
                        projection.key.eq_ignore_ascii_case(DATA)
                            && projection.children().is_some_and(|fields| {
                                fields.iter().all(|field| {
                                    field.as_str().is_some()
                                        && FIELDS
                                            .iter()
                                            .any(|key| field.key.eq_ignore_ascii_case(key))
                                })
                            })
                    })
                })
        })
}

pub(crate) fn side_lists(entity: &Entity) -> impl Iterator<Item = &str> {
    entity
        .extra
        .iter()
        .filter(|node| node.key.eq_ignore_ascii_case(TRANSITION))
        .flat_map(|node| node.children().into_iter().flatten())
        .filter(|node| node.key.eq_ignore_ascii_case(DATA))
        .flat_map(|node| node.children().into_iter().flatten())
        .filter(|node| node.key.eq_ignore_ascii_case("sides"))
        .filter_map(Node::as_str)
}

pub(crate) fn projections(entity: &Entity) -> Result<Vec<(Location, Definition)>, String> {
    if !is_transition(entity) {
        return Err("Expected an info_overlay_transition entity".into());
    }
    if entity.solids.iter().any(|solid| !solid.hidden) {
        return Err("info_overlay_transition is a point helper; remove its brush geometry".into());
    }
    let mut definitions = Vec::new();
    for (transition, block) in entity.extra.iter().enumerate() {
        if !block.key.eq_ignore_ascii_case(TRANSITION) {
            continue;
        }
        let children = block
            .children()
            .ok_or("Water overlaytransition must be a block")?;
        if children.is_empty() {
            return Err("Water overlaytransition needs at least one overlaydata block".into());
        }
        for (projection, data) in children.iter().enumerate() {
            if !data.key.eq_ignore_ascii_case(DATA) {
                return Err(
                    "Water overlaytransition supports only explicit overlaydata blocks".into(),
                );
            }
            if definitions.len() >= bsp::MAX_WATER_OVERLAYS {
                return Err(format!(
                    "Water overlays exceed their {}-record allocation budget",
                    bsp::MAX_WATER_OVERLAYS
                ));
            }
            let fields = data.children().ok_or("Water overlaydata must be a block")?;
            if fields.len() > MAX_FIELDS {
                return Err(format!(
                    "Water overlaydata exceeds its {MAX_FIELDS}-field budget"
                ));
            }
            let mut source = Entity::new(entity.id, "info_overlay");
            let mut keys = BTreeSet::new();
            for field in fields {
                let value = field
                    .as_str()
                    .ok_or("Water overlaydata fields must be strings")?;
                if field.key.len() > MAX_FIELD_BYTES || value.len() > MAX_FIELD_BYTES {
                    return Err(format!(
                        "Water overlaydata field exceeds its {MAX_FIELD_BYTES}-byte budget"
                    ));
                }
                if !keys.insert(field.key.to_ascii_lowercase()) {
                    return Err(format!("Water overlaydata repeats field {}", field.key));
                }
                if REGULAR_FIELDS
                    .iter()
                    .any(|key| field.key.eq_ignore_ascii_case(key))
                {
                    return Err(format!("Water overlaydata does not support {}", field.key));
                }
                source
                    .properties
                    .0
                    .push((field.key.clone(), value.to_owned()));
            }
            let definition = super::parse(&source).map_err(|error| {
                format!(
                    "Water overlay projection {}: {error}",
                    definitions.len() + 1
                )
            })?;
            definitions.push((
                Location {
                    transition,
                    projection,
                },
                definition,
            ));
        }
    }
    if definitions.is_empty() {
        return Err(
            "info_overlay_transition needs nested overlaytransition/overlaydata projection blocks"
                .into(),
        );
    }
    Ok(definitions)
}

fn one_projection(entity: &Entity) -> Result<(Location, Definition), String> {
    let mut projections = projections(entity)?;
    if projections.len() != 1 {
        return Err("Editing a water transition requires exactly one projection; imported multi-projection helpers are preserved".into());
    }
    if ["sides", "sides2"].into_iter().any(|key| {
        entity
            .properties
            .get_all(key)
            .any(|value| !value.trim().is_empty())
    }) {
        return Err("Editing requires nested water transition bindings without legacy helper sides or sides2".into());
    }
    Ok(projections.pop().unwrap())
}

pub(super) fn editable(entity: &Entity) -> Result<Definition, String> {
    one_projection(entity).map(|(_, projection)| projection)
}

pub fn validate_options(options: &Options) -> Result<(), String> {
    if options.material.len() > MAX_FIELD_BYTES {
        return Err(format!(
            "Water transition material exceeds its {MAX_FIELD_BYTES}-byte budget"
        ));
    }
    let defaults = Options::default();
    if options
        .targetname
        .as_deref()
        .is_some_and(|name| !name.trim().is_empty())
        || options.render_order != defaults.render_order
        || options.fade_min != defaults.fade_min
        || options.fade_max != defaults.fade_max
    {
        return Err(
            "Water transitions have no runtime name, render order or distance fades".into(),
        );
    }
    Ok(())
}

pub(super) fn encode(fields: Vec<(String, String)>) -> Node {
    let fields = fields
        .into_iter()
        .filter(|(key, _)| {
            !REGULAR_FIELDS
                .iter()
                .any(|regular| key.eq_ignore_ascii_case(regular))
        })
        .map(|(key, value)| Node::string(key, value))
        .collect();
    Node::block(TRANSITION, vec![Node::block(DATA, fields)])
}

pub(super) fn update(
    entity: &mut Entity,
    fields: impl IntoIterator<Item = (String, String)>,
) -> Result<(), String> {
    let (location, _) = one_projection(entity)?;
    update_at(entity, location, fields)
}

pub(crate) fn update_at(
    entity: &mut Entity,
    location: Location,
    fields: impl IntoIterator<Item = (String, String)>,
) -> Result<(), String> {
    let Some(Node {
        value: Value::Block(projections),
        ..
    }) = entity
        .extra
        .get_mut(location.transition)
        .filter(|node| node.key.eq_ignore_ascii_case(TRANSITION))
    else {
        return Err("Water transition block changed before applying its projection".into());
    };
    let Some(Node {
        value: Value::Block(nodes),
        ..
    }) = projections
        .get_mut(location.projection)
        .filter(|node| node.key.eq_ignore_ascii_case(DATA))
    else {
        return Err("Water projection block changed before applying edits".into());
    };
    for (key, value) in fields {
        if let Some(node) = nodes
            .iter_mut()
            .find(|node| node.key.eq_ignore_ascii_case(&key))
        {
            node.value = Value::String(value);
        } else {
            nodes.push(Node::string(key, value));
        }
    }
    Ok(())
}
