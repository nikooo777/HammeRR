use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use glam::{DQuat, DVec2, DVec3, EulerRot};
use source_formats::{
    bsp::{Bsp, StaticProp},
    gamefs::GameFs,
    model::Model,
    vmf::Vmf,
    vmt::Material,
};

use super::asset_tracking::Tracker;
use super::bake::{Allocation, Budget};
use super::shadows::{ShadowIndex, ShadowObject, Triangle};
use crate::{Diagnostic, Report, map::MAX_COMPILE_COORD};

const MAX_TRIANGLES: usize = 262_144;
const MAX_ASSET_BYTES: usize = 256 * 1024 * 1024;
const MAX_MATERIAL_BYTES: usize = 64 * 1024;
const MAX_MATERIAL_LOOKUPS: usize = 65_536;
const MAX_TRACKING_BYTES: usize = 8 * MAX_MATERIAL_LOOKUPS * size_of::<Allocation>();
const NO_SHADOW: u32 = 0x10;
const CAST_TEXTURE_SHADOWS: u32 = 0x0004_0000;

mod alpha;
#[cfg(test)]
pub(super) use alpha::fractional_triangle;

#[derive(Debug, Default)]
pub(crate) struct Stats {
    pub props: usize,
    pub models: usize,
    pub triangles: usize,
    pub disabled: usize,
    pub work: usize,
    pub alpha_triangles: usize,
    pub alpha_materials: usize,
    pub alpha_textures: usize,
    pub alpha_bytes: usize,
}

pub(super) struct PropTriangle {
    triangle: Triangle,
    owner: Option<u32>,
    alpha: Option<(Arc<alpha::Material>, [DVec2; 3])>,
}

impl PropTriangle {
    pub(super) fn new(points: [DVec3; 3]) -> Result<Self, String> {
        Ok(Self {
            triangle: Triangle::new(points)?,
            owner: None,
            alpha: None,
        })
    }

    pub(super) fn set_owner(&mut self, owner: u32) {
        self.owner = Some(owner);
    }

    pub(super) fn point_contact(&self, point: DVec3, receiver: u32) -> Result<bool, String> {
        if self.owner == Some(receiver) {
            return Ok(false);
        }
        self.triangle.point_contact(point)
    }

    fn fraction(&self, start: DVec3, end: DVec3) -> Result<Option<f64>, String> {
        let Some((fraction, weights)) = self.triangle.intersection(start, end)? else {
            return Ok(None);
        };
        let Some((material, coordinates)) = &self.alpha else {
            return Ok(Some(fraction));
        };
        let uv = coordinates
            .iter()
            .zip(weights)
            .map(|(point, weight)| *point * weight)
            .sum::<DVec2>();
        Ok(material.blocks(uv)?.then_some(fraction))
    }

    fn coverage(&self, start: DVec3, end: DVec3) -> Result<u8, String> {
        let Some((_, weights)) = self.triangle.intersection(start, end)? else {
            return Ok(0);
        };
        let Some((material, coordinates)) = &self.alpha else {
            return Ok(255);
        };
        let uv = coordinates
            .iter()
            .zip(weights)
            .map(|(point, weight)| *point * weight)
            .sum();
        material.coverage(uv)
    }
}

impl ShadowObject for PropTriangle {
    const MAX_OBJECTS: usize = MAX_TRIANGLES;

    fn bounds(&self) -> (DVec3, DVec3) {
        self.triangle.bounds()
    }

    fn work(&self) -> usize {
        self.triangle.work() + if self.alpha.is_some() { 24 } else { 0 }
    }

    fn intersects(&self, start: DVec3, end: DVec3) -> Result<bool, String> {
        Ok(self.fraction(start, end)?.is_some())
    }

    fn fractional(&self) -> bool {
        self.alpha
            .as_ref()
            .is_some_and(|(material, _)| material.fractional())
    }
}

impl ShadowIndex<PropTriangle> {
    pub(super) fn visibility_skipping_owner(
        &self,
        start: DVec3,
        end: DVec3,
        skip_owner: Option<u32>,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<f64, String> {
        if !self.has_fractional() {
            return Ok(
                if self.blocked_skipping_owner(start, end, skip_owner, consume)? {
                    0.0
                } else {
                    1.0
                },
            );
        }
        let mut coverage = 0_u8;
        self.visit(start, end, consume, &mut |triangle| {
            if skip_owner.is_none() || triangle.owner != skip_owner {
                coverage = coverage.saturating_add(triangle.coverage(start, end)?);
            }
            Ok(if coverage == 255 {
                super::shadows::Visit::Stop
            } else {
                super::shadows::Visit::Continue(1.0)
            })
        })?;
        Ok(f64::from(255 - coverage) / 255.0)
    }

    pub(super) fn blocked_skipping_owner(
        &self,
        start: DVec3,
        end: DVec3,
        skip_owner: Option<u32>,
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<bool, String> {
        let Some(owner) = skip_owner else {
            return self.blocked(start, end, consume);
        };
        Ok(self
            .nearest(start, end, consume, &mut |triangle| {
                if triangle.owner == Some(owner) {
                    return Ok(None);
                }
                Ok(triangle
                    .fraction(start, end)?
                    .map(|fraction| (fraction, ())))
            })?
            .is_some())
    }
}

pub(super) struct ProbeBounds {
    mins: DVec3,
    maxs: DVec3,
}

impl ShadowObject for ProbeBounds {
    const MAX_OBJECTS: usize = source_formats::bsp::MAX_STATIC_PROPS;

    fn bounds(&self) -> (DVec3, DVec3) {
        (self.mins, self.maxs)
    }

    fn work(&self) -> usize {
        1
    }

    fn intersects(&self, start: DVec3, end: DVec3) -> Result<bool, String> {
        if start != end {
            return Err("Prop probe bounds require a point query".into());
        }
        Ok(start.cmpge(self.mins).all() && start.cmple(self.maxs).all())
    }
}

pub(super) struct Prepared {
    pub shadows: ShadowIndex<PropTriangle>,
    pub bounds: ShadowIndex<ProbeBounds>,
    pub stats: Stats,
    pub allocations: Vec<Allocation>,
}

struct Buffers {
    triangles: Vec<PropTriangle>,
    bounds: Vec<ProbeBounds>,
    allocation: Allocation,
}

impl Buffers {
    fn new(props: usize, budget: &Budget<'_>) -> Result<Self, String> {
        if props > source_formats::bsp::MAX_STATIC_PROPS {
            return Err("Static-prop probe bounds exceed the prop allocation limit".into());
        }
        let triangles = if props == 0 { 0 } else { MAX_TRIANGLES };
        let bytes = ShadowIndex::<PropTriangle>::allocation_bytes(triangles)?
            .checked_add(ShadowIndex::<ProbeBounds>::allocation_bytes(props)?)
            .and_then(|bytes| bytes.checked_add(MAX_TRACKING_BYTES))
            .ok_or("Lighting prop shadow allocation size overflow")?;
        let allocation = budget.reserve_bytes(bytes)?;
        Ok(Self {
            triangles: Vec::with_capacity(triangles),
            bounds: Vec::with_capacity(props),
            allocation,
        })
    }
}

struct Assets<'fs, 'tracking> {
    fs: &'fs GameFs,
    tracking: Option<&'tracking mut Tracker<'fs>>,
    bytes: usize,
    lookups: usize,
    textures: BTreeMap<String, Arc<alpha::Texture>>,
    alpha_bytes: usize,
    allocations: Vec<Allocation>,
}

impl Assets<'_, '_> {
    fn read(&mut self, path: &str, budget: &mut Budget<'_>) -> Result<Vec<u8>, String> {
        let limit = source_formats::mdl::MAX_MODEL_BYTES.min(MAX_ASSET_BYTES - self.bytes);
        let data = if let Some(tracking) = &mut self.tracking {
            tracking.read(path, limit, budget)?
        } else {
            budget.consume(1)?;
            let data = self
                .fs
                .read_limited(path, limit)
                .map_err(|error| format!("{path}: {error}"))?;
            if let Some(data) = &data {
                budget.consume(data.len().div_ceil(64))?;
            }
            data
        }
        .ok_or_else(|| format!("Static-prop shadow asset {path} was not found"))?;
        self.bytes += data.len();
        Ok(data)
    }

    fn model(&mut self, path: &str, budget: &mut Budget<'_>) -> Result<Model, String> {
        let stem = path
            .strip_suffix(".mdl")
            .ok_or("Static-prop model must end in .mdl")?;
        let mdl = self.read(path, budget)?;
        let vvd = self.read(&format!("{stem}.vvd"), budget)?;
        let vtx = self.read(&format!("{stem}.dx90.vtx"), budget)?;
        budget.consume(1)?;
        let model = Model::parse(&mdl, &vvd, &vtx)
            .map_err(|error| format!("Static-prop shadow model {path}: {error}"))?;
        budget.consume(
            model.vertices.len()
                + model
                    .parts
                    .iter()
                    .map(|part| part.indices.len())
                    .sum::<usize>(),
        )?;
        Ok(model)
    }

    fn material(
        &mut self,
        model: &Model,
        index: usize,
        budget: &mut Budget<'_>,
    ) -> Result<Option<Material>, String> {
        let name = &model.materials[index];
        let name = name.strip_suffix(".vmt").unwrap_or(name);
        for candidate in
            source_formats::model::material_candidates(name, &model.material_directories)
        {
            budget.consume(1)?;
            self.lookups += 1;
            if self.lookups > MAX_MATERIAL_LOOKUPS {
                return Err(
                    "Static-prop shadow material lookup exceeds its 65,536 lookup limit".into(),
                );
            }
            let material = if let Some(tracking) = &mut self.tracking {
                Material::load_with_reader(
                    &source_formats::vmt::material_path(&candidate),
                    &mut |path| {
                        tracking
                            .read(path, MAX_MATERIAL_BYTES, budget)
                            .map_err(std::io::Error::other)
                    },
                )
            } else {
                Material::load_limited(self.fs, &candidate, MAX_MATERIAL_BYTES)
            }
            .map_err(|error| format!("Static-prop shadow material {candidate}: {error}"))?;
            if let Some(material) = material {
                return Ok(Some(material));
            }
        }
        Ok(None)
    }
}

fn needs_material_warning(material: &Material) -> bool {
    !matches!(
        material.shader.as_str(),
        "vertexlitgeneric" | "unlitgeneric" | "lightmappedgeneric"
    ) || material.translucent()
        || material.alpha_test()
        || material
            .get("$alpha")
            .is_some_and(|value| value.trim().parse::<f64>().is_ok_and(|alpha| alpha < 1.0))
        || material
            .get("$additive")
            .is_some_and(|value| value.trim() != "0")
        || !material.blocks.is_empty()
}

fn append(
    model: &Model,
    prop: &StaticProp,
    materials: &BTreeMap<usize, Option<Arc<alpha::Material>>>,
    triangles: &mut Vec<PropTriangle>,
    budget: &mut Budget<'_>,
) -> Result<usize, String> {
    let angles = DVec3::from_array(prop.angles.map(f64::from)).map(f64::to_radians);
    let rotation = DQuat::from_euler(EulerRot::ZYX, angles.y, angles.x, angles.z);
    let origin = DVec3::from_array(prop.origin.map(f64::from));
    let mut degenerate = 0;
    let skin = model
        .skins
        .get(prop.skin as usize)
        .ok_or("Static-prop shadow skin is unavailable")?;
    for part in model.parts_for_body(0) {
        let material = *skin
            .get(part.material)
            .ok_or("Static-prop shadow skin material is unavailable")?;
        for indices in part.indices.as_chunks::<3>().0 {
            budget.consume(4)?;
            let points = std::array::from_fn(|corner| {
                origin
                    + rotation
                        * DVec3::from_array(
                            model.vertices[indices[corner] as usize]
                                .position
                                .map(f64::from),
                        )
            });
            if points
                .iter()
                .any(|point| !point.is_finite() || point.abs().max_element() > MAX_COMPILE_COORD)
            {
                return Err(format!(
                    "Static-prop shadow vertices must fit ±{MAX_COMPILE_COORD} units"
                ));
            }
            if (points[1] - points[0])
                .cross(points[2] - points[0])
                .length_squared()
                == 0.0
            {
                degenerate += 1;
                continue;
            }
            if triangles.len() >= MAX_TRIANGLES {
                return Err("Static-prop shadows exceed the 262,144 world-space triangle limit; disable shadows on unnecessary props".into());
            }
            let mut triangle = PropTriangle::new(points)?;
            if let Some(Some(alpha)) = materials.get(&material) {
                let coordinates = indices.map(|index| {
                    DVec2::from_array(model.vertices[index as usize].uv.map(f64::from))
                });
                triangle.alpha = Some((Arc::clone(alpha), coordinates));
            }
            triangles.push(triangle);
        }
    }
    Ok(degenerate)
}

pub(super) fn build<'a>(
    bsp: &Bsp,
    vmf: &Vmf,
    fs: &'a GameFs,
    tracking: Option<&mut Tracker<'a>>,
    generate_reflections: bool,
    report: &mut Report,
    budget: &mut Budget<'_>,
) -> Result<Prepared, String> {
    let mut triangles = Vec::new();
    let mut probe_bounds = Vec::new();
    let mut stats = Stats::default();
    let mut allocations = Vec::new();
    let start_work = budget.work();
    if let Some(props) = bsp.static_props().map_err(|error| error.to_string())? {
        let entities: Vec<_> = vmf
            .entities
            .iter()
            .filter(|entity| {
                !entity.hidden
                    && entity
                        .classname()
                        .is_some_and(|name| name.eq_ignore_ascii_case("prop_static"))
            })
            .collect();
        if entities.len() != props.props.len() {
            return Err("Static-prop shadow source records do not match emitted props".into());
        }
        let mut assets = Assets {
            fs,
            tracking,
            bytes: 0,
            lookups: 0,
            textures: BTreeMap::new(),
            alpha_bytes: 0,
            allocations: Vec::new(),
        };
        stats.disabled = props
            .props
            .iter()
            .filter(|prop| prop.flags & NO_SHADOW != 0)
            .count();
        let shadow_props = props.props.len() - stats.disabled;
        if shadow_props > 0 {
            let buffers = Buffers::new(shadow_props, budget)?;
            triangles = buffers.triangles;
            probe_bounds = buffers.bounds;
            allocations.push(buffers.allocation);
        }
        let mut by_model = vec![Vec::new(); props.models.len()];
        for (index, prop) in props.props.iter().enumerate() {
            budget.consume(1)?;
            if prop.flags & NO_SHADOW == 0 {
                by_model[usize::from(prop.model)].push(index);
            }
        }
        for (model_index, instances) in by_model
            .iter()
            .enumerate()
            .filter(|(_, instances)| !instances.is_empty())
        {
            budget.consume(1)?;
            let path = props
                .model_name(model_index)
                .map_err(|error| error.to_string())?;
            let first = entities[instances[0]];
            let model = match assets.model(path, budget) {
                Ok(model) => model,
                Err(message) => {
                    budget.consume(0)?;
                    report.diagnostics.push(Diagnostic::error(
                        message,
                        Some(format!("entity:{}", first.id)),
                        first.origin(),
                    ));
                    return Err("Static-prop mesh shadow preparation failed".into());
                }
            };
            stats.models += 1;
            let mut materials = BTreeMap::new();
            let mut reflective_materials = BTreeSet::new();
            for &index in instances {
                let entity = entities[index];
                let prop = &props.props[index];
                let result = (|| {
                    let skin = model
                        .skins
                        .get(prop.skin as usize)
                        .ok_or("Static-prop shadow skin is unavailable")?;
                    let mut reflection_material = None;
                    for part in model.parts_for_body(0) {
                        budget.consume(1)?;
                        let material = *skin
                            .get(part.material)
                            .ok_or("Static-prop shadow skin material is unavailable")?;
                        if let std::collections::btree_map::Entry::Vacant(entry) =
                            materials.entry(material)
                        {
                            let loaded = assets.material(&model, material, budget)?;
                            if !generate_reflections
                                && loaded.as_ref().is_some_and(|material| {
                                    material.get("$envmap").is_some_and(|value| {
                                        value.trim().eq_ignore_ascii_case("env_cubemap")
                                    })
                                })
                            {
                                reflective_materials.insert(material);
                            }
                            let cutout = if model.header.flags & CAST_TEXTURE_SHADOWS != 0 {
                                let loaded = loaded.as_ref().ok_or_else(|| format!(
                                    "{path}: requested texture-shadow material {} was not found", model.materials[material]
                                ))?;
                                alpha::prepare(loaded, &mut assets, budget).map_err(|error| {
                                    format!(
                                        "{path}: texture-shadow material {}: {error}",
                                        model.materials[material]
                                    )
                                })?
                            } else {
                                match loaded {
                                Some(material) if !needs_material_warning(&material) => {}
                                Some(_) => report.diagnostics.push(Diagnostic::warning(
                                    format!("{path}: static-prop shadows use opaque mesh triangles; texture alpha, translucency, nonstandard shaders and material animation/conditional blocks are not evaluated"),
                                    Some(format!("entity:{}", entity.id)), entity.origin())),
                                None => report.diagnostics.push(Diagnostic::warning(
                                    format!("{path}: shadow material {} was not found; mesh casts opaque shadows", model.materials[material]),
                                    Some(format!("entity:{}", entity.id)), entity.origin())),
                                }
                                None
                            };
                            if cutout.is_some() {
                                stats.alpha_materials += 1;
                            }
                            entry.insert(cutout);
                        }
                        if reflective_materials.contains(&material) {
                            reflection_material.get_or_insert(material);
                        }
                    }
                    if let Some(material) = reflection_material {
                        report.diagnostics.push(Diagnostic::warning(
                            format!("{path}: material {} declares $envmap env_cubemap, but Generate reflections is disabled. Review or place env_cubemap samples and enable Generate reflections in Build settings to capture map-local reflections", model.materials[material]),
                            Some(format!("entity:{}", entity.id)),
                            entity.origin(),
                        ));
                    }
                    let before = triangles.len();
                    let degenerate = append(&model, prop, &materials, &mut triangles, budget)?;
                    if triangles.len() > before {
                        budget.consume(triangles.len() - before)?;
                        let mut mins = DVec3::splat(f64::INFINITY);
                        let mut maxs = DVec3::splat(f64::NEG_INFINITY);
                        for triangle in &mut triangles[before..] {
                            triangle.set_owner(index as u32);
                            let (low, high) = triangle.bounds();
                            mins = mins.min(low);
                            maxs = maxs.max(high);
                        }
                        probe_bounds.push(ProbeBounds { mins, maxs });
                        stats.props += 1;
                    }
                    if degenerate > 0 {
                        report.diagnostics.push(Diagnostic::warning(
                            format!("{path}: ignored {degenerate} collapsed shadow triangles"),
                            Some(format!("entity:{}", entity.id)),
                            entity.origin(),
                        ));
                    }
                    Ok::<_, String>(())
                })();
                if let Err(message) = result {
                    budget.consume(0)?;
                    report.diagnostics.push(Diagnostic::error(
                        message,
                        Some(format!("entity:{}", entity.id)),
                        entity.origin(),
                    ));
                    return Err("Static-prop mesh shadow preparation failed".into());
                }
            }
        }
        stats.alpha_textures = assets.textures.len();
        stats.alpha_bytes = assets.alpha_bytes;
        allocations.append(&mut assets.allocations);
    }
    stats.triangles = triangles.len();
    stats.alpha_triangles = triangles
        .iter()
        .filter(|triangle| triangle.alpha.is_some())
        .count();
    let index = ShadowIndex::new(triangles, &mut |amount| budget.consume(amount))?;
    let probe_bounds = ShadowIndex::new(probe_bounds, &mut |amount| budget.consume(amount))?;
    stats.work = budget.work() - start_work;
    if stats.props > 0 {
        report.diagnostics.push(Diagnostic::info(format!("Static-prop mesh shadows: {} props, {} models, {} LOD0 triangles; {} props disable shadows. {} triangles use {} alpha-texture materials and {} textures ({} decoded alpha bytes). Two-sided opaque, cutout and requested translucent triangles attenuate direct, sky, bounced and ambient rays. Texture shadows use mip0 nearest texels; translucent coverage adds and saturates per intersected triangle. Reflected prop sources and animated props remain unfinished", stats.props, stats.models, stats.triangles, stats.disabled, stats.alpha_triangles, stats.alpha_materials, stats.alpha_textures, stats.alpha_bytes), None, None));
    }
    Ok(Prepared {
        shadows: index,
        bounds: probe_bounds,
        stats,
        allocations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Control;
    use source_formats::model::{Part, Vertex};

    fn triangle_model() -> Model {
        let header = source_formats::mdl::ModelHeader {
            checksum: 0,
            flags: source_formats::mdl::STATIC_PROP_FLAG,
            hull_mins: [-100.0; 3],
            hull_maxs: [100.0; 3],
            render_mins: [-100.0; 3],
            render_maxs: [100.0; 3],
            illumination: [0.0; 3],
            skins: 1,
        };
        Model {
            header,
            vertices: [[-8.0, -8.0, 0.0], [8.0, -8.0, 0.0], [0.0, 8.0, 0.0]]
                .map(|position| Vertex {
                    position,
                    normal: [0.0, 0.0, 1.0],
                    uv: [0.0; 2],
                })
                .to_vec(),
            parts: vec![Part {
                material: 0,
                indices: vec![0, 1, 2],
            }],
            materials: vec!["test".into()],
            material_directories: Vec::new(),
            skins: vec![vec![0]],
            bounds: [[-8.0; 3], [8.0; 3]],
            body_parts: 1,
            reference_bones: 0,
            body_groups: Vec::new(),
            attachments: Vec::new(),
        }
    }

    #[test]
    fn prop_buffers_admit_full_retained_capacities_before_growth() {
        let control = Control::default();
        let budget = Budget::new(&control);
        let buffers = Buffers::new(1, &budget).unwrap();
        assert_eq!(buffers.triangles.capacity(), MAX_TRIANGLES);
        assert_eq!(buffers.bounds.capacity(), 1);
        assert!(
            budget.live_bytes()
                >= buffers.triangles.capacity() * size_of::<PropTriangle>()
                    + buffers.bounds.capacity() * size_of::<ProbeBounds>()
        );
        let required = budget.live_bytes();
        drop(buffers);
        assert_eq!(budget.live_bytes(), 0);
        let held = budget
            .reserve_bytes(super::super::bake::MAX_LIGHTING_LIVE_BYTES - required + 1)
            .unwrap();
        let before = budget.live_bytes();
        assert!(Buffers::new(1, &budget).is_err());
        assert_eq!(budget.live_bytes(), before);
        drop(held);
        control.cancel().unwrap();
        assert!(Buffers::new(1, &budget).is_err());
    }

    #[test]
    fn prop_mesh_transform_two_sided_rays_and_endpoints_are_exact() {
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let mut triangles = Vec::new();
        let prop = StaticProp {
            origin: [32.0, 16.0, 64.0],
            angles: [90.0, 0.0, 0.0],
            ..Default::default()
        };
        append(
            &triangle_model(),
            &prop,
            &BTreeMap::new(),
            &mut triangles,
            &mut budget,
        )
        .unwrap();
        let index = ShadowIndex::new(triangles, &mut |work| budget.consume(work)).unwrap();
        let middle = DVec3::new(32.0, 16.0, 64.0);
        for (start, end, expected) in [
            (middle - DVec3::X * 16.0, middle + DVec3::X * 16.0, true),
            (middle + DVec3::X * 16.0, middle - DVec3::X * 16.0, true),
            (middle, middle + DVec3::X * 16.0, false),
            (middle - DVec3::X * 16.0, middle, false),
            (
                middle + DVec3::Y * 64.0 - DVec3::X * 16.0,
                middle + DVec3::Y * 64.0 + DVec3::X * 16.0,
                false,
            ),
        ] {
            assert_eq!(
                index
                    .blocked(start, end, &mut |work| budget.consume(work))
                    .unwrap(),
                expected
            );
        }
        control.cancel().unwrap();
        assert!(
            index
                .blocked(middle - DVec3::X, middle + DVec3::X, &mut |work| budget
                    .consume(work))
                .is_err()
        );
    }

    #[test]
    fn mesh_preparation_rejects_world_bounds_and_limits_and_checks_cancellation() {
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let mut triangles = Vec::new();
        let mut model = triangle_model();
        model.vertices[0].position = [100_000.0, 0.0, 0.0];
        assert!(
            append(
                &model,
                &StaticProp::default(),
                &BTreeMap::new(),
                &mut triangles,
                &mut budget
            )
            .unwrap_err()
            .contains("vertices must fit")
        );
        assert!(triangles.is_empty());
        let mut model = triangle_model();
        model.parts[0].indices = [0, 1, 2].repeat(MAX_TRIANGLES + 1);
        assert!(
            append(
                &model,
                &StaticProp::default(),
                &BTreeMap::new(),
                &mut triangles,
                &mut budget
            )
            .unwrap_err()
            .contains("triangle limit")
        );
        assert_eq!(triangles.len(), MAX_TRIANGLES);
        triangles.clear();
        control.cancel().unwrap();
        assert!(
            append(
                &triangle_model(),
                &StaticProp::default(),
                &BTreeMap::new(),
                &mut triangles,
                &mut budget
            )
            .is_err()
        );
        assert!(triangles.is_empty());
    }

    #[test]
    fn collapsed_mesh_faces_are_counted_without_becoming_box_shadows() {
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let mut model = triangle_model();
        model.vertices[2].position = [0.0, -8.0, 0.0];
        let mut triangles = Vec::new();
        assert_eq!(
            append(
                &model,
                &StaticProp::default(),
                &BTreeMap::new(),
                &mut triangles,
                &mut budget
            )
            .unwrap(),
            1
        );
        assert!(triangles.is_empty());
    }
}
