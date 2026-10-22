use std::{
    collections::{BTreeMap, btree_map::Entry},
    sync::Arc,
};

use glam::{DVec2, DVec3};
use source_formats::{gamefs::GameFs, vmt};

use super::{
    bake::{Allocation, Budget},
    props::{Assets, alpha},
};
use crate::{asset_sources::BuildMaterialSources, map::Map};

#[derive(Debug, Clone)]
pub(super) struct Face {
    material: Arc<alpha::Material>,
    projection: [[f32; 4]; 2],
}

impl Face {
    pub(super) fn covered(&self, point: DVec3) -> Result<bool, String> {
        let dimensions = self.material.dimensions();
        let coordinate = |axis: usize| {
            let row = self.projection[axis];
            (DVec3::new(f64::from(row[0]), f64::from(row[1]), f64::from(row[2])).dot(point)
                + f64::from(row[3]))
                / dimensions[axis] as f64
        };
        self.material
            .blocks(DVec2::new(coordinate(0), coordinate(1)))
            .map_err(|error| format!("Brush texture shadows: {error}"))
    }
}

pub(super) struct Prepared {
    materials: BTreeMap<usize, Option<Arc<alpha::Material>>>,
    _allocations: Vec<Allocation>,
}

impl Prepared {
    pub(super) fn contains(&self, material: usize) -> bool {
        self.materials.get(&material).is_some_and(Option::is_some)
    }

    pub(super) fn face(&self, material: usize, projection: [[f32; 4]; 2]) -> Option<Face> {
        self.materials
            .get(&material)
            .and_then(Option::as_ref)
            .map(|material| Face {
                material: Arc::clone(material),
                projection,
            })
    }
}

fn requested(material: &vmt::Material) -> Result<bool, String> {
    if !material.blocks.is_empty() {
        return Err(
            "Brush texture shadows cannot classify conditional or animated material blocks".into(),
        );
    }
    let mut requested = false;
    for (key, neutral) in [
        ("$alphatest", 0.0),
        ("$translucent", 0.0),
        ("$additive", 0.0),
        ("$vertexalpha", 0.0),
        ("$distancealpha", 0.0),
        ("$alpha", 1.0),
    ] {
        if let Some(value) = material.get(key) {
            let value = value
                .trim()
                .parse::<f64>()
                .map_err(|_| format!("Brush texture shadows cannot classify invalid {key}"))?;
            if !value.is_finite() {
                return Err(format!(
                    "Brush texture shadows cannot classify nonfinite {key}"
                ));
            }
            requested |= value != neutral;
        }
    }
    Ok(requested)
}

pub(super) fn prepare(
    map: &Map<'_>,
    fs: &GameFs,
    sources: &BuildMaterialSources,
    budget: &mut Budget<'_>,
) -> Result<Prepared, String> {
    let sides = map
        .brushes
        .iter()
        .try_fold(0usize, |count, brush| {
            count.checked_add(brush.sides.iter().filter(|side| !side.bevel).count())
        })
        .ok_or("Brush texture shadows boundary count overflow")?;
    if sides > crate::MAX_BRUSH_SHADOW_SIDES {
        return Err("Brush texture shadows exceeds 65,536 boundary sides".into());
    }
    let _read_metadata = budget
        .reserve::<Allocation>(2 * (crate::MAX_BRUSH_SHADOW_MATERIAL_LOOKUPS + 5 * sides + 2))?;
    let allocation = budget.reserve_bytes(
        sides
            .checked_mul(
                size_of::<Option<Face>>()
                    + 4 * size_of::<(usize, Option<Arc<alpha::Material>>)>()
                    + 16 * size_of::<usize>(),
            )
            .ok_or("Brush texture shadows boundary allocation overflow")?,
    )?;
    budget.consume(sides)?;
    let mut materials = BTreeMap::new();
    let mut assets = Assets::for_brushes(fs, sources);
    for brush in &map.brushes {
        for side in brush
            .sides
            .iter()
            .filter(|side| !side.bevel && side.visible)
        {
            let info = map.materials.get(side.material);
            budget.consume(materials.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
            if let Entry::Vacant(entry) = materials.entry(side.material) {
                let _parsed_material = budget.reserve_bytes(
                    crate::MAX_BRUSH_SHADOW_MATERIAL_BYTES * 128 * (vmt::MAX_PATCH_DEPTH + 1),
                )?;
                let material = assets
                    .brush_material(&info.name, budget)
                    .map_err(|error| format!("solid:{}: {error}", brush.id))?;
                budget.consume(material.params.len() + 1)?;
                let alpha = if requested(&material).map_err(|error| {
                    format!("solid:{} material {}: {error}", brush.id, info.name)
                })? {
                    Some(
                        alpha::prepare_brush(&material, &mut assets, budget).map_err(|error| {
                            format!(
                                "Brush texture shadows solid:{} material {}: {error}",
                                brush.id, info.name
                            )
                        })?,
                    )
                } else {
                    None
                };
                entry.insert(alpha);
            }
            if brush.entity != 0 && materials[&side.material].is_some() {
                return Err(format!(
                    "Brush texture shadows does not support brush-model alpha casters (solid:{}, material {})",
                    brush.id, info.name
                ));
            }
        }
    }
    let mut allocations = assets.into_allocations();
    allocations.push(allocation);
    allocations.push(_read_metadata);
    Ok(Prepared {
        materials,
        _allocations: allocations,
    })
}

#[cfg(test)]
pub(super) fn test_face(alpha: [u8; 4]) -> Face {
    Face {
        material: alpha::test_cutout(alpha),
        projection: [[0.0, 2.0, 0.0, 0.0], [0.0, 0.0, 2.0, 0.0]],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_uses_numeric_neutral_values_and_refuses_unknown_opacity() {
        let opaque = vmt::Material::parse(
            "LightmappedGeneric { $alpha 1.0 $alphatest 0e0 $translucent -0.0 }",
        )
        .unwrap();
        assert!(!requested(&opaque).unwrap());
        for value in ["0", "0.5", "2"] {
            let material =
                vmt::Material::parse(&format!("LightmappedGeneric {{ $alpha {value} }}")).unwrap();
            assert!(requested(&material).unwrap());
        }
        for text in [
            "LightmappedGeneric { $alpha NaN }",
            "LightmappedGeneric { $alphatest invalid }",
            "LightmappedGeneric { Proxies { Add { resultVar $alpha } } }",
        ] {
            assert!(requested(&vmt::Material::parse(text).unwrap()).is_err());
        }
    }

    #[test]
    fn diffuse_projection_uses_texture_dimensions_before_alpha_sampling() {
        let mut face = test_face([0, 127, 128, 255]);
        for (point, covered) in [
            (DVec3::new(0.0, 0.25, 0.25), false),
            (DVec3::new(0.0, 0.75, 0.25), false),
            (DVec3::new(0.0, 0.25, 0.75), true),
            (DVec3::new(0.0, 0.75, 0.75), true),
            (DVec3::new(100.0, -0.25, 1.75), true),
        ] {
            assert_eq!(face.covered(point).unwrap(), covered);
        }
        face.projection[1][3] = 1.0;
        assert!(face.covered(DVec3::new(0.0, 0.25, 0.25)).unwrap());
        face.projection[0][0] = f32::NAN;
        assert!(face.covered(DVec3::ZERO).is_err());
    }
}
