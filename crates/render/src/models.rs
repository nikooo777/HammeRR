use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    Arc, RwLock, RwLockReadGuard,
    atomic::{AtomicUsize, Ordering},
};

use glam::{DMat3, DQuat, DVec3, EulerRot};
use map_core::{
    ObjectId,
    query::{Hit, Ray},
};
use sha2::{Digest, Sha256};
use source_formats::{
    gamefs::{GameFs, asset_relative},
    model::Model,
    vmf::{Entity, parse_vec3},
};

const MAX_CACHE_BYTES: usize = 256 * 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 128;
const MAX_MATERIAL_LOOKUPS: usize = 65_536;
const MAX_MATERIAL_ERROR_CHARS: usize = 1024;
const MATERIAL_BINDING_RESERVE_BYTES: usize = MAX_MATERIAL_ERROR_CHARS * 4;
const MATERIAL_BINDINGS_ERROR: &str = "Model material state was interrupted; reload models";
pub const MAX_SCENE_VERTICES: usize = 1_000_000;
pub const MAX_SCENE_INDICES: usize = 6_000_000;
pub const MIN_MODEL_SCALE: f32 = 1.0 / 64.0;
pub const MAX_MODEL_SCALE: f32 = 64.0;
pub const MAX_MODEL_SCALE_TEXT_BYTES: usize = 64;

#[cfg(test)]
mod tests;

#[derive(Debug)]
pub struct Asset {
    pub path: String,
    pub model: Model,
    materials: RwLock<Result<Vec<String>, String>>,
    material_bytes: AtomicUsize,
    source_identity: Option<[u8; 32]>,
}

#[derive(Debug)]
pub struct MaterialBindings<'a>(RwLockReadGuard<'a, Result<Vec<String>, String>>);

impl std::ops::Deref for MaterialBindings<'_> {
    type Target = [String];

    fn deref(&self) -> &Self::Target {
        match &*self.0 {
            Ok(materials) => materials,
            Err(_) => unreachable!(),
        }
    }
}

fn material_binding_bytes(materials: &Result<Vec<String>, String>) -> usize {
    match materials {
        Ok(materials) => {
            materials.capacity() * std::mem::size_of::<String>()
                + materials.iter().map(String::capacity).sum::<usize>()
        }
        Err(error) => error.capacity(),
    }
    .max(MATERIAL_BINDING_RESERVE_BYTES)
}

impl Asset {
    fn new(path: String, model: Model, materials: Result<Vec<String>, String>) -> Self {
        let material_bytes = AtomicUsize::new(material_binding_bytes(&materials));
        Self {
            path,
            model,
            materials: RwLock::new(materials),
            material_bytes,
            source_identity: None,
        }
    }

    pub fn materials(&self) -> Result<MaterialBindings<'_>, String> {
        let materials = self
            .materials
            .read()
            .map_err(|_| MATERIAL_BINDINGS_ERROR.to_owned())?;
        if let Err(error) = &*materials {
            return Err(error.clone());
        }
        Ok(MaterialBindings(materials))
    }

    pub fn memory_bytes(&self) -> usize {
        self.memory_bytes_with(self.material_bytes.load(Ordering::Relaxed))
    }

    fn memory_bytes_with(&self, material_bytes: usize) -> usize {
        self.model.memory_bytes() + std::mem::size_of::<Self>() - std::mem::size_of::<Model>()
            + 2 * std::mem::size_of::<usize>()
            + self.path.capacity()
            + material_bytes
    }
}

struct Entry {
    result: Result<Arc<Asset>, String>,
    used: u64,
    bytes: usize,
    material_bindings: bool,
}

#[derive(Default)]
pub struct Library {
    entries: BTreeMap<String, Entry>,
    serial: u64,
    bytes: usize,
}

pub fn model_path(path: &str) -> Result<String, String> {
    if path.len() > 1024 || path.chars().any(char::is_control) || path.contains(['*', '?']) {
        return Err(
            "Use a bounded model asset path without control characters or wildcards".into(),
        );
    }
    let path = asset_relative(path.trim()).map_err(|error| error.to_string())?;
    if !path.starts_with("models/") || !path.ends_with(".mdl") {
        return Err("Model must name a models/*.mdl asset".into());
    }
    Ok(path)
}

fn read(fs: &GameFs, path: &str) -> Result<Vec<u8>, String> {
    fs.read_limited(path, source_formats::mdl::MAX_MODEL_BYTES)
        .map_err(|error| format!("{path}: {error}"))?
        .ok_or_else(|| format!("Model asset {path} was not found"))
}

pub fn source_identity(mdl: &[u8], vvd: &[u8], vtx: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    for bytes in [mdl, vvd, vtx] {
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    digest.finalize().into()
}

fn load(fs: &GameFs, path: &str, material_bindings: bool) -> Result<Asset, String> {
    let stem = path.strip_suffix(".mdl").unwrap();
    let mdl = read(fs, path)?;
    source_formats::mdl::ModelHeader::parse(&mdl).map_err(|error| format!("{path}: {error}"))?;
    let vvd = read(fs, &format!("{stem}.vvd"))?;
    let vtx = read(fs, &format!("{stem}.dx90.vtx"))?;
    let model = Model::parse(&mdl, &vvd, &vtx).map_err(|error| format!("{path}: {error}"))?;
    let materials = if material_bindings {
        resolve_materials(fs, &model)
    } else {
        Err("Flat editor guide does not request material bindings".into())
    };
    let mut asset = Asset::new(path.into(), model, materials);
    asset.source_identity = Some(source_identity(&mdl, &vvd, &vtx));
    Ok(asset)
}

fn resolve_materials(fs: &GameFs, model: &Model) -> Result<Vec<String>, String> {
    resolve_materials_checked(fs, model)
        .map_err(|error| error.chars().take(MAX_MATERIAL_ERROR_CHARS).collect())
}

fn resolve_materials_checked(fs: &GameFs, model: &Model) -> Result<Vec<String>, String> {
    let mut materials = Vec::with_capacity(model.materials.len());
    let mut resolved = BTreeMap::<String, String>::new();
    let mut work = 0;
    for name in &model.materials {
        let name = name.strip_suffix(".vmt").unwrap_or(name);
        if let Some(path) = resolved.get(name) {
            materials.push(path.clone());
            continue;
        }
        let candidates =
            source_formats::model::material_candidates(name, &model.material_directories);
        let mut path = name.to_owned();
        for candidate in candidates {
            work += 1;
            if work > MAX_MATERIAL_LOOKUPS {
                return Err("Model material lookup exceeds its work limit".into());
            }
            if fs
                .contains(&format!("materials/{candidate}.vmt"))
                .map_err(|error| format!("Model material {candidate}: {error}"))?
            {
                path = candidate;
                break;
            }
        }
        resolved.insert(name.to_owned(), path.clone());
        materials.push(path);
    }
    Ok(materials)
}

#[derive(Default)]
pub struct MaterialReloadReport {
    pub refreshed: usize,
    pub changed: usize,
    pub materials: Vec<String>,
    pub failures: Vec<(String, String)>,
}

impl Library {
    pub fn get_verified(
        &mut self,
        fs: &GameFs,
        path: &str,
        expected: [u8; 32],
    ) -> Result<Arc<Asset>, String> {
        let path = model_path(path)?;
        if let Some(entry) = self.entries.get_mut(&path) {
            let matches = entry
                .result
                .as_ref()
                .is_ok_and(|asset| asset.source_identity == Some(expected));
            if matches {
                entry.material_bindings = false;
            } else {
                if entry
                    .result
                    .as_ref()
                    .is_ok_and(|asset| Arc::strong_count(asset) != 1)
                {
                    return Err("A cached model with older bytes is still in use; Reload models and compile again before preparing details".into());
                }
                self.bytes -= self.entries.remove(&path).unwrap().bytes;
            }
        }
        let asset = self.get(fs, &path)?;
        if asset.source_identity != Some(expected) {
            return Err("Model bytes changed during verified preview loading".into());
        }
        Ok(asset)
    }

    pub fn get(&mut self, fs: &GameFs, path: &str) -> Result<Arc<Asset>, String> {
        self.get_with_materials(fs, path, true)
    }

    pub fn get_geometry(&mut self, fs: &GameFs, path: &str) -> Result<Arc<Asset>, String> {
        self.get_with_materials(fs, path, false)
    }

    fn get_with_materials(
        &mut self,
        fs: &GameFs,
        path: &str,
        material_bindings: bool,
    ) -> Result<Arc<Asset>, String> {
        let path = model_path(path)?;
        self.serial = self.serial.wrapping_add(1);
        if let Some(entry) = self.entries.get_mut(&path) {
            entry.used = self.serial;
            if material_bindings && !entry.material_bindings {
                if let Ok(asset) = &entry.result {
                    let mut materials = resolve_materials(fs, &asset.model);
                    let mut bytes = asset.memory_bytes_with(material_binding_bytes(&materials));
                    if self.bytes - entry.bytes + bytes > MAX_CACHE_BYTES {
                        materials =
                            Err("Model material resolution exceeds the cache budget".into());
                        bytes = asset.memory_bytes_with(material_binding_bytes(&materials));
                    }
                    let material_bytes = material_binding_bytes(&materials);
                    *asset
                        .materials
                        .write()
                        .map_err(|_| MATERIAL_BINDINGS_ERROR)? = materials;
                    asset
                        .material_bytes
                        .store(material_bytes, Ordering::Relaxed);
                    self.bytes = self.bytes - entry.bytes + bytes;
                    entry.bytes = bytes;
                }
                entry.material_bindings = true;
            }
            return entry.result.clone();
        }
        let result = load(fs, &path, material_bindings)
            .map(Arc::new)
            .map_err(|error| error.chars().take(1024).collect::<String>());
        let bytes = result.as_ref().map_or(0, |asset| asset.memory_bytes());
        while self.entries.len() >= MAX_CACHE_ENTRIES || self.bytes + bytes > MAX_CACHE_BYTES {
            let candidate = self
                .entries
                .iter()
                .filter(|(_, entry)| {
                    entry
                        .result
                        .as_ref()
                        .map_or(true, |asset| Arc::strong_count(asset) == 1)
                })
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone());
            let Some(candidate) = candidate else {
                return Err(
                    "Model asset cache budget is in use; this model remains a helper".into(),
                );
            };
            self.bytes -= self.entries.remove(&candidate).unwrap().bytes;
        }
        self.bytes += bytes;
        self.entries.insert(
            path,
            Entry {
                result: result.clone(),
                used: self.serial,
                bytes,
                material_bindings,
            },
        );
        result
    }

    pub fn reload_materials(&mut self, fs: &GameFs) -> MaterialReloadReport {
        let mut report = MaterialReloadReport::default();
        let mut names = BTreeSet::new();
        for (path, entry) in &mut self.entries {
            if !entry.material_bindings {
                continue;
            }
            let Ok(asset) = &entry.result else {
                continue;
            };
            report.refreshed += 1;
            let mut materials = resolve_materials(fs, &asset.model);
            let mut bytes = asset.memory_bytes_with(material_binding_bytes(&materials));
            if self.bytes - entry.bytes + bytes > MAX_CACHE_BYTES {
                materials = Err("Model material resolution exceeds the cache budget".into());
                bytes = asset.memory_bytes_with(material_binding_bytes(&materials));
            }
            let Ok(mut bindings) = asset.materials.write() else {
                report
                    .failures
                    .push((path.clone(), MATERIAL_BINDINGS_ERROR.into()));
                continue;
            };
            match &materials {
                Ok(materials) => names.extend(materials.iter().cloned()),
                Err(error) => report.failures.push((path.clone(), error.clone())),
            }
            if materials != *bindings {
                report.changed += 1;
                self.bytes = self.bytes - entry.bytes + bytes;
                entry.bytes = bytes;
                asset
                    .material_bytes
                    .store(material_binding_bytes(&materials), Ordering::Relaxed);
                *bindings = materials;
            }
        }
        report.materials = names.into_iter().collect();
        report
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

#[derive(Clone)]
pub struct Instance {
    pub id: ObjectId,
    pub asset: Arc<Asset>,
    pub origin: DVec3,
    pub rotation: DQuat,
    pub scale: f32,
    pub skin: usize,
    pub body: u32,
    pub tint: [u8; 3],
    pub appearance: source_formats::render_color::ModelAppearance,
    helper_bounds: (DVec3, DVec3),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshSurfaceHit {
    pub surface: map_core::query::SurfaceHit,
    pub part: usize,
    pub triangle: usize,
    pub vertex: u32,
    pub position: DVec3,
}

struct MeshTriangleHit {
    distance: f64,
    normal: DVec3,
    part: usize,
    triangle: usize,
    indices: [u32; 3],
}

pub fn appearance(
    entity: &Entity,
) -> Result<source_formats::render_color::ModelAppearance, String> {
    let appearance = source_formats::render_color::model_appearance(
        entity.properties.get("rendermode"),
        entity.properties.get("renderamt"),
        entity.properties.get("renderfx"),
    )?;
    if appearance.mode != source_formats::render_color::NORMAL
        && entity
            .classname()
            .is_some_and(|name| name.eq_ignore_ascii_case("prop_static"))
    {
        return Err("Static-prop output does not support authored render modes; use prop_dynamic for model opacity".into());
    }
    Ok(appearance)
}

pub fn scale_editable(entity: &Entity) -> bool {
    !entity.classname().is_some_and(|name| {
        [
            "prop_static",
            "prop_detail",
            "detail_prop",
            "prop_detail_sprite",
        ]
        .iter()
        .any(|class| name.eq_ignore_ascii_case(class))
    })
}

pub fn scale(entity: &Entity) -> Result<f32, String> {
    let raw = entity.properties.get("modelscale");
    let scale = raw.map_or(Ok(1.0), |raw| {
        if raw.len() > MAX_MODEL_SCALE_TEXT_BYTES {
            return Err(format!("Model scale exceeds {MAX_MODEL_SCALE_TEXT_BYTES} UTF-8 bytes"));
        }
        raw.parse::<f32>()
            .ok()
            .filter(|value| value.is_finite() && (MIN_MODEL_SCALE..=MAX_MODEL_SCALE).contains(value))
            .ok_or_else(|| format!("Model scale must be a finite native float from {MIN_MODEL_SCALE} to {MAX_MODEL_SCALE}; these are editor numeric limits"))
    })?;
    if !scale_editable(entity) {
        if entity
            .classname()
            .is_some_and(|name| name.eq_ignore_ascii_case("prop_static"))
        {
            if scale != 1.0 {
                return Err("Static-prop output does not support modelscale other than 1; use a compatible dynamic model entity for scale".into());
            }
        } else if raw.is_some_and(|value| value.parse::<f64>().ok() != Some(1.0)) {
            return Err("Native detail records require modelscale 1; dynamic model scaling does not apply to detail props".into());
        }
    }
    Ok(scale)
}

impl Instance {
    pub fn compiled_detail(
        asset: Arc<Asset>,
        origin: [f32; 3],
        angles: [f32; 3],
    ) -> Result<Self, String> {
        if !asset.model.header.is_static_prop() || asset.model.skins.is_empty() {
            return Err("Compiled detail models require a static-prop asset with skin 0".into());
        }
        let origin = DVec3::from_array(origin.map(f64::from));
        let angles = DVec3::from_array(angles.map(f64::from));
        if !origin.is_finite() || !angles.is_finite() {
            return Err("Compiled detail model transforms must be finite".into());
        }
        let radians = angles.map(|angle| angle.rem_euclid(360.0).to_radians());
        Self {
            id: ObjectId::Entity(0),
            asset,
            origin,
            rotation: DQuat::from_euler(EulerRot::ZYX, radians.y, radians.x, radians.z),
            scale: 1.0,
            skin: 0,
            body: 0,
            tint: [255; 3],
            appearance: source_formats::render_color::model_appearance(None, None, None)?,
            helper_bounds: map_core::query::point_entity_bounds(origin),
        }
        .checked_bounds()
    }

    pub fn new(entity: &Entity, asset: Arc<Asset>) -> Result<Self, String> {
        Self::with_appearance(entity, asset, true)
    }

    pub fn editor_guide(entity: &Entity, asset: Arc<Asset>) -> Result<Self, String> {
        Self::with_appearance(entity, asset, false)
    }

    fn with_appearance(entity: &Entity, asset: Arc<Asset>, authored: bool) -> Result<Self, String> {
        let tint = source_formats::render_color::rgb(entity.properties.get("rendercolor"))
            .map_err(|error| error.to_string())?;
        if tint != [255; 3]
            && entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("prop_static"))
        {
            return Err("Tinted static props are not supported by the current compiler output; use prop_dynamic for model tint".into());
        }
        let appearance = if authored {
            appearance(entity)?
        } else {
            source_formats::render_color::model_appearance(None, None, None)?
        };
        if entity
            .classname()
            .is_some_and(|name| name.eq_ignore_ascii_case("prop_static"))
            && !asset.model.header.is_static_prop()
        {
            return Err("This model requires prop_dynamic, prop_physics or another compatible model entity; prop_static requires a static-prop asset".into());
        }
        let origin = entity
            .origin()
            .filter(|point| point.is_finite() && point.abs().max_element() <= 65536.0)
            .ok_or("Model origin must fit finite coordinates within ±65536")?;
        let scale = if authored {
            scale(entity)?
        } else if entity
            .properties
            .get("modelscale")
            .is_some_and(|value| value.parse::<f64>().ok() != Some(1.0))
        {
            return Err("Literal FGD model guides require modelscale 1".into());
        } else {
            1.0
        };
        let body = entity.properties.get("body").map_or(Ok(0), |value| {
            value
                .parse::<u32>()
                .map_err(|_| "Model body must be a nonnegative native integer")
        })?;
        if body > i32::MAX as u32 {
            return Err("Model body must fit a nonnegative native integer".into());
        }
        if body != 0
            && entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("prop_static"))
        {
            return Err("Static props require body 0; use prop_dynamic for bodygroups".into());
        }
        let skin = entity.properties.get("skin").map_or(Ok(0), |value| {
            value
                .parse::<usize>()
                .map_err(|_| "Model skin must be a nonnegative integer")
        })?;
        if skin >= asset.model.skins.len() {
            return Err("Model skin is outside the asset's skin families".into());
        }
        let angles = if let Some(value) = entity.properties.get("angles") {
            parse_vec3(value).ok_or("Model angles require Pitch Yaw Roll")?
        } else if let Some(value) = entity.properties.get("angle") {
            match value
                .parse::<f64>()
                .map_err(|_| "Model angle must be a finite number")?
            {
                -1.0 => DVec3::new(-90.0, 0.0, 0.0),
                -2.0 => DVec3::new(90.0, 0.0, 0.0),
                yaw => DVec3::new(0.0, yaw, 0.0),
            }
        } else {
            DVec3::ZERO
        };
        if !angles.is_finite()
            || angles
                .to_array()
                .iter()
                .any(|&value| !(value as f32).is_finite())
        {
            return Err("Model angles must fit finite native values".into());
        }
        let angles = angles.map(|value| value.rem_euclid(360.0).to_radians());
        let instance = Self {
            id: ObjectId::Entity(entity.id),
            asset,
            origin,
            rotation: DQuat::from_euler(EulerRot::ZYX, angles.y, angles.x, angles.z),
            scale,
            skin,
            body,
            tint,
            appearance,
            helper_bounds: map_core::query::entity_bounds(entity)
                .unwrap_or_else(|| map_core::query::point_entity_bounds(origin)),
        };
        instance.checked_bounds()
    }

    fn checked_bounds(self) -> Result<Self, String> {
        let (mins, maxs) = self.bounds();
        if !mins.is_finite()
            || !maxs.is_finite()
            || mins.abs().max_element().max(maxs.abs().max_element()) > 65536.0
        {
            return Err("Transformed model bounds must fit ±65536".into());
        }
        Ok(self)
    }

    pub fn point(&self, point: [f32; 3]) -> DVec3 {
        let point = DVec3::from_array(point.map(f64::from));
        self.origin
            + self.rotation
                * if self.scale == 1.0 {
                    point
                } else {
                    point * f64::from(self.scale)
                }
    }

    pub fn bounds(&self) -> (DVec3, DVec3) {
        self.bounds_in_basis(DMat3::IDENTITY)
    }

    pub fn has_geometry(&self) -> bool {
        self.asset.model.bounds_for_body(self.body).is_some()
    }

    pub fn attachment(&self, index: usize) -> Option<glam::DAffine3> {
        let attachment = self.asset.model.attachments.get(index.checked_sub(1)?)?;
        let parent = if self.scale == 1.0 {
            glam::DAffine3::from_rotation_translation(self.rotation, self.origin)
        } else {
            glam::DAffine3::from_scale_rotation_translation(
                DVec3::splat(f64::from(self.scale)),
                self.rotation,
                self.origin,
            )
        };
        let mut transform = parent * attachment.transform;
        if attachment.world_aligned() {
            transform.matrix3 = DMat3::IDENTITY;
        }
        Some(transform)
    }

    pub fn bounds_in_basis(&self, basis: DMat3) -> (DVec3, DVec3) {
        let Some([mins, maxs]) = self.asset.model.bounds_for_body(self.body) else {
            return transform_bounds(self.helper_bounds, |point| basis.transpose() * point);
        };
        let mut low = DVec3::splat(f64::INFINITY);
        let mut high = DVec3::splat(f64::NEG_INFINITY);
        for x in [mins[0], maxs[0]] {
            for y in [mins[1], maxs[1]] {
                for z in [mins[2], maxs[2]] {
                    let point = basis.transpose() * self.point([x, y, z]);
                    low = low.min(point);
                    high = high.max(point);
                }
            }
        }
        (low, high)
    }

    pub fn pick(&self, ray: &Ray) -> Option<Hit> {
        self.pick_surface(ray).map(|surface| surface.hit)
    }

    pub fn pick_surface(&self, ray: &Ray) -> Option<map_core::query::SurfaceHit> {
        self.pick_mesh_surface(ray).map(|hit| hit.surface)
    }

    pub fn pick_mesh_surface(&self, ray: &Ray) -> Option<MeshSurfaceHit> {
        let bounds = self.asset.model.bounds_for_body(self.body)?;
        if !ray.origin.is_finite() {
            return None;
        }
        let world_ray = Ray {
            origin: ray.origin,
            direction: ray.direction.try_normalize()?,
        };
        let ray = &world_ray;
        let mut ray = Ray {
            origin: self.rotation.conjugate() * (ray.origin - self.origin),
            direction: self.rotation.conjugate() * ray.direction,
        };
        if self.scale != 1.0 {
            ray.origin /= f64::from(self.scale);
        }
        let model = &self.asset.model;
        if !map_core::query::ray_intersects_bounds(
            &ray,
            DVec3::from_array(bounds[0].map(f64::from)),
            DVec3::from_array(bounds[1].map(f64::from)),
        ) {
            return None;
        }
        let mut nearest: Option<MeshTriangleHit> = None;
        for (part_id, part) in model.indexed_parts_for_body(self.body) {
            for (triangle, indices) in part.indices.as_chunks::<3>().0.iter().enumerate() {
                let points = std::array::from_fn(|axis| {
                    DVec3::from_array(
                        model.vertices[indices[axis] as usize]
                            .position
                            .map(f64::from),
                    )
                });
                if let Some(distance) = map_core::query::ray_triangle(&ray, points)
                    && nearest.as_ref().is_none_or(|old| distance < old.distance)
                {
                    let mut normal = self.rotation
                        * (points[2] - points[0])
                            .cross(points[1] - points[0])
                            .try_normalize()?;
                    if normal.dot(world_ray.direction) > 0.0 {
                        normal = -normal;
                    }
                    nearest = Some(MeshTriangleHit {
                        distance,
                        normal,
                        part: part_id,
                        triangle,
                        indices: *indices,
                    });
                }
            }
        }
        let MeshTriangleHit {
            distance,
            normal,
            part,
            triangle,
            indices,
        } = nearest?;
        let distance = if self.scale == 1.0 {
            distance
        } else {
            distance * f64::from(self.scale)
        };
        let point = world_ray.origin + world_ray.direction * distance;
        let vertex = indices.into_iter().min_by(|&a, &b| {
            self.point(model.vertices[a as usize].position)
                .distance_squared(point)
                .total_cmp(
                    &self
                        .point(model.vertices[b as usize].position)
                        .distance_squared(point),
                )
                .then_with(|| a.cmp(&b))
        })?;
        Some(MeshSurfaceHit {
            part,
            triangle,
            vertex,
            position: self.point(model.vertices[vertex as usize].position),
            surface: map_core::query::SurfaceHit {
                normal,
                terrain_vertex: None,
                hit: Hit {
                    object: self.id,
                    solid: None,
                    side: None,
                    distance,
                    point,
                },
            },
        })
    }
}

fn transform_bounds(bounds: (DVec3, DVec3), transform: impl Fn(DVec3) -> DVec3) -> (DVec3, DVec3) {
    let mut low = DVec3::splat(f64::INFINITY);
    let mut high = DVec3::splat(f64::NEG_INFINITY);
    for x in [bounds.0.x, bounds.1.x] {
        for y in [bounds.0.y, bounds.1.y] {
            for z in [bounds.0.z, bounds.1.z] {
                let point = transform(DVec3::new(x, y, z));
                low = low.min(point);
                high = high.max(point);
            }
        }
    }
    (low, high)
}
