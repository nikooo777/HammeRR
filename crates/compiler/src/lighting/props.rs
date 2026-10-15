use std::collections::BTreeSet;

use glam::{DQuat, DVec3, EulerRot};
use source_formats::{
    bsp::{Bsp, StaticProp},
    gamefs::GameFs,
    model::Model,
    vmf::Vmf,
    vmt::Material,
};

use super::bake::Budget;
use super::shadows::{ShadowIndex, ShadowObject, Triangle};
use crate::{Diagnostic, Report, map::MAX_COMPILE_COORD};

const MAX_TRIANGLES: usize = 262_144;
const MAX_ASSET_BYTES: usize = 256 * 1024 * 1024;
const MAX_MATERIAL_BYTES: usize = 64 * 1024;
const MAX_MATERIAL_LOOKUPS: usize = 65_536;
const NO_SHADOW: u32 = 0x10;
const CAST_TEXTURE_SHADOWS: u32 = 0x0004_0000;

#[derive(Debug, Default)]
pub(crate) struct Stats {
    pub props: usize,
    pub models: usize,
    pub triangles: usize,
    pub disabled: usize,
    pub work: usize,
}

pub(super) struct PropTriangle(Triangle);

impl PropTriangle {
    pub(super) fn new(points: [DVec3; 3]) -> Result<Self, String> {
        Triangle::new(points).map(Self)
    }
}

impl ShadowObject for PropTriangle {
    const MAX_OBJECTS: usize = MAX_TRIANGLES;

    fn bounds(&self) -> (DVec3, DVec3) {
        self.0.bounds()
    }

    fn work(&self) -> usize {
        self.0.work()
    }

    fn intersects(&self, start: DVec3, end: DVec3) -> Result<bool, String> {
        self.0.intersects(start, end)
    }
}

struct Assets<'a> {
    fs: &'a GameFs,
    bytes: usize,
    lookups: usize,
}

impl Assets<'_> {
    fn read(&mut self, path: &str, budget: &mut Budget<'_>) -> Result<Vec<u8>, String> {
        budget.consume(1)?;
        let limit = source_formats::mdl::MAX_MODEL_BYTES.min(MAX_ASSET_BYTES - self.bytes);
        let data = self
            .fs
            .read_limited(path, limit)
            .map_err(|error| format!("{path}: {error}"))?
            .ok_or_else(|| format!("Static-prop shadow asset {path} was not found"))?;
        self.bytes += data.len();
        budget.consume(data.len().div_ceil(64))?;
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
        for candidate in model
            .material_directories
            .iter()
            .map(|directory| {
                if directory.is_empty() {
                    name.to_owned()
                } else {
                    format!("{directory}/{name}")
                }
            })
            .chain(std::iter::once(name.to_owned()))
        {
            budget.consume(1)?;
            self.lookups += 1;
            if self.lookups > MAX_MATERIAL_LOOKUPS {
                return Err(
                    "Static-prop shadow material lookup exceeds its 65,536 lookup limit".into(),
                );
            }
            if let Some(material) = Material::load_limited(self.fs, &candidate, MAX_MATERIAL_BYTES)
                .map_err(|error| format!("Static-prop shadow material {candidate}: {error}"))?
            {
                return Ok(Some(material));
            }
        }
        Ok(None)
    }
}

fn transparent(material: &Material) -> bool {
    material.translucent()
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
    triangles: &mut Vec<PropTriangle>,
    budget: &mut Budget<'_>,
) -> Result<usize, String> {
    let angles = DVec3::from_array(prop.angles.map(f64::from)).map(f64::to_radians);
    let rotation = DQuat::from_euler(EulerRot::ZYX, angles.y, angles.x, angles.z);
    let origin = DVec3::from_array(prop.origin.map(f64::from));
    let mut degenerate = 0;
    for part in model.parts_for_body(0) {
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
            triangles.push(PropTriangle::new(points)?);
        }
    }
    Ok(degenerate)
}

pub(super) fn build(
    bsp: &Bsp,
    vmf: &Vmf,
    fs: &GameFs,
    report: &mut Report,
    budget: &mut Budget<'_>,
) -> Result<(ShadowIndex<PropTriangle>, Stats), String> {
    let mut triangles = Vec::new();
    let mut stats = Stats::default();
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
            bytes: 0,
            lookups: 0,
        };
        stats.disabled = props
            .props
            .iter()
            .filter(|prop| prop.flags & NO_SHADOW != 0)
            .count();
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
            let mut warned_materials = BTreeSet::new();
            for &index in instances {
                let entity = entities[index];
                let prop = &props.props[index];
                let result = (|| {
                    let skin = model
                        .skins
                        .get(prop.skin as usize)
                        .ok_or("Static-prop shadow skin is unavailable")?;
                    for part in model.parts_for_body(0) {
                        budget.consume(1)?;
                        let material = *skin
                            .get(part.material)
                            .ok_or("Static-prop shadow skin material is unavailable")?;
                        if warned_materials.insert(material) {
                            match assets.material(&model, material, budget)? {
                                Some(material) if !transparent(&material) && model.header.flags & CAST_TEXTURE_SHADOWS == 0 => {}
                                Some(_) => report.diagnostics.push(Diagnostic::warning(
                                    format!("{path}: static-prop shadows use opaque mesh triangles; texture alpha, translucency and material animation/conditional blocks are not evaluated"),
                                    Some(format!("entity:{}", entity.id)), entity.origin())),
                                None => report.diagnostics.push(Diagnostic::warning(
                                    format!("{path}: shadow material {} was not found; mesh casts opaque shadows", model.materials[material]),
                                    Some(format!("entity:{}", entity.id)), entity.origin())),
                            }
                        }
                    }
                    let before = triangles.len();
                    let degenerate = append(&model, prop, &mut triangles, budget)?;
                    if triangles.len() > before {
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
    }
    stats.triangles = triangles.len();
    let index = ShadowIndex::new(triangles, &mut |amount| budget.consume(amount))?;
    stats.work = budget.work() - start_work;
    if stats.props > 0 {
        report.diagnostics.push(Diagnostic::info(format!("Static-prop mesh shadows: {} props, {} models, {} LOD0 triangles; {} props disable shadows. Opaque triangles block direct, sky, bounced and ambient rays. Prop vertex/lightmap baking, reflected prop sources and animated props remain unfinished", stats.props, stats.models, stats.triangles, stats.disabled), None, None));
    }
    Ok((index, stats))
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
    fn prop_mesh_transform_two_sided_rays_and_endpoints_are_exact() {
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let mut triangles = Vec::new();
        let prop = StaticProp {
            origin: [32.0, 16.0, 64.0],
            angles: [90.0, 0.0, 0.0],
            ..Default::default()
        };
        append(&triangle_model(), &prop, &mut triangles, &mut budget).unwrap();
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
            append(&model, &StaticProp::default(), &mut triangles, &mut budget)
                .unwrap_err()
                .contains("vertices must fit")
        );
        assert!(triangles.is_empty());
        let mut model = triangle_model();
        model.parts[0].indices = [0, 1, 2].repeat(MAX_TRIANGLES + 1);
        assert!(
            append(&model, &StaticProp::default(), &mut triangles, &mut budget)
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
            append(&model, &StaticProp::default(), &mut triangles, &mut budget).unwrap(),
            1
        );
        assert!(triangles.is_empty());
    }
}
