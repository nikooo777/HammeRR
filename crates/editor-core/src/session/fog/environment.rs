use compiler::asset_sources::{AssetSourceGuard, BuildMaterialSources};
use compiler::map::FogEnvironmentGeometry;
use compiler::{Control, Level, Report};
use glam::DVec3;
use serde::Serialize;
use source_formats::gamefs::GameFs;
use source_formats::vmf::{Entity, Vmf};

mod materials;

pub(in crate::session) use super::catalog::MAX_CONTROLLERS;
pub(in crate::session) const MAX_LIQUIDS: usize = 256;
pub(in crate::session) const MAX_SOURCE_ENTITIES: usize = 16_384;
pub(in crate::session) const MAX_SOURCE_BRUSHES: usize = 4096;
pub(in crate::session) const MAX_SOURCE_SIDES: usize = 16_384;
pub(in crate::session) const MAX_SOURCE_WORK: usize = 16_000_000;
pub(in crate::session) const MAX_SOURCE_METADATA: usize = 2 * 1024 * 1024;

const MAX_PROPERTY_BYTES: usize = 4 * 1024 * 1024;
const MAX_QUERY_WORK: usize = 262_144;
const MAX_PLANES: usize = 131_072;
const MAX_PLANES_PER_BRUSH: usize = 1024;
const MAX_MATERIAL_NAME_BYTES: usize = 256;
const MAX_SOURCE_PATHS: usize = 1024;
const MAX_SOURCE_LOOKUPS: usize = 2048;
const MASTER_FLAG: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(in crate::session) struct UnderwaterFog {
    pub solid: u32,
    pub source_solids: Vec<u32>,
    pub contents: i32,
    pub material: String,
    pub surface_material: String,
    pub surface_z: f32,
    pub color_normalized: [f32; 3],
    pub start: f32,
    pub end: f32,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(in crate::session) struct Selection {
    pub controller: Option<u32>,
    pub reason: &'static str,
    pub underwater: Option<UnderwaterFog>,
}

#[derive(Debug)]
struct Liquid {
    fog: UnderwaterFog,
}

#[derive(Debug)]
pub(in crate::session) struct Snapshot {
    pub sources: AssetSourceGuard,
    controllers: Vec<u32>,
    master: Option<u32>,
    master_reason: &'static str,
    liquids: Vec<Liquid>,
    geometry: FogEnvironmentGeometry,
}

fn property<'a>(entity: &'a Entity, key: &str) -> Result<Option<&'a str>, String> {
    let mut values = entity
        .properties
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(key));
    let value = values.next().map(|(_, value)| value.trim());
    if values.next().is_some() {
        return Err(format!(
            "Fog environment entity:{} has duplicate {key} keys",
            entity.id
        ));
    }
    Ok(value)
}

fn flags(entity: &Entity) -> Result<u32, String> {
    let value = property(entity, "spawnflags")?.unwrap_or("0");
    if value.len() > 16 {
        return Err(format!(
            "Fog controller entity:{} has an oversized spawnflags value",
            entity.id
        ));
    }
    value
        .parse::<u32>()
        .or_else(|_| value.parse::<i32>().map(|value| value as u32))
        .map_err(|_| {
            format!(
                "Fog controller entity:{} needs a 32-bit integer spawnflags value",
                entity.id
            )
        })
}

fn controllers(vmf: &Vmf) -> Result<(Vec<u32>, Option<u32>, &'static str), String> {
    let mut controllers = Vec::new();
    let mut selected = None;
    let mut reason = "no_controller";
    for entity in vmf.entities.iter().filter(|entity| !entity.hidden) {
        let Some(classname) = property(entity, "classname")? else {
            continue;
        };
        if ["fog_volume", "trigger_fog"]
            .iter()
            .any(|name| classname.eq_ignore_ascii_case(name))
        {
            return Err(format!(
                "Fog environment entity:{} uses {classname}, whose installed selection contract is unavailable",
                entity.id
            ));
        }
        if !classname.eq_ignore_ascii_case("env_fog_controller") {
            continue;
        }
        if !entity.solids.is_empty() {
            return Err(format!(
                "Fog controller entity:{} must be a point entity",
                entity.id
            ));
        }
        if controllers.len() >= MAX_CONTROLLERS {
            return Err(format!(
                "Fog environment exceeds {MAX_CONTROLLERS} controllers"
            ));
        }
        controllers.push(entity.id);
        if selected.is_none() {
            selected = Some(entity.id);
            reason = "first_controller";
        }
        if flags(entity)? & MASTER_FLAG != 0 {
            selected = Some(entity.id);
            reason = "last_master";
        }
    }
    Ok((controllers, selected, reason))
}

fn preflight(vmf: &Vmf) -> Result<(), String> {
    if vmf.entities.len() > MAX_SOURCE_ENTITIES {
        return Err("Fog environment exceeds 16384 source entities".into());
    }
    let mut property_bytes = 0usize;
    for entity in &vmf.entities {
        for (key, value) in entity.properties.iter() {
            property_bytes = property_bytes
                .saturating_add(key.len())
                .saturating_add(value.len());
            if property_bytes > MAX_PROPERTY_BYTES {
                return Err("Fog environment exceeds 4 MiB of entity property bytes".into());
            }
        }
    }
    let mut brushes = 0usize;
    let mut sides = 0usize;
    let mut work = 0usize;
    let solids = vmf.world.solids.iter().chain(
        vmf.entities
            .iter()
            .filter(|entity| !entity.hidden && entity.classname() == Some("func_detail"))
            .flat_map(|entity| &entity.solids),
    );
    for solid in solids.filter(|solid| !solid.hidden) {
        if !(4..=map_core::subobject::MAX_SIDES).contains(&solid.sides.len()) {
            return Err(format!(
                "Fog environment brush {} requires 4–64 sides",
                solid.id
            ));
        }
        brushes += 1;
        sides = sides.saturating_add(solid.sides.len());
        work = work.saturating_add(solid.sides.len().pow(3));
        if brushes > MAX_SOURCE_BRUSHES || sides > MAX_SOURCE_SIDES || work > MAX_SOURCE_WORK {
            return Err("Fog environment exceeds its brush, side or preparation work limit".into());
        }
        if solid.sides.iter().any(|side| {
            side.material.len() > MAX_MATERIAL_NAME_BYTES || side.material.trim().is_empty()
        }) {
            return Err(format!(
                "Fog environment brush {} needs nonempty material names of at most 256 bytes",
                solid.id
            ));
        }
    }
    Ok(())
}

fn preparation_error(report: &Report, error: String) -> String {
    match report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.level == Level::Error)
    {
        Some(diagnostic) => format!(
            "Fog environment {}: {}",
            diagnostic.object.as_deref().unwrap_or("geometry"),
            diagnostic.message
        ),
        None => format!("Fog environment: {error}"),
    }
}

pub(in crate::session) fn prepare(
    vmf: &Vmf,
    game: &GameFs,
    control: &Control,
) -> Result<Snapshot, String> {
    preflight(vmf)?;
    vmf.validate_ids().map_err(|error| error.to_string())?;
    let (controllers, master, master_reason) = controllers(vmf)?;
    let sources = BuildMaterialSources::bounded(
        compiler::materials::MAX_GEOMETRY_MATERIAL_TOTAL_BYTES,
        compiler::materials::MAX_GEOMETRY_MATERIAL_READS,
        MAX_SOURCE_PATHS,
        MAX_SOURCE_LOOKUPS,
    );
    let mut report = Report::default();
    let mut prepared = compiler::map::Map::prepare_collision_geometry_controlled(
        vmf,
        game,
        &sources,
        &mut report,
        control,
    )
    .map_err(|error| preparation_error(&report, error))?;
    let plane_count = prepared
        .map
        .brushes
        .iter()
        .map(|brush| brush.sides.len())
        .sum::<usize>();
    if plane_count > MAX_PLANES
        || prepared
            .map
            .brushes
            .iter()
            .any(|brush| brush.sides.len() > MAX_PLANES_PER_BRUSH)
    {
        return Err("Fog environment exceeds its prepared brush-plane limits".into());
    }
    let geometry = prepared
        .map
        .fog_environment_geometry(&sources, &mut report, control)
        .map_err(|error| preparation_error(&report, error))?;
    if geometry.liquids.len() > MAX_LIQUIDS {
        return Err("Fog environment exceeds 256 liquid records".into());
    }
    for material in prepared.map.materials.iter() {
        if !material.found || !material.unsupported_flags.is_empty() {
            return Err(format!(
                "Fog environment material {:?} is missing or has unsupported compile flags",
                material.name
            ));
        }
    }
    let mut liquids = Vec::with_capacity(geometry.liquids.len());
    let mut reads = 0;
    for volume in &geometry.liquids {
        control.checkpoint()?;
        liquids.push(Liquid {
            fog: materials::load(volume, game, &sources, control, &mut reads)?,
        });
    }
    sources
        .original_source_guard(control, MAX_SOURCE_METADATA)
        .map_err(|error| format!("Fog environment source metadata: {error}"))?;
    let sources = sources
        .source_guard(game, control)
        .map_err(|error| format!("Fog environment source changed: {error}"))?;
    control.checkpoint()?;
    Ok(Snapshot {
        sources,
        controllers,
        master,
        master_reason,
        liquids,
        geometry,
    })
}

impl Snapshot {
    pub(in crate::session) fn initial_controller(&self) -> Option<u32> {
        self.master
    }

    pub(in crate::session) fn select(&self, eye: DVec3) -> Result<Selection, String> {
        if !eye.is_finite() || eye.abs().max_element() > map_core::checks::SOURCE_COORDINATE_LIMIT {
            return Err("Fog environment eye position must be finite within ±65536 units".into());
        }
        let underwater = self.liquid_at(eye)?.map(|liquid| liquid.fog.clone());
        Ok(Selection {
            controller: self.master,
            reason: self.master_reason,
            underwater,
        })
    }

    pub(in crate::session) fn controllers(&self) -> &[u32] {
        &self.controllers
    }

    pub(in crate::session) fn liquid_count(&self) -> usize {
        self.geometry.liquid_brushes
    }

    pub(in crate::session) fn solid_count(&self) -> usize {
        self.geometry.solid_brushes
    }

    pub(in crate::session) fn tree_nodes(&self) -> usize {
        self.geometry.nodes.len()
    }

    fn liquid_at(&self, eye: DVec3) -> Result<Option<&Liquid>, String> {
        if self.geometry.nodes.is_empty() {
            return Ok(None);
        }
        let point = eye.as_vec3();
        let mut index = 0;
        for _ in 0..MAX_QUERY_WORK {
            let node = self
                .geometry
                .nodes
                .get(index)
                .ok_or("Fog environment BSP node is missing")?;
            let Some((plane, children)) = node.split else {
                return node
                    .liquid
                    .map(|liquid| {
                        self.liquids
                            .get(liquid)
                            .ok_or_else(|| "Fog environment water record is missing".to_string())
                    })
                    .transpose();
            };
            let normal = plane.normal.as_vec3();
            let z = point.z * normal.z - plane.dist as f32;
            let zy = z + point.y * normal.y;
            let distance = zy + point.x * normal.x;
            index = children[usize::from(distance < 0.0)];
        }
        Err("Fog environment BSP query exceeded its work limit".into())
    }
}
