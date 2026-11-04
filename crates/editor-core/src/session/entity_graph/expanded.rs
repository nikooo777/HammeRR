use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use compiler::instances::{
    self, BrushModelPreparation, DependencyGuard, OccurrencePath, SourceContext, StaticPropPose,
};
use glam::DVec3;
use map_core::{Document, ObjectId, references};
use sha2::{Digest, Sha256};

use super::{MAX_GRAPH_NODES, MAX_MAP_NODES, MAX_SCRATCH_BYTES, Session};

const AUXILIARY_BYTES: usize = 4 * 1024 * 1024;
const CONTEXT_CACHE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(super) struct Origin {
    pub id: String,
    pub owner: u32,
    pub file: Option<String>,
    pub child: Option<u32>,
    pub occurrence: Option<Arc<OccurrencePath>>,
}

impl Origin {
    pub(super) fn provenance(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "file": self.file.as_deref()?,
            "entity": self.child?,
            "occurrence": self.occurrence.as_deref()?,
        }))
    }
}

pub(super) struct Source {
    document: Document,
    origins: BTreeMap<u32, Origin>,
    roots: BTreeMap<u32, Vec<u32>>,
    dependencies: Option<DependencyGuard>,
    source: Option<SourceContext>,
    deferred: BTreeSet<u32>,
    poses: BTreeMap<u32, Vec<StaticPropPose>>,
    fingerprint: String,
    retained_bytes: usize,
}

fn occurrence_digest(path: &OccurrencePath) -> String {
    let mut hash = Sha256::new();
    for step in path.steps() {
        hash.update(step.helper.to_le_bytes());
        hash.update((step.file.len() as u64).to_le_bytes());
        hash.update(step.file.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

fn add_bytes(bytes: &mut usize, additional: usize) -> Result<(), String> {
    *bytes = bytes
        .checked_add(additional)
        .filter(|&bytes| bytes <= MAX_SCRATCH_BYTES)
        .ok_or("Expanded entity graph exceeds its retained-storage limit")?;
    Ok(())
}

impl Source {
    pub(super) fn prepare(session: &Session) -> Result<Self, String> {
        let control = compiler::Control::default();
        let vmf = session.document.vmf();
        references::admit_snapshot_storage(vmf, MAX_SCRATCH_BYTES, &mut || control.checkpoint())?;
        let source = if instances::requires_source(vmf) {
            let path = session
                .path
                .as_ref()
                .ok_or("Save the parent VMF before preparing an expanded entity graph")?;
            Some(SourceContext::capture(path, &control).map_err(|error| error.to_string())?)
        } else {
            None
        };
        let mut report = compiler::Report::default();
        let prepared = instances::resolve(vmf, source.as_ref(), &control, &mut report)
            .map_err(|error| format!("Expanded entity graph is unavailable: {}", error.message))?;
        if report.has_errors() {
            return Err("Expanded entity graph preparation contains unresolved errors".into());
        }
        if prepared.vmf().entities.len() > MAX_MAP_NODES {
            return Err(format!(
                "Expanded entity graph admits at most {MAX_MAP_NODES} entities"
            ));
        }
        let storage =
            references::admit_snapshot_storage(prepared.vmf(), MAX_SCRATCH_BYTES, &mut || {
                control.checkpoint()
            })?;
        let mut retained_bytes = AUXILIARY_BYTES;
        add_bytes(&mut retained_bytes, storage)?;
        add_bytes(&mut retained_bytes, storage)?;
        add_bytes(&mut retained_bytes, prepared.occurrence_bytes())?;
        if map_core::context::has_storage(prepared.vmf()) {
            add_bytes(&mut retained_bytes, CONTEXT_CACHE_BYTES)?;
        }
        let mut fingerprint = Sha256::new();
        fingerprint.update(b"entity-graph-expanded-v1");
        if let Some(source) = &source {
            let path = source
                .path()
                .to_str()
                .ok_or("Expanded graph source path must be UTF-8")?;
            fingerprint.update((path.len() as u64).to_le_bytes());
            fingerprint.update(path.as_bytes());
            fingerprint.update(
                source
                    .root_identity()
                    .sha256()
                    .ok_or("Expanded graph parent source has no content identity")?
                    .as_bytes(),
            );
        }
        if let Some(stats) = prepared.stats() {
            for dependency in &stats.dependencies {
                fingerprint.update((dependency.path.len() as u64).to_le_bytes());
                fingerprint.update(dependency.path.as_bytes());
                fingerprint.update(dependency.sha256.as_bytes());
            }
        }
        let mut roots = vmf
            .entities
            .iter()
            .filter(|entity| {
                !entity.hidden
                    && entity
                        .classname()
                        .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
            })
            .map(|entity| (entity.id, Vec::new()))
            .collect::<BTreeMap<_, _>>();
        let authored = vmf
            .entities
            .iter()
            .map(|entity| entity.id)
            .collect::<BTreeSet<_>>();
        let mut origins = BTreeMap::new();
        let mut identities = BTreeSet::new();
        let mut deferred = BTreeSet::new();
        let mut poses = BTreeMap::new();
        for entity in prepared
            .vmf()
            .entities
            .iter()
            .filter(|entity| !entity.hidden)
        {
            control.checkpoint()?;
            let origin = if let Some((owner, file, child)) = prepared.entity_origin(entity.id) {
                let occurrence = prepared
                    .entity_occurrence(entity.id)
                    .cloned()
                    .ok_or("Expanded graph entity has no source occurrence")?;
                let seeds = roots
                    .get_mut(&owner)
                    .ok_or("Expanded graph entity has no authored root instance owner")?;
                seeds.push(entity.id);
                if prepared.brush_model_preparation() == BrushModelPreparation::Deferred
                    && !entity.solids.is_empty()
                    && entity.classname().is_some_and(|class| {
                        class.eq_ignore_ascii_case("func_brush")
                            || class.eq_ignore_ascii_case("func_wall")
                    })
                {
                    deferred.insert(entity.id);
                }
                if let Some(stages) = prepared.static_prop_poses(entity.id) {
                    add_bytes(&mut retained_bytes, std::mem::size_of_val(stages))?;
                    poses.insert(entity.id, stages.to_vec());
                }
                Origin {
                    id: format!(
                        "instance:{owner}:{}:entity:{child}",
                        occurrence_digest(&occurrence)
                    ),
                    owner,
                    file: Some(file.into()),
                    child: Some(child),
                    occurrence: Some(occurrence),
                }
            } else {
                if !authored.contains(&entity.id) {
                    return Err(
                        "Expanded graph entity has no authored or instance provenance".into(),
                    );
                }
                Origin {
                    id: format!("entity:{}", entity.id),
                    owner: entity.id,
                    file: None,
                    child: None,
                    occurrence: None,
                }
            };
            add_bytes(
                &mut retained_bytes,
                size_of::<Origin>()
                    + 256
                    + origin.id.len() * 2
                    + origin.file.as_ref().map_or(0, String::len),
            )?;
            if !identities.insert(origin.id.clone()) || origins.insert(entity.id, origin).is_some()
            {
                return Err("Expanded graph source identities are ambiguous".into());
            }
        }
        drop(identities);
        let dependencies = prepared.dependencies().cloned();
        if source.is_some() && dependencies.is_none() {
            return Err("Expanded graph preparation retained no instance dependency guard".into());
        }
        let source = Self {
            document: Document::try_new(prepared.into_vmf()).map_err(|error| error.to_string())?,
            origins,
            roots,
            dependencies,
            source,
            deferred,
            poses,
            fingerprint: format!("{:x}", fingerprint.finalize()),
            retained_bytes,
        };
        source.verify()?;
        Ok(source)
    }

    pub(super) fn document(&self) -> &Document {
        &self.document
    }

    pub(super) fn node(&self, id: u32) -> Result<&Origin, String> {
        self.origins
            .get(&id)
            .ok_or_else(|| "Expanded graph node is missing or hidden".into())
    }

    pub(super) fn resolve(&self, id: &str) -> Option<u32> {
        self.origins
            .iter()
            .find_map(|(&entity, origin)| (origin.id == id).then_some(entity))
    }

    pub(super) fn seeds(&self, authored: &[u32]) -> Result<Vec<u32>, String> {
        let mut seeds = BTreeSet::new();
        for &id in authored {
            if let Some(children) = self.roots.get(&id) {
                if children.is_empty() {
                    return Err(format!(
                        "Root instance entity:{id} contains no admitted entity graph nodes"
                    ));
                }
                seeds.extend(children.iter().copied());
            } else {
                let node = self.node(id)?;
                if node.child.is_some() {
                    return Err("Expanded graph roots must be authored root entity IDs".into());
                }
                seeds.insert(id);
            }
            if seeds.len() > MAX_GRAPH_NODES {
                return Err("Expanded graph roots exceed the reached-node limit".into());
            }
        }
        Ok(seeds.into_iter().collect())
    }

    pub(super) fn selection(&self, ids: &[u32]) -> Result<Vec<u32>, String> {
        ids.iter()
            .map(|&id| self.node(id).map(|origin| origin.owner))
            .collect::<Result<BTreeSet<_>, _>>()
            .map(|owners| owners.into_iter().collect())
    }

    pub(super) fn snapshot_digest(&self) -> &str {
        &self.fingerprint
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub(super) fn verify(&self) -> Result<(), String> {
        let control = compiler::Control::default();
        if let Some(dependencies) = &self.dependencies {
            dependencies
                .verify(&control)
                .map_err(|error| format!("Expanded graph dependencies changed: {error}"))?;
        } else if let Some(source) = &self.source {
            source.verify(&control).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub(super) fn bounds(
        &self,
        session: &Session,
        ids: &[u32],
    ) -> Result<Vec<(DVec3, DVec3)>, String> {
        for &id in ids {
            self.node(id)?;
            if self.deferred.contains(&id) {
                return Err("Expanded brush-model framing requires resolved material-aware pivots; reference preparation cannot provide these bounds".into());
            }
        }
        let presentation = session.presentation_bounds_in(&self.document, None);
        let models = session.model_instances(&self.document);
        let mut bounds = Vec::with_capacity(ids.len());
        for &id in ids {
            let origin = self.node(id)?;
            if let Some(stages) = self.poses.get(&id) {
                if let Some(error) = models.issues.get(&id) {
                    return Err(format!(
                        "Expanded graph node {} cannot be framed: {error}",
                        origin.id
                    ));
                }
                let model = models
                    .instances
                    .iter()
                    .find(|model| model.id == ObjectId::Entity(id))
                    .filter(|model| model.has_geometry())
                    .ok_or_else(|| {
                        format!(
                            "Expanded graph node {} has no prepared model presentation",
                            origin.id
                        )
                    })?;
                for stage in stages {
                    stage.validate_model_bounds(model.asset.model.header)?;
                }
            }
            let (mins, maxs) = super::super::models::object_bounds(
                &self.document,
                ObjectId::Entity(id),
                &presentation,
            )
            .ok_or_else(|| format!("Expanded graph node {} has no framing bounds", origin.id))?;
            if !mins.is_finite() || !maxs.is_finite() || !mins.cmple(maxs).all() {
                return Err(format!(
                    "Expanded graph node {} has invalid framing bounds",
                    origin.id
                ));
            }
            bounds.push((mins, maxs));
        }
        self.verify()?;
        Ok(bounds)
    }
}
