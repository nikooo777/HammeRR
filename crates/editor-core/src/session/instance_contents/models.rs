use std::collections::{BTreeMap, BTreeSet};

use compiler::instances::{OccurrencePath, PreparedInstances, StaticPropPose};
use render::models::{Instance, MAX_SCENE_INDICES, MAX_SCENE_VERTICES};
use source_formats::vmf::Entity;

use super::*;

pub(super) struct Prop {
    owner: u32,
    child: u32,
    entity: Entity,
    poses: Vec<StaticPropPose>,
    occurrence: Arc<OccurrencePath>,
}

impl Prop {
    fn error(&self, error: &str) -> String {
        bounded(&format!(
            "Static prop {} entity:{}: {error}; source {}",
            self.entity
                .properties
                .get("model")
                .unwrap_or("<missing model>"),
            self.child,
            self.occurrence.label()
        ))
    }
}

pub(super) struct Geometry {
    pub instances: Vec<Instance>,
    pub bounds: BTreeMap<u32, [DVec3; 2]>,
    pub vertices: usize,
    pub indices: usize,
    pub lines: usize,
    pub cpu_bytes: usize,
    pub gpu_bytes: usize,
}

pub(super) struct Cached {
    pub token: u64,
    pub owners: BTreeSet<u32>,
    pub result: Result<Geometry, String>,
}

pub(super) fn sources(
    prepared: &PreparedInstances<'_>,
    control: &compiler::Control,
) -> Result<(Vec<Prop>, usize), String> {
    let mut props = Vec::new();
    let mut bytes = prepared.occurrence_bytes();
    for entity in &prepared.vmf().entities {
        control.checkpoint()?;
        let Some(poses) = prepared.static_prop_poses(entity.id) else {
            continue;
        };
        let (owner, _, child) = prepared
            .entity_origin(entity.id)
            .ok_or("Instance static-prop provenance is missing")?;
        let occurrence = prepared
            .entity_occurrence(entity.id)
            .ok_or("Instance static-prop occurrence is missing")?
            .clone();
        if poses.is_empty() || poses.len() > instances::MAX_EXPANSION_DEPTH + 1 || entity.hidden {
            return Err("Instance static-prop preparation is incomplete".into());
        }
        let property_bytes = entity
            .properties
            .iter()
            .try_fold(0usize, |total, (key, value)| {
                total
                    .checked_add(size_of::<(String, String)>())?
                    .checked_add(key.len())?
                    .checked_add(value.len())
            })
            .ok_or("Instance static-prop property size overflow")?;
        bytes = bytes
            .checked_add(property_bytes)
            .and_then(|bytes| bytes.checked_add(std::mem::size_of_val(poses)))
            .ok_or("Instance static-prop source size overflow")?;
        if bytes
            .checked_add((props.len() + 1) * size_of::<Prop>())
            .is_none_or(|bytes| bytes > references::MAX_CPU_BYTES - AUXILIARY_BYTES)
        {
            return Err("Instance static-prop sources exceed the 64 MiB CPU budget".into());
        }
        reserve(&mut props, 1, instances::MAX_OBJECTS)?;
        let mut copy = Entity::new(entity.id, "prop_static");
        copy.properties = entity.properties.clone();
        props.push(Prop {
            owner,
            child,
            entity: copy,
            poses: poses.to_vec(),
            occurrence,
        });
    }
    if props.is_empty() {
        return Ok((props, 0));
    }
    let bytes = props
        .iter()
        .try_fold(prepared.occurrence_bytes(), |bytes, prop| {
            prop.entity.properties.0.iter().try_fold(
                bytes
                    .checked_add(
                        prop.entity.properties.0.capacity() * size_of::<(String, String)>(),
                    )?
                    .checked_add(prop.poses.capacity() * size_of::<StaticPropPose>())?,
                |bytes, (key, value)| {
                    bytes
                        .checked_add(key.capacity())?
                        .checked_add(value.capacity())
                },
            )
        })
        .and_then(|bytes| bytes.checked_add(props.capacity() * size_of::<Prop>()))
        .ok_or("Instance static-prop retained source size overflow")?;
    Ok((props, bytes))
}

impl Session {
    pub(in crate::session) fn invalidate_instance_model_preview(&mut self) {
        *self.instance_contents.models.get_mut() = None;
        self.invalidate_instance_contents_render();
    }

    pub(super) fn cached_instance_models(&self) -> Option<Arc<Cached>> {
        self.instance_contents
            .models
            .borrow()
            .as_ref()
            .filter(|cached| cached.token == self.instance_contents.serial)
            .cloned()
    }

    pub(super) fn prepare_instance_models(&self, ready: &Ready) -> Arc<Cached> {
        let owners = ready
            .owners
            .iter()
            .filter(|owner| self.instance_owner_enabled(owner))
            .map(|owner| owner.id)
            .collect::<BTreeSet<_>>();
        if let Some(cached) = self.cached_instance_models()
            && cached.owners == owners
        {
            return cached;
        }
        let result = self.build_instance_models(ready, &owners);
        let cached = Arc::new(Cached {
            token: self.instance_contents.serial,
            owners,
            result,
        });
        *self.instance_contents.models.borrow_mut() = Some(cached.clone());
        cached
    }

    fn build_instance_models(
        &self,
        ready: &Ready,
        owners: &BTreeSet<u32>,
    ) -> Result<Geometry, String> {
        ready.control.checkpoint()?;
        if let Some(guard) = &ready.dependencies {
            guard
                .verify_metadata()
                .map_err(|error| bounded(&error.to_string()))?;
        }
        let root = self.model_instances(&self.document);
        let root_vertices = root
            .instances
            .iter()
            .filter(|instance| instance.has_geometry())
            .map(|instance| instance.asset.model.vertices.len())
            .sum::<usize>();
        let root_indices = root
            .instances
            .iter()
            .flat_map(|instance| instance.asset.model.parts_for_body(instance.body))
            .map(|part| part.indices.len())
            .sum::<usize>();
        let mut geometry = Geometry {
            instances: Vec::new(),
            bounds: BTreeMap::new(),
            vertices: 0,
            indices: 0,
            lines: 0,
            cpu_bytes: ready.cpu_bytes,
            gpu_bytes: ready.gpu_bytes,
        };
        reserve(
            &mut geometry.instances,
            ready
                .props
                .iter()
                .filter(|prop| owners.contains(&prop.owner))
                .count(),
            instances::MAX_OBJECTS,
        )?;
        geometry.cpu_bytes = geometry
            .cpu_bytes
            .checked_add(geometry.instances.capacity() * size_of::<Instance>() * 2)
            .ok_or("Instance model allocation size overflow")?;
        if geometry.cpu_bytes > references::MAX_CPU_BYTES {
            return Err("Instance model allocation exceeds the 64 MiB CPU budget".into());
        }
        let mut assets = BTreeSet::new();
        for prop in ready
            .props
            .iter()
            .filter(|prop| owners.contains(&prop.owner))
        {
            ready.control.checkpoint()?;
            let path = prop
                .entity
                .properties
                .get("model")
                .ok_or_else(|| prop.error("Prepared model path is missing"))?;
            let instance = self
                .models
                .borrow_mut()
                .get(&self.game, path)
                .and_then(|asset| {
                    asset.materials()?;
                    for (index, pose) in prop.poses.iter().enumerate() {
                        ready.control.checkpoint()?;
                        pose.validate_model_bounds(asset.model.header)
                            .map_err(|error| {
                                format!("source/intermediate pose {index}: {error}")
                            })?;
                    }
                    Instance::new(&prop.entity, asset)
                })
                .map_err(|error| prop.error(&error))?;
            if !instance.has_geometry() {
                return Err(prop.error("Selected model body has no drawable geometry"));
            }
            let vertices = instance.asset.model.vertices.len();
            let indices = instance
                .asset
                .model
                .parts_for_body(instance.body)
                .map(|part| part.indices.len())
                .sum::<usize>();
            let lines = indices
                .checked_mul(2)
                .and_then(|lines| lines.checked_add(48))
                .ok_or_else(|| prop.error("Model line count overflow"))?;
            geometry.vertices = geometry
                .vertices
                .checked_add(vertices)
                .ok_or_else(|| prop.error("Model vertex count overflow"))?;
            geometry.indices = geometry
                .indices
                .checked_add(indices)
                .ok_or_else(|| prop.error("Model index count overflow"))?;
            geometry.lines = geometry
                .lines
                .checked_add(lines)
                .ok_or_else(|| prop.error("Model line count overflow"))?;
            let gpu_bytes = count_bytes(vertices, indices, lines)
                .ok_or_else(|| prop.error("Model GPU size overflow"))?;
            geometry.gpu_bytes = geometry
                .gpu_bytes
                .checked_add(gpu_bytes)
                .ok_or_else(|| prop.error("Model GPU size overflow"))?;
            let asset_bytes = if assets.insert(Arc::as_ptr(&instance.asset) as usize) {
                instance.asset.memory_bytes()
            } else {
                0
            };
            geometry.cpu_bytes = geometry
                .cpu_bytes
                .checked_add(gpu_bytes)
                .and_then(|bytes| bytes.checked_add(asset_bytes))
                .and_then(|bytes| bytes.checked_add(size_of::<(u32, [DVec3; 2])>() + 128))
                .ok_or_else(|| prop.error("Model CPU size overflow"))?;
            if geometry.vertices + ready.mesh.vertices.len() > references::MAX_VERTICES
                || geometry.indices + ready.mesh.indices.len() > references::MAX_INDICES
                || geometry.lines + ready.mesh.edges.len() > references::MAX_LINES
                || (geometry.indices + ready.mesh.indices.len()) / 3 > references::MAX_TRIANGLES
                || geometry.vertices + root_vertices > MAX_SCENE_VERTICES
                || geometry.indices + root_indices > MAX_SCENE_INDICES
                || geometry.gpu_bytes > references::MAX_GPU_BYTES
                || geometry.cpu_bytes > references::MAX_CPU_BYTES
            {
                return Err(prop.error("Instance contents exceed the shared model/reference geometry or memory budget; ordinary scene retained"));
            }
            let (low, high) = instance.bounds();
            geometry
                .bounds
                .entry(prop.owner)
                .and_modify(|bounds| *bounds = [bounds[0].min(low), bounds[1].max(high)])
                .or_insert([low, high]);
            let mut instance = instance;
            instance.id = ObjectId::Entity(prop.owner);
            geometry.instances.push(instance);
        }
        if let Some(guard) = &ready.dependencies {
            guard
                .verify_metadata()
                .map_err(|error| bounded(&error.to_string()))?;
        }
        ready.control.checkpoint()?;
        Ok(geometry)
    }

    pub(super) fn instance_owner_bounds(&self, owner: &Owner) -> Option<[DVec3; 2]> {
        let cached = self.cached_instance_models();
        let model = cached
            .as_ref()
            .and_then(|cached| cached.result.as_ref().ok())
            .and_then(|geometry| geometry.bounds.get(&owner.id))
            .copied();
        match (owner.bounds, model) {
            (Some(a), Some(b)) => Some([a[0].min(b[0]), a[1].max(b[1])]),
            (a, b) => a.or(b),
        }
    }

    pub(super) fn instance_models_json(&self, ready: &Ready) -> Value {
        let cached = self.cached_instance_models();
        let geometry = cached
            .as_ref()
            .and_then(|cached| cached.result.as_ref().ok());
        let error = cached
            .as_ref()
            .and_then(|cached| cached.result.as_ref().err());
        let rows = geometry.map(|geometry| ready.props.iter()
            .filter(|prop| geometry.bounds.contains_key(&prop.owner))
            .zip(&geometry.instances)
            .take(MAX_ROWS)
            .map(|(prop, instance)| {let (low,high)=instance.bounds();json!({
                "owner":format!("entity:{}",prop.owner),"file":prop.occurrence.steps().last().map(|step|step.file.as_str()),
                "child_entity":prop.child,"occurrence":prop.occurrence.as_ref(),"model":instance.asset.path,
                "origin":instance.origin.to_array(),"skin":instance.skin,"body":instance.body,
                "bounds":[low.to_array(),high.to_array()]
            })}).collect::<Vec<_>>()).unwrap_or_default();
        json!({
            "available":geometry.is_some_and(|geometry|!geometry.instances.is_empty()),
            "state":if error.is_some(){"unavailable"}else if geometry.is_some(){"ready"}else{"not_loaded"},
            "reason":if cached.is_none(){Some(STATIC_PROP_PREVIEW_REASON)}else{None},"error":error,
            "models":rows,"total":geometry.map_or(0,|geometry|geometry.instances.len()),
            "truncated":geometry.is_some_and(|geometry|geometry.instances.len()>MAX_ROWS),
            "geometry":geometry.map(|geometry|json!({"vertices":geometry.vertices,"indices":geometry.indices,"line_vertices_bound":geometry.lines,"cpu_bytes_bound":geometry.cpu_bytes,"gpu_bytes_bound":geometry.gpu_bytes})),
            "material_policy":"authored_root_model_path","lighting_preview":false,
            "asset_currentness":"shared_model_cache_reload_models"
        })
    }
}
