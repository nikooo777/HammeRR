use std::sync::Arc;

use map_core::{EditError, Transaction, edit};
use source_formats::keyvalues;

use super::*;

struct Child {
    vmf: Vmf,
    identity: FileIdentity,
    bytes: usize,
}

pub(super) struct ExpandedSources {
    pub stats: InstanceStats,
    pub provenance: Provenance,
    pub files: Vec<(String, FileIdentity)>,
}

pub(super) struct Expansion<'a> {
    source: &'a SourceContext,
    control: &'a Control,
    report: &'a mut Report,
    files: BTreeMap<String, Arc<Child>>,
    bytes: usize,
    nodes: usize,
    work: usize,
    objects: usize,
    sides: usize,
    terrain: terrain::Budget,
    stats: InstanceStats,
    provenance: Provenance,
}

impl<'a> Expansion<'a> {
    pub fn new(
        vmf: &Vmf,
        source: &'a SourceContext,
        control: &'a Control,
        report: &'a mut Report,
    ) -> Self {
        Self {
            source,
            control,
            report,
            files: BTreeMap::new(),
            bytes: 0,
            nodes: 0,
            work: 0,
            objects: vmf.object_ids().count(),
            sides: vmf.side_ids().count(),
            terrain: terrain::Budget::default(),
            stats: InstanceStats::default(),
            provenance: BTreeMap::new(),
        }
    }

    fn child(
        &mut self,
        relative: &str,
        owner: &Entity,
        ancestors: &[FileIdentity],
    ) -> Result<Arc<Child>, String> {
        if let Some(child) = self.files.get(relative) {
            check_cycle(relative, &child.identity, ancestors)?;
            return Ok(Arc::clone(child));
        }
        if self.files.len() >= MAX_FILES {
            return Err(format!("Instances exceed {MAX_FILES} unique files"));
        }
        let path = resolve_path(self.source, relative)
            .map_err(|error| format!("Instance {relative}: {error}"))?;
        let limit = MAX_FILE_BYTES.min(MAX_TOTAL_BYTES.saturating_sub(self.bytes));
        let (identity, input) = FileIdentity::read_bounded(&path, limit as u64, self.control)
            .map_err(|error| format!("Instance {relative}: {error}"))?;
        check_cycle(relative, &identity, ancestors)?;
        self.bytes = self.bytes.saturating_add(input.len());
        if self.bytes > MAX_TOTAL_BYTES {
            return Err(format!("Instance sources exceed {MAX_TOTAL_BYTES} bytes"));
        }
        let text = std::str::from_utf8(&input).map_err(|error| error.to_string())?;
        let parsed = keyvalues::parse_bounded(
            text,
            MAX_DEPTH,
            MAX_FILE_NODES.min(MAX_TOTAL_NODES.saturating_sub(self.nodes)),
        )
        .map_err(|error| error.to_string())?;
        let mut stack = parsed.iter().map(|node| (node, false)).collect::<Vec<_>>();
        while let Some((node, hidden)) = stack.pop() {
            self.control.checkpoint()?;
            self.nodes += 1;
            if let Some(children) = node.children() {
                if node.key == "dispinfo" {
                    let result = if hidden {
                        terrain::hidden_source_grid(children, &mut self.terrain, self.control)
                    } else {
                        terrain::source_grid(children, &mut self.terrain, self.control)
                    };
                    result.map_err(|error| format!("Instance {relative}: {error}"))?;
                }
                let hidden = hidden || node.key == "hidden";
                stack.extend(children.iter().map(|child| (child, hidden)));
            }
        }
        if self.nodes > MAX_TOTAL_NODES {
            return Err(format!("Instance sources exceed {MAX_TOTAL_NODES} nodes"));
        }
        let child = Vmf::from_nodes(&parsed).map_err(|error| error.to_string())?;
        child_admission(&child).map_err(|error| format!("Instance {relative}: {error}"))?;
        let ignored = child
            .world
            .properties
            .iter()
            .filter(|(key, _)| {
                !key.eq_ignore_ascii_case("classname") && !key.eq_ignore_ascii_case("mapversion")
            })
            .map(|(key, _)| key)
            .collect::<Vec<_>>();
        if !ignored.is_empty() {
            let keys = ignored
                .iter()
                .take(16)
                .map(|key| map_core::entity_helpers::bounded_preview(key, 64))
                .collect::<Vec<_>>()
                .join(", ");
            self.report.diagnostics.push(Diagnostic::warning(
                format!(
                    "Instance {relative}: {} child world settings are ignored ({keys}; showing at most 16 keys); root world settings remain authoritative",
                    ignored.len(),
                ),
                Some(format!("entity:{}", owner.id)),
                owner.origin(),
            ));
        }
        let child = Arc::new(Child {
            vmf: child,
            identity,
            bytes: input.len(),
        });
        self.files.insert(relative.to_owned(), Arc::clone(&child));
        Ok(child)
    }

    pub fn helper(
        &mut self,
        tx: &mut Transaction<'_>,
        helper: &Entity,
        owner: &Entity,
        transforms: &mut Vec<edit::Transform>,
        ancestors: &mut Vec<FileIdentity>,
    ) -> Result<(), EditError> {
        self.control.checkpoint().map_err(EditError::Invalid)?;
        let depth = transforms.len() + 1;
        if depth > MAX_EXPANSION_DEPTH {
            return Err(EditError::Invalid(format!(
                "Instance expansion exceeds {MAX_EXPANSION_DEPTH} helper levels"
            )));
        }
        if self.stats.instances >= MAX_INSTANCES {
            return Err(EditError::Invalid(format!(
                "Instance expansion exceeds {MAX_INSTANCES} total helper occurrences"
            )));
        }
        helper_admission(helper).map_err(EditError::Invalid)?;
        let relative =
            filename(helper.properties.get("file").unwrap()).map_err(EditError::Invalid)?;
        let child = self
            .child(&relative, owner, ancestors)
            .map_err(EditError::Invalid)?;
        let angles = vector(helper, "angles").map_err(EditError::Invalid)?;
        transforms.push(edit::Transform {
            translation: vector(helper, "origin").map_err(EditError::Invalid)?,
            rotation: DVec3::new(angles.z, angles.x, angles.y),
            texture_lock: true,
            texture_scale_lock: Some(true),
            ..Default::default()
        });
        ancestors.push(child.identity.clone());
        self.stats.instances += 1;
        self.stats.max_expansion_depth = self.stats.max_expansion_depth.max(depth);
        let result: Result<(), EditError> = (|| {
            for solid in child.vmf.world.solids.iter().filter(|solid| !solid.hidden) {
                self.solid(tx, solid, false, owner.id, &relative, transforms)?;
            }
            for entity in child.vmf.entities.iter().filter(|entity| !entity.hidden) {
                self.control.checkpoint().map_err(EditError::Invalid)?;
                if entity
                    .classname()
                    .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
                {
                    self.helper(tx, entity, owner, transforms, ancestors)?;
                } else {
                    for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
                        self.solid(tx, solid, true, owner.id, &relative, transforms)?;
                    }
                }
            }
            Ok(())
        })();
        ancestors.pop();
        transforms.pop();
        result.map_err(|error| EditError::Invalid(format!("Instance {relative}: {error}")))
    }

    fn solid(
        &mut self,
        tx: &mut Transaction<'_>,
        original: &Solid,
        detail: bool,
        owner: u32,
        relative: &str,
        transforms: &[edit::Transform],
    ) -> Result<(), EditError> {
        self.control.checkpoint().map_err(EditError::Invalid)?;
        self.objects = self.objects.saturating_add(1 + usize::from(detail));
        self.sides = self.sides.saturating_add(original.sides.len());
        let work = original
            .sides
            .len()
            .checked_pow(3)
            .and_then(|work| work.checked_mul(transforms.len()))
            .and_then(|work| self.work.checked_add(work));
        let Some(work) = work else {
            return Err(EditError::Invalid(
                "Instance expansion geometry-work overflow".into(),
            ));
        };
        if self.objects > MAX_OBJECTS || self.sides > MAX_SIDES || work > MAX_GEOMETRY_WORK {
            return Err(EditError::Invalid(
                "Instance expansion exceeds combined objects, sides or geometry-work limits".into(),
            ));
        }
        self.work = work;
        self.terrain
            .expanded(original, transforms.len(), self.control)
            .map_err(EditError::Invalid)?;
        checked_solid_controlled(original, self.control).map_err(EditError::Invalid)?;
        let mut solid = original.clone();
        solid.id = tx.new_object_id()?;
        solid.editor = None;
        self.provenance.insert(
            format!("solid:{}", solid.id),
            (owner, relative.to_owned(), original.id, None),
        );
        for side in &mut solid.sides {
            let original_side = side.id;
            side.id = tx.new_side_id()?;
            self.provenance.insert(
                format!("side:{}", side.id),
                (owner, relative.to_owned(), original_side, Some(original.id)),
            );
        }
        let id = if detail {
            let id = tx.new_object_id()?;
            let mut entity = Entity::new(id, "func_detail");
            entity.solids.push(solid);
            self.provenance.insert(
                format!("entity:{id}"),
                (owner, relative.to_owned(), original.id, None),
            );
            self.stats.detail_brushes += 1;
            tx.add_entity(entity)?
        } else {
            self.stats.world_brushes += 1;
            tx.add_solid(solid)?
        };
        for transform in transforms.iter().rev() {
            self.control.checkpoint().map_err(EditError::Invalid)?;
            edit::transform(tx, &[id], *transform)?;
            match id {
                ObjectId::Solid(id) => checked_solid_controlled(tx.solid_mut(id)?, self.control),
                ObjectId::Entity(id) => {
                    checked_solid_controlled(&tx.entity_mut(id)?.solids[0], self.control)
                }
            }
            .map_err(EditError::Invalid)?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> ExpandedSources {
        self.stats.terrain_patches = self.terrain.patches;
        self.stats.terrain_vertices = self.terrain.vertices;
        self.stats.files = self.files.len();
        self.stats.dependencies = self
            .files
            .iter()
            .map(|(path, child)| DependencyInfo {
                path: path.clone(),
                bytes: child.bytes,
                sha256: child.identity.sha256().unwrap().to_owned(),
            })
            .collect();
        ExpandedSources {
            stats: self.stats,
            provenance: self.provenance,
            files: self
                .files
                .into_iter()
                .map(|(path, child)| (path, child.identity.clone()))
                .collect(),
        }
    }
}

fn check_cycle(
    relative: &str,
    identity: &FileIdentity,
    ancestors: &[FileIdentity],
) -> Result<(), String> {
    if ancestors
        .iter()
        .any(|ancestor| identity.same_file(ancestor))
    {
        return Err(format!(
            "Instance {relative} forms an ancestor reference cycle"
        ));
    }
    Ok(())
}
