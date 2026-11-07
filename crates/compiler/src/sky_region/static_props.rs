use std::collections::BTreeMap;
use std::io;

use glam::{DAffine3, DVec3};
use map_core::ObjectId;
use source_formats::bsp::StaticProp;
use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::model::{HardwareCharge, HardwareLimits, Model};
use source_formats::vmf::Entity;

use crate::Control;
use crate::asset_sources::{AssetSourceGuard, BuildMaterialSources, CapturedBuildAsset};

pub const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PARSE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_METADATA_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_WORK: usize = 4_000_000;
pub const MAX_VERTICES: usize = 32_768;
pub const MAX_LOGICAL_PATHS: usize = 64;
pub const MAX_PHYSICAL_PATHS: usize = 96;
pub const MAX_LOOKUP_ROWS: usize = 192;
pub const MAX_MATERIAL_BYTES: usize = 64 * 1024;
pub(super) const MAX_MATERIAL_PATH_BYTES: usize = 1_024;
const MAX_INSTANCES: usize = source_formats::bsp::MAX_STATIC_PROPS;
const MATERIAL_SCRATCH_BYTES: usize = MAX_MATERIAL_BYTES * 128;
pub const CPU_RESERVATION_BYTES: usize = MAX_SOURCE_BYTES * 2
    + MAX_PARSE_BYTES
    + MAX_METADATA_BYTES * 2
    + MATERIAL_SCRATCH_BYTES
    + MAX_INSTANCES
        * (std::mem::size_of::<Instance>() + std::mem::size_of::<super::SourceModel>() + 384);

pub(super) struct Capture<'a> {
    game: &'a GameFs,
    control: &'a Control,
    sources: BuildMaterialSources,
    parsed: HardwareCharge,
    models: BTreeMap<String, ModelEnvelope>,
    excluded: usize,
    instances: Vec<Instance>,
    vertices: usize,
}

impl<'a> Capture<'a> {
    pub(super) fn new(game: &'a GameFs, control: &'a Control) -> Self {
        Self {
            game,
            control,
            sources: BuildMaterialSources::bounded(
                MAX_SOURCE_BYTES,
                MAX_LOGICAL_PATHS,
                MAX_PHYSICAL_PATHS,
                MAX_LOOKUP_ROWS,
            ),
            parsed: HardwareCharge::default(),
            models: BTreeMap::new(),
            excluded: 0,
            instances: Vec::new(),
            vertices: 0,
        }
    }

    pub(super) fn admit(
        &mut self,
        entity: &Entity,
        admission: &mut super::Admission<'_>,
    ) -> Result<(), String> {
        crate::instances::admit_static_prop(entity)?;
        let origin = entity
            .origin()
            .ok_or("Static prop requires a finite origin")?;
        let path = entity
            .properties
            .get("model")
            .ok_or("Static prop requires model")?;
        if asset_relative(path).map_err(|error| error.to_string())? != path
            || path.len() >= source_formats::bsp::STATIC_PROP_NAME_BYTES
        {
            return Err("Static prop requires a canonical models/*.mdl path".into());
        }
        let basename = path
            .strip_suffix(".mdl")
            .ok_or("Static prop requires a .mdl model")?;
        if !self.models.contains_key(path) {
            let envelope = self.model(path, basename)?;
            self.charge(HardwareCharge {
                bytes: path.len() + 256,
                work: 1,
            })
            .map_err(|error| error.to_string())?;
            self.models.insert(path.to_owned(), envelope);
        }
        let skin = entity
            .properties
            .get("skin")
            .unwrap_or("0")
            .parse::<usize>()
            .map_err(|_| "Static prop requires a nonnegative skin")?;
        let envelope = &self.models[path];
        if skin >= envelope.model.skins.len() {
            return Err(format!(
                "{path}: static-prop skin exceeds the original model families"
            ));
        }
        let count = envelope.model.vertices.len();
        let parts = envelope.model.parts.len();
        let magnitude = envelope.magnitude;
        let transform = StaticProp {
            origin: origin.as_vec3().to_array(),
            angles: crate::static_props::angles(entity)?.as_vec3().to_array(),
            ..StaticProp::default()
        }
        .transform()
        .map_err(|error| error.to_string())?;
        self.charge(HardwareCharge {
            bytes: 0,
            work: 3 * 3usize.pow(4) + count * 32 + parts,
        })
        .map_err(|error| error.to_string())?;
        let mut bounds = rotation_envelope(magnitude, origin)?;
        for (index, vertex) in self.models[path].model.vertices.iter().enumerate() {
            if index.is_multiple_of(1_024) {
                self.control.checkpoint()?;
            }
            let point = transformed_position(vertex, transform)?;
            let packed = point.as_vec3().as_dvec3();
            bounds.0 = bounds.0.min(point).min(packed);
            bounds.1 = bounds.1.max(point).max(packed);
        }
        if admission.outside(bounds)? {
            self.excluded += 1;
        } else if admission.contains(bounds)? {
            let model = &self.models[path];
            for part in model.model.parts_for_body(0) {
                let material = selected_material(model, skin, part.material)?;
                if !material.ordinary {
                    return Err(format!(
                        "{}: model materials cannot use SKY, tool or nonordinary compile semantics",
                        material.name
                    ));
                }
            }
            if self.instances.len() >= MAX_INSTANCES {
                return Err("Sky region exceeded its inside static-prop count budget".into());
            }
            self.instances.push(Instance {
                entity: entity.id,
                path: path.to_owned(),
                skin,
                transform,
            });
        } else {
            return Err("complete authored/native model bounds cannot be certified inside or outside the selected sky_camera component".into());
        }
        Ok(())
    }

    fn model(&mut self, path: &str, basename: &str) -> Result<ModelEnvelope, String> {
        let mdl = self.required(path)?;
        let vvd = self.required(&format!("{basename}.vvd"))?;
        let vtx = self.required(&format!("{basename}.dx90.vtx"))?;
        let model = source_formats::model::validate_fixed_static_geometry(
            path,
            mdl.bytes(),
            vvd.bytes(),
            vtx.bytes(),
            HardwareLimits {
                vertices: MAX_VERTICES,
                sections: 4_096,
                work: MAX_WORK,
                bytes: MAX_PARSE_BYTES,
            },
            &mut |charge| self.charge(charge),
        )
        .map_err(|error| format!("{path}: {error}"))?;
        self.vertices = self
            .vertices
            .checked_add(model.vertices.len())
            .filter(|&vertices| vertices <= MAX_VERTICES)
            .ok_or("Sky region exceeded its static-prop vertex budget")?;
        self.charge(HardwareCharge {
            bytes: 0,
            work: model.vertices.len() * 12,
        })
        .map_err(|error| error.to_string())?;
        let materials = self.materials(&model)?;
        Ok(ModelEnvelope {
            magnitude: magnitude(&model, self.control)?,
            model,
            materials,
        })
    }

    fn charge(&mut self, charge: HardwareCharge) -> io::Result<()> {
        self.control.checkpoint().map_err(io::Error::other)?;
        self.parsed.bytes = self
            .parsed
            .bytes
            .checked_add(charge.bytes)
            .filter(|&bytes| bytes <= MAX_PARSE_BYTES)
            .ok_or_else(|| {
                io::Error::other("Sky static-prop aggregate parse byte budget exceeded")
            })?;
        self.parsed.work = self
            .parsed
            .work
            .checked_add(charge.work)
            .filter(|&work| work <= MAX_WORK)
            .ok_or_else(|| {
                io::Error::other("Sky static-prop aggregate parse work budget exceeded")
            })?;
        Ok(())
    }

    fn read(&mut self, path: &str, maximum: usize) -> io::Result<Option<CapturedBuildAsset>> {
        self.charge(HardwareCharge { bytes: 0, work: 1 })?;
        let asset = self.sources.read_asset(self.game, path, maximum)?;
        if let Some(asset) = &asset {
            self.charge(HardwareCharge {
                bytes: 0,
                work: asset.bytes().len().div_ceil(64),
            })?;
        }
        Ok(asset)
    }

    fn required(&mut self, path: &str) -> Result<CapturedBuildAsset, String> {
        self.read(path, MAX_SOURCE_BYTES)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Static-prop original asset is missing: {path}"))
    }

    fn materials(&mut self, model: &Model) -> Result<Vec<FixedMaterial>, String> {
        self.charge(HardwareCharge {
            bytes: model.materials.len() * std::mem::size_of::<FixedMaterial>(),
            work: model.materials.len(),
        })
        .map_err(|error| error.to_string())?;
        let mut materials = Vec::with_capacity(model.materials.len());
        for name in &model.materials {
            self.control.checkpoint()?;
            let mut found = false;
            for candidate in
                source_formats::model::material_candidates(name, &model.material_directories)
            {
                let path = format!("materials/{candidate}.vmt");
                let material = crate::lighting::load_prop_vertex_material(&path, &mut |path| {
                    self.read(path, MAX_MATERIAL_BYTES)
                        .map(|asset| asset.map(|asset| asset.bytes().to_vec()))
                })?;
                if let Some(material) = material {
                    if candidate.len() > MAX_MATERIAL_PATH_BYTES {
                        return Err(
                            "Sky model winning material path exceeded its byte budget".into()
                        );
                    }
                    self.charge(HardwareCharge {
                        bytes: candidate.len(),
                        work: 1,
                    })
                    .map_err(|error| error.to_string())?;
                    let semantics = material.compile_semantics();
                    materials.push(FixedMaterial {
                        name: candidate,
                        ordinary: !material.is_tool()
                            && semantics.draws
                            && semantics.contents == source_formats::bsp::contents::SOLID
                            && semantics.surface
                                & (source_formats::bsp::surf::SKY
                                    | source_formats::bsp::surf::SKY2D
                                    | source_formats::bsp::surf::WARP
                                    | source_formats::bsp::surf::NODRAW)
                                == 0
                            && semantics.unsupported_flags.is_empty(),
                    });
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(format!("Static-prop fixed material is missing: {name}"));
            }
        }
        Ok(materials)
    }

    pub(super) fn append(
        &mut self,
        include: &impl Fn(ObjectId) -> bool,
        surfaces: &mut Vec<super::Surface>,
        stats: &mut super::Stats,
        limits: super::Limits,
        admission: &mut super::Admission<'_>,
    ) -> Result<Vec<super::SourceModel>, String> {
        let mut sources = Vec::new();
        sources
            .try_reserve_exact(self.instances.len())
            .map_err(|error| format!("Sky model source allocation failed: {error}"))?;
        for instance in std::mem::take(&mut self.instances) {
            admission.consume(1)?;
            if !include(ObjectId::Entity(instance.entity)) {
                continue;
            }
            let cached = &self.models[&instance.path];
            admission.consume(cached.model.parts.len() * 2)?;
            let count = cached
                .model
                .parts_for_body(0)
                .try_fold(0usize, |count, part| {
                    if !part.indices.len().is_multiple_of(3) {
                        return Err("Sky model requires complete source triangle lists");
                    }
                    count
                        .checked_add(part.indices.len() / 3)
                        .ok_or("Sky model triangle count overflow")
                })?;
            let total = super::checked_total(
                surfaces.len(),
                count,
                limits.faces.faces,
                "model output surface",
            )?;
            if count == 0 {
                return Err(format!(
                    "entity:{}: sky model has no authored LOD0 triangles",
                    instance.entity
                ));
            }
            stats.vertices =
                super::checked_total(stats.vertices, count * 3, limits.vertices, "output vertex")?;
            surfaces
                .try_reserve_exact(total - surfaces.len())
                .map_err(|error| format!("Sky model output allocation failed: {error}"))?;
            for part in cached.model.parts_for_body(0) {
                let material = selected_material(cached, instance.skin, part.material)?;
                for triangle in part.indices.as_chunks::<3>().0 {
                    admission.consume(3 * 32 + material.name.len().div_ceil(64))?;
                    let mut points = Vec::with_capacity(3);
                    let mut normals = Vec::with_capacity(3);
                    let mut uvs = Vec::with_capacity(3);
                    for &index in triangle {
                        let vertex = cached
                            .model
                            .vertices
                            .get(index as usize)
                            .ok_or("Sky model lost its validated vertex mapping")?;
                        let (point, normal) = transformed(vertex, instance.transform)
                            .map_err(|error| format!("entity:{}: {error}", instance.entity))?;
                        points.push(point);
                        normals.push(normal);
                        uvs.push(vertex.uv);
                    }
                    let normal = (points[2] - points[0]).cross(points[1] - points[0]);
                    let packed =
                        [points[0], points[1], points[2]].map(|point| point.as_vec3().as_dvec3());
                    let packed_normal = (packed[2] - packed[0]).cross(packed[1] - packed[0]);
                    if !normal.is_finite()
                        || normal.length_squared() < 1e-12
                        || !packed_normal.is_finite()
                        || packed_normal.length_squared() < 1e-12
                        || normal.dot(packed_normal) <= 0.0
                    {
                        return Err(format!(
                            "entity:{}: sky model triangle collapses or reverses in display precision",
                            instance.entity
                        ));
                    }
                    surfaces.push(super::Surface {
                        vertices: points,
                        normal,
                        material: material.name.clone(),
                        texture_vectors: [[0.0; 4]; 2],
                        texture_points: None,
                        normals: Some(normals),
                        normalized_uvs: Some(uvs),
                        source_model: Some(instance.entity),
                        flags: 0,
                        source_sides: Vec::new(),
                    });
                }
            }
            stats.inside_static_props += 1;
            stats.static_prop_triangles += count;
            sources.push(super::SourceModel {
                entity: instance.entity,
                model: instance.path,
                skin: instance.skin,
            });
        }
        stats.surfaces = surfaces.len();
        Ok(sources)
    }

    pub(super) fn finish(
        self,
        stats: &mut super::Stats,
    ) -> Result<Option<AssetSourceGuard>, String> {
        let guard = if self.models.is_empty() {
            None
        } else {
            let guard = self
                .sources
                .source_guard(self.game, self.control)
                .map_err(|error| error.to_string())?;
            guard
                .verify_original_metadata(self.control, MAX_METADATA_BYTES)
                .map_err(|error| error.to_string())?;
            Some(guard)
        };
        stats.excluded_static_props = self.excluded;
        stats.static_prop_models = self.models.len();
        stats.static_prop_vertices = self.vertices;
        Ok(guard)
    }
}

struct ModelEnvelope {
    magnitude: f32,
    model: Model,
    materials: Vec<FixedMaterial>,
}

struct FixedMaterial {
    name: String,
    ordinary: bool,
}

struct Instance {
    entity: u32,
    path: String,
    skin: usize,
    transform: DAffine3,
}

fn selected_material(
    model: &ModelEnvelope,
    skin: usize,
    material: usize,
) -> Result<&FixedMaterial, String> {
    let index = model
        .model
        .skins
        .get(skin)
        .and_then(|skin| skin.get(material))
        .ok_or("Sky model lost its selected skin mapping")?;
    model
        .materials
        .get(*index)
        .ok_or_else(|| "Sky model lost its winning material path".into())
}

fn transformed(
    vertex: &source_formats::model::Vertex,
    transform: DAffine3,
) -> Result<(DVec3, DVec3), String> {
    let point = transformed_position(vertex, transform)?;
    let normal = transform.transform_vector3(DVec3::from_array(vertex.normal.map(f64::from)));
    let length = normal.length_squared();
    if point.abs().max_element() > crate::map::MAX_COMPILE_COORD
        || !normal.is_finite()
        || !length.is_finite()
        || length == 0.0
        || vertex.uv.iter().any(|value| !value.is_finite())
    {
        return Err(
            "Sky model requires finite transformed positions, normals and normalized UVs".into(),
        );
    }
    let normal = normal.normalize();
    let packed = normal.as_vec3();
    if !packed.is_finite() || packed.length_squared() == 0.0 {
        return Err("Sky model normal collapses in display precision".into());
    }
    Ok((point, normal))
}

fn transformed_position(
    vertex: &source_formats::model::Vertex,
    transform: DAffine3,
) -> Result<DVec3, String> {
    let point = transform.transform_point3(DVec3::from_array(vertex.position.map(f64::from)));
    if !point.is_finite() || !point.as_vec3().is_finite() {
        return Err(
            "Sky model transformed positions must fit finite world and display coordinates".into(),
        );
    }
    Ok(point)
}

fn magnitude(model: &Model, control: &Control) -> Result<f32, String> {
    if model.vertices.is_empty() {
        return Err("Static-prop geometry has no validated vertices".into());
    }
    let mut magnitude = f32::MIN_POSITIVE;
    for (index, vertex) in model.vertices.iter().enumerate() {
        if index.is_multiple_of(1_024) {
            control.checkpoint()?;
        }
        for value in vertex.position {
            while magnitude < value.abs() {
                magnitude *= 2.0;
            }
        }
    }
    Ok(magnitude)
}

fn rotation_envelope(magnitude: f32, origin: DVec3) -> Result<(DVec3, DVec3), String> {
    // Native AngleMatrix coefficients are products or sums of two products of
    // sincosf outputs in [-1, 1]. A power-of-two magnitude bounds each rounded
    // transform product by 2M; every sum association includes directed rounding.
    let term = Interval {
        low: -2.0 * magnitude,
        high: 2.0 * magnitude,
    };
    let radius = 6.0 * f64::from(magnitude);
    let native_origin = origin.as_vec3();
    let mut mins = DVec3::ZERO;
    let mut maxs = DVec3::ZERO;
    for axis in 0..3 {
        let native = sum_envelope(term, native_origin[axis])?;
        mins[axis] = (origin[axis] - radius)
            .next_down()
            .min(f64::from(native.low));
        maxs[axis] = (origin[axis] + radius)
            .next_up()
            .max(f64::from(native.high));
    }
    if !mins.is_finite() || !maxs.is_finite() {
        return Err("Static-prop geometry exceeded finite world bounds".into());
    }
    Ok((mins, maxs))
}

#[derive(Clone, Copy)]
struct Interval {
    low: f32,
    high: f32,
}

impl Interval {
    fn union(self, other: Self) -> Self {
        Self {
            low: self.low.min(other.low),
            high: self.high.max(other.high),
        }
    }

    fn add(self, other: Self) -> Self {
        let low = f64::from(self.low) + f64::from(other.low);
        let high = f64::from(self.high) + f64::from(other.high);
        let mut interval = Self {
            low: (low as f32).next_down(),
            high: (high as f32).next_up(),
        };
        if low < f64::from(f32::MIN_POSITIVE) && high > -f64::from(f32::MIN_POSITIVE) {
            interval = interval.union(Self {
                low: 0.0,
                high: 0.0,
            });
        }
        interval
    }
}

fn sum_envelope(term: Interval, origin: f32) -> Result<Interval, String> {
    let origin = if origin.is_subnormal() {
        Interval {
            low: origin.min(0.0),
            high: origin.max(0.0),
        }
    } else {
        Interval {
            low: origin,
            high: origin,
        }
    };
    let mut sums = [None; 16];
    sums[1] = Some(term);
    sums[2] = Some(term);
    sums[4] = Some(term);
    sums[8] = Some(origin);
    for mask in 1usize..16 {
        if mask.is_power_of_two() {
            continue;
        }
        let mut left = (mask - 1) & mask;
        while left != 0 {
            let right = mask ^ left;
            if left < right
                && let (Some(a), Some(b)) = (sums[left], sums[right])
            {
                let interval = a.add(b);
                sums[mask] = Some(sums[mask].map_or(interval, |current| current.union(interval)));
            }
            left = (left - 1) & mask;
        }
    }
    sums[15].ok_or_else(|| "Static-prop transform interval is incomplete".into())
}
