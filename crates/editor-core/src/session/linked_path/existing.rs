use super::*;

use map_core::linked_path::Graph;

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
                core: None,
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
        map_core::linked_path::inspect(&self.document, seed).map_err(|error| error.to_string())
    }
}
