use std::sync::Arc;

use map_core::{EditError, Transaction, edit};
use source_formats::keyvalues;

use super::*;

struct Child {
    vmf: Vmf,
    identity: FileIdentity,
    bytes: usize,
    names: names::LocalIndex,
    binding_parameters: bool,
    targets: targets::LocalIndex,
}

struct EntitySpec {
    kind: entities::Kind,
    name: Option<(&'static str, String)>,
    binding: Option<crate::lighting_origins::Binding>,
}

pub(super) struct ExpandedSources {
    pub stats: InstanceStats,
    pub provenance: Provenance,
    pub occurrence_budget: occurrences::OccurrenceBudget,
    pub static_prop_poses: StaticPropPoses,
    pub static_prop_bindings: StaticPropBindings,
    pub files: Vec<(String, FileIdentity)>,
    pub material_sources: Option<crate::asset_sources::BuildMaterialSources>,
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
    occurrence_budget: occurrences::OccurrenceBudget,
    current_occurrence: Option<Arc<OccurrencePath>>,
    static_prop_poses: StaticPropPoses,
    static_prop_bindings: StaticPropBindings,
    name_scopes: Vec<names::Scope>,
    name_registry: Option<names::Registry>,
    purpose: PreparationPurpose<'a>,
    materials: Option<crate::materials::Materials<'a>>,
    material_sources: Option<crate::asset_sources::BuildMaterialSources>,
    checked_cutouts: std::collections::BTreeSet<usize>,
}

impl<'a> Expansion<'a> {
    pub fn new(
        vmf: &Vmf,
        source: &'a SourceContext,
        control: &'a Control,
        report: &'a mut Report,
        purpose: PreparationPurpose<'a>,
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
            occurrence_budget: occurrences::OccurrenceBudget::default(),
            current_occurrence: None,
            static_prop_poses: BTreeMap::new(),
            static_prop_bindings: BTreeMap::new(),
            name_scopes: Vec::new(),
            name_registry: None,
            purpose,
            materials: None,
            material_sources: None,
            checked_cutouts: std::collections::BTreeSet::new(),
        }
    }

    fn record_origin(&mut self, key: String, legacy: LegacyOrigin) -> Result<(), EditError> {
        let occurrence = self.current_occurrence.as_ref().ok_or_else(|| {
            EditError::Invalid("Expanded source has no captured helper occurrence".into())
        })?;
        self.occurrence_budget
            .slot(self.control)
            .map_err(EditError::Invalid)?;
        let source = SourceOrigin {
            legacy,
            occurrence: Arc::clone(occurrence),
        };
        if self.provenance.insert(key, source).is_some() {
            return Err(EditError::Invalid(
                "Expanded source provenance is duplicated".into(),
            ));
        }
        Ok(())
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
        let mut stack = parsed
            .iter()
            .map(|node| {
                (
                    node,
                    false,
                    node.key == map_core::texture_atlas::regions::KEY,
                )
            })
            .collect::<Vec<_>>();
        while let Some((node, hidden, editor_only)) = stack.pop() {
            self.control.checkpoint()?;
            self.nodes += 1;
            if let Some(children) = node.children() {
                if !editor_only && node.key == "dispinfo" {
                    let result = if hidden {
                        terrain::hidden_source_grid(children, &mut self.terrain, self.control)
                    } else {
                        terrain::source_grid(children, &mut self.terrain, self.control)
                    };
                    result.map_err(|error| format!("Instance {relative}: {error}"))?;
                }
                let hidden = hidden || node.key == "hidden";
                stack.extend(children.iter().map(|child| (child, hidden, editor_only)));
            }
        }
        if self.nodes > MAX_TOTAL_NODES {
            return Err(format!("Instance sources exceed {MAX_TOTAL_NODES} nodes"));
        }
        let child = Vmf::from_nodes(&parsed).map_err(|error| error.to_string())?;
        if crate::lighting::emission::file(&child.world.properties)?.is_some() {
            return Err(format!(
                "Instance {relative}: child Emission file is unsupported; select the assembled material declaration file in the root World settings"
            ));
        }
        let targets = targets::LocalIndex::new(&child, &mut self.work, self.control)
            .map_err(|error| format!("Instance {relative}: {error}"))?;
        let positions = targets.positions(&mut self.work, self.control)?;
        child_admission(&child, self.control, &positions)
            .map_err(|error| format!("Instance {relative}: {error}"))?;
        if matches!(self.purpose, PreparationPurpose::Collapse) {
            collapse::child(&child).map_err(|error| format!("Instance {relative}: {error}"))?;
        }
        drop(positions);
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
        let names = names::LocalIndex::new(&child, &mut self.work, self.control)
            .map_err(|error| format!("Instance {relative}: {error}"))?;
        let binding_parameters = child.entities.iter().any(|entity| {
            !entity.hidden && (bindings::has_parameters(entity) || bindings::has_forwarding(entity))
        });
        let child = Arc::new(Child {
            binding_parameters,
            names,
            targets,
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
        let bindings = bindings::Bindings::new(helper).map_err(EditError::Invalid)?;
        bindings
            .check_child(
                &child.vmf,
                child.bytes,
                child.binding_parameters,
                &mut self.work,
                self.control,
            )
            .map_err(|error| EditError::Invalid(format!("Instance {relative}: {error}")))?;
        let angles = vector(helper, "angles").map_err(EditError::Invalid)?;
        let scope = names::Scope::capture(helper, &mut self.work, self.control)
            .map_err(EditError::Invalid)?;
        self.name_scopes.push(scope);
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
        let previous_occurrence = self.current_occurrence.clone();
        self.current_occurrence = Some(
            self.occurrence_budget
                .enter(
                    previous_occurrence.clone(),
                    helper.id,
                    &relative,
                    self.control,
                )
                .map_err(EditError::Invalid)?,
        );
        let result: Result<(), EditError> = (|| {
            let target_positions = child
                .targets
                .positions(&mut self.work, self.control)
                .map_err(EditError::Invalid)?;
            let mut local_helpers = BTreeMap::new();
            let mut local_sides = child
                .vmf
                .entities
                .iter()
                .any(|entity| {
                    !entity.hidden
                        && (cubemaps::has_manual_bindings(entity) || overlays::is_helper(entity))
                })
                .then(BTreeMap::new);
            let mut displacement_sides = child
                .vmf
                .entities
                .iter()
                .any(|entity| !entity.hidden && overlays::is_helper(entity))
                .then(BTreeMap::new);
            let mut manual_cubemaps = Vec::new();
            let mut overlay_bindings = Vec::new();
            for entity in child.vmf.entities.iter().filter(|entity| {
                !entity.hidden
                    && matches!(entity.classname(), Some("info_lighting" | "info_target"))
            }) {
                let kind = entities::kind(entity).unwrap();
                let result: Result<u32, EditError> = (|| {
                    let name = names::compose(
                        &self.name_scopes,
                        entity.properties.get("targetname").unwrap(),
                        &mut self.work,
                        self.control,
                    )
                    .map_err(EditError::Invalid)?;
                    if self.name_registry.is_none() {
                        self.name_registry = Some(
                            names::Registry::new(tx.vmf(), &mut self.work, self.control)
                                .map_err(EditError::Invalid)?,
                        );
                    }
                    let registry = self.name_registry.as_mut().unwrap();
                    if matches!(kind, entities::Kind::SpotlightTarget) {
                        registry.insert_target(&name, tx.vmf(), &mut self.work, self.control)
                    } else {
                        registry.insert(&name, &mut self.work, self.control)
                    }
                    .map_err(EditError::Invalid)?;
                    self.entity(
                        tx,
                        entity,
                        EntitySpec {
                            kind,
                            name: Some(("targetname", name)),
                            binding: None,
                        },
                        owner.id,
                        &relative,
                        transforms,
                    )
                })();
                let generated = result.map_err(|error| {
                    EditError::Invalid(format!("Child {} {}: {error}", kind.label(), entity.id))
                })?;
                if matches!(self.purpose, PreparationPurpose::Collapse) {
                    tx.entity_mut(generated)?.editor = entity.editor.clone();
                }
                names::charge(&mut self.work, 1, self.control).map_err(EditError::Invalid)?;
                local_helpers.insert(entity.id, generated);
            }
            for solid in child.vmf.world.solids.iter().filter(|solid| !solid.hidden) {
                self.solid(
                    tx,
                    solid,
                    false,
                    owner.id,
                    &relative,
                    transforms,
                    local_sides.as_mut(),
                    displacement_sides.as_mut(),
                )?;
            }
            for entity in child.vmf.entities.iter().filter(|entity| !entity.hidden) {
                self.control.checkpoint().map_err(EditError::Invalid)?;
                if entity
                    .classname()
                    .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
                {
                    let effective = bindings
                        .resolve_helper(entity, &mut self.work, self.control)
                        .map_err(|error| {
                            EditError::Invalid(format!("Child instance {}: {error}", entity.id))
                        })?;
                    self.helper(tx, &effective, owner, transforms, ancestors)?;
                } else if brush_models::is_model(entity) {
                    self.brush_model(tx, entity, owner.id, &relative, transforms)
                        .map_err(|error| {
                            EditError::Invalid(format!("Child brush model {}: {error}", entity.id))
                        })?;
                } else if visclusters::is_helper(entity) {
                    self.viscluster(tx, entity, owner.id, &relative, transforms)
                        .map_err(|error| {
                            EditError::Invalid(format!("Child viscluster {}: {error}", entity.id))
                        })?;
                } else if occluders::is_helper(entity) {
                    self.occluder(tx, entity, owner.id, &relative, transforms)
                        .map_err(|error| {
                            EditError::Invalid(format!("Child occluder {}: {error}", entity.id))
                        })?;
                } else if overlays::is_helper(entity) {
                    let generated = self
                        .overlay(tx, entity, owner.id, &relative, transforms)
                        .map_err(|error| {
                            EditError::Invalid(format!("Child overlay {}: {error}", entity.id))
                        })?;
                    overlay_bindings.push((entity, generated));
                } else if cubemaps::is_helper(entity) {
                    let generated = self
                        .cubemap(tx, entity, owner.id, &relative, transforms)
                        .map_err(|error| {
                            EditError::Invalid(format!("Child cubemap {}: {error}", entity.id))
                        })?;
                    if matches!(self.purpose, PreparationPurpose::Collapse) {
                        tx.entity_mut(generated)?.editor = entity.editor.clone();
                    }
                    if cubemaps::has_manual_bindings(entity) {
                        manual_cubemaps.push((entity, generated));
                    }
                } else if let Some(kind) = entities::kind(entity) {
                    if matches!(
                        kind,
                        entities::Kind::LightingOrigin | entities::Kind::SpotlightTarget
                    ) {
                        continue;
                    }
                    let result: Result<(), EditError> = (|| {
                        let mut spec = EntitySpec {
                            kind,
                            name: None,
                            binding: None,
                        };
                        if matches!(kind, entities::Kind::Spot)
                            && let Some(reference) = entity
                                .properties
                                .get("target")
                                .filter(|value| !value.is_empty())
                        {
                            let name = names::compose(
                                &self.name_scopes,
                                reference,
                                &mut self.work,
                                self.control,
                            )
                            .map_err(EditError::Invalid)?;
                            spec.name = Some(("target", name));
                        }
                        if matches!(kind, entities::Kind::StaticProp)
                            && let Some(reference) = entity
                                .properties
                                .get("lightingorigin")
                                .filter(|value| !value.is_empty())
                        {
                            let name = names::compose(
                                &self.name_scopes,
                                reference,
                                &mut self.work,
                                self.control,
                            )
                            .map_err(EditError::Invalid)?;
                            spec.name = Some(("lightingorigin", name));
                            spec.binding = Some(
                                child
                                    .names
                                    .helpers
                                    .get(reference)
                                    .and_then(|id| local_helpers.get(id))
                                    .copied()
                                    .map_or(
                                        crate::lighting_origins::Binding::Missing,
                                        crate::lighting_origins::Binding::Helper,
                                    ),
                            );
                        }
                        let effective = bindings
                            .resolve(
                                entity,
                                kind,
                                &target_positions,
                                &mut self.work,
                                self.control,
                            )
                            .map_err(EditError::Invalid)?;
                        if matches!(kind, entities::Kind::Spot) {
                            targets::check_poses(
                                &effective,
                                &target_positions,
                                transforms,
                                &mut self.work,
                                self.control,
                            )
                            .map_err(EditError::Invalid)?;
                        }
                        let generated =
                            self.entity(tx, &effective, spec, owner.id, &relative, transforms)?;
                        if matches!(self.purpose, PreparationPurpose::Collapse) {
                            tx.entity_mut(generated)?.editor = entity.editor.clone();
                        }
                        Ok(())
                    })();
                    result.map_err(|error| {
                        EditError::Invalid(format!("Child {} {}: {error}", kind.label(), entity.id))
                    })?;
                } else {
                    for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
                        let generated = self.solid(
                            tx,
                            solid,
                            true,
                            owner.id,
                            &relative,
                            transforms,
                            local_sides.as_mut(),
                            displacement_sides.as_mut(),
                        )?;
                        if matches!(self.purpose, PreparationPurpose::Collapse) {
                            let ObjectId::Entity(id) = generated else {
                                unreachable!()
                            };
                            tx.entity_mut(id)?.editor = entity.editor.clone();
                        }
                    }
                }
            }
            for (original, generated) in manual_cubemaps {
                let rewritten = cubemaps::remap_sides(
                    original,
                    local_sides.as_ref().unwrap(),
                    &mut self.work,
                    self.control,
                )
                .map_err(|error| {
                    EditError::Invalid(format!("Child cubemap {}: {error}", original.id))
                })?;
                tx.entity_mut(generated)?.properties.set("sides", rewritten);
            }
            for (original, generated) in overlay_bindings {
                let rewritten = overlays::remap_sides(
                    original,
                    local_sides.as_ref().unwrap(),
                    displacement_sides.as_ref().unwrap(),
                    &mut self.work,
                    self.control,
                )
                .map_err(|error| {
                    EditError::Invalid(format!("Child overlay {}: {error}", original.id))
                })?;
                tx.entity_mut(generated)?.properties.set("sides", rewritten);
            }
            Ok(())
        })();
        self.current_occurrence = previous_occurrence;
        ancestors.pop();
        transforms.pop();
        self.name_scopes.pop();
        result.map_err(|error| EditError::Invalid(format!("Instance {relative}: {error}")))
    }

    fn overlay(
        &mut self,
        tx: &mut Transaction<'_>,
        original: &Entity,
        owner: u32,
        relative: &str,
        transforms: &[edit::Transform],
    ) -> Result<u32, EditError> {
        self.control.checkpoint().map_err(EditError::Invalid)?;
        self.objects = self.objects.saturating_add(1);
        let property_bytes = original
            .properties
            .iter()
            .try_fold(0_usize, |bytes, (key, value)| {
                bytes.checked_add(key.len())?.checked_add(value.len())
            });
        let work = property_bytes
            .and_then(|bytes| bytes.checked_add(overlays::POSE_WORK))
            .and_then(|work| work.checked_mul(transforms.len() + 2))
            .and_then(|work| self.work.checked_add(work));
        let Some(work) = work else {
            return Err(EditError::Invalid("Instance overlay-work overflow".into()));
        };
        if self.objects > MAX_OBJECTS || work > MAX_GEOMETRY_WORK {
            return Err(EditError::Invalid(
                "Instance expansion exceeds combined objects or geometry-work limits".into(),
            ));
        }
        self.work = work;
        overlays::check_pose(original).map_err(EditError::Invalid)?;
        let mut entity = original.clone();
        entity.id = tx.new_object_id()?;
        entity.editor = None;
        for transform in transforms.iter().rev() {
            self.control.checkpoint().map_err(EditError::Invalid)?;
            overlays::transform(&mut entity, *transform)?;
        }
        self.record_origin(
            format!("entity:{}", entity.id),
            (owner, relative.to_owned(), original.id, None),
        )?;
        let generated = entity.id;
        tx.add_entity(entity)?;
        self.stats.overlays += 1;
        Ok(generated)
    }

    fn cubemap(
        &mut self,
        tx: &mut Transaction<'_>,
        original: &Entity,
        owner: u32,
        relative: &str,
        transforms: &[edit::Transform],
    ) -> Result<u32, EditError> {
        self.control.checkpoint().map_err(EditError::Invalid)?;
        self.objects = self.objects.saturating_add(1);
        let bytes = original
            .properties
            .iter()
            .try_fold(0_usize, |bytes, (key, value)| {
                bytes.checked_add(key.len())?.checked_add(value.len())
            });
        let work = bytes
            .and_then(|bytes| bytes.checked_mul(transforms.len() + 1))
            .and_then(|work| self.work.checked_add(work));
        let Some(work) = work else {
            return Err(EditError::Invalid("Instance entity-work overflow".into()));
        };
        if self.objects > MAX_OBJECTS || work > MAX_GEOMETRY_WORK {
            return Err(EditError::Invalid(
                "Instance expansion exceeds combined objects or geometry-work limits".into(),
            ));
        }
        self.work = work;
        cubemaps::point(original).map_err(EditError::Invalid)?;
        let mut entity = original.clone();
        entity.id = tx.new_object_id()?;
        entity.editor = None;
        for transform in transforms.iter().rev() {
            self.control.checkpoint().map_err(EditError::Invalid)?;
            let point = cubemaps::point(&entity).map_err(EditError::Invalid)?;
            let point = map_core::transform::transform_point(point, *transform)?;
            entity
                .properties
                .set("origin", source_formats::vmf::format_vec3(point));
            cubemaps::point(&entity).map_err(EditError::Invalid)?;
        }
        self.record_origin(
            format!("entity:{}", entity.id),
            (owner, relative.to_owned(), original.id, None),
        )?;
        let generated = entity.id;
        tx.add_entity(entity)?;
        self.stats.cubemaps += 1;
        Ok(generated)
    }

    fn occluder(
        &mut self,
        tx: &mut Transaction<'_>,
        original: &Entity,
        owner: u32,
        relative: &str,
        transforms: &[edit::Transform],
    ) -> Result<(), EditError> {
        self.control.checkpoint().map_err(EditError::Invalid)?;
        let Some(shape) = occluders::shape(original).map_err(EditError::Invalid)? else {
            return Ok(());
        };
        self.objects = self.objects.saturating_add(1 + shape.brushes);
        self.sides = self.sides.saturating_add(shape.sides);
        let property_bytes = original
            .properties
            .iter()
            .try_fold(0_usize, |bytes, (key, value)| {
                bytes.checked_add(key.len())?.checked_add(value.len())
            });
        let work = original
            .solids
            .iter()
            .filter(|solid| !solid.hidden)
            .try_fold(0_usize, |work, solid| {
                let geometry_work = solid
                    .sides
                    .len()
                    .checked_pow(3)?
                    .checked_mul(transforms.len().checked_mul(2)?.checked_add(1)?)?;
                work.checked_add(geometry_work)
            })
            .and_then(|work| {
                let field_work = property_bytes?.checked_mul(transforms.len() + 1)?;
                work.checked_add(field_work)
            })
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
        occluders::check_pose(original, self.control).map_err(EditError::Invalid)?;
        let mut entity = original.clone();
        entity.id = tx.new_object_id()?;
        entity.editor = None;
        entity.solids.clear();
        self.record_origin(
            format!("entity:{}", entity.id),
            (owner, relative.to_owned(), original.id, None),
        )?;
        for original_solid in original.solids.iter().filter(|solid| !solid.hidden) {
            self.control.checkpoint().map_err(EditError::Invalid)?;
            let mut solid = original_solid.clone();
            solid.id = tx.new_object_id()?;
            solid.editor = None;
            self.record_origin(
                format!("solid:{}", solid.id),
                (owner, relative.to_owned(), original_solid.id, None),
            )?;
            for side in &mut solid.sides {
                self.control.checkpoint().map_err(EditError::Invalid)?;
                let original_side = side.id;
                side.id = tx.new_side_id()?;
                self.record_origin(
                    format!("side:{}", side.id),
                    (
                        owner,
                        relative.to_owned(),
                        original_side,
                        Some(original_solid.id),
                    ),
                )?;
            }
            entity.solids.push(solid);
        }
        let generated = entity.id;
        let id = tx.add_entity(entity)?;
        for transform in transforms.iter().rev() {
            self.control.checkpoint().map_err(EditError::Invalid)?;
            edit::transform(tx, &[id], *transform)?;
            occluders::check_pose(tx.entity_mut(generated)?, self.control)
                .map_err(EditError::Invalid)?;
        }
        self.stats.occluders += 1;
        Ok(())
    }

    fn brush_model(
        &mut self,
        tx: &mut Transaction<'_>,
        original: &Entity,
        owner: u32,
        relative: &str,
        transforms: &[edit::Transform],
    ) -> Result<(), EditError> {
        self.control.checkpoint().map_err(EditError::Invalid)?;
        let Some(shape) = brush_models::shape(original).map_err(EditError::Invalid)? else {
            return Ok(());
        };
        let classname = brush_models::classname(original).ok_or_else(|| {
            EditError::Invalid("Child brush model has no admitted classname".into())
        })?;
        let rotation = crate::brush_model_pose::initial_rotation(classname, &original.properties.0)
            .map_err(EditError::Invalid)?;
        let lower_pose =
            matches!(self.purpose, PreparationPurpose::Compile(..)) && rotation.is_some();
        self.objects = self.objects.saturating_add(1 + shape.brushes);
        self.sides = self.sides.saturating_add(shape.sides);
        let property_bytes = original
            .properties
            .iter()
            .try_fold(0_usize, |bytes, (key, value)| {
                bytes.checked_add(key.len())?.checked_add(value.len())
            });
        let work = transforms
            .len()
            .checked_mul(2)
            .and_then(|poses| {
                poses.checked_add(if matches!(self.purpose, PreparationPurpose::Compile(..)) {
                    2
                } else {
                    1
                })
            })
            .and_then(|poses| poses.checked_add(2 * usize::from(lower_pose)))
            .and_then(|poses| shape.geometry_work.checked_mul(poses))
            .and_then(|geometry| {
                let geometry = if lower_pose {
                    geometry.checked_add(shape.sides.checked_mul(8)?)?
                } else {
                    geometry
                };
                let field_bytes = property_bytes?.checked_add(
                    if matches!(self.purpose, PreparationPurpose::Compile(..)) {
                        MAX_FIELD_BYTES
                    } else {
                        0
                    },
                )?;
                let fields = field_bytes.checked_mul(transforms.len() + 1)?;
                geometry.checked_add(fields)
            })
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
        brush_models::check_source(original, self.control).map_err(EditError::Invalid)?;
        let origins = if let PreparationPurpose::Compile(fs, brush_texture_shadows) = self.purpose {
            if self.materials.is_none() {
                let sources = crate::asset_sources::BuildMaterialSources::default();
                self.materials = Some(crate::materials::Materials::with_build_sources(
                    fs,
                    crate::materials::Requirements::GeometrySemantics,
                    &sources,
                ));
                self.material_sources = Some(sources);
            }
            Some(
                brush_models::plan_origin(
                    self.materials.as_mut().unwrap(),
                    original,
                    self.control,
                    brush_texture_shadows,
                    self.material_sources.as_ref().unwrap(),
                    &mut self.checked_cutouts,
                )
                .map_err(EditError::Invalid)?,
            )
        } else {
            None
        };
        let mut entity = Entity::new(tx.new_object_id()?, classname);
        entity.properties = original.properties.clone();
        if let Some(origins) = &origins
            && !origins.volumes.is_empty()
        {
            entity.properties.remove("origin");
            entity.properties.set(
                "origin",
                source_formats::vmf::format_vec3(origins.position.unwrap()),
            );
        }
        entity
            .solids
            .try_reserve_exact(shape.brushes)
            .map_err(|_| EditError::Invalid("Cannot reserve child brush-model solids".into()))?;
        self.record_origin(
            format!("entity:{}", entity.id),
            (owner, relative.to_owned(), original.id, None),
        )?;
        for original_solid in original.solids.iter().filter(|solid| !solid.hidden) {
            self.control.checkpoint().map_err(EditError::Invalid)?;
            let mut solid = original_solid.clone();
            solid.id = tx.new_object_id()?;
            solid.editor = None;
            self.record_origin(
                format!("solid:{}", solid.id),
                (owner, relative.to_owned(), original_solid.id, None),
            )?;
            for side in &mut solid.sides {
                self.control.checkpoint().map_err(EditError::Invalid)?;
                let original_side = side.id;
                side.id = tx.new_side_id()?;
                self.record_origin(
                    format!("side:{}", side.id),
                    (
                        owner,
                        relative.to_owned(),
                        original_side,
                        Some(original_solid.id),
                    ),
                )?;
            }
            if origins
                .as_ref()
                .is_some_and(|origins| origins.volumes.contains(&original_solid.id))
            {
                let mut validation = Document::empty();
                validation
                    .edit("validate consumed instance origin", |tx| {
                        let id = tx.add_solid(solid)?;
                        for transform in transforms.iter().rev() {
                            self.control.checkpoint().map_err(EditError::Invalid)?;
                            edit::transform(tx, &[id], *transform)?;
                            let ObjectId::Solid(id) = id else {
                                unreachable!()
                            };
                            checked_solid_controlled(tx.solid_mut(id)?, self.control)
                                .map_err(EditError::Invalid)?;
                        }
                        Ok(())
                    })
                    .map_err(|error| {
                        EditError::Invalid(format!(
                            "Consumed origin solid {}: {error}",
                            original_solid.id
                        ))
                    })?;
                continue;
            }
            entity.solids.push(solid);
        }
        if origins.is_some() {
            brush_models::check_pose(&entity, self.control).map_err(EditError::Invalid)?;
            if let Some(rotation) = rotation {
                brush_models::lower_initial_pose(&mut entity, rotation, self.control)
                    .map_err(EditError::Invalid)?;
            }
        }
        let generated = entity.id;
        let id = tx.add_entity(entity)?;
        for transform in transforms.iter().rev() {
            self.control.checkpoint().map_err(EditError::Invalid)?;
            edit::transform(tx, &[id], *transform)?;
            let entity = tx.entity_mut(generated)?;
            if origins.is_some() {
                brush_models::check_pose(entity, self.control)
            } else {
                brush_models::check_source(entity, self.control)
            }
            .map_err(EditError::Invalid)?;
        }
        self.stats.brush_models += 1;
        self.stats.consumed_origin_brushes +=
            origins.as_ref().map_or(0, |origins| origins.volumes.len());
        Ok(())
    }

    fn viscluster(
        &mut self,
        tx: &mut Transaction<'_>,
        original: &Entity,
        owner: u32,
        relative: &str,
        transforms: &[edit::Transform],
    ) -> Result<(), EditError> {
        self.control.checkpoint().map_err(EditError::Invalid)?;
        let Some(original_solid) =
            crate::visclusters::source_solid(original).map_err(EditError::Invalid)?
        else {
            return Ok(());
        };
        self.objects = self.objects.saturating_add(2);
        self.sides = self.sides.saturating_add(original_solid.sides.len());
        let property_bytes = original
            .properties
            .iter()
            .try_fold(0_usize, |bytes, (key, value)| {
                bytes.checked_add(key.len())?.checked_add(value.len())
            });
        let work = original_solid
            .sides
            .len()
            .checked_pow(3)
            .and_then(|work| work.checked_mul(transforms.len()))
            .and_then(|work| {
                let hull_work = crate::visclusters::hull_work(original_solid.sides.len())
                    .checked_mul(transforms.len() + 1)?;
                work.checked_add(hull_work)
            })
            .and_then(|work| {
                let field_work = property_bytes?.checked_mul(transforms.len() + 1)?;
                work.checked_add(field_work)
            })
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
        self.viscluster_pose(original)?;
        let mut entity = original.clone();
        entity.id = tx.new_object_id()?;
        entity.editor = None;
        self.record_origin(
            format!("entity:{}", entity.id),
            (owner, relative.to_owned(), original.id, None),
        )?;
        let mut solid = original_solid.clone();
        solid.id = tx.new_object_id()?;
        solid.editor = None;
        self.record_origin(
            format!("solid:{}", solid.id),
            (owner, relative.to_owned(), original_solid.id, None),
        )?;
        for side in &mut solid.sides {
            let original_side = side.id;
            side.id = tx.new_side_id()?;
            self.record_origin(
                format!("side:{}", side.id),
                (
                    owner,
                    relative.to_owned(),
                    original_side,
                    Some(original_solid.id),
                ),
            )?;
        }
        entity.solids = vec![solid];
        let generated = entity.id;
        let id = tx.add_entity(entity)?;
        for transform in transforms.iter().rev() {
            self.control.checkpoint().map_err(EditError::Invalid)?;
            edit::transform(tx, &[id], *transform)?;
            self.viscluster_pose(tx.entity_mut(generated)?)?;
        }
        self.stats.visclusters += 1;
        Ok(())
    }

    fn viscluster_pose(&mut self, entity: &Entity) -> Result<(), EditError> {
        self.control.checkpoint().map_err(EditError::Invalid)?;
        visclusters::check_pose(entity).map_err(EditError::Invalid)?;
        let Some(solid) = crate::visclusters::source_solid(entity).map_err(EditError::Invalid)?
        else {
            return Err(EditError::Invalid(
                "Expanded viscluster has no visible brush".into(),
            ));
        };
        crate::visclusters::source_rules(entity, solid).map_err(EditError::Invalid)?;
        let geometry =
            checked_solid_geometry_controlled(solid, self.control).map_err(EditError::Invalid)?;
        let refund =
            crate::visclusters::checked_hull(solid, &geometry).map_err(EditError::Invalid)?;
        self.control.checkpoint().map_err(EditError::Invalid)?;
        self.work -= refund;
        Ok(())
    }

    fn entity(
        &mut self,
        tx: &mut Transaction<'_>,
        original: &Entity,
        spec: EntitySpec,
        owner: u32,
        relative: &str,
        transforms: &[edit::Transform],
    ) -> Result<u32, EditError> {
        let kind = spec.kind;
        self.control.checkpoint().map_err(EditError::Invalid)?;
        self.objects = self.objects.saturating_add(1);
        let bytes = original
            .properties
            .iter()
            .try_fold(0_usize, |bytes, (key, value)| {
                bytes.checked_add(key.len())?.checked_add(value.len())
            });
        let work = bytes
            .and_then(|bytes| bytes.checked_mul(transforms.len() + 1))
            .and_then(|work| self.work.checked_add(work));
        let Some(work) = work else {
            return Err(EditError::Invalid("Instance entity-work overflow".into()));
        };
        if self.objects > MAX_OBJECTS || work > MAX_GEOMETRY_WORK {
            return Err(EditError::Invalid(
                "Instance expansion exceeds combined objects or geometry-work limits".into(),
            ));
        }
        self.work = work;
        if let Some((key, name)) = &spec.name {
            let work = name
                .len()
                .checked_add(key.len())
                .and_then(|bytes| bytes.checked_mul(transforms.len() + 1))
                .ok_or_else(|| EditError::Invalid("Instance name-work overflow".into()))?;
            names::charge(&mut self.work, work, self.control).map_err(EditError::Invalid)?;
        }
        let mut poses = Vec::new();
        if matches!(kind, entities::Kind::StaticProp) {
            poses.reserve(transforms.len() + 1);
            poses.push(props::check_pose(original).map_err(EditError::Invalid)?);
        } else {
            entities::check_pose(original, kind).map_err(EditError::Invalid)?;
        }
        let mut entity = original.clone();
        entity.id = tx.new_object_id()?;
        entity.editor = None;
        if let Some((key, name)) = spec.name {
            entity.properties.set(key, name);
        }
        if matches!(kind, entities::Kind::LightingOrigin) {
            entity.properties.remove("angles");
            entity.properties.remove("angle");
        }
        let generated = entity.id;
        self.record_origin(
            format!("entity:{}", entity.id),
            (owner, relative.to_owned(), original.id, None),
        )?;
        let id = tx.add_entity(entity)?;
        for transform in transforms.iter().rev() {
            self.control.checkpoint().map_err(EditError::Invalid)?;
            edit::transform(tx, &[id], *transform)?;
            let ObjectId::Entity(entity) = id else {
                unreachable!()
            };
            if matches!(kind, entities::Kind::StaticProp) {
                poses.push(props::check_pose(tx.entity_mut(entity)?).map_err(EditError::Invalid)?);
            } else {
                entities::check_pose(tx.entity_mut(entity)?, kind).map_err(EditError::Invalid)?;
            }
        }
        match kind {
            entities::Kind::Point => self.stats.point_lights += 1,
            entities::Kind::Spot => self.stats.spot_lights += 1,
            entities::Kind::Environment => self.stats.environment_lights += 1,
            entities::Kind::Fog => self.stats.fog_controllers += 1,
            entities::Kind::LightingOrigin => self.stats.lighting_origins += 1,
            entities::Kind::SpotlightTarget => self.stats.spotlight_targets += 1,
            entities::Kind::PlayerSpawn => self.stats.player_spawns += 1,
            entities::Kind::StaticProp => {
                self.stats.static_props += 1;
                let ObjectId::Entity(entity) = id else {
                    unreachable!()
                };
                self.static_prop_poses.insert(entity, poses);
                if let Some(binding) = spec.binding {
                    names::charge(&mut self.work, 1, self.control).map_err(EditError::Invalid)?;
                    self.static_prop_bindings.insert(entity, binding);
                }
            }
        }
        Ok(generated)
    }

    #[allow(clippy::too_many_arguments)]
    fn solid(
        &mut self,
        tx: &mut Transaction<'_>,
        original: &Solid,
        detail: bool,
        owner: u32,
        relative: &str,
        transforms: &[edit::Transform],
        local_sides: Option<&mut BTreeMap<u32, u32>>,
        mut displacement_sides: Option<&mut BTreeMap<u32, u32>>,
    ) -> Result<ObjectId, EditError> {
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
        if local_sides.is_some() {
            cubemaps::charge_bindings(&mut self.work, original.sides.len() * 2, self.control)
                .map_err(EditError::Invalid)?;
        }
        if displacement_sides.is_some() {
            let sides = original
                .sides
                .iter()
                .filter(|side| side.displacement.is_some())
                .count();
            cubemaps::charge_side_bindings(&mut self.work, sides * 2, self.control, "overlay")
                .map_err(EditError::Invalid)?;
        }
        let mut local_sides = local_sides.filter(|_| {
            original
                .sides
                .iter()
                .all(|side| side.displacement.is_none())
        });
        let mut solid = original.clone();
        solid.id = tx.new_object_id()?;
        if !matches!(self.purpose, PreparationPurpose::Collapse) {
            solid.editor = None;
        }
        self.record_origin(
            format!("solid:{}", solid.id),
            (owner, relative.to_owned(), original.id, None),
        )?;
        for side in &mut solid.sides {
            let original_side = side.id;
            side.id = tx.new_side_id()?;
            self.record_origin(
                format!("side:{}", side.id),
                (owner, relative.to_owned(), original_side, Some(original.id)),
            )?;
            if let Some(local_sides) = &mut local_sides {
                local_sides.insert(original_side, side.id);
            }
            if side.displacement.is_some()
                && let Some(displacement_sides) = &mut displacement_sides
            {
                displacement_sides.insert(original_side, side.id);
            }
        }
        let id = if detail {
            let id = tx.new_object_id()?;
            let mut entity = Entity::new(id, "func_detail");
            entity.solids.push(solid);
            self.record_origin(
                format!("entity:{id}"),
                (owner, relative.to_owned(), original.id, None),
            )?;
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
        Ok(id)
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
            occurrence_budget: self.occurrence_budget,
            static_prop_poses: self.static_prop_poses,
            static_prop_bindings: self.static_prop_bindings,
            material_sources: self.material_sources,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn capture_test_occurrence(expansion: &mut Expansion<'_>) {
        if !expansion.control.cancelled() {
            expansion.current_occurrence = Some(
                expansion
                    .occurrence_budget
                    .enter(None, 2000, "child.vmf", expansion.control)
                    .unwrap(),
            );
        }
    }

    fn fixture() -> (tempfile::TempDir, Vmf, Entity, SourceContext) {
        let directory = tempfile::tempdir().unwrap();
        let vmf = crate::test_common::room();
        let path = directory.path().join("parent.vmf");
        std::fs::write(&path, vmf.write(&Default::default())).unwrap();
        let source = SourceContext::capture(&path, &Control::default()).unwrap();
        let mut helper = Entity::new(1000, "func_viscluster");
        helper.solids.push(vmf.world.solids[0].clone());
        (directory, vmf, helper, source)
    }

    #[test]
    fn nonzero_model_pose_reserves_its_added_work_before_geometry_or_material_reads() {
        let (_directory, vmf, mut helper, source) = fixture();
        helper.properties.set("classname", "func_brush");
        helper.properties.set("origin", "0 0 0");
        helper.properties.set("angles", "0 90 0");
        let shape = brush_models::shape(&helper).unwrap().unwrap();
        let transforms = [edit::Transform::default(); 2];
        let fields: usize = helper
            .properties
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum();
        let cost = shape.geometry_work * (2 * transforms.len() + 4)
            + shape.sides * 8
            + (fields + MAX_FIELD_BYTES) * (transforms.len() + 1);
        let control = Control::default();
        let game = crate::test_common::empty_game();
        let mut report = Report::default();
        let mut expansion = Expansion::new(
            &vmf,
            &source,
            &control,
            &mut report,
            PreparationPurpose::Compile(&game, false),
        );
        capture_test_occurrence(&mut expansion);
        expansion.work = MAX_GEOMETRY_WORK - cost + 1;
        helper.solids[0].sides[0].plane.0 = [DVec3::ZERO; 3];
        let mut document = Document::new(vmf.clone());
        let error = document
            .edit("bounded posed model", |tx| {
                expansion.brush_model(tx, &helper, 2000, "child.vmf", &transforms)
            })
            .unwrap_err();
        assert!(error.to_string().contains("geometry-work limits"));
        assert!(expansion.material_sources.is_none());
        assert!(expansion.provenance.is_empty());
        assert_eq!(document.vmf(), &vmf);
    }

    #[test]
    fn compound_model_reserves_entity_solids_sides_and_every_pose_before_geometry() {
        for classname in ["func_brush", "func_wall"] {
            check_compound_model_budgets(classname);
        }
    }

    fn check_compound_model_budgets(classname: &str) {
        let (_directory, vmf, mut helper, source) = fixture();
        helper.properties.set("classname", classname);
        helper.properties.set("origin", "0 0 0");
        let mut second = helper.solids[0].clone();
        second.id += 10_000;
        for side in &mut second.sides {
            side.id += 10_000;
        }
        helper.solids.push(second);
        let mut hidden = helper.solids[0].clone();
        hidden.hidden = true;
        hidden.sides[0].plane.0 = [DVec3::ZERO; 3];
        helper.solids.push(hidden);
        let shape = brush_models::shape(&helper).unwrap().unwrap();
        let transforms = [edit::Transform::default(); 2];
        let fields: usize = helper
            .properties
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum();
        let cost =
            shape.geometry_work * (2 * transforms.len() + 1) + fields * (transforms.len() + 1);
        for exceeded in 0..3 {
            let control = Control::default();
            let mut report = Report::default();
            let mut expansion = Expansion::new(
                &vmf,
                &source,
                &control,
                &mut report,
                PreparationPurpose::Reference,
            );
            capture_test_occurrence(&mut expansion);
            expansion.objects = MAX_OBJECTS - (1 + shape.brushes) + usize::from(exceeded == 0);
            expansion.sides = MAX_SIDES - shape.sides + usize::from(exceeded == 1);
            expansion.work = MAX_GEOMETRY_WORK - cost + usize::from(exceeded == 2);
            let mut invalid = helper.clone();
            invalid.solids[0].sides[0].plane.0 = [DVec3::ZERO; 3];
            let mut document = Document::new(vmf.clone());
            let error = document
                .edit("bounded whole model", |tx| {
                    expansion.brush_model(tx, &invalid, 2000, "child.vmf", &transforms)
                })
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("combined objects, sides or geometry-work limits")
            );
            assert!(expansion.provenance.is_empty());
            assert_eq!(expansion.stats.brush_models, 0);
            assert_eq!(document.vmf(), &vmf);
        }
        let control = Control::default();
        let mut report = Report::default();
        let mut expansion = Expansion::new(
            &vmf,
            &source,
            &control,
            &mut report,
            PreparationPurpose::Reference,
        );
        capture_test_occurrence(&mut expansion);
        expansion.objects = MAX_OBJECTS - (1 + shape.brushes);
        expansion.sides = MAX_SIDES - shape.sides;
        expansion.work = MAX_GEOMETRY_WORK - cost;
        let mut document = Document::new(vmf);
        document
            .edit("exact whole-model budget", |tx| {
                expansion.brush_model(tx, &helper, 2000, "child.vmf", &transforms)
            })
            .unwrap();
        assert_eq!(
            (expansion.objects, expansion.sides, expansion.work),
            (MAX_OBJECTS, MAX_SIDES, MAX_GEOMETRY_WORK)
        );
        assert_eq!(expansion.stats.brush_models, 1);
        assert_eq!(expansion.stats.detail_brushes, 0);
        let generated = document
            .vmf()
            .entities
            .iter()
            .find(|entity| entity.classname() == Some(classname))
            .unwrap();
        assert_eq!(generated.solids.len(), 2);
        assert!(generated.properties.get("angles").is_none());
    }

    #[test]
    fn consumed_origins_keep_source_budgets_and_material_owner_before_cloning() {
        for classname in ["func_brush", "func_wall"] {
            check_consumed_origin_budgets(classname);
        }
    }

    fn check_consumed_origin_budgets(classname: &str) {
        use source_formats::gamefs::{Mount, MountedPath};

        let (directory, vmf, mut helper, source) = fixture();
        helper.properties.set("classname", classname);
        let mut origin = helper.solids[0].clone();
        origin.id += 10_000;
        for side in &mut origin.sides {
            side.id += 10_000;
            side.material = "pivot".into();
        }
        let origin_id = origin.id;
        helper.solids.push(origin);
        let original = helper.clone();
        let materials = directory.path().join("materials");
        std::fs::create_dir_all(materials.join("dev")).unwrap();
        std::fs::write(
            materials.join("dev/dev_measuregeneric01.vmt"),
            "LightmappedGeneric {}",
        )
        .unwrap();
        std::fs::write(
            materials.join("pivot.vmt"),
            "Patch { include materials/only.vmt }",
        )
        .unwrap();
        std::fs::write(
            materials.join("only.vmt"),
            "LightmappedGeneric { %compileorigin 1 }",
        )
        .unwrap();
        let mut game = crate::test_common::empty_game();
        game.mounts.push(MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().to_owned()),
        });
        let shape = brush_models::shape(&helper).unwrap().unwrap();
        let transforms = [edit::Transform::default(); 2];
        let fields: usize = helper
            .properties
            .iter()
            .map(|(key, value)| key.len() + value.len())
            .sum();
        let cost = shape.geometry_work * (2 * transforms.len() + 2)
            + (fields + MAX_FIELD_BYTES) * (transforms.len() + 1);
        for exceeded in 0..4 {
            let control = Control::default();
            if exceeded == 3 {
                control.cancel().unwrap();
            }
            let mut report = Report::default();
            let mut expansion = Expansion::new(
                &vmf,
                &source,
                &control,
                &mut report,
                PreparationPurpose::Compile(&game, false),
            );
            capture_test_occurrence(&mut expansion);
            expansion.objects = MAX_OBJECTS - (1 + shape.brushes) + usize::from(exceeded == 0);
            expansion.sides = MAX_SIDES - shape.sides + usize::from(exceeded == 1);
            expansion.work = MAX_GEOMETRY_WORK - cost + usize::from(exceeded == 2);
            let mut invalid = helper.clone();
            invalid.solids[0].sides[0].plane.0 = [DVec3::ZERO; 3];
            let mut document = Document::new(vmf.clone());
            let error = document
                .edit("bounded consumed origin", |tx| {
                    expansion.brush_model(tx, &invalid, 2000, "child.vmf", &transforms)
                })
                .unwrap_err();
            assert!(error.to_string().contains(if exceeded == 3 {
                "cancelled"
            } else {
                "combined objects, sides or geometry-work limits"
            }));
            assert!(expansion.material_sources.is_none());
            assert!(expansion.provenance.is_empty());
            assert_eq!(expansion.stats.consumed_origin_brushes, 0);
            assert_eq!(document.vmf(), &vmf);
        }
        let control = Control::default();
        let mut report = Report::default();
        let mut expansion = Expansion::new(
            &vmf,
            &source,
            &control,
            &mut report,
            PreparationPurpose::Compile(&game, false),
        );
        capture_test_occurrence(&mut expansion);
        expansion.objects = MAX_OBJECTS - (1 + shape.brushes);
        expansion.sides = MAX_SIDES - shape.sides;
        expansion.work = MAX_GEOMETRY_WORK - cost;
        let mut document = Document::new(vmf.clone());
        document
            .edit("consume bounded origin", |tx| {
                expansion.brush_model(tx, &helper, 2000, "child.vmf", &transforms)
            })
            .unwrap();
        assert_eq!(
            (expansion.objects, expansion.sides, expansion.work),
            (MAX_OBJECTS, MAX_SIDES, MAX_GEOMETRY_WORK)
        );
        assert_eq!(expansion.stats.brush_models, 1);
        assert_eq!(expansion.stats.consumed_origin_brushes, 1);
        assert!(
            expansion
                .provenance
                .values()
                .map(|source| &source.legacy)
                .any(|(_, file, original, _)| file == "child.vmf" && *original == origin_id)
        );
        let generated = document
            .vmf()
            .entities
            .iter()
            .find(|entity| entity.classname() == Some(classname))
            .unwrap();
        assert_eq!(generated.origin(), Some(DVec3::new(0.0, 0.0, -8.0)));
        assert_eq!(generated.solids.len(), 1);
        assert_eq!(helper, original);
        let sources = expansion.finish().material_sources.unwrap();
        assert!(sources.is_observed("materials/only.vmt").unwrap());
        sources.source_guard(&game, &control).unwrap();
        std::fs::write(materials.join("only.vmt"), "LightmappedGeneric {}").unwrap();
        assert!(sources.source_guard(&game, &control).is_err());
    }

    #[test]
    fn viscluster_work_is_reserved_for_every_pose_before_invalid_geometry_is_evaluated() {
        let (_directory, vmf, mut helper, source) = fixture();
        helper.solids[0].sides[0].plane.0 = [DVec3::ZERO; 3];
        let control = Control::default();
        let mut report = Report::default();
        let mut expansion = Expansion::new(
            &vmf,
            &source,
            &control,
            &mut report,
            PreparationPurpose::Reference,
        );
        capture_test_occurrence(&mut expansion);
        expansion.work =
            MAX_GEOMETRY_WORK - crate::visclusters::hull_work(helper.solids[0].sides.len());
        let mut document = Document::new(vmf.clone());
        let error = document
            .edit("reserved helper work", |tx| {
                expansion.viscluster(
                    tx,
                    &helper,
                    2000,
                    "child.vmf",
                    &[edit::Transform::default()],
                )
            })
            .unwrap_err();
        assert!(
            error.to_string().contains("geometry-work limits"),
            "{error}"
        );
        assert!(expansion.provenance.is_empty());
        assert_eq!(expansion.stats.visclusters, 0);
        assert_eq!(document.vmf(), &vmf);
    }

    #[test]
    fn viscluster_point_work_refunds_allow_repeated_small_hulls_without_a_local_helper_cap() {
        let (_directory, vmf, helper, source) = fixture();
        let control = Control::default();
        let mut report = Report::default();
        let mut expansion = Expansion::new(
            &vmf,
            &source,
            &control,
            &mut report,
            PreparationPurpose::Reference,
        );
        capture_test_occurrence(&mut expansion);
        let sides = helper.solids[0].sides.len();
        let reserve = crate::visclusters::hull_work(sides) * 2 + sides.pow(3);
        expansion.work = MAX_GEOMETRY_WORK - reserve - 200_000;
        let mut document = Document::new(vmf);
        document
            .edit("repeated helper work", |tx| {
                for _ in 0..32 {
                    expansion.viscluster(
                        tx,
                        &helper,
                        2000,
                        "child.vmf",
                        &[edit::Transform::default()],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(expansion.stats.visclusters, 32);
        assert!(expansion.work <= MAX_GEOMETRY_WORK);
        assert_eq!(
            document
                .vmf()
                .entities
                .iter()
                .filter(|entity| visclusters::is_helper(entity))
                .count(),
            32,
        );
    }

    #[test]
    fn occluders_reserve_combined_objects_sides_and_every_pose_before_geometry() {
        let (_directory, vmf, mut helper, source) = fixture();
        helper.properties.set("classname", "func_occluder");
        let mut second = helper.solids[0].clone();
        second.id += 100;
        for side in &mut second.sides {
            side.id += 100;
        }
        helper.solids.push(second);
        let mut hidden = helper.solids[0].clone();
        hidden.hidden = true;
        hidden.sides.clear();
        helper.solids.push(hidden);
        helper.solids[0].sides[0].plane.0 = [DVec3::ZERO; 3];
        let shape = occluders::shape(&helper).unwrap().unwrap();
        let transforms = [edit::Transform::default(); 2];
        for exhausted in ["objects", "sides", "work"] {
            let control = Control::default();
            let mut report = Report::default();
            let mut expansion = Expansion::new(
                &vmf,
                &source,
                &control,
                &mut report,
                PreparationPurpose::Reference,
            );
            capture_test_occurrence(&mut expansion);
            match exhausted {
                "objects" => expansion.objects = MAX_OBJECTS - shape.brushes,
                "sides" => expansion.sides = MAX_SIDES - shape.sides + 1,
                "work" => {
                    expansion.work = MAX_GEOMETRY_WORK
                        - helper.solids[0].sides.len().pow(3) * shape.brushes * transforms.len();
                }
                _ => unreachable!(),
            }
            let mut document = Document::new(vmf.clone());
            let error = document
                .edit("reserved occluder work", |tx| {
                    expansion.occluder(tx, &helper, 2000, "child.vmf", &transforms)
                })
                .unwrap_err();
            assert!(
                error.to_string().contains("geometry-work limits"),
                "{exhausted}: {error}"
            );
            assert!(expansion.provenance.is_empty());
            assert_eq!(expansion.stats.occluders, 0);
            assert_eq!(document.vmf(), &vmf);
        }
    }

    #[test]
    fn overlays_reserve_objects_and_all_pose_and_fixup_work_before_invalid_projection() {
        let (_directory, vmf, _, source) = fixture();
        let mut helper = Entity::new(1000, "info_overlay");
        helper.properties.set("BasisOrigin", "NaN 0 0");
        let transforms = [edit::Transform::default(); 2];
        for exhausted in ["objects", "work", "overflow"] {
            let control = Control::default();
            let mut report = Report::default();
            let mut expansion = Expansion::new(
                &vmf,
                &source,
                &control,
                &mut report,
                PreparationPurpose::Reference,
            );
            capture_test_occurrence(&mut expansion);
            match exhausted {
                "objects" => expansion.objects = MAX_OBJECTS,
                "work" => expansion.work = MAX_GEOMETRY_WORK - overlays::POSE_WORK * 3,
                "overflow" => expansion.work = usize::MAX,
                _ => unreachable!(),
            }
            let mut document = Document::new(vmf.clone());
            let error = document
                .edit("reserved overlay work", |tx| {
                    expansion
                        .overlay(tx, &helper, 2000, "child.vmf", &transforms)
                        .map(|_| ())
                })
                .unwrap_err();
            let expected = if exhausted == "overflow" {
                "Instance overlay-work overflow"
            } else {
                "combined objects or geometry-work limits"
            };
            assert!(error.to_string().contains(expected), "{exhausted}: {error}");
            assert!(expansion.provenance.is_empty());
            assert_eq!(expansion.stats.overlays, 0);
            assert_eq!(document.vmf(), &vmf);
        }
    }

    #[test]
    fn overlay_displacement_map_reserves_work_before_cloning_and_excludes_cage_sides() {
        let (_directory, vmf, _, source) = fixture();
        let solid = &vmf.world.solids[0];
        let geometry = BrushGeometry::from_solid(solid).unwrap();
        let face = geometry
            .faces
            .iter()
            .find(|face| face.plane.normal.z > 0.99)
            .unwrap();
        let target = map_core::surface::FaceId {
            solid: solid.id,
            side: solid.sides[face.side].id,
        };
        let mut authored = Document::new(vmf.clone());
        authored
            .edit("terrain target", |tx| {
                map_core::displacement::edit::create(tx, &[target], 2, 0).map(|_| ())
            })
            .unwrap();
        let original = &authored.vmf().world.solids[0];
        let baseline_work = original.sides.len().pow(3) + original.sides.len() * 2;
        for exhausted in [false, true] {
            let control = Control::default();
            let mut report = Report::default();
            let mut expansion = Expansion::new(
                &vmf,
                &source,
                &control,
                &mut report,
                PreparationPurpose::Reference,
            );
            capture_test_occurrence(&mut expansion);
            if exhausted {
                expansion.work = MAX_GEOMETRY_WORK - baseline_work - 1;
            }
            let mut ordinary = BTreeMap::new();
            let mut displacement = BTreeMap::new();
            let mut document = Document::new(vmf.clone());
            let result = document.edit("terrain binding map", |tx| {
                expansion.solid(
                    tx,
                    original,
                    false,
                    2000,
                    "child.vmf",
                    &[edit::Transform::default()],
                    Some(&mut ordinary),
                    Some(&mut displacement),
                )
            });
            assert!(ordinary.is_empty());
            if exhausted {
                assert!(
                    result.unwrap_err().to_string().contains(
                        "Instance overlay bindings exceed the shared geometry-work limit"
                    )
                );
                assert!(displacement.is_empty());
                assert!(expansion.provenance.is_empty());
                assert_eq!(document.vmf(), &vmf);
            } else {
                result.unwrap();
                assert_eq!(expansion.work, baseline_work + 2);
                assert_eq!(displacement.len(), 1);
                let generated = displacement[&target.side];
                assert_eq!(
                    expansion.provenance[&format!("side:{generated}")].legacy,
                    (2000, "child.vmf".into(), target.side, Some(target.solid))
                );
            }
        }
    }
}
