use std::collections::BTreeMap;
use std::sync::Arc;

use glam::{DMat3, DQuat, DVec3, EulerRot};
use map_core::{
    ObjectId,
    query::{Hit, Ray},
};
use source_formats::{
    gamefs::{GameFs, asset_relative},
    model::Model,
    vmf::{Entity, parse_vec3},
};

const MAX_CACHE_BYTES: usize = 256 * 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 128;
const MAX_MATERIAL_LOOKUPS: usize = 65_536;
pub const MAX_SCENE_VERTICES: usize = 1_000_000;
pub const MAX_SCENE_INDICES: usize = 6_000_000;

#[cfg(test)]
mod tests;

#[derive(Debug)]
pub struct Asset {
    pub path: String,
    pub model: Model,
    pub materials: Vec<String>,
}

impl Asset {
    pub fn memory_bytes(&self) -> usize {
        self.model.memory_bytes() + std::mem::size_of::<Self>() - std::mem::size_of::<Model>()
            + self.path.capacity()
            + self.materials.capacity() * std::mem::size_of::<String>()
            + self.materials.iter().map(String::capacity).sum::<usize>()
    }
}

struct Entry {
    result: Result<Arc<Asset>, String>,
    used: u64,
    bytes: usize,
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

fn load(fs: &GameFs, path: &str) -> Result<Asset, String> {
    let stem = path.strip_suffix(".mdl").unwrap();
    let mdl = read(fs, path)?;
    source_formats::mdl::ModelHeader::parse(&mdl).map_err(|error| format!("{path}: {error}"))?;
    let vvd = read(fs, &format!("{stem}.vvd"))?;
    let vtx = read(fs, &format!("{stem}.dx90.vtx"))?;
    let model = Model::parse(&mdl, &vvd, &vtx).map_err(|error| format!("{path}: {error}"))?;
    let mut materials = Vec::with_capacity(model.materials.len());
    let mut resolved = BTreeMap::<String, String>::new();
    let mut work = 0;
    for name in &model.materials {
        let name = name.strip_suffix(".vmt").unwrap_or(name);
        if let Some(path) = resolved.get(name) {
            materials.push(path.clone());
            continue;
        }
        let candidates = model
            .material_directories
            .iter()
            .map(|directory| {
                if directory.is_empty() {
                    name.to_owned()
                } else {
                    format!("{directory}/{name}")
                }
            })
            .chain(std::iter::once(name.to_owned()));
        let mut path = name.to_owned();
        for candidate in candidates {
            work += 1;
            if work > MAX_MATERIAL_LOOKUPS {
                return Err("Model material lookup exceeds its work limit".into());
            }
            if fs
                .contains(&format!("materials/{candidate}.vmt"))
                .map_err(|error| error.to_string())?
            {
                path = candidate;
                break;
            }
        }
        resolved.insert(name.to_owned(), path.clone());
        materials.push(path);
    }
    Ok(Asset {
        path: path.into(),
        model,
        materials,
    })
}

impl Library {
    pub fn get(&mut self, fs: &GameFs, path: &str) -> Result<Arc<Asset>, String> {
        let path = model_path(path)?;
        self.serial = self.serial.wrapping_add(1);
        if let Some(entry) = self.entries.get_mut(&path) {
            entry.used = self.serial;
            return entry.result.clone();
        }
        let result = load(fs, &path)
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
            },
        );
        result
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
    pub skin: usize,
}

impl Instance {
    pub fn new(entity: &Entity, asset: Arc<Asset>) -> Result<Self, String> {
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
        if entity
            .properties
            .get("modelscale")
            .is_some_and(|value| value.parse::<f64>().ok() != Some(1.0))
        {
            return Err("Scaled model previews remain unsupported".into());
        }
        if entity
            .properties
            .get("body")
            .is_some_and(|value| value.parse::<u32>().ok() != Some(0))
        {
            return Err("Model previews currently use body 0".into());
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
            skin,
        };
        let (mins, maxs) = instance.bounds();
        if !mins.is_finite()
            || !maxs.is_finite()
            || mins.abs().max_element().max(maxs.abs().max_element()) > 65536.0
        {
            return Err("Transformed model bounds must fit ±65536".into());
        }
        Ok(instance)
    }

    pub fn point(&self, point: [f32; 3]) -> DVec3 {
        self.origin + self.rotation * DVec3::from_array(point.map(f64::from))
    }

    pub fn bounds(&self) -> (DVec3, DVec3) {
        self.bounds_in_basis(DMat3::IDENTITY)
    }

    pub fn bounds_in_basis(&self, basis: DMat3) -> (DVec3, DVec3) {
        let [mins, maxs] = self.asset.model.bounds;
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
        if !ray.origin.is_finite() {
            return None;
        }
        let world_ray = Ray {
            origin: ray.origin,
            direction: ray.direction.try_normalize()?,
        };
        let ray = &world_ray;
        let ray = Ray {
            origin: self.rotation.conjugate() * (ray.origin - self.origin),
            direction: self.rotation.conjugate() * ray.direction,
        };
        let model = &self.asset.model;
        if !map_core::query::ray_intersects_bounds(
            &ray,
            DVec3::from_array(model.bounds[0].map(f64::from)),
            DVec3::from_array(model.bounds[1].map(f64::from)),
        ) {
            return None;
        }
        let mut nearest: Option<f64> = None;
        for part in &model.parts {
            for indices in part.indices.as_chunks::<3>().0 {
                let points = std::array::from_fn(|axis| {
                    DVec3::from_array(
                        model.vertices[indices[axis] as usize]
                            .position
                            .map(f64::from),
                    )
                });
                if let Some(distance) = map_core::query::ray_triangle(&ray, points)
                    && nearest.is_none_or(|old| distance < old)
                {
                    nearest = Some(distance);
                }
            }
        }
        let distance = nearest?;
        Some(Hit {
            object: self.id,
            solid: None,
            side: None,
            distance,
            point: world_ray.origin + world_ray.direction * distance,
        })
    }
}
