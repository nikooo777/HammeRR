use super::*;

fn neutral_number(entity: &Entity, key: &str, neutral: f64) -> Result<(), EditError> {
    if entity.properties.get_all(key).count() > 1 {
        return Err(invalid(format!(
            "Entity {} repeats {key}; resolve it before converting the path",
            entity.id
        )));
    }
    if let Some(value) = entity.properties.get(key) {
        let number = value
            .parse::<f64>()
            .map_err(|_| invalid(format!("Entity {} has invalid {key}", entity.id)))?;
        if !number.is_finite() || number != neutral {
            return Err(invalid(format!(
                "Entity {} has nonneutral {key}; path class conversion cannot preserve that behavior",
                entity.id
            )));
        }
    }
    Ok(())
}

fn check_conversion(
    document: &Document,
    graph: &Graph,
    classname: &str,
    catalog: &Catalog,
) -> Result<(), EditError> {
    if !associated_trains(document, graph)?.is_empty() {
        return Err(invalid(
            "This path has an attached train; detach it and explicitly convert its class before converting the path",
        ));
    }
    let ids = graph
        .nodes
        .iter()
        .copied()
        .map(ObjectId::Entity)
        .collect::<Vec<_>>();
    let names = repeat::source_names(document, &ids)?;
    let sources = graph.nodes.iter().copied().collect::<BTreeSet<_>>();
    repeat::validate_opaque_links(document, &sources, &names)?;
    let scan = references::scan(document, catalog);
    if scan.len().saturating_mul(graph.nodes.len()) > MAX_REFERENCE_WORK {
        return Err(invalid(
            "Path conversion reference inspection exceeds four million checks",
        ));
    }
    for &id in &graph.nodes {
        let entity = document
            .entity(id)
            .ok_or(EditError::NotFound(ObjectId::Entity(id)))?;
        for (key, neutral) in [
            ("spawnflags", 0.0),
            ("wait", 0.0),
            ("speed", 0.0),
            ("radius", 0.0),
            ("orientationtype", 1.0),
        ] {
            neutral_number(entity, key, neutral)?;
        }
        for (key, value) in &entity.properties.0 {
            if key.eq_ignore_ascii_case("classname") {
                continue;
            }
            validate_property(catalog, classname, key, Some(value))?;
            let source = catalog
                .class(&graph.classname)
                .and_then(|class| class.property(key));
            let target = catalog
                .class(classname)
                .and_then(|class| class.property(key));
            let neutral = [
                "targetname",
                "target",
                "origin",
                "spawnflags",
                "wait",
                "speed",
                "radius",
                "orientationtype",
                "altpath",
                "parentname",
            ]
            .iter()
            .any(|known| key.eq_ignore_ascii_case(known));
            if source.is_some() && target.is_none() && !neutral && !value.is_empty() {
                return Err(invalid(format!(
                    "Entity {id} has class-specific property {key}; convert that behavior explicitly first"
                )));
            }
        }
        if entity.connections.iter().flatten().any(|connection| {
            let common = ["OnPass", "OnUser1", "OnUser2", "OnUser3", "OnUser4"]
                .iter()
                .any(|output| connection.output.eq_ignore_ascii_case(output));
            let source = catalog.class(&graph.classname).and_then(|class| {
                class
                    .outputs
                    .iter()
                    .find(|output| output.name.eq_ignore_ascii_case(&connection.output))
            });
            let target = catalog.class(classname).and_then(|class| {
                class
                    .outputs
                    .iter()
                    .find(|output| output.name.eq_ignore_ascii_case(&connection.output))
            });
            !common
                && !source
                    .zip(target)
                    .is_some_and(|(source, target)| source.kind == target.kind)
        }) {
            return Err(invalid(format!(
                "Entity {id} owns an output unavailable across path classes; edit it explicitly first"
            )));
        }
        let name = entity
            .properties
            .get("targetname")
            .ok_or_else(|| invalid("Path node name changed"))?;
        for reference in &scan {
            let references::Location::Connection {
                index, field: 0, ..
            } = &reference.location
            else {
                continue;
            };
            if !connections::name_matches(name, &reference.target)
                && !(reference.target == "!self" && reference.source == id)
            {
                continue;
            }
            let source = reference.source;
            let connection = document
                .entity(source)
                .ok_or(EditError::NotFound(ObjectId::Entity(source)))?
                .connections
                .as_ref()
                .ok_or_else(|| invalid("Path input source changed"))?
                .get(*index)
                .ok_or_else(|| invalid("Path input source changed"))?;
            let spec = connections::parse(connection).map_err(invalid)?;
            let common = matches!(
                spec.input.to_ascii_lowercase().as_str(),
                "kill"
                    | "killhierarchy"
                    | "fireuser1"
                    | "fireuser2"
                    | "fireuser3"
                    | "fireuser4"
                    | "inpass"
            );
            let dynamic = ["AddOutput", "RunScriptCode", "RunScriptFile"]
                .iter()
                .any(|input| spec.input.eq_ignore_ascii_case(input));
            let old_input = catalog.class(&graph.classname).and_then(|class| {
                class
                    .inputs
                    .iter()
                    .find(|input| input.name.eq_ignore_ascii_case(&spec.input))
            });
            let new_input = catalog.class(classname).and_then(|class| {
                class
                    .inputs
                    .iter()
                    .find(|input| input.name.eq_ignore_ascii_case(&spec.input))
            });
            let shared = old_input
                .zip(new_input)
                .is_some_and(|(old, new)| old.kind == new.kind);
            if dynamic || !common && !shared {
                return Err(invalid(format!(
                    "Entity {source} sends {} to a path node; class conversion cannot preserve that input",
                    spec.input
                )));
            }
        }
    }
    if scan.iter().any(|reference| {
        connections::name_matches(classname, &reference.target)
            || connections::name_matches(&graph.classname, &reference.target)
    }) {
        return Err(invalid(
            "A reference addresses a path classname; convert that reference explicitly first",
        ));
    }
    Ok(())
}

pub(super) fn apply(
    tx: &mut Transaction,
    seed: u32,
    classname: &str,
    closed: bool,
    catalog: &Catalog,
) -> Result<Outcome, EditError> {
    let graph = inspect(tx.document(), seed)?;
    let classname = classname.to_ascii_lowercase();
    if !matches!(classname.as_str(), "path_corner" | "path_track") {
        return Err(invalid("Choose path_corner or path_track"));
    }
    validate_class(catalog, &classname, ClassKind::Point)?;
    if graph.classname != classname {
        check_conversion(tx.document(), &graph, &classname, catalog)?;
    }
    let names = graph
        .nodes
        .iter()
        .map(|&id| {
            tx.document()
                .entity(id)
                .and_then(|entity| entity.properties.get("targetname"))
                .map(str::to_owned)
                .ok_or_else(|| invalid("Path node name changed"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (index, &id) in graph.nodes.iter().enumerate() {
        let target = names
            .get(index + 1)
            .or_else(|| closed.then(|| &names[0]))
            .cloned();
        let previous = tx
            .document()
            .entity(id)
            .and_then(|entity| entity.properties.get("target"))
            .unwrap_or_default();
        let mut changes = Vec::new();
        if !previous.eq_ignore_ascii_case(target.as_deref().unwrap_or_default()) {
            validate_property(catalog, &classname, "target", target.as_deref())?;
            changes.push(("target".into(), target));
        }
        if graph.classname != classname {
            validate_property(catalog, &classname, "classname", Some(&classname))?;
            changes.push(("classname".into(), Some(classname.clone())));
        }
        if !changes.is_empty() {
            edit::set_properties(tx, id, &changes)?;
        }
    }
    let result = inspect(tx.document(), graph.nodes[0])?;
    let mut result = outcome(result);
    result.unselected_incoming = incoming_sources(tx.document(), &result.objects, catalog)?;
    Ok(result)
}
