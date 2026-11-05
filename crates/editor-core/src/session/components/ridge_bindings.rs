use super::*;
use compiler::cubemap_assets::{CubemapBindingQuery, CubemapBindingSources};
use source_formats::vmf::{Entity, Vmf};
use std::sync::Arc;

#[derive(Clone, PartialEq, Eq)]
pub(in crate::session) struct Write {
    helper: u32,
    key: String,
    before: String,
    after: String,
    source_sides: Vec<u32>,
    exterior_sides: Vec<u32>,
}

#[derive(Clone, Default)]
pub(in crate::session) struct Plan {
    pub writes: Vec<Write>,
    pub sources: Vec<Arc<CubemapBindingSources>>,
    context: Option<String>,
}

pub(super) fn is_cubemap(entity: &Entity) -> bool {
    entity
        .classname()
        .is_some_and(|name| name.eq_ignore_ascii_case("env_cubemap"))
}

pub(super) fn writes(vmf: &Vmf, plan: &subobject::CreasePlan) -> Result<Vec<Write>, String> {
    let mut mapping: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for fragment in &plan.fragments {
        for face in &fragment.faces {
            if face.exterior {
                let targets = mapping.entry(face.source_side).or_default();
                if !targets.contains(&face.side) {
                    targets.push(face.side);
                }
            }
        }
    }
    if plan.fragments.is_empty() {
        for change in &plan.changes {
            if mapping
                .insert(
                    change.source.side,
                    vec![change.source.side, change.new_side],
                )
                .is_some()
            {
                return Err("Ridge source roof IDs are not unique".into());
            }
        }
    }
    writes_for_mapping(vmf, &mapping)
}

pub(in crate::session) fn writes_for_mapping(
    vmf: &Vmf,
    mapping: &BTreeMap<u32, Vec<u32>>,
) -> Result<Vec<Write>, String> {
    if mapping.is_empty() {
        return Ok(Vec::new());
    }
    let existing = vmf.side_ids().collect::<BTreeSet<_>>();
    let mut result = Vec::new();
    let mut owners = BTreeMap::new();
    let mut work = 0usize;
    for entity in vmf.entities.iter().filter(|entity| is_cubemap(entity)) {
        if entity.properties.iter().any(|(key, _)| {
            !key.eq_ignore_ascii_case("sides")
                && (key.trim().eq_ignore_ascii_case("sides")
                    || key
                        .split_once('#')
                        .is_some_and(|(base, _)| base.trim().eq_ignore_ascii_case("sides")))
        }) {
            return Err(format!(
                "entity:{}: ambiguous cubemap sides property spelling",
                entity.id
            ));
        }
        let Some((key, value)) = entity
            .properties
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("sides"))
        else {
            continue;
        };
        let ids = super::ridge::parsed_side_references(entity, key, value, &existing)?;
        work = work
            .checked_add(ids.len())
            .filter(|work| *work <= compiler::cubemap_assets::MAX_ASSIGNMENT_WORK)
            .ok_or("Cubemap binding comparison budget exceeded")?;
        for id in &ids {
            if let Some(previous) = owners.insert(*id, entity.id) {
                return Err(format!(
                    "side:{id} has duplicate manual cubemap ownership by entity:{previous} and entity:{}",
                    entity.id
                ));
            }
        }
        if !ids.iter().any(|id| mapping.contains_key(id)) {
            continue;
        }
        if result.len() >= compiler::cubemap_assets::MAX_SAMPLES {
            return Err("Cubemap binding plan exceeds the sample limit".into());
        }
        let mut seen = BTreeSet::new();
        let mut output = Vec::new();
        let mut source_sides = Vec::new();
        let mut exterior_sides = Vec::new();
        for id in ids {
            if let Some(targets) = mapping.get(&id) {
                source_sides.push(id);
                for &target in targets {
                    if seen.insert(target) {
                        output.push(target);
                        exterior_sides.push(target);
                    }
                }
            } else if seen.insert(id) {
                output.push(id);
            }
        }
        if output.len() > super::super::cubemap_bindings::MAX_FACES {
            return Err(format!(
                "entity:{}: cubemap binding exceeds the manual face limit",
                entity.id
            ));
        }
        if output.is_empty() {
            return Err(format!(
                "entity:{}: the cut removes every manually bound cubemap face; rebind the helper first",
                entity.id
            ));
        }
        let after = output
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        if after.len() > 16 * 1024 {
            return Err("Cubemap binding exceeds the side-list text limit".into());
        }
        result.push(Write {
            helper: entity.id,
            key: key.to_owned(),
            before: value.to_owned(),
            after,
            source_sides,
            exterior_sides,
        });
    }
    Ok(result)
}

pub(in crate::session) fn apply(
    tx: &mut map_core::Transaction,
    writes: &[Write],
) -> Result<(), EditError> {
    for write in writes {
        let entity = tx.entity_mut(write.helper)?;
        if !is_cubemap(entity)
            || entity.properties.get_all(&write.key).count() != 1
            || entity
                .properties
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(&write.key))
                .is_none_or(|(key, value)| key != write.key || value != write.before)
        {
            return Err(EditError::Invalid(
                "Cubemap binding differs from the reviewed topology source".into(),
            ));
        }
        entity.properties.set(&write.key, &write.after);
    }
    Ok(())
}

impl Plan {
    pub(in crate::session) fn retain_after_selection(&mut self, session: &Session) {
        if self.context.is_some() {
            self.context = Some(session.cubemap_binding_source());
        }
    }

    pub(in crate::session) fn stale_error(&self, session: &Session) -> Option<String> {
        if self
            .context
            .as_ref()
            .is_some_and(|context| *context != session.cubemap_binding_source())
        {
            return Some(
                "Cubemap binding context changed; preview the topology change again".into(),
            );
        }
        self.sources
            .iter()
            .find_map(|sources| sources.verify_metadata().err())
    }

    pub(in crate::session) fn report(&self) -> Value {
        json!({"helpers":self.writes.len(),"source_sides":self.writes.iter().map(|write|write.source_sides.len()).sum::<usize>(),
            "exterior_sides":self.writes.iter().map(|write|write.exterior_sides.len()).sum::<usize>(),
            "writes":self.writes.iter().map(|write|json!({"helper":format!("entity:{}",write.helper),"key":write.key,"before":write.before,"after":write.after,
                "source_sides":write.source_sides,"exterior_sides":write.exterior_sides})).collect::<Vec<_>>()})
    }
}

impl Session {
    pub(super) fn prepare_ridge_cubemap_bindings(
        &self,
        document: &mut Document,
        plan: &subobject::CreasePlan,
    ) -> Result<Plan, String> {
        if plan.fragments.is_empty() {
            self.check_ordinary_ridge_references(plan)?;
        }
        let writes = writes(self.document.vmf(), plan)?;
        self.prepare_topology_cubemap_writes(document, writes)
    }

    pub(in crate::session) fn prepare_fragment_cubemap_bindings(
        &self,
        document: &mut Document,
        mapping: &BTreeMap<u32, Vec<u32>>,
    ) -> Result<Plan, String> {
        let writes = writes_for_mapping(self.document.vmf(), mapping)?;
        self.prepare_topology_cubemap_writes(document, writes)
    }

    fn prepare_topology_cubemap_writes(
        &self,
        document: &mut Document,
        writes: Vec<Write>,
    ) -> Result<Plan, String> {
        if writes.is_empty() {
            return Ok(Plan::default());
        }
        let namespace = self.cubemap_binding_namespace()?;
        let source_sides = writes
            .iter()
            .flat_map(|write| write.source_sides.iter().copied())
            .collect::<BTreeSet<_>>();
        let exterior_sides = writes
            .iter()
            .flat_map(|write| write.exterior_sides.iter().copied())
            .collect::<BTreeSet<_>>();
        let mut query = CubemapBindingQuery {
            brush_texture_shadows: self.compile_options().brush_texture_shadows,
            namespace: Some(namespace),
            face_ids: source_sides.into_iter().collect(),
            limit: super::super::cubemap_bindings::MAX_FACES,
            ..Default::default()
        };
        let original = self.inspect_cubemap_binding_map(self.document.vmf(), &query)?;
        for write in &writes {
            if original
                .report
                .helpers
                .iter()
                .find(|helper| helper.helper_id == write.helper)
                .is_none_or(|helper| helper.hidden || helper.excluded)
            {
                return Err(format!(
                    "entity:{}: bound cubemap helper is hidden or excluded",
                    write.helper
                ));
            }
        }
        document
            .edit("preview topology cubemap bindings", |tx| apply(tx, &writes))
            .map_err(|error| error.to_string())?;
        query.face_ids = exterior_sides.into_iter().collect();
        let candidate = self.inspect_cubemap_binding_map(document.vmf(), &query)?;
        for write in &writes {
            for side in &write.exterior_sides {
                if !candidate.report.assignments.iter().any(|assignment| {
                    assignment.face_id == *side
                        && assignment.helper_id == write.helper
                        && assignment.manual
                }) {
                    return Err(format!(
                        "side:{side}: exterior cubemap binding was not preserved"
                    ));
                }
            }
        }
        Ok(Plan {
            writes,
            sources: vec![original.sources, candidate.sources],
            context: Some(self.cubemap_binding_source()),
        })
    }
}
