use compiler::asset_sources::{AssetSourceGuard, BuildMaterialSources};
use compiler::{Control, Level, Report};
use glam::DVec3;
use map_core::rope_collision::{self, Brush, TerrainPatch, Triangle, World};
use source_formats::bsp::contents;
use source_formats::gamefs::GameFs;
use source_formats::vmf::{Solid, Vmf};

pub(in crate::session) const MASK: i32 =
    contents::SOLID | contents::WINDOW | contents::GRATE | contents::MOVEABLE;
pub(in crate::session) const MAX_SOURCE_SIDES: usize = 16_384;
pub(in crate::session) const MAX_TERRAIN_FACES: usize = 128;
pub(in crate::session) const MAX_PREPARE_WORK: usize = 16_000_000;
pub(in crate::session) const MAX_ENTITIES: usize = 65_536;
pub(in crate::session) const MAX_SOURCE_METADATA: usize = 2 * 1024 * 1024;

const MAX_MATERIAL_NAME_BYTES: usize = 256;
const MAX_SOURCE_PATHS: usize = 1024;
const MAX_SOURCE_LOOKUPS: usize = 2048;

#[derive(Debug)]
pub(in crate::session) struct Snapshot {
    pub world: World,
    pub sources: AssetSourceGuard,
    pub excluded_brush_entities: usize,
    pub excluded_model_entities: usize,
    pub disabled_terrain_faces: usize,
}

#[derive(Default)]
struct Admission {
    brushes: usize,
    sides: usize,
    terrain_faces: usize,
    work: usize,
    excluded_brush_entities: usize,
    excluded_model_entities: usize,
}

impl Admission {
    fn solid(&mut self, solid: &Solid) -> Result<(), String> {
        if solid.hidden {
            return Ok(());
        }
        if !(4..=map_core::subobject::MAX_SIDES).contains(&solid.sides.len()) {
            return Err(format!(
                "Rope collision brush {} requires 4–64 authored sides",
                solid.id
            ));
        }
        self.brushes += 1;
        self.sides = self.sides.saturating_add(solid.sides.len());
        self.work = self.work.saturating_add(solid.sides.len().pow(3));
        self.terrain_faces = self.terrain_faces.saturating_add(
            solid
                .sides
                .iter()
                .filter(|side| side.displacement.is_some())
                .count(),
        );
        if self.brushes > rope_collision::MAX_BRUSHES
            || self.sides > MAX_SOURCE_SIDES
            || self.terrain_faces > MAX_TERRAIN_FACES
            || self.work > MAX_PREPARE_WORK
        {
            return Err(
                "Rope collision preparation exceeds its brush, side, terrain or work limit".into(),
            );
        }
        if solid.sides.iter().any(|side| {
            side.material.len() > MAX_MATERIAL_NAME_BYTES || side.material.trim().is_empty()
        }) {
            return Err(format!(
                "Rope collision brush {} needs nonempty material names of at most 256 bytes",
                solid.id
            ));
        }
        Ok(())
    }

    fn map(vmf: &Vmf, control: &Control) -> Result<Self, String> {
        if vmf.entities.len() > MAX_ENTITIES {
            return Err("Rope collision preparation exceeds 65536 source entities".into());
        }
        let mut admission = Self::default();
        for solid in &vmf.world.solids {
            control.checkpoint()?;
            admission.solid(solid)?;
        }
        for entity in vmf.entities.iter().filter(|entity| !entity.hidden) {
            control.checkpoint()?;
            if entity.classname() == Some("func_detail") {
                if entity.properties.get_all("classname").count() != 1 {
                    return Err(format!(
                        "Rope collision detail entity {} has ambiguous classnames",
                        entity.id
                    ));
                }
                for solid in &entity.solids {
                    admission.solid(solid)?;
                }
            } else if entity.solids.iter().any(|solid| !solid.hidden) {
                admission.excluded_brush_entities += 1;
            } else if entity
                .properties
                .get_all("model")
                .any(|model| !model.trim().is_empty())
            {
                admission.excluded_model_entities += 1;
            }
        }
        Ok(admission)
    }
}

fn preparation_error(report: &Report, error: String) -> String {
    match report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.level == Level::Error)
    {
        Some(diagnostic) => match &diagnostic.object {
            Some(object) => format!("Rope collision {object}: {}", diagnostic.message),
            None => format!("Rope collision: {}", diagnostic.message),
        },
        None => format!("Rope collision preparation: {error}"),
    }
}

pub(in crate::session) fn prepare(
    vmf: &Vmf,
    game: &GameFs,
    control: &Control,
) -> Result<Snapshot, String> {
    let admission = Admission::map(vmf, control)?;
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
    for material in prepared.map.materials.iter() {
        control.checkpoint()?;
        if !material.found || !material.unsupported_flags.is_empty() {
            return Err(format!(
                "Rope collision material {:?} is missing or has unsupported compile flags",
                material.name
            ));
        }
    }
    let mut brushes = Vec::new();
    for brush in prepared
        .map
        .brushes
        .iter()
        .filter(|brush| brush.contents & MASK != 0)
    {
        control.checkpoint()?;
        let planes = brush
            .sides
            .iter()
            .map(|side| {
                let plane = prepared.map.planes.get(side.plane);
                map_core::Plane {
                    normal: plane.normal.as_vec3().as_dvec3(),
                    dist: f64::from(plane.dist as f32),
                }
            })
            .collect();
        brushes.push(Brush {
            solid: brush.id,
            mins: brush.mins,
            maxs: brush.maxs,
            planes,
        });
    }
    let mut triangles = Vec::new();
    let mut patches = Vec::new();
    let mut disabled_terrain_faces = 0;
    for terrain in &prepared.map.terrains {
        control.checkpoint()?;
        let positions = map_core::displacement::native::positions(
            terrain.base,
            terrain.info,
            terrain.vertices.clone(),
        )?
        .into_iter()
        .map(|point| DVec3::from_array(point.map(f64::from)))
        .collect::<Vec<_>>();
        let (mins, maxs) = positions
            .iter()
            .fold((DVec3::MAX, DVec3::MIN), |(mins, maxs), point| {
                (mins.min(*point), maxs.max(*point))
            });
        patches.push(TerrainPatch {
            solid: terrain.solid,
            side: terrain.side,
            base: terrain
                .info
                .start_oriented_quad(terrain.base.map(|point| {
                    map_core::displacement::native::stored_vertex(point)
                        .as_vec3()
                        .to_array()
                }))
                .map_err(|error| error.to_string())?
                .map(|point| DVec3::from_array(point.map(f64::from))),
            mins: mins - DVec3::ONE,
            maxs: maxs + DVec3::ONE,
            matches_mask: terrain.info.contents & MASK != 0,
        });
        if terrain.info.contents & MASK == 0 {
            continue;
        }
        if terrain.info.min_tessellation & 4 != 0 {
            disabled_terrain_faces += 1;
            continue;
        }
        let indices = terrain
            .info
            .triangle_indices()
            .map_err(|error| error.to_string())?;
        if triangles.len().saturating_add(indices.len()) > rope_collision::MAX_TRIANGLES {
            return Err("Rope collision exceeds 65536 native terrain triangles".into());
        }
        for indices in indices {
            let mut points = [DVec3::ZERO; 3];
            for (point, index) in points.iter_mut().zip(indices) {
                *point = *positions
                    .get(index)
                    .ok_or("Native terrain rope collision index is invalid")?;
            }
            triangles.push(Triangle {
                solid: terrain.solid,
                side: terrain.side,
                positions: points,
            });
        }
    }
    let mut world = World::new(brushes, triangles)
        .and_then(|world| world.with_terrain_patches(patches))
        .map_err(|error| error.to_string())?;
    if world.stationary_topology_required() {
        let topology = prepared.map.prepare_rope_collision_topology(
            world.terrain_patch_metadata(),
            MASK,
            control,
        )?;
        world = world
            .with_stationary_topology(topology)
            .map_err(|error| error.to_string())?;
    }
    sources
        .original_source_guard(control, MAX_SOURCE_METADATA)
        .map_err(|error| format!("Rope collision source metadata: {error}"))?;
    let sources = sources
        .source_guard(game, control)
        .map_err(|error| format!("Rope collision source changed: {error}"))?;
    control.checkpoint()?;
    Ok(Snapshot {
        world,
        sources,
        excluded_brush_entities: admission.excluded_brush_entities,
        excluded_model_entities: admission.excluded_model_entities,
        disabled_terrain_faces,
    })
}
