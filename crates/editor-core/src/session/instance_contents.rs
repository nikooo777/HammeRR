use std::collections::BTreeSet;
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::Arc;

use compiler::instances::{self, DependencyGuard, InstanceStats, SourceLocation};
use glam::DVec3;
use map_core::query::{Hit, Ray};
use map_core::{BrushGeometry, Document, ObjectId};
use render::references::{self, Brush, Mesh};
use render::scene::{LineVertex, Vertex};
use serde_json::{Value, json};
use source_formats::vmf::{Vmf, parse_vec3};

use super::worker::{Slot, Ticket};
use super::{Actor, Session};

const MAX_DIAGNOSTICS: usize = 64;
const MAX_ROWS: usize = 64;
// Includes bounded dependency paths, source stamps and retained reports.
const AUXILIARY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone)]
struct Key {
    generation: u64,
    revision: u64,
    path: Option<PathBuf>,
}

impl Key {
    fn capture(session: &Session) -> Self {
        Self {
            generation: session.document_generation,
            revision: session.document.revision(),
            path: session.path.clone(),
        }
    }

    fn current(&self, session: &Session) -> bool {
        self.generation == session.document_generation
            && self.revision == session.document.revision()
            && self.path == session.path
    }

    fn json(&self) -> Value {
        json!({"generation":self.generation,"revision":self.revision,"path":self.path.as_ref().map(|path|bounded(&path.to_string_lossy()))})
    }
}

struct Pending {
    token: u64,
    identity: Arc<()>,
    control: compiler::Control,
    key: Key,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) worker: Slot,
    serial: u64,
    render_serial: u64,
    pending: Option<Pending>,
    ready: Option<Arc<Ready>>,
    status: &'static str,
    error: Option<String>,
    rendered_report: Value,
    presentation_key: Option<(Option<&'static str>, Vec<u32>)>,
}

pub struct Request {
    token: u64,
    identity: Arc<()>,
    control: compiler::Control,
    key: Key,
    vmf: Option<Vmf>,
    location: Option<SourceLocation>,
    worker: Ticket,
}

pub struct RefreshResult {
    token: u64,
    identity: Arc<()>,
    control: compiler::Control,
    key: Key,
    result: Result<Ready, String>,
    _worker: Ticket,
}

struct Owner {
    id: u32,
    file: String,
    origin: DVec3,
    angles: DVec3,
    bounds: Option<[DVec3; 2]>,
    world_brushes: usize,
    detail_brushes: usize,
}

impl Owner {
    fn json(&self) -> Value {
        json!({"owner":format!("entity:{}",self.id),"file":self.file,"origin":self.origin.to_array(),"angles":self.angles.to_array(),"bounds":self.bounds.map(|bounds|bounds.map(|point|point.to_array())),"world_brushes":self.world_brushes,"detail_brushes":self.detail_brushes})
    }
}

struct BrushRecord {
    bounds: [DVec3; 2],
    child: u32,
    detail: bool,
    faces: Vec<FaceRecord>,
}

struct FaceRecord {
    side: u32,
    indices: std::ops::Range<u32>,
}

struct Ready {
    mesh: Arc<Mesh>,
    key: Key,
    owners: Vec<Owner>,
    brushes: Vec<BrushRecord>,
    dependencies: Option<DependencyGuard>,
    stats: InstanceStats,
    diagnostics: Vec<Value>,
    diagnostics_truncated: bool,
    cpu_bytes: usize,
    gpu_bytes: usize,
}

pub struct Presentation {
    pub mesh: Option<Arc<Mesh>>,
    pub owners: BTreeSet<u32>,
    pub report: Value,
}

fn bounded(value: &str) -> String {
    map_core::entity_helpers::bounded_preview(value, instances::MAX_FIELD_BYTES)
}

fn limits() -> Value {
    json!({"instances":instances::MAX_INSTANCES,"files":instances::MAX_FILES,"file_bytes":instances::MAX_FILE_BYTES,"total_bytes":instances::MAX_TOTAL_BYTES,"file_nodes":instances::MAX_FILE_NODES,"total_nodes":instances::MAX_TOTAL_NODES,"depth":instances::MAX_DEPTH,"objects":instances::MAX_OBJECTS,"sides":instances::MAX_SIDES,"sides_per_brush":instances::MAX_BRUSH_SIDES,"geometry_work":instances::MAX_GEOMETRY_WORK,"field_bytes":instances::MAX_FIELD_BYTES,"parent_bytes":instances::MAX_PARENT_BYTES,"vertices":references::MAX_VERTICES,"indices":references::MAX_INDICES,"line_vertices":references::MAX_LINES,"pick_triangles":references::MAX_TRIANGLES,"cpu_bytes":references::MAX_CPU_BYTES,"gpu_bytes":references::MAX_GPU_BYTES,"rows":MAX_ROWS,"diagnostics":MAX_DIAGNOSTICS,"active_workers":1,"cached_snapshots":1})
}

fn intersects(a: [DVec3; 2], b: [DVec3; 2]) -> bool {
    a[0].cmple(b[1]).all() && a[1].cmpge(b[0]).all()
}

fn count_bytes(vertices: usize, indices: usize, edges: usize) -> Option<usize> {
    vertices
        .checked_mul(size_of::<Vertex>())?
        .checked_add(indices.checked_mul(size_of::<u32>())?)?
        .checked_add(edges.checked_mul(size_of::<LineVertex>())?)
}

fn reserve<T>(values: &mut Vec<T>, additional: usize, maximum: usize) -> Result<(), String> {
    let needed = values
        .len()
        .checked_add(additional)
        .ok_or("Instance capacity overflow")?;
    if needed > maximum {
        return Err("Instance capacity exceeds its geometry limit".into());
    }
    if needed > values.capacity() {
        let target = needed
            .checked_next_power_of_two()
            .unwrap_or(maximum)
            .min(maximum);
        values
            .try_reserve_exact(target - values.len())
            .map_err(|error| error.to_string())?;
    }
    if values.capacity() > maximum {
        return Err("Instance retained capacity exceeds its geometry limit".into());
    }
    Ok(())
}

fn allocation(
    mesh: &Mesh,
    records: &[BrushRecord],
    records_capacity: usize,
    owners: &[Owner],
    owners_capacity: usize,
) -> Option<usize> {
    count_bytes(
        mesh.vertices.capacity(),
        mesh.indices.capacity(),
        mesh.edges.capacity(),
    )?
    .checked_add(mesh.brushes.capacity().checked_mul(size_of::<Brush>())?)?
    .checked_add(records_capacity.checked_mul(size_of::<BrushRecord>())?)?
    .checked_add(
        records
            .iter()
            .map(|record| record.faces.capacity() * size_of::<FaceRecord>())
            .sum::<usize>(),
    )?
    .checked_add(owners_capacity.checked_mul(size_of::<Owner>())?)?
    .checked_add(
        owners
            .iter()
            .map(|owner| owner.file.capacity())
            .sum::<usize>(),
    )?
    .checked_add(AUXILIARY_BYTES)
}

impl Request {
    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn run(self) -> RefreshResult {
        self.run_with(Self::prepare)
    }

    fn run_with(self, prepare: impl FnOnce(&Self) -> Result<Ready, String>) -> RefreshResult {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prepare(&self)))
            .unwrap_or_else(|_| Err("Instance contents worker panicked".into()));
        RefreshResult {
            token: self.token,
            identity: self.identity,
            control: self.control,
            key: self.key,
            result,
            _worker: self.worker,
        }
    }

    fn prepare(&self) -> Result<Ready, String> {
        self.control.checkpoint()?;
        let Some(vmf) = self.vmf.as_ref() else {
            return Ok(Ready {
                mesh: Arc::new(Mesh::default()),
                key: self.key.clone(),
                owners: Vec::new(),
                brushes: Vec::new(),
                dependencies: None,
                stats: InstanceStats::default(),
                diagnostics: Vec::new(),
                diagnostics_truncated: false,
                cpu_bytes: 0,
                gpu_bytes: 0,
            });
        };
        let source = self
            .location
            .clone()
            .map(|location| location.hydrate(&self.control))
            .transpose()
            .map_err(|error| bounded(&error.to_string()))?;
        let mut report = compiler::Report::default();
        let prepared = instances::resolve(vmf, source.as_ref(), &self.control, &mut report)
            .map_err(|error| bounded(&error.message))?;
        let mut owners = vmf
            .entities
            .iter()
            .filter(|entity| {
                !entity.hidden
                    && entity
                        .classname()
                        .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
            })
            .map(|entity| Owner {
                id: entity.id,
                file: bounded(entity.properties.get("file").unwrap_or_default()),
                origin: entity.origin().unwrap_or(DVec3::ZERO),
                angles: entity
                    .properties
                    .get("angles")
                    .and_then(parse_vec3)
                    .unwrap_or(DVec3::ZERO),
                bounds: None,
                world_brushes: 0,
                detail_brushes: 0,
            })
            .collect::<Vec<_>>();
        let mut mesh = Mesh::default();
        let mut records = Vec::<BrushRecord>::new();
        for brush in prepared.generated_brushes() {
            self.control.checkpoint()?;
            let geometry = BrushGeometry::from_solid(brush.solid)
                .map_err(|error| bounded(&error.to_string()))?;
            let vertex_count = geometry
                .faces
                .iter()
                .map(|face| face.vertices.len())
                .sum::<usize>();
            let index_count = geometry
                .faces
                .iter()
                .map(|face| face.vertices.len().saturating_sub(2) * 3)
                .sum::<usize>();
            let edge_count = vertex_count
                .checked_mul(2)
                .ok_or("Instance edge count overflow")?;
            let vertices = mesh
                .vertices
                .len()
                .checked_add(vertex_count)
                .ok_or("Instance vertex count overflow")?;
            let indices = mesh
                .indices
                .len()
                .checked_add(index_count)
                .ok_or("Instance index count overflow")?;
            let edges = mesh
                .edges
                .len()
                .checked_add(edge_count)
                .ok_or("Instance edge count overflow")?;
            let gpu_bytes =
                count_bytes(vertices, indices, edges).ok_or("Instance GPU size overflow")?;
            if vertices > references::MAX_VERTICES
                || indices > references::MAX_INDICES
                || edges > references::MAX_LINES
                || indices / 3 > references::MAX_TRIANGLES
                || gpu_bytes > references::MAX_GPU_BYTES
            {
                return Err("Instance contents exceed the flat geometry vertex/index/edge/triangle/GPU budget; compilation remains available".into());
            }
            let cpu_bytes = gpu_bytes
                .checked_add(
                    (mesh.brushes.len() + 1) * (size_of::<Brush>() + size_of::<BrushRecord>()),
                )
                .and_then(|bytes| {
                    bytes.checked_add(
                        AUXILIARY_BYTES
                            + owners.len() * (size_of::<Owner>() + instances::MAX_FIELD_BYTES),
                    )
                })
                .and_then(|bytes| {
                    bytes.checked_add(
                        records
                            .iter()
                            .map(|record| record.faces.capacity() * size_of::<FaceRecord>())
                            .sum::<usize>()
                            + geometry.faces.len() * size_of::<FaceRecord>(),
                    )
                })
                .ok_or("Instance CPU size overflow")?;
            if cpu_bytes > references::MAX_CPU_BYTES {
                return Err("Instance contents exceed the 64 MiB CPU budget".into());
            }
            reserve(&mut mesh.vertices, vertex_count, references::MAX_VERTICES)?;
            reserve(&mut mesh.indices, index_count, references::MAX_INDICES)?;
            reserve(&mut mesh.edges, edge_count, references::MAX_LINES)?;
            reserve(&mut mesh.brushes, 1, instances::MAX_OBJECTS)?;
            reserve(&mut records, 1, instances::MAX_OBJECTS)?;
            let first_vertex = mesh.vertices.len() as u32;
            let first_index = mesh.indices.len() as u32;
            let first_edge = mesh.edges.len() as u32;
            let mut faces = Vec::new();
            faces
                .try_reserve_exact(geometry.faces.len())
                .map_err(|error| error.to_string())?;
            for face in &geometry.faces {
                self.control.checkpoint()?;
                let side = &brush.solid.sides[face.side];
                let first = mesh.vertices.len() as u32;
                let face_first = mesh.indices.len() as u32;
                for &position in &face.vertices {
                    let position = position.as_vec3();
                    let normal = face.plane.normal.as_vec3();
                    let uv = map_core::texel_uv(side, position.as_dvec3()).as_vec2();
                    if !position.is_finite() || !normal.is_finite() || !uv.is_finite() {
                        return Err(
                            "Instance contents contain nonrepresentable preview vertices or UVs"
                                .into(),
                        );
                    }
                    mesh.vertices.push(Vertex {
                        position: position.to_array(),
                        normal: normal.to_array(),
                        uv: uv.to_array(),
                        color: [255, 255, 255, 0],
                        lightmap: [0.0, 0.0, -1.0],
                        smoothing: 0,
                        opacity: 1.0,
                        texture_alpha: 0,
                        tangent: [0.0; 4],
                        lighting_receiver: 0,
                    });
                }
                for index in 1..face.vertices.len() - 1 {
                    mesh.indices
                        .extend([first, first + index as u32, first + index as u32 + 1]);
                }
                let (_, _, _, original_side) = prepared
                    .face_origin(side.id)
                    .ok_or("Instance child face provenance missing")?;
                faces.push(FaceRecord {
                    side: original_side,
                    indices: face_first..mesh.indices.len() as u32,
                });
                for index in 0..face.vertices.len() {
                    for position in [
                        face.vertices[index],
                        face.vertices[(index + 1) % face.vertices.len()],
                    ] {
                        mesh.edges.push(LineVertex {
                            position: position.as_vec3().to_array(),
                            color: [124, 180, 220, 255],
                        });
                    }
                }
            }
            let bounds = mesh.vertices[first_vertex as usize..]
                .iter()
                .map(|vertex| DVec3::from_array(vertex.position.map(f64::from)))
                .fold(
                    [DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)],
                    |bounds, point| [bounds[0].min(point), bounds[1].max(point)],
                );
            mesh.brushes.push(Brush {
                owner: brush.owner,
                vertices: first_vertex..mesh.vertices.len() as u32,
                indices: first_index..mesh.indices.len() as u32,
                edges: first_edge..mesh.edges.len() as u32,
            });
            records.push(BrushRecord {
                bounds,
                child: brush.original,
                detail: brush.detail,
                faces,
            });
            let owner = owners
                .iter_mut()
                .find(|owner| owner.id == brush.owner)
                .ok_or("Instance owner missing")?;
            owner.file = brush.file.to_owned();
            owner.bounds = Some(
                owner
                    .bounds
                    .map_or(bounds, |old| [old[0].min(bounds[0]), old[1].max(bounds[1])]),
            );
            if brush.detail {
                owner.detail_brushes += 1;
            } else {
                owner.world_brushes += 1;
            }
            if allocation(
                &mesh,
                &records,
                records.capacity(),
                &owners,
                owners.capacity(),
            )
            .is_none_or(|bytes| bytes > references::MAX_CPU_BYTES)
            {
                return Err(
                    "Instance contents exceed the 64 MiB retained CPU capacity budget".into(),
                );
            }
        }
        let dependencies = prepared.dependencies().cloned();
        if let Some(guard) = &dependencies {
            guard
                .verify(&self.control)
                .map_err(|error| bounded(&error.to_string()))?;
        }
        self.control.checkpoint()?;
        let cpu_bytes = allocation(
            &mesh,
            &records,
            records.capacity(),
            &owners,
            owners.capacity(),
        )
        .ok_or("Instance CPU size overflow")?;
        let gpu_bytes = count_bytes(mesh.vertices.len(), mesh.indices.len(), mesh.edges.len())
            .ok_or("Instance GPU size overflow")?;
        let diagnostics=report.diagnostics.iter().take(MAX_DIAGNOSTICS).map(|diagnostic|json!({"level":format!("{:?}",diagnostic.level).to_ascii_lowercase(),"object":diagnostic.object,"message":bounded(&diagnostic.message),"message_truncated":diagnostic.message.len()>instances::MAX_FIELD_BYTES})).collect();
        Ok(Ready {
            mesh: Arc::new(mesh),
            key: self.key.clone(),
            owners,
            brushes: records,
            dependencies,
            stats: prepared.stats().cloned().unwrap_or_default(),
            diagnostics,
            diagnostics_truncated: report.diagnostics.len() > MAX_DIAGNOSTICS,
            cpu_bytes,
            gpu_bytes,
        })
    }
}

impl Session {
    pub fn instance_contents_serial(&self) -> u64 {
        self.instance_contents.render_serial
    }

    pub(super) fn invalidate_instance_contents_render(&mut self) {
        self.instance_contents.render_serial = self.instance_contents.render_serial.wrapping_add(1);
    }

    pub fn begin_instance_contents_refresh(&mut self, actor: Actor) -> Result<Request, String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        if self.instance_contents.worker.busy() {
            return Err("Wait for the active instance contents worker to finish".into());
        }
        self.instance_contents.pending = None;
        self.instance_contents.serial = self.instance_contents.serial.wrapping_add(1);
        self.instance_contents.ready = None;
        self.instance_contents.error = None;
        self.invalidate_instance_contents_render();
        let has_instances = instances::requires_source(self.document.vmf());
        let admission = (|| {
            let vmf = self.document.vmf();
            if !has_instances {
                if let Some(diagnostic) = vmf
                    .entities
                    .iter()
                    .filter(|entity| {
                        entity
                            .classname()
                            .is_some_and(|class| class.eq_ignore_ascii_case("func_instance_parms"))
                    })
                    .find_map(compiler::map::unresolved_instance_diagnostic)
                {
                    return Err(bounded(&diagnostic.message));
                }
                return Ok(None);
            }
            if vmf.object_ids().take(instances::MAX_OBJECTS + 1).count() > instances::MAX_OBJECTS
                || vmf.side_ids().take(instances::MAX_SIDES + 1).count() > instances::MAX_SIDES
            {
                return Err(
                    "Instance contents exceed the compiler's root object/side limits".into(),
                );
            }
            let mut count = 0;
            for entity in vmf.entities.iter().filter(|entity| {
                !entity.hidden
                    && entity
                        .classname()
                        .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
            }) {
                count += 1;
                if count > instances::MAX_INSTANCES {
                    return Err(format!(
                        "Instance contents exceed {} visible references",
                        instances::MAX_INSTANCES
                    ));
                }
                instances::helper_admission(entity)?;
            }
            let location = if instances::requires_source(vmf) {
                Some(
                    SourceLocation::capture(
                        self.path
                            .as_deref()
                            .ok_or("Save the parent VMF before resolving instances")?,
                    )
                    .map_err(|error| bounded(&error.to_string()))?,
                )
            } else {
                None
            };
            Ok(location)
        })();
        let location = match admission {
            Ok(location) => location,
            Err(error) => {
                self.instance_contents.status = "failed";
                self.instance_contents.error = Some(error.clone());
                return Err(error);
            }
        };
        let worker = self
            .instance_contents
            .worker
            .begin()
            .ok_or("Wait for the active instance contents worker to finish")?;
        let key = Key::capture(self);
        let control = compiler::Control::default();
        let token = self.instance_contents.serial;
        let identity = Arc::new(());
        self.instance_contents.pending = Some(Pending {
            token,
            identity: identity.clone(),
            control: control.clone(),
            key: key.clone(),
        });
        self.instance_contents.status = "loading";
        self.log(actor, "refreshing instance contents");
        Ok(Request {
            token,
            identity,
            control,
            key,
            vmf: has_instances.then(|| self.document.vmf().clone()),
            location,
            worker,
        })
    }

    pub fn finish_instance_contents_refresh(
        &mut self,
        actor: Actor,
        result: RefreshResult,
    ) -> Result<Value, String> {
        if self
            .instance_contents
            .pending
            .as_ref()
            .is_none_or(|pending| {
                pending.token != result.token || !Arc::ptr_eq(&pending.identity, &result.identity)
            })
        {
            return Err("Instance contents result belongs to an expired request".into());
        }
        self.instance_contents.pending = None;
        if self.instance_contents.serial != result.token
            || !result.key.current(self)
            || self.closing
        {
            if self.instance_contents.serial == result.token {
                self.instance_contents.status = "stale";
                self.instance_contents.error =
                    Some("Map or referring path changed while resolving instance contents".into());
                self.invalidate_instance_contents_render();
            }
            return Err("Instance contents result is stale; Refresh again".into());
        }
        let prepared = if result.control.cancelled() {
            Err("Instance contents refresh cancelled".into())
        } else {
            result.result.and_then(|ready| {
                if let Some(guard) = &ready.dependencies {
                    guard
                        .verify_metadata()
                        .map_err(|error| bounded(&error.to_string()))?;
                }
                Ok(ready)
            })
        };
        match prepared {
            Ok(ready) => {
                self.instance_contents.status = if ready.owners.is_empty() {
                    "empty"
                } else {
                    "ready"
                };
                self.instance_contents.error = None;
                self.instance_contents.ready = Some(Arc::new(ready));
            }
            Err(error) => {
                self.instance_contents.status = if result.control.cancelled() {
                    "cancelled"
                } else {
                    "failed"
                };
                self.instance_contents.error = Some(bounded(&error));
            }
        }
        self.invalidate_instance_contents_render();
        self.log(
            actor,
            format!("instance contents {}", self.instance_contents.status),
        );
        self.inspect_instance_contents(0, MAX_ROWS)
    }

    pub fn preflight_instance_contents(&mut self) -> bool {
        if let Some(pending) = &self.instance_contents.pending
            && !pending.key.current(self)
        {
            let _ = pending.control.cancel();
        }
        let error = self.instance_contents.ready.as_ref().and_then(|ready| {
            if !ready.key.current(self) {
                Some("Map or referring path changed; Refresh instance contents".into())
            } else {
                ready
                    .dependencies
                    .as_ref()
                    .and_then(|guard| guard.verify_metadata().err())
                    .map(|error| bounded(&error.to_string()))
            }
        });
        if let Some(error) = error {
            self.instance_contents.ready = None;
            self.instance_contents.status = "stale";
            self.instance_contents.error = Some(error);
            self.invalidate_instance_contents_render();
            self.instance_contents.presentation_key = None;
            return true;
        }
        let key = self.current_instance_contents().map(|ready| {
            (
                self.instance_contents_suppression(),
                ready
                    .owners
                    .iter()
                    .filter(|owner| self.instance_owner_visible(owner))
                    .map(|owner| owner.id)
                    .collect::<Vec<_>>(),
            )
        });
        if key != self.instance_contents.presentation_key {
            self.instance_contents.presentation_key = key;
            self.invalidate_instance_contents_render();
            return true;
        }
        false
    }

    pub fn cancel_instance_contents_refresh(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        let pending = self
            .instance_contents
            .pending
            .as_ref()
            .filter(|pending| pending.token == token)
            .ok_or("Instance contents request token is expired")?;
        pending.control.cancel()?;
        self.instance_contents.status = "cancelled";
        self.invalidate_instance_contents_render();
        self.log(actor, "cancelled instance contents refresh");
        self.inspect_instance_contents(0, MAX_ROWS)
    }

    pub fn fail_instance_contents_refresh(
        &mut self,
        actor: Actor,
        token: u64,
        error: String,
    ) -> Result<Value, String> {
        let Some(pending) = self
            .instance_contents
            .pending
            .as_ref()
            .filter(|pending| pending.token == token)
        else {
            return Err("Instance contents request token is expired".into());
        };
        let current = self.instance_contents.serial == token && pending.key.current(self);
        if let Some(pending) = self.instance_contents.pending.take() {
            let _ = pending.control.cancel();
        }
        if !current {
            return Err("Instance contents request is stale".into());
        }
        self.instance_contents.ready = None;
        self.instance_contents.status = "failed";
        self.instance_contents.error = Some(bounded(&error));
        self.invalidate_instance_contents_render();
        self.log(actor, "instance contents worker could not start");
        self.inspect_instance_contents(0, MAX_ROWS)
    }

    pub fn clear_instance_contents(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        if token != self.instance_contents.serial {
            return Err("Instance contents snapshot token is expired".into());
        }
        if self.instance_contents.worker.busy() {
            return Err("Cancel and wait for the instance contents worker before Clear".into());
        }
        self.instance_contents.pending = None;
        self.instance_contents.serial = self.instance_contents.serial.wrapping_add(1);
        self.instance_contents.ready = None;
        self.instance_contents.status = "not_loaded";
        self.instance_contents.error = None;
        self.invalidate_instance_contents_render();
        self.log(actor, "cleared instance contents");
        self.inspect_instance_contents(0, MAX_ROWS)
    }

    pub(super) fn reset_instance_contents(&mut self) {
        self.invalidate_instance_contents_render();
        if let Some(pending) = &self.instance_contents.pending {
            let _ = pending.control.cancel();
        }
        self.instance_contents.serial = self.instance_contents.serial.wrapping_add(1);
        self.instance_contents.ready = None;
        self.instance_contents.status = "not_loaded";
        self.instance_contents.error = None;
        self.instance_contents.rendered_report = Value::Null;
        self.instance_contents.presentation_key = None;
    }

    fn instance_contents_suppression(&self) -> Option<&'static str> {
        if self.face_mode {
            Some("faces_mode")
        } else if self.viewport_pick_conflict()
            || self.measurement_picking()
            || !self.component_selection().is_empty()
            || self.paint_enabled()
            || !std::ptr::eq(self.render_document(), &self.document)
            || self.transform_preview().is_some()
            || !self.surface_placement()["token"].is_null()
            || self.block_preview.is_some()
        {
            Some("active_editing_tool_or_draft")
        } else {
            None
        }
    }

    fn current_instance_contents(&self) -> Option<&Ready> {
        self.instance_contents.ready.as_deref().filter(|ready| {
            ready.key.current(self)
                && ready
                    .dependencies
                    .as_ref()
                    .is_none_or(|guard| guard.verify_metadata().is_ok())
        })
    }

    fn instance_owner_visible(&self, owner: &Owner) -> bool {
        if self
            .document
            .entity(owner.id)
            .is_none_or(|entity| entity.hidden)
        {
            return false;
        }
        if self.editor_object_hidden(map_core::ObjectId::Entity(owner.id)) {
            return false;
        }
        if !self.working_region.enabled {
            return true;
        }
        self.working_region.bounds.is_some_and(|region| {
            owner
                .bounds
                .is_some_and(|bounds| intersects(bounds, region))
                || intersects(
                    [
                        owner.origin - DVec3::splat(8.0),
                        owner.origin + DVec3::splat(8.0),
                    ],
                    region,
                )
        })
    }

    fn instance_report(&self, enabled: bool, offset: usize, limit: usize) -> Value {
        let ready = self.current_instance_contents();
        let suppression = self.instance_contents_suppression();
        let available = ready.is_some_and(|ready| !ready.mesh.indices.is_empty());
        let visible = ready
            .map(|ready| {
                ready
                    .owners
                    .iter()
                    .filter(|owner| self.instance_owner_visible(owner))
                    .count()
            })
            .unwrap_or_default();
        let displayed = enabled && available && suppression.is_none() && visible > 0;
        let stored = if self.instance_contents.status.is_empty() {
            "not_loaded"
        } else {
            self.instance_contents.status
        };
        let status = if !enabled {
            "disabled"
        } else if suppression.is_some() && available {
            "context_suppressed"
        } else {
            stored
        };
        let rows = ready
            .map(|ready| {
                ready
                    .owners
                    .iter()
                    .skip(offset)
                    .take(limit)
                    .map(|owner| {
                        let mut row = owner.json();
                        row["editor_hidden"] =
                            json!(self.editor_object_hidden(ObjectId::Entity(owner.id)));
                        row["display_eligible"] = json!(self.instance_owner_visible(owner));
                        row
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let summary = ready.map_or_else(
            || match stored {
                "loading" => "Refreshing instance contents…",
                "cancelled" => "Instance refresh cancelled",
                "stale" => "Instance contents changed; refresh to display them",
                "failed" => "Instance refresh failed",
                "empty" => "No visible instances to display",
                _ => "Instance contents have not been refreshed",
            }.to_owned(),
            |ready| {
                if ready.stats.instances == 0 {
                    return "No visible instances to display".into();
                }
                let display = if displayed {
                    "displayed"
                } else if !enabled {
                    "display is off"
                } else if !available {
                    "no displayable brushes"
                } else if suppression.is_some() {
                    "temporarily hidden"
                } else {
                    "excluded by editor visibility filters"
                };
                format!(
                    "{} instance{} from {} file{} · {} world brush{} and {} detail brush{} · {display}",
                    ready.stats.instances,
                    if ready.stats.instances == 1 { "" } else { "s" },
                    ready.stats.files,
                    if ready.stats.files == 1 { "" } else { "s" },
                    ready.stats.world_brushes,
                    if ready.stats.world_brushes == 1 { "" } else { "es" },
                    ready.stats.detail_brushes,
                    if ready.stats.detail_brushes == 1 { "" } else { "es" },
                )
            },
        );
        let brushes=ready.map(|ready|ready.mesh.brushes.iter().zip(&ready.brushes).take(MAX_ROWS).map(|(brush,record)|json!({"owner":format!("entity:{}",brush.owner),"child_brush":record.child,"detail":record.detail,"bounds":record.bounds.map(|point|point.to_array())})).collect::<Vec<_>>()).unwrap_or_default();
        let faces=ready.map(|ready|ready.mesh.brushes.iter().zip(&ready.brushes).flat_map(|(brush,record)|record.faces.iter().map(move |face|json!({"owner":format!("entity:{}",brush.owner),"child_brush":record.child,"child_side":face.side,"triangles":(face.indices.end-face.indices.start)/3}))).take(MAX_ROWS).collect::<Vec<_>>()).unwrap_or_default();
        let face_count = ready.map_or(0, |ready| {
            ready
                .brushes
                .iter()
                .map(|record| record.faces.len())
                .sum::<usize>()
        });
        json!({
            "token":self.instance_contents.serial,"status":status,"state":stored,"summary":summary,
            "requested":enabled,"displayed":displayed,"available":available,
            "loading":self.instance_contents.pending.is_some(),
            "worker_active":self.instance_contents.worker.busy(),
            "cancellable":self.instance_contents.pending.as_ref().is_some_and(|pending|!pending.control.cancelled()),
            "suppression_reason":suppression,"error":self.instance_contents.error,
            "source":ready.map(|ready|ready.key.json()),"stats":ready.map(|ready|&ready.stats),
            "owners":rows,"total":ready.map_or(0,|ready|ready.owners.len()),"offset":offset,"limit":limit,
            "has_more":ready.is_some_and(|ready|ready.owners.len()>offset.saturating_add(limit)),
            "visible_owners":visible,"brushes":brushes,"brushes_total":ready.map_or(0,|ready|ready.brushes.len()),
            "brushes_truncated":ready.is_some_and(|ready|ready.brushes.len()>MAX_ROWS),
            "face_provenance":faces,"face_provenance_total":face_count,"face_provenance_truncated":face_count>MAX_ROWS,
            "diagnostics":ready.map(|ready|&ready.diagnostics),"diagnostics_truncated":ready.is_some_and(|ready|ready.diagnostics_truncated),
            "geometry":ready.map(|ready|json!({"vertices":ready.mesh.vertices.len(),"indices":ready.mesh.indices.len(),"line_vertices":ready.mesh.edges.len(),"triangles":ready.mesh.indices.len()/3,"cpu_bytes":ready.cpu_bytes,"gpu_bytes":ready.gpu_bytes})),
            "policy":"flat_read_only_world_detail_root_owner_picking_no_material_loading",
            "authoring_bounds":"ordinary_root_objects_unchanged","limits":limits()
        })
    }

    pub fn inspect_instance_contents(
        &mut self,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        if limit == 0 || limit > MAX_ROWS {
            return Err(format!("Instance contents limit must be 1..{MAX_ROWS}"));
        }
        self.preflight_instance_contents();
        Ok(self.instance_report(self.view.instance_contents, offset, limit))
    }

    pub fn instance_contents_preview(&mut self, enabled: bool) -> Presentation {
        self.preflight_instance_contents();
        let report = self.instance_report(enabled, 0, MAX_ROWS);
        let ready = (enabled && self.instance_contents_suppression().is_none())
            .then(|| self.current_instance_contents())
            .flatten();
        let owners = ready
            .map(|ready| {
                ready
                    .owners
                    .iter()
                    .filter(|owner| self.instance_owner_visible(owner))
                    .map(|owner| owner.id)
                    .collect()
            })
            .unwrap_or_default();
        Presentation {
            mesh: ready.map(|ready| ready.mesh.clone()),
            owners,
            report,
        }
    }

    pub fn instance_contents_hits(&mut self, ray: &Ray, enabled: Option<bool>) -> Vec<Hit> {
        self.preflight_instance_contents();
        if !enabled.unwrap_or(self.view.instance_contents)
            || self.instance_contents_suppression().is_some()
            || !ray.origin.is_finite()
        {
            return Vec::new();
        }
        let Some(direction) = ray.direction.try_normalize() else {
            return Vec::new();
        };
        let ray = Ray {
            origin: ray.origin,
            direction,
        };
        let Some(ready) = self.current_instance_contents() else {
            return Vec::new();
        };
        let mut hits = Vec::new();
        for owner in ready
            .owners
            .iter()
            .filter(|owner| self.instance_owner_visible(owner))
        {
            let Some(bounds) = owner.bounds else {
                continue;
            };
            if !map_core::query::ray_intersects_bounds(&ray, bounds[0], bounds[1]) {
                continue;
            }
            let mut nearest: Option<f64> = None;
            for (brush, record) in ready
                .mesh
                .brushes
                .iter()
                .zip(&ready.brushes)
                .filter(|(brush, _)| brush.owner == owner.id)
            {
                if !map_core::query::ray_intersects_bounds(&ray, record.bounds[0], record.bounds[1])
                {
                    continue;
                }
                for indices in ready.mesh.indices
                    [brush.indices.start as usize..brush.indices.end as usize]
                    .as_chunks::<3>()
                    .0
                    .iter()
                {
                    let positions = std::array::from_fn(|index| {
                        DVec3::from_array(
                            ready.mesh.vertices[indices[index] as usize]
                                .position
                                .map(f64::from),
                        )
                    });
                    if let Some(distance) = map_core::query::ray_triangle(&ray, positions)
                        && nearest.is_none_or(|old| distance < old)
                    {
                        nearest = Some(distance);
                    }
                }
            }
            if let Some(distance) = nearest {
                hits.push(Hit {
                    object: ObjectId::Entity(owner.id),
                    solid: None,
                    side: None,
                    distance,
                    point: ray.origin + ray.direction * distance,
                });
            }
        }
        hits
    }

    pub fn framing_bounds_in(
        &self,
        document: &Document,
        ids: &[ObjectId],
        enabled: Option<bool>,
    ) -> Option<(DVec3, DVec3)> {
        let ordinary = self.object_bounds_in(document, ids);
        if !enabled.unwrap_or(self.view.instance_contents)
            || self.instance_contents_suppression().is_some()
            || !std::ptr::eq(document, &self.document)
        {
            return ordinary;
        }
        let contents = self
            .current_instance_contents()
            .into_iter()
            .flat_map(|ready| &ready.owners)
            .filter(|owner| {
                ids.contains(&ObjectId::Entity(owner.id)) && self.instance_owner_visible(owner)
            })
            .filter_map(|owner| owner.bounds.map(|bounds| (bounds[0], bounds[1])));
        map_core::query::union(ordinary.into_iter().chain(contents))
    }

    pub fn frame_instance_contents(
        &mut self,
        actor: Actor,
        token: u64,
        ids: &[ObjectId],
        aspect: f32,
    ) -> Result<Value, String> {
        self.preflight_instance_contents();
        if token != self.instance_contents.serial {
            return Err("Instance contents snapshot token is expired".into());
        }
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("Viewport aspect must be positive and finite".into());
        }
        if self.instance_contents_suppression().is_some() {
            return Err("Finish the active editing tool before framing instance contents".into());
        }
        let ready = self
            .current_instance_contents()
            .ok_or("Refresh instance contents before framing")?;
        if ids.iter().any(|id| {
            !ready
                .owners
                .iter()
                .any(|owner| *id == ObjectId::Entity(owner.id))
        }) {
            return Err("Frame instance contents accepts current root instance owners only".into());
        }
        let bounds = map_core::query::union(
            ready
                .owners
                .iter()
                .filter(|owner| {
                    (ids.is_empty() || ids.contains(&ObjectId::Entity(owner.id)))
                        && self.instance_owner_visible(owner)
                })
                .filter_map(|owner| owner.bounds.map(|bounds| (bounds[0], bounds[1]))),
        )
        .ok_or("Instance contents have no visible geometry to frame")?;
        self.set_camera(
            self.camera
                .frame_bounds(bounds.0.as_vec3(), bounds.1.as_vec3(), aspect),
        );
        self.log(actor, "framed instance contents");
        self.inspect_instance_contents(0, MAX_ROWS)
    }

    pub fn retain_instance_contents_render_report(&mut self, report: Value) {
        self.instance_contents.rendered_report = report;
    }

    pub fn instance_contents_rendered_report(&self) -> Value {
        self.instance_contents.rendered_report.clone()
    }
}

impl State {
    pub(super) fn close_work(&self) -> serde_json::Value {
        serde_json::json!({
            "sequence": self.serial,
            "status": self.status,
            "worker_active": self.worker.busy(),
            "request": self.pending
                .as_ref()
                .map(|pending| serde_json::json!({"token": pending.token, "cancelled": pending.control.cancelled()}))
        })
    }
}

#[cfg(test)]
mod tests;
