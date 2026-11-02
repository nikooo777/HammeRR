use super::*;

pub const MAX_TRAIN_SPEED: f64 = 4096.0;
pub const MAX_TRAINS: usize = 64;
const STARTUP_MARKER: &str = "hammerr_linked_path_startup";
const REMOVE_ON_FIRE: u64 = 1;

fn required_train_class(graph: &Graph) -> &str {
    if graph.classname == "path_corner" {
        "func_train"
    } else {
        "func_tracktrain"
    }
}

pub fn associated_trains(document: &Document, graph: &Graph) -> Result<Vec<u32>, EditError> {
    let names = graph
        .nodes
        .iter()
        .map(|&id| {
            document
                .entity(id)
                .and_then(|entity| entity.properties.get("targetname"))
                .ok_or_else(|| invalid("Path node name changed"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut trains = Vec::new();
    let mut work = 0usize;
    for entity in &document.vmf().entities {
        work = work.saturating_add(entity.properties.0.len() + names.len());
        if work > MAX_REFERENCE_WORK {
            return Err(invalid(
                "Train association inspection exceeds four million checks",
            ));
        }
        if !entity.classname().is_some_and(|classname| {
            classname.eq_ignore_ascii_case("func_train")
                || classname.eq_ignore_ascii_case("func_tracktrain")
        }) {
            continue;
        }
        if entity.properties.get_all("target").any(|target| {
            names
                .iter()
                .any(|name| connections::name_matches(name, target))
        }) {
            if entity.properties.get_all("target").count() != 1
                || !entity.properties.get("target").is_some_and(exact_name)
            {
                return Err(invalid(format!(
                    "Train {} has an ambiguous path target; edit it explicitly first",
                    entity.id
                )));
            }
            if trains.len() >= MAX_TRAINS {
                return Err(invalid(format!(
                    "Linked paths support inspection of at most {MAX_TRAINS} associated trains"
                )));
            }
            trains.push(entity.id);
        }
    }
    Ok(trains)
}

pub(super) fn validate(
    document: &Document,
    graph: &Graph,
    id: u32,
    catalog: &Catalog,
) -> Result<(), EditError> {
    document
        .vmf()
        .validate_ids()
        .map_err(|error| invalid(error.to_string()))?;
    let entity = document
        .entity(id)
        .ok_or(EditError::NotFound(ObjectId::Entity(id)))?;
    let classname = required_train_class(graph);
    if entity.hidden
        || entity.solids.is_empty()
        || !entity
            .classname()
            .is_some_and(|actual| actual.eq_ignore_ascii_case(classname))
    {
        return Err(invalid(format!(
            "{} setup requires a visible {classname} brush entity",
            graph.classname
        )));
    }
    validate_class(catalog, classname, ClassKind::Solid)?;
    for key in [
        "classname",
        "origin",
        "targetname",
        "target",
        "spawnflags",
        "angles",
        "angle",
        "parentname",
        "speed",
        "startspeed",
    ] {
        let count = entity.properties.get_all(key).count();
        if count > 1 || key == "classname" && count != 1 {
            return Err(invalid(format!(
                "Train {id} needs an unambiguous {key} property"
            )));
        }
    }
    if entity
        .properties
        .get("parentname")
        .is_some_and(|name| !name.is_empty())
    {
        return Err(invalid(
            "Train setup requires an unparented root brush entity",
        ));
    }
    edit::checked_entity_origin(entity)?;
    if entity.properties.get("angles").is_some() && entity.properties.get("angle").is_some() {
        return Err(invalid(
            "Train setup requires one unambiguous zero orientation",
        ));
    }
    if let Some(angles) = entity.properties.get("angles") {
        let angles = source_formats::vmf::parse_vec3(angles)
            .ok_or_else(|| invalid("Train angles must be a finite zero vector"))?;
        if !angles.is_finite() || angles != DVec3::ZERO {
            return Err(invalid(
                "Train setup currently supports zero-angle root brush entities",
            ));
        }
    }
    if let Some(angle) = entity.properties.get("angle") {
        let angle = angle
            .parse::<f64>()
            .map_err(|_| invalid("Train angle must be zero"))?;
        if angle != 0.0 {
            return Err(invalid(
                "Train setup currently supports zero-angle root brush entities",
            ));
        }
    }
    repeat::source_names(document, &[ObjectId::Entity(id)])?;
    for solid in &entity.solids {
        if solid.hidden || solid.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(invalid(
                "Train setup requires visible ordinary convex brushes without terrain",
            ));
        }
        crate::transform::validated_geometry(solid)?;
    }
    let (mins, maxs) = crate::query::bounds_of(document, &[ObjectId::Entity(id)])
        .ok_or_else(|| invalid("Train brush bounds are unavailable"))?;
    if !mins.is_finite()
        || !maxs.is_finite()
        || mins.abs().max(maxs.abs()).max_element() > crate::primitive::MAX_COORDINATE
    {
        return Err(invalid(
            "Train brushes must remain within ±65536 world units",
        ));
    }
    Ok(())
}

fn marked(entity: &Entity) -> bool {
    entity
        .editor
        .as_ref()
        .and_then(|editor| editor.get(STARTUP_MARKER))
        == Some("1")
}

pub fn startup_helpers(document: &Document, train: u32) -> Result<Vec<u32>, EditError> {
    let train = document
        .entity(train)
        .ok_or(EditError::NotFound(ObjectId::Entity(train)))?;
    let Some(name) = train
        .properties
        .get("targetname")
        .filter(|name| !name.is_empty())
    else {
        return Ok(Vec::new());
    };
    let mut helpers = Vec::new();
    let mut work = 0usize;
    for entity in &document.vmf().entities {
        work = work.saturating_add(
            entity.properties.0.len() + entity.connections.as_ref().map_or(0, Vec::len) + 1,
        );
        if work > MAX_REFERENCE_WORK {
            return Err(invalid(
                "Train startup helper inspection exceeds four million checks",
            ));
        }
        if !marked(entity) {
            continue;
        }
        let targets_train = entity.connections.iter().flatten().any(|connection| {
            connection
                .fields()
                .next()
                .is_some_and(|target| connections::name_matches(name, target))
        });
        if !targets_train {
            continue;
        }
        let connections = entity.connections.as_deref().unwrap_or_default();
        let spec = if connections.len() == 1 {
            connections::parse(&connections[0]).ok()
        } else {
            None
        };
        let valid_output = spec.is_some_and(|spec| {
            spec.output.eq_ignore_ascii_case("OnMapSpawn")
                && spec.target.eq_ignore_ascii_case(name)
                && spec.input.eq_ignore_ascii_case("Start")
                && spec.parameter.is_empty()
                && spec.delay == 0.0
                && spec.times == 1
        });
        let valid_properties = entity.properties.0.iter().all(|(key, value)| {
            if ["classname", "origin"]
                .iter()
                .any(|allowed| key.eq_ignore_ascii_case(allowed))
            {
                return true;
            }
            if key.eq_ignore_ascii_case("spawnflags") {
                return value == "1";
            }
            value.is_empty()
                || key.eq_ignore_ascii_case("angles")
                    && source_formats::vmf::parse_vec3(value) == Some(DVec3::ZERO)
        });
        if !entity
            .classname()
            .is_some_and(|classname| classname.eq_ignore_ascii_case("logic_auto"))
            || entity.hidden
            || !entity.solids.is_empty()
            || !entity.extra.is_empty()
            || !valid_output
            || !valid_properties
            || entity.properties.get_all("classname").count() != 1
            || entity.properties.get_all("spawnflags").count() != 1
            || entity.properties.get_all("origin").count() != 1
            || entity.origin().is_none_or(|origin| !origin.is_finite())
        {
            return Err(invalid(format!(
                "Managed train startup helper {} changed; edit or remove it explicitly first",
                entity.id
            )));
        }
        helpers.push(entity.id);
    }
    if helpers.len() > 1 {
        return Err(invalid(
            "This train has multiple managed startup helpers; resolve them explicitly first",
        ));
    }
    Ok(helpers)
}

fn create_startup(
    tx: &mut Transaction,
    train: u32,
    origin: DVec3,
    catalog: &Catalog,
) -> Result<u32, EditError> {
    validate_class(catalog, "logic_auto", ClassKind::Point)?;
    let name = tx
        .document()
        .entity(train)
        .and_then(|entity| entity.properties.get("targetname"))
        .filter(|name| exact_name(name))
        .ok_or_else(|| invalid("Automatic train startup needs a unique exact targetname"))?
        .to_owned();
    let mut properties = catalog
        .class("logic_auto")
        .map_or_else(BTreeMap::new, |class| class.defaults());
    for (key, value) in &properties {
        let required = key.eq_ignore_ascii_case("spawnflags") || key.eq_ignore_ascii_case("origin");
        let empty = value.is_empty()
            || key.eq_ignore_ascii_case("angles")
                && source_formats::vmf::parse_vec3(value) == Some(DVec3::ZERO);
        if !required && !empty {
            return Err(invalid(format!(
                "logic_auto default {key} is outside the train startup helper catalog; edit the definitions explicitly first"
            )));
        }
    }
    properties.insert("spawnflags".into(), REMOVE_ON_FIRE.to_string());
    for (key, value) in &properties {
        validate_property(catalog, "logic_auto", key, Some(value))?;
    }
    let id = edit::create_entity(
        tx,
        "logic_auto",
        Some(origin),
        &properties.into_iter().collect::<Vec<_>>(),
        Vec::new(),
    )?;
    let ObjectId::Entity(id) = id else {
        return Err(invalid("Train startup helper is not an entity"));
    };
    tx.entity_mut(id)?
        .editor
        .get_or_insert_with(Default::default)
        .set(STARTUP_MARKER, "1");
    connections::set(
        tx,
        id,
        None,
        &connections::Spec {
            output: "OnMapSpawn".into(),
            target: name,
            input: "Start".into(),
            parameter: String::new(),
            delay: 0.0,
            times: 1,
        },
    )?;
    Ok(id)
}

pub(super) fn apply(
    tx: &mut Transaction,
    seed: u32,
    train: u32,
    speed: f64,
    start_on: bool,
    catalog: &Catalog,
) -> Result<Outcome, EditError> {
    let graph = inspect(tx.document(), seed)?;
    validate_class(catalog, &graph.classname, ClassKind::Point)?;
    validate(tx.document(), &graph, train, catalog)?;
    if !speed.is_finite() || !(f64::from(f32::MIN_POSITIVE)..=MAX_TRAIN_SPEED).contains(&speed) {
        return Err(invalid(format!(
            "Train speed must be positive and at most {MAX_TRAIN_SPEED} units per second"
        )));
    }
    let entity = tx
        .document()
        .entity(train)
        .ok_or(EditError::NotFound(ObjectId::Entity(train)))?;
    let named = entity
        .properties
        .get("targetname")
        .is_some_and(|name| !name.is_empty());
    let corner = graph.classname == "path_corner";
    if corner && !named && !start_on {
        return Err(invalid(
            "A stopped func_train needs a unique targetname for its Start input; name it explicitly first",
        ));
    }
    let first = tx
        .document()
        .entity(graph.nodes[0])
        .ok_or_else(|| invalid("First path node changed"))?;
    let target = first
        .properties
        .get("targetname")
        .ok_or_else(|| invalid("First path node name changed"))?
        .to_owned();
    let origin = first
        .origin()
        .ok_or_else(|| invalid("First path node origin changed"))?;
    let classname = required_train_class(&graph);
    let mut changes: Vec<(String, Option<String>)> = vec![
        ("target".into(), Some(target)),
        (
            "speed".into(),
            Some(if corner || start_on {
                speed.to_string()
            } else {
                "0".into()
            }),
        ),
    ];
    if !corner {
        changes.push(("startspeed".into(), Some(speed.to_string())));
    }
    for (key, value) in &changes {
        validate_property(catalog, classname, key, value.as_deref())?;
    }
    let mut helpers = startup_helpers(tx.document(), train)?;
    let mut removed_helpers = Vec::new();
    edit::set_properties(tx, train, &changes)?;
    if corner && named && start_on {
        if helpers.is_empty() {
            helpers.push(create_startup(tx, train, origin, catalog)?);
        }
    } else if !helpers.is_empty() {
        edit::delete(
            tx,
            &helpers
                .iter()
                .copied()
                .map(ObjectId::Entity)
                .collect::<Vec<_>>(),
        )?;
        removed_helpers = std::mem::take(&mut helpers);
    }
    let mut result = outcome(graph);
    result.objects.push(ObjectId::Entity(train));
    result
        .objects
        .extend(helpers.iter().copied().map(ObjectId::Entity));
    result.train = Some(train);
    result.helpers = helpers;
    result.removed_helpers = removed_helpers;
    result.unselected_incoming = incoming_sources(tx.document(), &result.objects, catalog)?;
    Ok(result)
}
