use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    sync::Arc,
};

use glam::{DVec2, DVec3};
use source_formats::{bsp::contents, gamefs::GameFs, vmt};

use super::{
    bake::{Allocation, Budget},
    props::{Assets, alpha},
    transport::Coverage,
};
use crate::{asset_sources::BuildMaterialSources, map::Map};

#[derive(Debug, Clone)]
pub(super) struct Face {
    material: Arc<alpha::Material>,
    projection: [[f32; 4]; 2],
}

impl Face {
    pub(super) fn exact_mapping(&self) -> (&alpha::Material, [[f32; 4]; 2]) {
        (&self.material, self.projection)
    }

    pub(super) fn fractional(&self) -> bool {
        self.material.fractional()
    }

    pub(super) fn coverage(&self, point: DVec3) -> Result<Coverage, String> {
        let dimensions = self.material.dimensions();
        let coordinate = |axis: usize| {
            let row = self.projection[axis];
            (DVec3::new(f64::from(row[0]), f64::from(row[1]), f64::from(row[2])).dot(point)
                + f64::from(row[3]))
                / dimensions[axis] as f64
        };
        self.material
            .coverage(DVec2::new(coordinate(0), coordinate(1)))
            .map_err(|error| format!("Brush texture shadows: {error}"))
    }
}

pub(super) struct Prepared {
    materials: BTreeMap<usize, Option<Arc<alpha::Material>>>,
    model_alpha: bool,
    global_alpha: BTreeSet<usize>,
    _allocations: Vec<Allocation>,
}

impl Prepared {
    pub(super) fn contains(&self, material: usize) -> bool {
        self.materials.get(&material).is_some_and(Option::is_some)
    }

    pub(super) fn has_model_alpha(&self) -> bool {
        self.model_alpha
    }

    pub(super) fn has_global_alpha(
        &self,
        entity: usize,
        budget: &mut Budget<'_>,
    ) -> Result<bool, String> {
        if self.global_alpha.is_empty() {
            return Ok(false);
        }
        budget.consume(self.global_alpha.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
        Ok(self.global_alpha.contains(&entity))
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

    pub(super) fn qualify_models(
        &self,
        map: &Map<'_>,
        scopes: &[super::models::Scope],
        receivers: &[bool],
        budget: &mut Budget<'_>,
    ) -> Result<(), String> {
        if !self.model_alpha {
            return Ok(());
        }
        if scopes.len() != receivers.len() {
            return Err("Brush texture shadow model scopes do not match receivers".into());
        }
        let _allocation = budget.reserve::<bool>(map.entities.len())?;
        budget.consume(map.entities.len())?;
        let mut admitted = vec![false; map.entities.len()];
        for (scope, &receiver) in scopes.iter().zip(receivers) {
            budget.consume(1)?;
            admitted[scope.entity] = receiver
                || crate::brush_shadow_casters::enabled(&map.entities[scope.entity].properties);
        }
        for brush in map.brushes.iter().filter(|brush| brush.entity != 0) {
            for side in brush
                .sides
                .iter()
                .filter(|side| !side.bevel && side.visible)
            {
                budget.consume(self.materials.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
                if self.contains(side.material)
                    && (!admitted[brush.entity]
                        || brush.contents & (contents::SOLID | contents::WINDOW | contents::GRATE)
                            == 0)
                {
                    return Err(format!(
                        "Brush texture shadows entity:{} solid:{} material {} has no applicable baked self-shadow or requested global scope",
                        map.entities[brush.entity].id,
                        brush.id,
                        map.materials.get(side.material).name
                    ));
                }
            }
        }
        Ok(())
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

fn prepare_material(
    index: usize,
    solid: u32,
    map: &Map<'_>,
    materials: &mut BTreeMap<usize, Option<Arc<alpha::Material>>>,
    assets: &mut Assets<'_, '_>,
    colored: bool,
    budget: &mut Budget<'_>,
) -> Result<bool, String> {
    budget.consume(materials.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
    match materials.entry(index) {
        Entry::Occupied(entry) => Ok(entry.get().is_some()),
        Entry::Vacant(entry) => {
            let info = map.materials.get(index);
            let _parsed_material = budget.reserve_bytes(
                crate::MAX_BRUSH_SHADOW_MATERIAL_BYTES * 128 * (vmt::MAX_PATCH_DEPTH + 1),
            )?;
            let material = assets
                .brush_material(&info.name, budget)
                .map_err(|error| format!("solid:{solid}: {error}"))?;
            budget.consume(material.params.len() + 1)?;
            let alpha = if requested(&material)
                .map_err(|error| format!("solid:{solid} material {}: {error}", info.name))?
            {
                drop(material);
                let _raw_material =
                    budget.reserve_bytes(crate::MAX_BRUSH_SHADOW_MATERIAL_BYTES * 128)?;
                let material = assets
                    .brush_cutout_material(&info.name, budget)
                    .map_err(|error| format!("solid:{solid}: {error}"))?;
                Some(
                    alpha::prepare_brush(&material, assets, budget, colored).map_err(|error| {
                        format!(
                            "Brush texture shadows solid:{solid} material {}: {error}",
                            info.name
                        )
                    })?,
                )
            } else {
                None
            };
            Ok(entry.insert(alpha).is_some())
        }
    }
}

pub(super) fn prepare(
    map: &Map<'_>,
    fs: &GameFs,
    sources: &BuildMaterialSources,
    colored: bool,
    budget: &mut Budget<'_>,
) -> Result<Prepared, String> {
    budget.consume(0)?;
    if !map.materials.uses_build_sources(sources) {
        return Err("Brush texture shadows require the original prepared material owner".into());
    }
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
    let surfaces = sides
        .checked_add(map.terrains.len())
        .ok_or("Brush texture shadows surface count overflow")?;
    let _read_metadata = budget
        .reserve::<Allocation>(2 * (crate::MAX_BRUSH_SHADOW_MATERIAL_LOOKUPS + 6 * surfaces + 2))?;
    let hull_bytes = map
        .brushes
        .len()
        .checked_mul(size_of::<super::OpticalHull>())
        .ok_or("Brush texture shadows hull allocation overflow")?;
    let allocation = budget.reserve_bytes(
        surfaces
            .checked_mul(
                size_of::<Option<Face>>()
                    + 4 * size_of::<(usize, Option<Arc<alpha::Material>>)>()
                    + 16 * size_of::<usize>(),
            )
            .and_then(|bytes| bytes.checked_add(hull_bytes))
            .ok_or("Brush texture shadows boundary allocation overflow")?,
    )?;
    budget.consume(surfaces)?;
    let mut materials = BTreeMap::new();
    let mut model_alpha = false;
    let mut global_alpha = BTreeSet::new();
    let mut global_allocations = Vec::new();
    let mut assets = Assets::for_brushes(fs, sources);
    for brush in &map.brushes {
        let mut brush_alpha = false;
        for side in brush
            .sides
            .iter()
            .filter(|side| !side.bevel && side.visible)
        {
            let alpha = prepare_material(
                side.material,
                brush.id,
                map,
                &mut materials,
                &mut assets,
                colored,
                budget,
            )?;
            let model_alpha_side = brush.entity != 0 && alpha;
            model_alpha |= model_alpha_side;
            brush_alpha |= model_alpha_side;
        }
        if brush_alpha {
            let properties = &map.entities[brush.entity].properties;
            budget.consume(properties.len())?;
            if crate::brush_shadow_casters::enabled(properties) {
                budget.consume(global_alpha.len().checked_ilog2().unwrap_or(0) as usize + 1)?;
                if !global_alpha.contains(&brush.entity) {
                    global_allocations.push(budget.reserve_bytes(16 * size_of::<usize>())?);
                    global_alpha.insert(brush.entity);
                }
            }
        }
    }
    for terrain in &map.terrains {
        prepare_material(
            terrain.material,
            terrain.solid,
            map,
            &mut materials,
            &mut assets,
            colored,
            budget,
        )?;
    }
    let mut allocations = assets.into_allocations();
    allocations.extend(global_allocations);
    allocations.push(allocation);
    allocations.push(_read_metadata);
    Ok(Prepared {
        materials,
        model_alpha,
        global_alpha,
        _allocations: allocations,
    })
}
