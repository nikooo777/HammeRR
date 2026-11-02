use std::collections::{BTreeMap, BTreeSet};

use glam::{DMat3, DVec3};
use source_formats::{
    fgd::{Catalog, ClassKind},
    vmf::Entity,
};

use crate::{Document, EditError, ObjectId, Transaction, connections, edit, references, repeat};

pub const MIN_POINTS: usize = 2;
pub const MAX_POINTS: usize = 64;
pub const MAX_PREFIX_BYTES: usize = 96;
pub const MAX_REFERENCE_WORK: usize = 4_000_000;

mod inspect;
mod topology;
mod train;
pub use inspect::inspect;
pub use train::{MAX_TRAIN_SPEED, MAX_TRAINS, associated_trains, startup_helpers};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Graph {
    pub classname: String,
    pub nodes: Vec<u32>,
    pub closed: bool,
}

#[derive(Clone, Debug)]
pub enum Request {
    Topology {
        seed: u32,
        classname: String,
        closed: bool,
    },
    Train {
        seed: u32,
        train: u32,
        speed: f64,
        start_on: bool,
    },
    Duplicate {
        seed: u32,
        train: Option<u32>,
        offset: DVec3,
        policies: repeat::Policies,
    },
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub graph: Graph,
    pub objects: Vec<ObjectId>,
    pub train: Option<u32>,
    pub helpers: Vec<u32>,
    pub removed_helpers: Vec<u32>,
    pub mapping: Vec<(ObjectId, ObjectId)>,
    pub names: Vec<repeat::Name>,
    pub remapped_references: usize,
    pub unselected_incoming: Vec<u32>,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn exact_name(name: &str) -> bool {
    !name.is_empty()
        && name.trim() == name
        && !name.starts_with('!')
        && !name.contains(['*', '?', ',', '"'])
        && !name.chars().any(char::is_control)
}

pub fn validate_points(points: &[DVec3]) -> Result<(), EditError> {
    if !(MIN_POINTS..=MAX_POINTS).contains(&points.len()) {
        return Err(invalid(format!(
            "Use {MIN_POINTS}–{MAX_POINTS} ordered path points"
        )));
    }
    for (index, point) in points.iter().enumerate() {
        if !point.is_finite() || point.abs().max_element() > crate::primitive::MAX_COORDINATE {
            return Err(invalid(
                "Path points must use finite coordinates within ±65536",
            ));
        }
        if points[..index].contains(point) {
            return Err(invalid(
                "Path points must be distinct; use Closed loop to link the last point to the first",
            ));
        }
    }
    Ok(())
}

fn validate_class(catalog: &Catalog, classname: &str, kind: ClassKind) -> Result<(), EditError> {
    if let Some(class) = catalog.class(classname)
        && class.kind != kind
    {
        return Err(invalid(format!(
            "{} must be an FGD {} class",
            class.name,
            kind.label()
        )));
    }
    Ok(())
}

fn validate_property(
    catalog: &Catalog,
    classname: &str,
    key: &str,
    value: Option<&str>,
) -> Result<(), EditError> {
    source_formats::keyvalues::validate_string(key).map_err(|error| invalid(error.to_string()))?;
    if let Some(value) = value {
        source_formats::keyvalues::validate_string(value)
            .map_err(|error| invalid(error.to_string()))?;
    }
    if let Some(property) = catalog
        .class(classname)
        .and_then(|class| class.property(key))
    {
        if property.read_only {
            return Err(invalid(format!("{key} is read-only")));
        }
        if let Some(value) = value {
            property.validate(value).map_err(invalid)?;
        }
    }
    Ok(())
}

pub fn set_positions(
    tx: &mut Transaction,
    graph: &Graph,
    points: &[DVec3],
    catalog: &Catalog,
) -> Result<(), EditError> {
    validate_points(points)?;
    if graph.nodes.len() != points.len() || inspect(tx.document(), graph.nodes[0])? != *graph {
        return Err(invalid(
            "Path topology changed; inspect it before positioning nodes",
        ));
    }
    validate_class(catalog, &graph.classname, ClassKind::Point)?;
    for (&id, &point) in graph.nodes.iter().zip(points) {
        if tx.document().entity(id).and_then(Entity::origin) == Some(point) {
            continue;
        }
        let value = source_formats::vmf::format_vec3(point);
        validate_property(catalog, &graph.classname, "origin", Some(&value))?;
        edit::set_properties(tx, id, &[("origin".into(), Some(value))])?;
    }
    Ok(())
}

fn outcome(graph: Graph) -> Outcome {
    Outcome {
        objects: graph.nodes.iter().copied().map(ObjectId::Entity).collect(),
        graph,
        train: None,
        helpers: Vec::new(),
        removed_helpers: Vec::new(),
        mapping: Vec::new(),
        names: Vec::new(),
        remapped_references: 0,
        unselected_incoming: Vec::new(),
    }
}

pub fn apply(
    tx: &mut Transaction,
    request: &Request,
    catalog: &Catalog,
) -> Result<Outcome, EditError> {
    tx.document()
        .vmf()
        .validate_ids()
        .map_err(|error| invalid(error.to_string()))?;
    match request {
        Request::Topology {
            seed,
            classname,
            closed,
        } => topology::apply(tx, *seed, classname, *closed, catalog),
        Request::Train {
            seed,
            train,
            speed,
            start_on,
        } => train::apply(tx, *seed, *train, *speed, *start_on, catalog),
        Request::Duplicate {
            seed,
            train,
            offset,
            policies,
        } => duplicate(tx, *seed, *train, *offset, policies, catalog),
    }
}

fn incoming_sources(
    document: &Document,
    ids: &[ObjectId],
    catalog: &Catalog,
) -> Result<Vec<u32>, EditError> {
    let sources = ids
        .iter()
        .filter_map(|id| match id {
            ObjectId::Entity(id) => Some(*id),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let names = sources
        .iter()
        .filter_map(|&id| {
            document
                .entity(id)
                .and_then(|entity| entity.properties.get("targetname"))
        })
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    let scan = references::scan(document, catalog);
    if scan.len().saturating_mul(names.len()) > MAX_REFERENCE_WORK {
        return Err(invalid(
            "Path reference inspection exceeds four million checks",
        ));
    }
    Ok(scan
        .iter()
        .filter(|reference| {
            !sources.contains(&reference.source)
                && names
                    .iter()
                    .any(|name| connections::name_matches(name, &reference.target))
        })
        .map(|reference| reference.source)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

fn duplicate(
    tx: &mut Transaction,
    seed: u32,
    train: Option<u32>,
    offset: DVec3,
    policies: &repeat::Policies,
    catalog: &Catalog,
) -> Result<Outcome, EditError> {
    let graph = inspect(tx.document(), seed)?;
    validate_class(catalog, &graph.classname, ClassKind::Point)?;
    if policies.internal != repeat::InternalReferences::Remap {
        return Err(invalid(
            "Linked graph duplication must remap internal references",
        ));
    }
    let mut ids = graph
        .nodes
        .iter()
        .copied()
        .map(ObjectId::Entity)
        .collect::<Vec<_>>();
    let mut helpers = Vec::new();
    if let Some(id) = train {
        train::validate(tx.document(), &graph, id, catalog)?;
        if !associated_trains(tx.document(), &graph)?.contains(&id) {
            return Err(invalid(
                "The duplicated train must already target this path",
            ));
        }
        ids.push(ObjectId::Entity(id));
        helpers = startup_helpers(tx.document(), id)?;
        ids.extend(helpers.iter().copied().map(ObjectId::Entity));
    }
    let unselected_incoming = incoming_sources(tx.document(), &ids, catalog)?;
    let result = repeat::create_linked_path(
        tx,
        &ids,
        repeat::Placement {
            translation: offset,
            rotation: DVec3::ZERO,
            scale: DVec3::ONE,
            pivot: DVec3::ZERO,
            basis: DMat3::IDENTITY,
            texture_lock: true,
        },
        policies.clone(),
        catalog,
    )?;
    let copy = result
        .copies
        .into_iter()
        .next()
        .ok_or_else(|| invalid("Linked graph copy is missing"))?;
    let mapping = copy.mapping.iter().copied().collect::<BTreeMap<_, _>>();
    let mapped = |id: u32| -> Result<u32, EditError> {
        match mapping.get(&ObjectId::Entity(id)) {
            Some(ObjectId::Entity(id)) => Ok(*id),
            _ => Err(invalid("Linked graph copy no longer matches its source")),
        }
    };
    let copied_graph = inspect(tx.document(), mapped(graph.nodes[0])?)?;
    Ok(Outcome {
        graph: copied_graph,
        objects: copy.objects,
        train: train.map(mapped).transpose()?,
        helpers: helpers.into_iter().map(mapped).collect::<Result<_, _>>()?,
        removed_helpers: Vec::new(),
        mapping: copy.mapping,
        names: copy.names,
        remapped_references: copy.remapped_references,
        unselected_incoming,
    })
}
