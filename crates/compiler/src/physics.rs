use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use glam::DVec3;
use serde::{Deserialize, Serialize};
use source_formats::bsp::{
    Bsp, CollisionHeader, MAX_PHYSICS_BYTES, MAX_PHYSICS_MODELS, MAX_PHYSICS_SOLIDS,
    PhysicsCollision, contents,
};

use crate::emit;
use crate::map::{MAX_COMPILE_COORD, Map};
use crate::{Control, Diagnostic, Report};

#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
mod native;

const MAX_REQUEST_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CONVEXES: usize = 8192;
const MAX_CORNERS: usize = 256;
const MAX_PLANES: usize = 256;
const MAX_SURFACE_PROPERTIES: usize = 127;
const MAX_VERTICES: usize = 1_000_000;
const MAX_MESH_TRIANGLES: usize = 65_536;
const WORKER_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const MASK_SOLID: i32 =
    contents::SOLID | contents::WINDOW | contents::GRATE | contents::MOVEABLE | contents::MONSTER;
const WORLD_MASKS: [i32; 3] = [MASK_SOLID, contents::PLAYERCLIP, contents::MONSTERCLIP];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Shape {
    brush: u32,
    corners: Vec<[f32; 3]>,
    planes: Vec<[f32; 4]>,
    #[serde(default)]
    materials: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    contents: i32,
    shapes: Vec<Shape>,
    #[serde(default)]
    surface: Option<f32>,
    #[serde(default)]
    mesh: Option<Mesh>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Mesh {
    vertices: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    #[serde(default)]
    materials: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Model {
    index: usize,
    bodies: Vec<Body>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    model_count: usize,
    models: Vec<Model>,
    #[serde(default = "default_materials")]
    materials: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct Stats {
    pub models: usize,
    pub solids: usize,
    pub convexes: usize,
    pub bytes: usize,
    pub fluids: usize,
    pub mesh_triangles: usize,
    pub surface_properties: usize,
}

fn default_materials() -> Vec<String> {
    vec!["default".into()]
}

fn valid_surface_property(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 127
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

fn material_index(map: &Map, material: usize, names: &mut Vec<String>) -> Result<u8, String> {
    let name = &map.materials.get(material).surface_prop;
    if !valid_surface_property(name) {
        return Err(format!(
            "Physics surface property {name:?} must use 1–127 ASCII letters, digits, underscores, dots or dashes"
        ));
    }
    if let Some(index) = names.iter().position(|value| value == name) {
        return Ok((index + 1) as u8);
    }
    if names.len() == MAX_SURFACE_PROPERTIES {
        return Err("World physics exceeds 127 surface properties including default".into());
    }
    names.push(name.clone());
    Ok(names.len() as u8)
}

fn hull(
    map: &Map,
    tag: usize,
    points: impl Iterator<Item = DVec3>,
    planes: impl Iterator<Item = usize>,
) -> Result<Shape, String> {
    let corners = points
        .map(|point| point.as_vec3().to_array())
        .collect::<Vec<_>>();
    let mut seen = BTreeSet::new();
    let corners = corners
        .into_iter()
        .filter(|point| seen.insert(point.map(f32::to_bits)))
        .collect();
    let planes = planes
        .map(|index| {
            let plane = map.planes.get(index);
            let normal = plane.normal.as_vec3();
            [normal.x, normal.y, normal.z, plane.dist as f32]
        })
        .collect();
    Ok(Shape {
        brush: tag
            .try_into()
            .map_err(|_| "Physics brush index exceeds native bounds")?,
        corners,
        planes,
        materials: Vec::new(),
    })
}

fn shape(map: &Map, index: usize, materials: Option<&mut Vec<String>>) -> Result<Shape, String> {
    let brush = &map.brushes[index];
    let mut result = hull(
        map,
        index,
        brush.points(),
        brush
            .sides
            .iter()
            .filter(|side| !side.bevel)
            .map(|side| side.plane),
    )?;
    if let Some(names) = materials {
        result.materials = brush
            .sides
            .iter()
            .filter(|side| !side.bevel)
            .map(|side| material_index(map, side.material, names))
            .collect::<Result<_, _>>()?;
    }
    Ok(result)
}

fn plan(
    map: &Map,
    models: &[emit::Model],
    water: &crate::water::Water,
    bsp: &Bsp,
    report: &mut Report,
    control: &Control,
) -> Result<Request, String> {
    let mut request = Request {
        model_count: models.len(),
        models: Vec::new(),
        materials: default_materials(),
    };
    for (model_index, model) in models.iter().enumerate() {
        control.checkpoint()?;
        let mut groups: BTreeMap<i32, Vec<Shape>> = BTreeMap::new();
        for &index in &map.entities[model.entity].brushes {
            control.checkpoint()?;
            let brush = &map.brushes[index];
            if brush.contents & (contents::WATER | contents::SLIME) != 0 {
                if model_index != 0 || !brush.structural() {
                    report.diagnostics.push(Diagnostic::warning("Engine fluid physics requires structural world water regions; this detail or brush-model liquid is omitted", Some(format!("solid:{}", brush.id)), None));
                }
                continue;
            }
            if model_index == 0 {
                for mask in WORLD_MASKS {
                    if brush.contents & mask != 0 {
                        groups.entry(mask).or_default().push(shape(
                            map,
                            index,
                            Some(&mut request.materials),
                        )?);
                    }
                }
            } else if brush.contents & (MASK_SOLID | contents::PLAYERCLIP | contents::MONSTERCLIP)
                != 0
            {
                groups
                    .entry(MASK_SOLID)
                    .or_default()
                    .push(shape(map, index, None)?);
            }
        }
        let mut bodies: Vec<_> = WORLD_MASKS
            .into_iter()
            .filter_map(|contents| {
                groups.remove(&contents).map(|shapes| Body {
                    contents,
                    shapes,
                    surface: None,
                    mesh: None,
                })
            })
            .collect();
        if model_index == 0 {
            let mut mesh = Mesh {
                vertices: Vec::new(),
                triangles: Vec::new(),
                materials: Vec::new(),
            };
            if let Some(data) = bsp.displacements().map_err(|e| e.to_string())? {
                for (index, terrain) in map.terrains.iter().enumerate() {
                    control.checkpoint()?;
                    if terrain.info.flags().unwrap_or(0) & 2 != 0 {
                        continue;
                    }
                    if mesh.triangles.len() + terrain.indices.len() > MAX_MESH_TRIANGLES {
                        return Err("Terrain physics exceeds its 65,536-triangle budget".into());
                    }
                    let offset = mesh.vertices.len() as u32;
                    let quad = terrain
                        .base
                        .map(|point| emit::stored_vertex(point).as_vec3().to_array());
                    mesh.vertices
                        .extend(data.positions(index, quad).map_err(|e| e.to_string())?);
                    mesh.triangles.extend(
                        terrain
                            .indices
                            .iter()
                            .map(|triangle| triangle.map(|index| index as u32 + offset)),
                    );
                    let material =
                        map.texdata[map.texinfos.infos[terrain.texinfo].texdata as usize];
                    let material = material_index(map, material, &mut request.materials)?;
                    mesh.materials
                        .extend(std::iter::repeat_n(material, terrain.indices.len()));
                }
            }
            if !mesh.triangles.is_empty() {
                bodies.push(Body {
                    contents: MASK_SOLID,
                    shapes: Vec::new(),
                    surface: None,
                    mesh: Some(mesh),
                });
            }
            let mut fluids = vec![Vec::new(); water.records.len()];
            for (index, node) in model.tree.nodes.iter().enumerate() {
                control.checkpoint()?;
                let region = water.leaves[index];
                if region >= 0 && !node.solid() && node.split.is_none() {
                    fluids[region as usize].push(hull(
                        map,
                        map.brushes.len() + index,
                        node.volume
                            .sides
                            .iter()
                            .flat_map(|side| side.winding.0.iter().copied()),
                        node.volume.sides.iter().map(|side| side.plane),
                    )?);
                }
            }
            for (record, shapes) in water.records.iter().zip(fluids) {
                if shapes.is_empty() {
                    continue;
                }
                let info = &map.texinfos.infos[record.surface_texinfo as usize];
                let material = map.texdata[info.texdata as usize];
                let contents =
                    map.materials.get(material).contents & (contents::WATER | contents::SLIME);
                bodies.push(Body {
                    contents,
                    shapes,
                    surface: Some(record.surface_z),
                    mesh: None,
                });
            }
        }
        if !bodies.is_empty() {
            request.models.push(Model {
                index: model_index,
                bodies,
            });
        }
    }
    validate(&request)?;
    Ok(request)
}

fn validate_materials(materials: &[u8], count: usize, names: usize) -> Result<(), String> {
    if !materials.is_empty()
        && (materials.len() != count
            || materials
                .iter()
                .any(|&index| index == 0 || usize::from(index) > names))
    {
        return Err("Physics surface mappings must match source geometry and declared seven-bit material indices".into());
    }
    Ok(())
}

fn validate(request: &Request) -> Result<(), String> {
    if request.materials.is_empty()
        || request.materials.len() > MAX_SURFACE_PROPERTIES
        || request.materials[0] != "default"
        || request
            .materials
            .iter()
            .any(|name| !valid_surface_property(name))
        || request.materials.iter().collect::<BTreeSet<_>>().len() != request.materials.len()
    {
        return Err(
            "Physics requires 1–127 unique safe surface names, starting with default".into(),
        );
    }
    if request.model_count == 0
        || request.model_count > MAX_PHYSICS_MODELS
        || request.models.len() > request.model_count
    {
        return Err("Physics model count exceeds native bounds".into());
    }
    let mut seen_models = BTreeSet::new();
    let mut convexes = 0;
    let mut vertices = 0;
    let mut mesh_triangles = 0;
    for model in &request.models {
        if model.index >= request.model_count
            || !seen_models.insert(model.index)
            || model.bodies.is_empty()
            || model.bodies.len() > MAX_PHYSICS_SOLIDS
        {
            return Err("Physics request has invalid or duplicate model records".into());
        }
        let mut seen_masks = BTreeSet::new();
        let mut seen_mesh = false;
        for body in &model.bodies {
            if let Some(mesh) = &body.mesh {
                validate_materials(
                    &mesh.materials,
                    mesh.triangles.len(),
                    request.materials.len(),
                )?;
                if model.index != 0
                    || body.surface.is_some()
                    || !body.shapes.is_empty()
                    || body.contents != MASK_SOLID
                    || seen_mesh
                {
                    return Err(
                        "Terrain mesh physics requires one separate world solid body".into(),
                    );
                }
                seen_mesh = true;
                vertices += mesh.vertices.len();
                mesh_triangles += mesh.triangles.len();
                if mesh.vertices.len() < 3
                    || vertices > MAX_VERTICES
                    || mesh.triangles.is_empty()
                    || mesh_triangles > MAX_MESH_TRIANGLES
                {
                    return Err("Terrain mesh physics counts exceed supported bounds".into());
                }
                if mesh.vertices.iter().any(|point| {
                    let point = DVec3::from_array(point.map(f64::from));
                    !point.is_finite() || point.abs().max_element() > MAX_COMPILE_COORD
                }) {
                    return Err("Terrain mesh physics needs finite Source coordinates".into());
                }
                for triangle in &mesh.triangles {
                    let [a, b, c] = triangle.map(|index| {
                        mesh.vertices
                            .get(index as usize)
                            .map(|point| DVec3::from_array(point.map(f64::from)))
                    });
                    let (Some(a), Some(b), Some(c)) = (a, b, c) else {
                        return Err("Terrain mesh triangle has a missing vertex".into());
                    };
                    if (b - a).cross(c - a).length_squared() < 1e-12 {
                        return Err("Terrain mesh physics has a collapsed triangle".into());
                    }
                }
                continue;
            }
            let fluid = body.surface.is_some();
            let valid = if let Some(surface) = body.surface {
                model.index == 0
                    && matches!(body.contents, contents::WATER | contents::SLIME)
                    && surface.is_finite()
                    && surface.abs() as f64 <= MAX_COMPILE_COORD
            } else {
                WORLD_MASKS.contains(&body.contents)
                    && seen_masks.insert(body.contents)
                    && (model.index == 0 || body.contents == MASK_SOLID)
            };
            if !valid || body.shapes.is_empty() {
                return Err("Physics request has invalid contents groups".into());
            }
            let mut seen_brushes = BTreeSet::new();
            for shape in &body.shapes {
                validate_materials(
                    &shape.materials,
                    shape.planes.len(),
                    request.materials.len(),
                )?;
                if !shape.materials.is_empty() && (model.index != 0 || fluid) {
                    return Err("Plane surface mappings require a non-fluid world body".into());
                }
                convexes += 1;
                vertices += shape.corners.len();
                if convexes > MAX_CONVEXES
                    || vertices > MAX_VERTICES
                    || !seen_brushes.insert(shape.brush)
                {
                    return Err(
                        "Engine physics exceeds 8,192 convexes or one million corners".into(),
                    );
                }
                if !(4..=MAX_CORNERS).contains(&shape.corners.len())
                    || !(4..=MAX_PLANES).contains(&shape.planes.len())
                {
                    return Err(format!(
                        "Physics brush {} needs 4–256 corners and planes",
                        shape.brush
                    ));
                }
                let mut mins = DVec3::splat(f64::INFINITY);
                let mut maxs = DVec3::splat(f64::NEG_INFINITY);
                let mut points = Vec::with_capacity(shape.corners.len());
                for point in &shape.corners {
                    let point = DVec3::from_array(point.map(f64::from));
                    if !point.is_finite() || point.abs().max_element() > MAX_COMPILE_COORD {
                        return Err("Physics corners require finite Source coordinates".into());
                    }
                    if fluid && point.z > body.surface.unwrap() as f64 + 0.02 {
                        return Err("Fluid hull extends above its surface plane".into());
                    }
                    mins = mins.min(point);
                    maxs = maxs.max(point);
                    points.push(point);
                }
                if !mins.cmplt(maxs).all() {
                    return Err("Physics convex has empty bounds".into());
                }
                let origin = points[0];
                let axis = points
                    .iter()
                    .map(|point| *point - origin)
                    .max_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))
                    .unwrap();
                let normal = points
                    .iter()
                    .map(|point| axis.cross(*point - origin))
                    .max_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))
                    .unwrap();
                if normal.length_squared() == 0.0
                    || points
                        .iter()
                        .all(|point| normal.normalize().dot(*point - origin).abs() < 1e-5)
                {
                    return Err("Physics convex corners are coplanar".into());
                }
                for plane in &shape.planes {
                    let normal = DVec3::from_array([
                        f64::from(plane[0]),
                        f64::from(plane[1]),
                        f64::from(plane[2]),
                    ]);
                    if !normal.is_finite()
                        || normal.length_squared() == 0.0
                        || !plane[3].is_finite()
                        || points.iter().any(|point| {
                            normal.dot(*point) > f64::from(plane[3]) + 0.02 * normal.length()
                        })
                    {
                        return Err("Physics corners escape their supporting planes".into());
                    }
                }
            }
        }
    }
    Ok(())
}

fn worker_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("HAMMERR_PHYSICS_WORKER") {
        return Ok(PathBuf::from(path));
    }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let directory = executable.parent().ok_or("Cannot locate physics worker")?;
    let directory = if directory.file_name().is_some_and(|name| name == "deps") {
        directory.parent().ok_or("Cannot locate physics worker")?
    } else {
        directory
    };
    let worker = directory.join("hammerr-physics");
    if !worker.is_file() {
        return Err("Engine physics needs hammerr-physics beside the editor/compiler; build with cargo build -p editor -p compiler".into());
    }
    Ok(worker)
}

struct Worker(Child);

impl Drop for Worker {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|error| error.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Physics worker data exceeds supported bounds".into());
    }
    Ok(bytes)
}

fn execute(
    mut command: Command,
    control: &Control,
    timeout: Duration,
) -> Result<ExitStatus, String> {
    #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
    {
        use std::os::unix::process::CommandExt;
        // The child hook only calls async-signal-safe setrlimit.
        unsafe {
            command.pre_exec(native::limits);
        }
    }
    control.checkpoint()?;
    let mut worker = Worker(
        command
            .spawn()
            .map_err(|error| format!("Cannot start physics worker: {error}"))?,
    );
    let started = Instant::now();
    loop {
        control.checkpoint()?;
        if let Some(status) = worker.0.try_wait().map_err(|error| error.to_string())? {
            return Ok(status);
        }
        if started.elapsed() > timeout {
            return Err("Engine physics exceeded its worker time limit".into());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

pub(crate) fn bake(
    map: &Map,
    models: &[emit::Model],
    water: &crate::water::Water,
    fs: &source_formats::gamefs::GameFs,
    bsp: &mut Bsp,
    report: &mut Report,
    control: &Control,
) -> Result<Stats, String> {
    let request = plan(map, models, water, bsp, report, control)?;
    let directory = fs.base_dir.join("bin/linux64");
    if !directory.join("vphysics.so").is_file() {
        return Err(
            "Engine physics requires the installed Linux64 CS:S vphysics.so library".into(),
        );
    }
    let temporary = tempfile::tempdir().map_err(|error| error.to_string())?;
    let input = temporary.path().join("request.json");
    let output = temporary.path().join("physics.bin");
    let log = temporary.path().join("worker.log");
    let bytes = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_REQUEST_BYTES {
        return Err("Physics request exceeds its 64 MiB limit".into());
    }
    let mut file = std::fs::File::create(&input).map_err(|error| error.to_string())?;
    for chunk in bytes.chunks(64 * 1024) {
        control.checkpoint()?;
        file.write_all(chunk).map_err(|error| error.to_string())?;
    }
    drop(file);
    let log_file = std::fs::File::create(&log).map_err(|error| error.to_string())?;
    let mut command = Command::new(worker_path()?);
    command
        .arg(&directory)
        .arg(&input)
        .arg(&output)
        .env("LD_LIBRARY_PATH", &directory)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            log_file.try_clone().map_err(|error| error.to_string())?,
        ))
        .stderr(Stdio::from(log_file));
    let status = execute(command, control, WORKER_TIMEOUT)?;
    if !status.success() {
        let log = String::from_utf8_lossy(&read_bounded(&log, 64 * 1024)?)
            .trim()
            .to_owned();
        return Err(format!("Physics worker failed ({status}): {log}"));
    }
    control.checkpoint()?;
    let bytes = read_bounded(&output, MAX_PHYSICS_BYTES as u64)?;
    let physics =
        PhysicsCollision::parse(&bytes, request.model_count).map_err(|error| error.to_string())?;
    if physics.models.len() != request.models.len()
        || physics
            .models
            .iter()
            .zip(&request.models)
            .any(|(actual, expected)| {
                actual.index != expected.index
                    || actual.solids.len() != expected.bodies.len()
                    || actual
                        .solids
                        .iter()
                        .any(|solid| !matches!(solid.header(), CollisionHeader::Convex { .. }))
            })
    {
        return Err("Physics worker output does not match its requested models and bodies".into());
    }
    let stats = Stats {
        models: physics.models.len(),
        solids: physics.models.iter().map(|model| model.solids.len()).sum(),
        convexes: request
            .models
            .iter()
            .flat_map(|model| &model.bodies)
            .map(|body| body.shapes.len())
            .sum(),
        bytes: bytes.len(),
        fluids: request
            .models
            .iter()
            .flat_map(|model| &model.bodies)
            .filter(|body| body.surface.is_some())
            .count(),
        mesh_triangles: request
            .models
            .iter()
            .flat_map(|model| &model.bodies)
            .filter_map(|body| body.mesh.as_ref())
            .map(|mesh| mesh.triangles.len())
            .sum(),
        surface_properties: if request.models.iter().any(|model| model.index == 0) {
            request.materials.len()
        } else {
            0
        },
    };
    bsp.set_physics(&physics)
        .map_err(|error| error.to_string())?;
    if models.len() > 1 {
        report.diagnostics.push(Diagnostic::warning("Engine physics uses authored world/terrain surface properties; brush models retain default surfaces and a one-kilogram mass. Material density and authored mass controls are unfinished", None, None));
    } else {
        report.diagnostics.push(Diagnostic::info("Engine physics compiles checked brush/fluid hulls and world terrain meshes with authored world/terrain surface properties; model surfaces/masses, density and broader collision validation remain unfinished", None, None));
    }
    Ok(stats)
}

pub fn run_worker() -> Result<(), String> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() != 3 {
        return Err(
            "usage: hammerr-physics <linux64-library-dir> <request.json> <physics.bin>".into(),
        );
    }
    let request: Request =
        serde_json::from_slice(&read_bounded(Path::new(&args[1]), MAX_REQUEST_BYTES)?)
            .map_err(|error| error.to_string())?;
    validate(&request)?;
    #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
    {
        native::write(&request, Path::new(&args[0]), Path::new(&args[2]))
    }
    #[cfg(not(all(target_os = "linux", target_pointer_width = "64")))]
    Err("Engine physics currently requires Linux64".into())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn terrain_mesh_requests_reject_bad_indices_bounds_counts_and_body_roles() {
        let mesh = Mesh {
            vertices: vec![[0.0, 0.0, 0.0], [32.0, 0.0, 0.0], [0.0, 32.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            materials: Vec::new(),
        };
        let request = Request {
            model_count: 1,
            materials: default_materials(),
            models: vec![Model {
                index: 0,
                bodies: vec![Body {
                    contents: MASK_SOLID,
                    shapes: Vec::new(),
                    surface: None,
                    mesh: Some(mesh),
                }],
            }],
        };
        validate(&request).unwrap();
        for case in 0..15 {
            let mut invalid = request.clone();
            let body = &mut invalid.models[0].bodies[0];
            let mesh = body.mesh.as_mut().unwrap();
            match case {
                0 => mesh.triangles[0][2] = 3,
                1 => mesh.vertices[0][0] = f32::NAN,
                2 => mesh.triangles[0] = [0, 0, 0],
                3 => mesh.triangles.resize(MAX_MESH_TRIANGLES + 1, [0, 1, 2]),
                4 => body.surface = Some(32.0),
                5 => body.contents = contents::PLAYERCLIP,
                6 => mesh.materials = vec![0],
                7 => mesh.materials = vec![2],
                8 => mesh.materials = vec![1, 1],
                9 => invalid.materials.push("default".into()),
                10 => invalid.materials[0] = "bad\"name".into(),
                11 => invalid
                    .materials
                    .extend((1..128).map(|index| format!("surface_{index}"))),
                12 => mesh.materials = vec![128],
                13 => invalid.materials.clear(),
                14 => invalid.materials.push("x".repeat(128)),
                _ => unreachable!(),
            }
            assert!(validate(&invalid).is_err());
        }
        let mut largest = request;
        largest
            .materials
            .extend((1..127).map(|index| format!("surface_{index}")));
        largest.models[0].bodies[0].mesh.as_mut().unwrap().materials = vec![127];
        validate(&largest).unwrap();
    }

    fn sleeping_worker(pid: &Path) -> Command {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("printf '%s' $$ > \"$1\"; exec sleep 30")
            .arg("physics-lifecycle-test")
            .arg(pid);
        command
    }

    #[test]
    fn cancelled_and_timed_out_workers_are_killed_and_reaped() {
        let directory = tempfile::tempdir().unwrap();
        let pid = directory.path().join("pid");
        let result = execute(
            sleeping_worker(&pid),
            &Control::default(),
            Duration::from_millis(300),
        );
        assert!(result.unwrap_err().contains("time limit"));
        let number = std::fs::read_to_string(&pid).unwrap();
        assert!(!Path::new("/proc").join(number).exists());
        std::fs::remove_file(&pid).unwrap();
        let control = Control::default();
        let cancellation = control.clone();
        let marker = pid.clone();
        let cancel = std::thread::spawn(move || {
            let started = Instant::now();
            while !marker.is_file() {
                assert!(started.elapsed() < Duration::from_secs(2));
                std::thread::sleep(Duration::from_millis(5));
            }
            cancellation.cancel().unwrap();
        });
        let result = execute(sleeping_worker(&pid), &control, Duration::from_secs(2));
        cancel.join().unwrap();
        assert!(result.unwrap_err().contains("cancelled"));
        let number = std::fs::read_to_string(&pid).unwrap();
        assert!(!Path::new("/proc").join(number).exists());
    }
}
