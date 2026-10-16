use super::*;

pub(super) struct Graph {
    pub classname: String,
    pub nodes: Vec<u32>,
    pub closed: bool,
}

struct Link {
    classname: String,
    name: String,
    target: String,
    alternates: Vec<String>,
    targets: Vec<String>,
    valid: bool,
}

fn consume(work: &mut usize, amount: usize) -> Result<(), String> {
    *work = work.saturating_add(amount);
    if *work > MAX_REFERENCE_WORK {
        return Err("Existing path inspection exceeds four million checks".into());
    }
    Ok(())
}

fn exact_name(name: &str) -> bool {
    !name.is_empty()
        && name.trim() == name
        && !name.starts_with('!')
        && !name.contains(['*', '?', ',', '"'])
        && !name.chars().any(char::is_control)
}

impl Session {
    pub(super) fn linked_path_change_plan(
        &self,
        request: &ChangeRequest,
    ) -> Result<(Request, Plan), String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        self.ensure_partitions_resolved()?;
        let points = request
            .nodes
            .iter()
            .map(|node| node.origin)
            .collect::<Vec<_>>();
        validate_points(&points)?;
        let graph = self.existing_linked_path(request.seed)?;
        let members = graph.nodes.iter().copied().collect::<BTreeSet<_>>();
        let retained = request
            .nodes
            .iter()
            .filter_map(|node| node.id)
            .collect::<Vec<_>>();
        let unique = retained.iter().copied().collect::<BTreeSet<_>>();
        if retained.is_empty() || unique.len() != retained.len() || !unique.is_subset(&members) {
            return Err("Keep at least one original node, use each retained ID once, and insert new nodes without an ID".into());
        }
        let removed = graph
            .nodes
            .iter()
            .copied()
            .filter(|id| !unique.contains(id))
            .collect::<Vec<_>>();
        self.check_linked_path_removals(&members, &removed)?;
        let added = request.nodes.len() - retained.len();
        let mut names = if added > 0 {
            self.linked_path_names(&request.prefix, added)?.into_iter()
        } else {
            Vec::new().into_iter()
        };
        let names = request
            .nodes
            .iter()
            .map(|node| {
                node.id.map_or_else(
                    || names.next().unwrap(),
                    |id| {
                        self.document
                            .entity(id)
                            .unwrap()
                            .properties
                            .get("targetname")
                            .unwrap()
                            .to_owned()
                    },
                )
            })
            .collect::<Vec<_>>();
        let mut properties = Vec::with_capacity(request.nodes.len());
        for (index, node) in request.nodes.iter().enumerate() {
            let target = names
                .get(index + 1)
                .or_else(|| graph.closed.then(|| &names[0]));
            let Some(id) = node.id else {
                let (_, values) =
                    self.linked_path_new_properties(&graph.classname, &names[index], target)?;
                if values.iter().any(|(key, value)| {
                    !value.is_empty()
                        && (key.eq_ignore_ascii_case("altpath")
                            || key.eq_ignore_ascii_case("parentname"))
                }) {
                    return Err("New path node defaults must not add an alternate path or parent; edit the entity definitions explicitly first".into());
                }
                properties.push(values);
                continue;
            };
            let entity = self.document.entity(id).unwrap();
            let class = entity.classname().unwrap();
            if self
                .definitions
                .class(class)
                .is_some_and(|class| class.kind != ClassKind::Point)
            {
                return Err("Existing paths require FGD point classes".into());
            }
            if entity.origin() != Some(node.origin) {
                self.validate_property(
                    class,
                    "origin",
                    Some(&source_formats::vmf::format_vec3(node.origin)),
                )?;
            }
            if !entity
                .properties
                .get("target")
                .unwrap_or_default()
                .eq_ignore_ascii_case(target.map_or("", String::as_str))
            {
                self.validate_property(class, "target", target.map(String::as_str))?;
            }
            properties.push(
                target
                    .into_iter()
                    .map(|target| ("target".into(), target.clone()))
                    .collect(),
            );
        }
        let description = Request {
            classname: graph.classname.clone(),
            prefix: request.prefix.clone(),
            points: points.clone(),
            closed: graph.closed,
        };
        Ok((
            description,
            Plan {
                classname: graph.classname,
                points,
                properties,
                existing: Some(request.clone()),
                removed,
            },
        ))
    }

    fn check_linked_path_removals(
        &self,
        members: &BTreeSet<u32>,
        removed: &[u32],
    ) -> Result<(), String> {
        if removed.is_empty() {
            return Ok(());
        }
        let scan = references::scan(&self.document, &self.definitions);
        if scan.len().saturating_mul(removed.len()) > MAX_REFERENCE_WORK {
            return Err(
                "Linked path deletion reference inspection exceeds four million checks".into(),
            );
        }
        for &id in removed {
            let entity = self.document.entity(id).unwrap();
            let name = entity.properties.get("targetname").unwrap();
            if entity
                .connections
                .as_ref()
                .is_some_and(|connections| !connections.is_empty())
            {
                return Err(format!(
                    "Cannot remove {name} (entity:{id}): it owns connections; edit those explicitly first"
                ));
            }
            for reference in &scan {
                let internal_target = members.contains(&reference.source)
                    && matches!(&reference.location, references::Location::Property { key, .. } if key.eq_ignore_ascii_case("target"));
                if !internal_target && connections::name_matches(name, &reference.target) {
                    return Err(format!(
                        "Cannot remove {name} (entity:{id}): entity:{} references it; edit that reference explicitly first",
                        reference.source
                    ));
                }
            }
        }
        Ok(())
    }

    pub(super) fn existing_linked_path(&self, seed: u32) -> Result<Graph, String> {
        let mut work = 0;
        let mut names = BTreeMap::<String, Vec<u32>>::new();
        let mut identities = BTreeMap::<u32, usize>::new();
        let mut links = BTreeMap::new();
        for entity in &self.document.vmf().entities {
            consume(&mut work, entity.properties.0.len() + 1)?;
            *identities.entry(entity.id).or_default() += 1;
            for name in entity.properties.get_all("targetname") {
                if !name.is_empty() {
                    names
                        .entry(name.to_ascii_lowercase())
                        .or_default()
                        .push(entity.id);
                }
            }
            let classname = entity.classname().unwrap_or_default().to_ascii_lowercase();
            if !matches!(classname.as_str(), "path_corner" | "path_track") {
                continue;
            }
            links.insert(
                entity.id,
                Link {
                    classname,
                    name: entity
                        .properties
                        .get("targetname")
                        .unwrap_or_default()
                        .into(),
                    target: entity.properties.get("target").unwrap_or_default().into(),
                    alternates: entity
                        .properties
                        .get_all("altpath")
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    targets: entity
                        .properties
                        .get_all("target")
                        .map(str::to_owned)
                        .collect(),
                    valid: !entity.hidden
                        && entity.solids.is_empty()
                        && entity.properties.get_all("targetname").count() == 1
                        && entity.properties.get_all("target").count() <= 1
                        && entity.properties.get_all("origin").count() == 1
                        && entity.properties.get_all("classname").count() == 1
                        && !entity
                            .properties
                            .get_all("parentname")
                            .any(|name| !name.is_empty()),
                },
            );
        }
        let classname = links
            .get(&seed)
            .ok_or("Select one existing path_corner or path_track node")?
            .classname
            .clone();
        let mut pending = vec![seed];
        let mut next = BTreeMap::new();
        while let Some(id) = pending.pop() {
            if next.contains_key(&id) {
                continue;
            }
            if next.len() >= MAX_POINTS {
                return Err("Existing paths support at most 64 nodes".into());
            }
            let link = links
                .get(&id)
                .ok_or("Path target leaves this simple path class")?;
            if !link.valid || link.classname != classname || identities[&id] != 1 {
                return Err("Path nodes must be visible, unparented point entities of one class with unique IDs and unambiguous name, origin and target keys".into());
            }
            if !exact_name(&link.name)
                || names
                    .get(&link.name.to_ascii_lowercase())
                    .is_none_or(|ids| ids.len() != 1)
            {
                return Err("Every path node needs a unique exact targetname; duplicate names, wildcards and special names are unsupported".into());
            }
            if !link.alternates.is_empty() {
                return Err(
                    "Alternate path branches are unsupported; edit this graph manually".into(),
                );
            }
            let target = if link.target.is_empty() {
                None
            } else {
                if !exact_name(&link.target) {
                    return Err("Path targets must use exact names; wildcards and special targets are unsupported".into());
                }
                let targets = names
                    .get(&link.target.to_ascii_lowercase())
                    .filter(|ids| ids.len() == 1)
                    .ok_or("Path target is missing or ambiguous")?;
                let target = targets[0];
                if !links
                    .get(&target)
                    .is_some_and(|target| target.classname == classname)
                {
                    return Err("Path target leaves this simple path class".into());
                }
                pending.push(target);
                Some(target)
            };
            let mut incoming = 0;
            for (&other, source) in &links {
                consume(&mut work, 1)?;
                if source
                    .alternates
                    .iter()
                    .any(|alternate| connections::name_matches(&link.name, alternate))
                {
                    return Err(
                        "An alternate path branch targets this chain; edit this graph manually"
                            .into(),
                    );
                }
                if source
                    .targets
                    .iter()
                    .any(|target| connections::name_matches(&link.name, target))
                {
                    if !exact_name(&source.target) {
                        return Err(
                            "A wildcard path target addresses this chain; edit this graph manually"
                                .into(),
                        );
                    }
                    incoming += 1;
                    if incoming > 1 {
                        return Err("Multiple path nodes target the same node; branched paths are unsupported".into());
                    }
                    pending.push(other);
                }
            }
            next.insert(id, target);
        }
        let targeted = next.values().flatten().copied().collect::<BTreeSet<_>>();
        let heads = next
            .keys()
            .filter(|&&id| !targeted.contains(&id))
            .copied()
            .collect::<Vec<_>>();
        let closed = heads.is_empty();
        if heads.len() > 1 {
            return Err("Path is not a single open chain or closed loop".into());
        }
        let first = heads.first().copied().unwrap_or(seed);
        let mut nodes = Vec::new();
        let mut current = Some(first);
        while let Some(id) = current {
            if nodes.contains(&id) {
                if closed && id == first {
                    break;
                }
                return Err("Path contains an unsupported cycle".into());
            }
            nodes.push(id);
            current = next[&id];
        }
        if nodes.len() != next.len() || nodes.len() < MIN_POINTS {
            return Err("Use one complete simple path with 2–64 nodes".into());
        }
        let points = nodes
            .iter()
            .map(|&id| {
                self.document
                    .entity(id)
                    .unwrap()
                    .origin()
                    .ok_or("Path node has an invalid origin")
            })
            .collect::<Result<Vec<_>, _>>()?;
        validate_points(&points)?;
        Ok(Graph {
            classname,
            nodes,
            closed,
        })
    }
}
