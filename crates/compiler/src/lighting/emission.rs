use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::PI;
use std::sync::Arc;

use glam::DVec3;
use source_formats::bsp::{self, Bsp, Edge, LightingKind, TexInfo, surf};
use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::vmf::{Entity, Properties, Solid};

use super::bake::{Allocation, Budget};
use super::radiance::{SourceBoundary, source_blocked};
use super::shadows::{ShadowIndex, Triangle};
use super::{Occluder, ReceiverAngularMode};
use crate::asset_sources::BuildMaterialSources;
use crate::materials::{Materials, Requirements};
use crate::{Control, Diagnostic, Options, Report, SkyQuality, map::Map};

pub const MATERIAL_EMISSION_FILE_KEY: &str = "_hammerr_emission_file";
pub const MAX_MATERIAL_EMISSION_FILE_PATH_BYTES: usize = 1024;
const MAX_FILE_BYTES: usize = 64 * 1024;
const MAX_RECORDS: usize = 4096;
const MAX_LINE_BYTES: usize = 1024;
const MAX_SOURCE_SIDES: usize = 65_536;
const MAX_TRIANGLES: usize = 262_144;
const MAX_CELLS: usize = 262_144;
const MAX_DEPTH: usize = 48;
const BOUNDARY_REFINEMENT: f64 = 16.0;
const LIGHTMAP_SCALE: f64 = 255.0;
const ARITHMETIC_OPERATIONS: f64 = 256.0;
const SAMPLE_WEIGHTS: [[f64; 3]; 4] = [
    [1.0 / 3.0; 3],
    [2.0 / 3.0, 1.0 / 6.0, 1.0 / 6.0],
    [1.0 / 6.0, 2.0 / 3.0, 1.0 / 6.0],
    [1.0 / 6.0, 1.0 / 6.0, 2.0 / 3.0],
];

#[derive(Debug, Clone, serde::Serialize)]
pub struct MaterialEmissionStats {
    pub file: String,
    pub source_sha256: String,
    pub declared_materials: usize,
    pub matched_sides: usize,
    pub source_faces: usize,
    pub source_triangles: usize,
    pub source_area: f64,
    pub channel_flux: [[f64; 3]; 2],
    pub maximum_cell_angle_degrees: f64,
    pub boundary_cell_angle_degrees: f64,
    pub channel_cells: [usize; 2],
    pub channel_rays: [usize; 2],
    pub native_policy: &'static str,
}

pub fn validate_material_emission_file(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(());
    }
    if value.len() > MAX_MATERIAL_EMISSION_FILE_PATH_BYTES
        || !value.is_ascii()
        || value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b'\\')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || asset_relative(value).is_err()
    {
        return Err("Emission file requires an ASCII mounted relative asset path of at most 1024 bytes, without traversal, empty components or backslashes".into());
    }
    Ok(())
}

pub fn is_material_emission_key(key: &str) -> bool {
    key.trim_end_matches(' ')
        .split('#')
        .next()
        .unwrap_or(key)
        .eq_ignore_ascii_case(MATERIAL_EMISSION_FILE_KEY)
}

pub fn material_emission_file_field(properties: &Properties) -> Result<Option<&str>, String> {
    let mut fields = properties
        .iter()
        .filter(|(key, _)| is_material_emission_key(key));
    let Some((key, value)) = fields.next() else {
        return Ok(None);
    };
    if key != MATERIAL_EMISSION_FILE_KEY || fields.next().is_some() {
        return Err(
            "Material emission requires one canonical _hammerr_emission_file world field".into(),
        );
    }
    Ok(Some(value))
}

pub(crate) fn file(properties: &Properties) -> Result<Option<&str>, String> {
    let Some(value) = material_emission_file_field(properties)? else {
        return Ok(None);
    };
    validate_material_emission_file(value)?;
    Ok((!value.trim().is_empty()).then_some(value.trim()))
}

#[derive(Clone, Copy, Default)]
struct Declaration {
    id: usize,
    channels: [Option<DVec3>; 2],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceOwner {
    World,
    Model(u32),
}

#[derive(Clone, Copy)]
struct AdmittedSide {
    declaration: usize,
    owner: SourceOwner,
}

pub(crate) struct Input<'a> {
    pub(super) budget: Budget<'a>,
    declarations: BTreeMap<String, Declaration>,
    sides: BTreeMap<u32, AdmittedSide>,
    sources: BuildMaterialSources,
    stats: MaterialEmissionStats,
    _allocation: Allocation,
}

fn material_name(value: &str) -> Result<String, String> {
    if value.is_empty() || !value.is_ascii() || value.len() > MAX_LINE_BYTES {
        return Err("Material emission needs a bounded ASCII material name".into());
    }
    let path = source_formats::vmt::material_path(value);
    let value = path
        .strip_prefix("materials/")
        .and_then(|value| value.strip_suffix(".vmt"))
        .ok_or("Emission material path is invalid")?;
    if value.is_empty() {
        return Err("Emission material name is empty".into());
    }
    validate_material_emission_file(value)?;
    Ok(value.to_owned())
}

fn decimal(value: &str) -> Result<f64, String> {
    if value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || b".+-eE".contains(&byte))
    {
        return Err("Material emission numbers require bounded decimal syntax".into());
    }
    let value = value
        .parse::<f64>()
        .map_err(|_| "Invalid material emission decimal")?;
    if !value.is_finite() || value < 0.0 {
        return Err("Material emission values must be finite and nonnegative".into());
    }
    Ok(value)
}

fn parse(text: &str, budget: &mut Budget<'_>) -> Result<BTreeMap<String, Declaration>, String> {
    if !text.is_ascii() {
        return Err("Material emission file must be ASCII".into());
    }
    let mut declarations = BTreeMap::<String, Declaration>::new();
    let mut records = 0;
    for (line, value) in text.lines().enumerate() {
        budget.consume(1 + value.len().div_ceil(64))?;
        if value.len() > MAX_LINE_BYTES {
            return Err(format!(
                "Emission file line {} exceeds 1024 bytes",
                line + 1
            ));
        }
        let value = value.trim();
        if value.is_empty() || value.starts_with("//") {
            continue;
        }
        records += 1;
        if records > MAX_RECORDS {
            return Err("Material emission exceeds 4096 records".into());
        }
        let checked = (|| {
            let mut values = value.split_ascii_whitespace();
            let first = values.next().ok_or("Missing emission material")?;
            let (channels, name) = match first {
                "ldr:" => (
                    [true, false],
                    values.next().ok_or("Missing LDR emission material")?,
                ),
                "hdr:" => (
                    [false, true],
                    values.next().ok_or("Missing HDR emission material")?,
                ),
                _ => ([true; 2], first),
            };
            let name = material_name(name)?;
            let mut numbers = [0.0; 4];
            for number in &mut numbers {
                *number = decimal(values.next().ok_or("Emission requires R G B brightness")?)?;
            }
            if values.next().is_some() || numbers[..3].iter().any(|value| *value > LIGHTMAP_SCALE) {
                return Err(
                    "Emission requires exactly R G B brightness, with RGB between 0 and 255".into(),
                );
            }
            let radiance = DVec3::new(numbers[0], numbers[1], numbers[2])
                .map(|value| (value / LIGHTMAP_SCALE).powf(2.2))
                * (numbers[3] / LIGHTMAP_SCALE);
            if !radiance.is_finite() {
                return Err("Material emission radiance overflow".into());
            }
            let declaration = declarations.entry(name.clone()).or_default();
            for (index, enabled) in channels.into_iter().enumerate() {
                if enabled && declaration.channels[index].replace(radiance).is_some() {
                    return Err(format!(
                        "Duplicate emission assignment for {name}, channel {index}"
                    ));
                }
            }
            Ok::<(), String>(())
        })();
        checked.map_err(|error| format!("Emission file line {}: {error}", line + 1))?;
    }
    for (id, declaration) in declarations.values_mut().enumerate() {
        declaration.id = id;
    }
    Ok(declarations)
}

fn maximum_angle(quality: SkyQuality) -> f64 {
    match quality {
        SkyQuality::Fast => 8.0,
        SkyQuality::Balanced => 4.0,
        SkyQuality::Detailed => 1.0,
    }
}

pub(crate) fn preflight<'a>(
    instances: &crate::instances::Prepared<'_>,
    caster_requests: &[&Entity],
    fs: &GameFs,
    sources: &BuildMaterialSources,
    options: Options,
    control: &'a Control,
    report: &mut Report,
) -> Result<Option<Input<'a>>, String> {
    let vmf = instances.compilation_vmf()?;
    let Some(path) = file(&vmf.world.properties)? else {
        return Ok(None);
    };
    let mut budget = Budget::with_options(control, options);
    let allocation =
        budget.reserve_bytes(MAX_FILE_BYTES * 128 + MAX_RECORDS * 1024 + MAX_SOURCE_SIDES * 128)?;
    budget.consume(1)?;
    let asset = sources
        .read_asset(fs, path, MAX_FILE_BYTES)
        .map_err(|error| format!("Emission file {path}: {error}"))?
        .ok_or_else(|| format!("Emission file {path} was not found in mounted assets"))?;
    budget.consume(asset.bytes().len().div_ceil(64))?;
    let text = std::str::from_utf8(asset.bytes()).map_err(|_| "Emission file must be ASCII")?;
    let declarations = parse(text, &mut budget)?;
    let _material_preparation =
        budget.reserve_bytes(crate::materials::MAX_GEOMETRY_MATERIAL_ALLOCATION_BYTES)?;
    let mut materials = Materials::with_build_sources(fs, Requirements::GeometrySemantics, sources);
    let mut assets = super::props::Assets::for_brushes(fs, sources);
    let _caster_allocation = budget.reserve_bytes(
        caster_requests
            .len()
            .checked_mul(128)
            .ok_or("Emission caster lookup allocation overflow")?,
    )?;
    let mut casters = BTreeSet::new();
    for entity in caster_requests {
        budget.consume(1)?;
        casters.insert(entity.id);
    }
    let mut checked = BTreeSet::new();
    let mut sides = BTreeMap::new();
    for (solid, owner) in vmf
        .world
        .solids
        .iter()
        .map(|solid| (solid, Some(SourceOwner::World)))
        .chain(
            vmf.entities
                .iter()
                .filter(|entity| !entity.hidden)
                .flat_map(|entity| {
                    let owner = if entity
                        .classname()
                        .is_some_and(|name| name.eq_ignore_ascii_case("func_detail"))
                    {
                        Some(SourceOwner::World)
                    } else if casters.contains(&entity.id) {
                        Some(SourceOwner::Model(entity.id))
                    } else {
                        None
                    };
                    entity.solids.iter().map(move |solid| (solid, owner))
                }),
        )
    {
        if solid.hidden {
            continue;
        }
        admit_solid(
            solid,
            owner,
            options,
            &declarations,
            &mut materials,
            &mut assets,
            &mut checked,
            &mut sides,
            &mut budget,
            instances,
        )?;
    }
    let angle = maximum_angle(options.sky_quality);
    let stats = MaterialEmissionStats {
        file: path.into(),
        source_sha256: crate::asset_sources::hash(asset.bytes()),
        declared_materials: declarations.len(),
        matched_sides: sides.len(),
        source_faces: 0,
        source_triangles: 0,
        source_area: 0.0,
        channel_flux: [[0.0; 3]; 2],
        maximum_cell_angle_degrees: angle,
        boundary_cell_angle_degrees: angle / BOUNDARY_REFINEMENT,
        channel_cells: [0; 2],
        channel_rays: [0; 2],
        native_policy: "indexed_ambient_only_no_emission_worldlights",
    };
    report.diagnostics.push(Diagnostic::info(format!(
        "Material emission: {} declarations match {} original world/detail or stationary Shadows Yes brush-model sides; {}. Uniform radiance uses actual finite source area, sampled segment shadows and indexed model ambient only; no native emission world lights or shader glow",
        declarations.len(), sides.len(), if options.lighting.performs_lighting() { "baking requested" } else { "not baked in Fullbright" }
    ), None, None));
    budget.consume(0)?;
    Ok(Some(Input {
        budget,
        declarations,
        sides,
        sources: sources.clone(),
        stats,
        _allocation: allocation,
    }))
}

#[allow(clippy::too_many_arguments)]
fn admit_solid(
    solid: &Solid,
    owner: Option<SourceOwner>,
    options: Options,
    declarations: &BTreeMap<String, Declaration>,
    materials: &mut Materials<'_>,
    assets: &mut super::props::Assets<'_, '_>,
    checked: &mut BTreeSet<usize>,
    sides: &mut BTreeMap<u32, AdmittedSide>,
    budget: &mut Budget<'_>,
    instances: &crate::instances::Prepared<'_>,
) -> Result<(), String> {
    let terrain = solid.sides.iter().any(|side| side.displacement.is_some());
    for side in &solid.sides {
        budget.consume(1 + side.material.len().div_ceil(64))?;
        if side.material.len() > MAX_LINE_BYTES || !side.material.is_ascii() {
            continue;
        }
        let Ok(name) = material_name(&side.material) else {
            continue;
        };
        let Some(declaration) = declarations.get(&name) else {
            continue;
        };
        let result = (|| {
            let Some(owner) = owner.filter(|_| !terrain) else {
                return Err("Emission sources require ordinary flat world/detail brushes or admitted stationary func_brush models with Shadows Yes; terrain, unrequested and moving model sources are unavailable".into());
            };
            if options.lighting.performs_lighting()
                && (declaration.channels[0].is_none()
                    || (options.lighting == crate::LightingMode::DirectLdrHdr
                        && declaration.channels[1].is_none()))
            {
                return Err(
                    "Emission source is missing a declaration for a requested lighting channel"
                        .into(),
                );
            }
            let index = if let Some(index) = materials.cached(&name) {
                index
            } else {
                budget.consume(crate::materials::MAX_GEOMETRY_MATERIAL_BYTES.div_ceil(64))?;
                materials.load(&name)?
            };
            let info = materials.get(index);
            if !crate::instances::brush_models::ordinary(info)
                || !info.draws
                || info.surface & (surf::NODRAW | surf::SKY | surf::SKY2D) != 0
            {
                return Err(format!(
                    "Material {name} is not an ordinary drawn opaque emission source"
                ));
            }
            if checked.insert(index) {
                let _parsed = budget.reserve_bytes(
                    crate::MAX_BRUSH_SHADOW_MATERIAL_BYTES
                        * 128
                        * (source_formats::vmt::MAX_PATCH_DEPTH + 1),
                )?;
                assets.brush_opaque_material(&name, budget)?;
            }
            if sides.len() == MAX_SOURCE_SIDES {
                return Err("Emission exceeds 65536 matched original sides".into());
            }
            if sides
                .insert(
                    side.id,
                    AdmittedSide {
                        declaration: declaration.id,
                        owner,
                    },
                )
                .is_some()
            {
                return Err("Emission original side identity is ambiguous".into());
            }
            Ok::<(), String>(())
        })();
        result.map_err(|error| {
            format!(
                "Material emission {}: {error}",
                source_location(instances, solid.id, side.id)
            )
        })?;
    }
    Ok(())
}

fn source_location(instances: &crate::instances::Prepared<'_>, solid: u32, side: u32) -> String {
    if let Some((owner, path, original_solid, original_side)) = instances.face_origin(side) {
        format!(
            "entity:{owner} instance {} child solid:{original_solid} side:{original_side} (assembled solid:{solid} side:{side})",
            map_core::entity_helpers::bounded_preview(path, 512)
        )
    } else {
        format!("solid:{solid} side:{side}")
    }
}

impl<'a> Input<'a> {
    pub(crate) fn stats(&self) -> MaterialEmissionStats {
        self.stats.clone()
    }

    pub(super) fn into_parts(self) -> (Budget<'a>, Declarations) {
        (
            self.budget,
            Declarations {
                declarations: self.declarations,
                sides: self.sides,
                sources: self.sources,
                stats: self.stats,
                _allocation: self._allocation,
            },
        )
    }
}

pub(super) struct Declarations {
    declarations: BTreeMap<String, Declaration>,
    sides: BTreeMap<u32, AdmittedSide>,
    sources: BuildMaterialSources,
    stats: MaterialEmissionStats,
    _allocation: Allocation,
}

struct Source {
    face: usize,
    location: String,
    triangles: Vec<[DVec3; 3]>,
    radiance: [DVec3; 2],
    boundary: SourceBoundary,
}

pub(super) struct Prepared {
    sources: Vec<Source>,
    stats: MaterialEmissionStats,
    cells: [Cell<usize>; 2],
    rays: [Cell<usize>; 2],
    _allocation: Allocation,
}

#[derive(Clone, Copy)]
struct Patch {
    points: [DVec3; 3],
    depth: usize,
}

pub(super) struct Context<'a> {
    pub shadows: &'a ShadowIndex<Occluder>,
    pub terrain: &'a ShadowIndex<Triangle>,
    pub props: &'a ShadowIndex<super::props::PropTriangle>,
    pub model: Option<&'a ShadowIndex<Occluder>>,
    pub skip_owner: Option<u32>,
}

pub(super) struct Sample {
    pub point: DVec3,
    pub normal: DVec3,
    pub origin: DVec3,
    pub face: Option<usize>,
    pub angular: ReceiverAngularMode,
}

pub(super) fn prepare(
    declarations: &Declarations,
    map: &Map<'_>,
    bsp: &Bsp,
    scopes: &[super::models::Scope],
    face_sources: &[Arc<[u32]>],
    instances: &crate::instances::Prepared<'_>,
    budget: &mut Budget<'_>,
) -> Result<Prepared, String> {
    budget.consume(0)?;
    if !map.materials.uses_build_sources(&declarations.sources) {
        return Err("Material emission requires the map's original material capture".into());
    }
    if map.entities[0]
        .origin
        .is_some_and(|origin| origin != DVec3::ZERO)
    {
        return Err("Material emission requires a zero-origin world coordinate frame".into());
    }
    let decoded_bytes = [
        bsp::lump::FACES,
        bsp::lump::TEXINFO,
        bsp::lump::VERTEXES,
        bsp::lump::EDGES,
        bsp::lump::SURFEDGES,
    ]
    .into_iter()
    .try_fold(0usize, |bytes, index| {
        bytes.checked_add(bsp.lumps[index].data.len())
    })
    .ok_or("Emission decoded geometry allocation overflow")?;
    let _decoded = budget.reserve_bytes(decoded_bytes)?;
    budget.consume(decoded_bytes.div_ceil(64))?;
    let faces = bsp
        .lighting_faces(LightingKind::Ldr)
        .map_err(|error| error.to_string())?;
    let infos = bsp
        .records::<TexInfo>()
        .map_err(|error| error.to_string())?;
    let vertices = bsp.vertices().map_err(|error| error.to_string())?;
    let edges = bsp.records::<Edge>().map_err(|error| error.to_string())?;
    let surfedges = bsp.surfedges().map_err(|error| error.to_string())?;
    if face_sources.len() != faces.len() {
        return Err("Emission face contributors disagree with emitted geometry".into());
    }
    let source_side_count = declarations.sides.len();
    let allocation = budget.reserve_bytes(
        source_side_count
            .checked_mul(128)
            .ok_or("Emission source-side allocation overflow")?
            + faces.len() * (size_of::<Source>() * 2 + 1024)
            + MAX_TRIANGLES * size_of::<[DVec3; 3]>()
            + (MAX_DEPTH + 2) * size_of::<Patch>()
            + map.brushes.len() * 128,
    )?;
    let mut owners = BTreeMap::new();
    for scope in scopes {
        budget.consume(1)?;
        let entity = &map.entities[scope.entity];
        let source_owner = if scope.entity == 0 {
            SourceOwner::World
        } else {
            SourceOwner::Model(entity.id)
        };
        for &owner in &entity.brushes {
            budget.consume(1)?;
            for side in &map.brushes[owner].sides {
                budget.consume(1)?;
                let Some(id) = side.source_id.filter(|_| !side.bevel) else {
                    continue;
                };
                let Some(admitted) = declarations.sides.get(&id) else {
                    continue;
                };
                if admitted.owner != source_owner
                    || (scope.entity != 0
                        && !crate::brush_shadow_casters::enabled(&entity.properties))
                {
                    return Err(
                        "Emission source lost its admitted world or stationary global-caster scope"
                            .into(),
                    );
                }
                if owners.insert(id, (owner, side, scope.entity)).is_some() {
                    return Err("Emission original side ownership is ambiguous".into());
                }
            }
        }
    }
    let mut sources = Vec::new();
    let mut stats = declarations.stats.clone();
    for (scope, index) in scopes
        .iter()
        .flat_map(|scope| scope.faces.clone().map(move |index| (scope, index)))
    {
        budget.consume(1)?;
        let face = &faces[index];
        let info = &infos[face.texinfo as usize];
        let material = map.materials.get(map.texdata[info.texdata as usize]);
        let name = material_name(&material.name).unwrap_or_else(|_| material.name.clone());
        budget.consume(face_sources[index].len())?;
        if !face_sources[index]
            .iter()
            .any(|id| declarations.sides.contains_key(id))
            && !declarations.declarations.contains_key(&name)
        {
            continue;
        }
        let result = (|| {
            let [id] = face_sources[index].as_ref() else {
                return Err(
                    "Emission sources require one original contributor per emitted face".into(),
                );
            };
            let &(owner, side, entity) = owners
                .get(id)
                .ok_or("Emission source has no admitted original brush owner")?;
            if entity != scope.entity {
                return Err("Emission face disagrees with its original model scope".into());
            }
            let original_name = material_name(&map.materials.get(side.material).name)?;
            let declaration = declarations
                .declarations
                .get(&original_name)
                .ok_or("Emission source material changed from its original contributor")?;
            if declarations.sides.get(id).map(|side| side.declaration) != Some(declaration.id)
                || face.dispinfo >= 0
            {
                return Err("Emission face lost its admitted original source".into());
            }
            if side.plane != usize::from(face.plane) || face.side != (face.plane & 1) as u8 {
                return Err(
                    "Emission source does not retain its exact outward plane identity".into(),
                );
            }
            if map.brushes[owner].contents & source_formats::bsp::contents::SOLID == 0 {
                return Err(
                    "Emission source owner is missing from the ordinary solid shadow scope".into(),
                );
            }
            let local_plane = map.planes.get(side.plane);
            let plane = scope.pose.plane(map_core::Plane {
                normal: local_plane.normal,
                dist: local_plane.dist,
            });
            budget.consume(face.edge_count as usize)?;
            let points = surfedges
                [face.first_edge as usize..face.first_edge as usize + face.edge_count as usize]
                .iter()
                .map(|edge| {
                    let vertices_on_edge = edges[edge.unsigned_abs() as usize].0;
                    let point = DVec3::from_array(
                        vertices[vertices_on_edge[usize::from(*edge < 0)] as usize].map(f64::from),
                    );
                    if scope.entity == 0 {
                        point
                    } else {
                        scope.pose.point(point)
                    }
                })
                .collect::<Vec<_>>();
            if points.len() < 3
                || points.iter().any(|point| {
                    !point.is_finite() || point.abs().max_element() > crate::map::MAX_COMPILE_COORD
                })
            {
                return Err("Emission source needs at least three finite native vertices within Source coordinate bounds".into());
            }
            let mut triangles = Vec::with_capacity(points.len() - 2);
            let mut face_area = 0.0;
            for pair in points[1..].windows(2) {
                budget.consume(1)?;
                let cross = (pair[1] - points[0]).cross(pair[0] - points[0]);
                let area = cross.length() * 0.5;
                if area == 0.0 {
                    continue;
                }
                if !area.is_finite() || cross.dot(plane.normal) <= 0.0 {
                    return Err("Emission fan needs finite nondegenerate outward triangles".into());
                }
                stats.source_triangles += 1;
                if stats.source_triangles > MAX_TRIANGLES {
                    return Err("Emission exceeds 262144 native source triangles".into());
                }
                stats.source_area += area;
                face_area += area;
                triangles.push([points[0], pair[1], pair[0]]);
            }
            if triangles.is_empty() {
                return Err("Emission source has no positive native area".into());
            }
            if !stats.source_area.is_finite() {
                return Err("Emission source area overflow".into());
            }
            for (channel, radiance) in declaration.channels.into_iter().enumerate() {
                let flux = DVec3::from_array(stats.channel_flux[channel])
                    + radiance.unwrap_or(DVec3::ZERO) * (PI * face_area);
                if !flux.is_finite() {
                    return Err("Material emission source power overflows finite RGB flux".into());
                }
                stats.channel_flux[channel] = flux.to_array();
            }
            sources.push(Source {
                location: source_location(instances, map.brushes[owner].id, *id),
                face: index,
                triangles,
                radiance: declaration
                    .channels
                    .map(|value| value.unwrap_or(DVec3::ZERO)),
                boundary: SourceBoundary {
                    plane: side.plane,
                    equation: plane,
                    sides: Arc::from([*id]),
                    owners: vec![owner],
                },
            });
            Ok::<(), String>(())
        })();
        result
            .map_err(|error| format!("Material emission face:{index} material {name}: {error}"))?;
    }
    stats.source_faces = sources.len();
    budget.consume(0)?;
    Ok(Prepared {
        sources,
        stats,
        cells: std::array::from_fn(|_| Cell::new(0)),
        rays: std::array::from_fn(|_| Cell::new(0)),
        _allocation: allocation,
    })
}

fn unit(value: DVec3) -> Result<DVec3, String> {
    value
        .try_normalize()
        .filter(|value| value.is_finite())
        .ok_or_else(|| "Emission sample touches an unconditioned source vertex".into())
}

fn clipped(
    points: [DVec3; 3],
    point: DVec3,
    normal: DVec3,
    angular: ReceiverAngularMode,
) -> ([DVec3; 6], usize) {
    let mut result = [DVec3::ZERO; 6];
    let mut count = 0;
    for index in 0..3 {
        let a = points[index];
        let b = points[(index + 1) % 3];
        let da = normal.dot(a - point);
        let db = normal.dot(b - point);
        if matches!(angular, ReceiverAngularMode::IgnoreNormals) || da >= 0.0 {
            result[count] = a;
            count += 1;
        }
        if matches!(angular, ReceiverAngularMode::Normal) && (da >= 0.0) != (db >= 0.0) {
            result[count] = a + (b - a) * (da / (da - db));
            count += 1;
        }
    }
    (result, count)
}

fn weight(points: [DVec3; 3], sample: &Sample) -> Result<f64, String> {
    let source_normal = (points[1] - points[0]).cross(points[2] - points[0]);
    if source_normal.dot(sample.point - points[0]) <= 0.0 {
        return Ok(0.0);
    }
    let (polygon, count) = clipped(points, sample.point, sample.normal, sample.angular);
    if count < 3 {
        return Ok(0.0);
    }
    let mut directions = [DVec3::ZERO; 6];
    for index in 0..count {
        directions[index] = unit(polygon[index] - sample.point)?;
    }
    let value = match sample.angular {
        ReceiverAngularMode::Normal => {
            let mut vector = DVec3::ZERO;
            for index in 0..count {
                let a = directions[index];
                let b = directions[(index + 1) % count];
                let cross = a.cross(b);
                let sine = cross.length();
                let cosine = a.dot(b).clamp(-1.0, 1.0);
                if sine == 0.0 {
                    if cosine < 0.0 {
                        return Err("Emission source angular edge is singular".into());
                    }
                    continue;
                }
                vector += cross * (sine.atan2(cosine) / sine);
            }
            -0.5 * sample.normal.dot(vector)
        }
        ReceiverAngularMode::IgnoreNormals => {
            let a = directions[0];
            let mut angle = 0.0;
            for index in 1..count - 1 {
                let b = directions[index];
                let c = directions[index + 1];
                angle += 2.0
                    * a.dot(b.cross(c))
                        .abs()
                        .atan2(1.0 + a.dot(b) + b.dot(c) + c.dot(a));
            }
            super::IGNORE_NORMALS_FACTOR * angle
        }
    };
    let allowance = ARITHMETIC_OPERATIONS * f64::EPSILON;
    if !value.is_finite() || value < -allowance {
        return Err("Emission finite-cell integral is nonfinite or negative".into());
    }
    Ok(value.max(0.0))
}

fn edge(points: [DVec3; 3], sample: &Sample) -> Result<(usize, f64), String> {
    let mut largest = (0, 1.0);
    for origin in [sample.point, sample.origin] {
        let directions = [
            unit(points[0] - origin)?,
            unit(points[1] - origin)?,
            unit(points[2] - origin)?,
        ];
        for index in 0..3 {
            let cosine = directions[index]
                .dot(directions[(index + 1) % 3])
                .clamp(-1.0, 1.0);
            if cosine < largest.1 {
                largest = (index, cosine);
            }
        }
    }
    Ok(largest)
}

fn split(patch: Patch, edge: usize) -> [Patch; 2] {
    let a = patch.points[edge];
    let b = patch.points[(edge + 1) % 3];
    let c = patch.points[(edge + 2) % 3];
    let middle = (a + b) * 0.5;
    [
        Patch {
            points: [a, middle, c],
            depth: patch.depth + 1,
        },
        Patch {
            points: [middle, b, c],
            depth: patch.depth + 1,
        },
    ]
}

impl Prepared {
    pub(super) fn has_sources(&self) -> bool {
        !self.sources.is_empty()
    }

    pub(super) fn stats(&self) -> MaterialEmissionStats {
        let mut stats = self.stats.clone();
        stats.channel_cells = self.cells.each_ref().map(Cell::get);
        stats.channel_rays = self.rays.each_ref().map(Cell::get);
        stats
    }

    pub(super) fn gather(
        &self,
        kind: LightingKind,
        sample: Sample,
        context: Context<'_>,
        budget: &mut Budget<'_>,
    ) -> Result<DVec3, String> {
        let channel = usize::from(kind == LightingKind::Hdr);
        budget.consume(2)?;
        budget.clear_emitter();
        let mut sample = sample;
        sample.normal = unit(sample.normal)?;
        if !sample.point.is_finite() || !sample.origin.is_finite() {
            return Err("Emission receiver must be finite".into());
        }
        let angle = self.stats.maximum_cell_angle_degrees.to_radians();
        let threshold = angle.cos();
        let boundary_threshold = (angle / BOUNDARY_REFINEMENT).cos();
        let mut total = DVec3::ZERO;
        let mut visited = 0usize;
        for source in &self.sources {
            budget.consume(1)?;
            if sample.face == Some(source.face) || source.radiance[channel] == DVec3::ZERO {
                continue;
            }
            let result = (|| {
                let mut integral = 0.0;
                for &triangle in &source.triangles {
                    let mut stack = [Patch {
                        points: [DVec3::ZERO; 3],
                        depth: 0,
                    }; MAX_DEPTH + 2];
                    stack[0] = Patch {
                        points: triangle,
                        depth: 0,
                    };
                    let mut pending = 1;
                    while pending > 0 {
                        pending -= 1;
                        let patch = stack[pending];
                        budget.consume(64)?;
                        visited += 1;
                        if visited > MAX_CELLS {
                            return Err("Emission exceeds 262144 cells for one receiver; increase source separation or reduce sampling quality".into());
                        }
                        self.cells[channel].set(
                            self.cells[channel]
                                .get()
                                .checked_add(1)
                                .ok_or("Emission cell count overflow")?,
                        );
                        let cell_weight = weight(patch.points, &sample)?;
                        if cell_weight == 0.0 {
                            continue;
                        }
                        let (longest, cosine) = edge(patch.points, &sample)?;
                        let mut subdivide = cosine < threshold;
                        let mut visibility = 0.0;
                        if !subdivide {
                            let mut minimum = 1.0_f64;
                            let mut maximum = 0.0_f64;
                            for weights in SAMPLE_WEIGHTS {
                                budget.consume(16)?;
                                let end = patch
                                    .points
                                    .into_iter()
                                    .zip(weights)
                                    .map(|(point, weight)| point * weight)
                                    .sum::<DVec3>();
                                let error = patch
                                    .points
                                    .into_iter()
                                    .zip(weights)
                                    .map(|(point, weight)| {
                                        source.boundary.equation.distance_to(point).abs()
                                            * weight.abs()
                                    })
                                    .sum::<f64>();
                                self.rays[channel].set(
                                    self.rays[channel]
                                        .get()
                                        .checked_add(1)
                                        .ok_or("Emission ray count overflow")?,
                                );
                                let blocked = source_blocked(
                                    context.shadows,
                                    sample.origin,
                                    end,
                                    Some(&source.boundary),
                                    error,
                                    budget,
                                )? || context
                                    .model
                                    .map(|model| {
                                        source_blocked(
                                            model,
                                            sample.origin,
                                            end,
                                            Some(&source.boundary),
                                            error,
                                            budget,
                                        )
                                    })
                                    .transpose()?
                                    .unwrap_or(false)
                                    || context.terrain.blocked(
                                        sample.origin,
                                        end,
                                        &mut |work| budget.consume(work),
                                    )?;
                                let value = if blocked {
                                    0.0
                                } else {
                                    context.props.visibility_skipping_owner(
                                        sample.origin,
                                        end,
                                        context.skip_owner,
                                        &mut |work| budget.consume(work),
                                    )?
                                };
                                minimum = minimum.min(value);
                                maximum = maximum.max(value);
                                visibility += value / SAMPLE_WEIGHTS.len() as f64;
                            }
                            subdivide = minimum != maximum && cosine < boundary_threshold;
                        }
                        if subdivide {
                            if patch.depth == MAX_DEPTH || pending + 2 > stack.len() {
                                return Err("Emission subdivision cannot reach the selected angular quality within depth 48".into());
                            }
                            let children = split(patch, longest);
                            if children.iter().any(|child| child.points == patch.points) {
                                return Err(
                                    "Emission subdivision lost native coordinate precision".into(),
                                );
                            }
                            stack[pending] = children[1];
                            stack[pending + 1] = children[0];
                            pending += 2;
                        } else {
                            integral += cell_weight * visibility;
                        }
                    }
                }
                Ok::<f64, String>(integral)
            })();
            let integral = result.map_err(|error| {
                format!(
                    "Material emission {} face:{} receiver {:#?}: {error}",
                    source.location, source.face, sample.point
                )
            })?;
            total += source.radiance[channel] * (LIGHTMAP_SCALE / PI * integral);
        }
        budget.consume(0)?;
        if !total.is_finite() || total.min_element() < 0.0 {
            return Err("Material emission illumination overflow".into());
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::super::HullSideSource;
    use super::*;
    use map_core::Plane;

    fn receiver(point: DVec3) -> Sample {
        Sample {
            point,
            origin: point,
            normal: -DVec3::Z,
            face: None,
            angular: ReceiverAngularMode::Normal,
        }
    }

    fn rectangle(bounds: [f64; 4], point: DVec3, tilt: f64) -> f64 {
        let [x0, y0, x1, y1] = bounds;
        let (mut a, b, c, d) = (x0 - point.x, x1 - point.x, y0 - point.y, y1 - point.y);
        let (beta, alpha) = tilt.sin_cos();
        if beta > 0.0 {
            a = a.max(-alpha * point.z / beta);
        }
        if a >= b {
            return 0.0;
        }
        let line = |x: f64, low: f64, high: f64| {
            let span = x.hypot(point.z);
            ((high / span).atan() - (low / span).atan()) / span
        };
        let ea = line(a, c, d);
        let eb = line(b, c, d);
        alpha * 0.5 * (b * eb - a * ea + d * line(d, a, b) - c * line(c, a, b))
            + beta * 0.5 * point.z * (ea - eb)
    }

    fn triangles(bounds: [f64; 4], z: f64) -> Vec<[DVec3; 3]> {
        let [x0, y0, x1, y1] = bounds;
        let a = DVec3::new(x0, y0, z);
        let b = DVec3::new(x1, y0, z);
        let c = DVec3::new(x1, y1, z);
        let d = DVec3::new(x0, y1, z);
        vec![[a, b, c], [a, c, d]]
    }

    fn emitter(bounds: [f64; 4], z: f64, quality: SkyQuality, budget: &Budget<'_>) -> Prepared {
        let angle = maximum_angle(quality);
        Prepared {
            sources: vec![Source {
                face: 0,
                location: "solid:7 side:42".into(),
                triangles: triangles(bounds, z),
                radiance: [DVec3::ONE * (PI / LIGHTMAP_SCALE); 2],
                boundary: SourceBoundary {
                    plane: 0,
                    equation: Plane {
                        normal: DVec3::Z,
                        dist: 0.0,
                    },
                    sides: Arc::from([42]),
                    owners: vec![0],
                },
            }],
            stats: MaterialEmissionStats {
                file: "test.rad".into(),
                source_sha256: String::new(),
                declared_materials: 1,
                matched_sides: 1,
                source_faces: 1,
                source_triangles: 2,
                source_area: (bounds[2] - bounds[0]) * (bounds[3] - bounds[1]),
                channel_flux: [[0.0; 3]; 2],
                maximum_cell_angle_degrees: angle,
                boundary_cell_angle_degrees: angle / BOUNDARY_REFINEMENT,
                channel_cells: [0; 2],
                channel_rays: [0; 2],
                native_policy: "indexed_ambient_only_no_emission_worldlights",
            },
            cells: std::array::from_fn(|_| Cell::new(0)),
            rays: std::array::from_fn(|_| Cell::new(0)),
            _allocation: budget.reserve_bytes(4096).unwrap(),
        }
    }

    fn hull(mins: DVec3, maxs: DVec3, source: bool) -> Occluder {
        let mut planes = Vec::new();
        for axis in 0..3 {
            let mut normal = DVec3::ZERO;
            normal[axis] = 1.0;
            planes.push(Plane {
                normal,
                dist: maxs[axis],
            });
            planes.push(Plane {
                normal: -normal,
                dist: -mins[axis],
            });
        }
        let hull = Occluder::new(planes, mins, maxs).unwrap();
        if source {
            hull.with_source(
                0,
                (0..6)
                    .map(|index| HullSideSource {
                        plane: if index == 4 { 0 } else { index + 1 },
                        side: (index == 4).then_some(42),
                    })
                    .collect(),
            )
            .unwrap()
        } else {
            hull
        }
    }

    #[test]
    fn declarations_are_explicit_bounded_and_independent_by_channel() {
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let parsed = parse(
            "// source\nldr: materials/test/panel.vmt 255 0 0 510\nhdr: test/panel 0 0 255 1020\n",
            &mut budget,
        )
        .unwrap();
        assert_eq!(
            parsed["test/panel"].channels,
            [Some(DVec3::X * 2.0), Some(DVec3::Z * 4.0)]
        );
        for value in [
            "a 255 255 255 1\na 255 255 255 2",
            "a 256 0 0 1",
            "a 1 2 3 nan",
            "a 1 2 3 1 extra",
            "materials/.vmt 1 2 3 1",
            "a 1 2 3 1e999",
        ] {
            assert!(parse(value, &mut budget).is_err(), "{value}");
        }
        for value in ["", " scripts/test.rad ", "scripts/sub/test.rad"] {
            validate_material_emission_file(value).unwrap();
        }
        for value in [
            "/test.rad",
            "../test.rad",
            "scripts//test.rad",
            "scripts/./test.rad",
            "scripts\\test.rad",
        ] {
            assert!(validate_material_emission_file(value).is_err(), "{value}");
        }
        let mut properties = Properties::default();
        properties.set(MATERIAL_EMISSION_FILE_KEY, "test.rad");
        properties
            .0
            .push(("_Hammerr_emission_file".into(), "other.rad".into()));
        assert!(file(&properties).is_err());
    }

    #[test]
    fn repeated_sides_charge_one_material_load_and_keep_the_original_capture() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/validation/material-emission-cache");
        std::fs::create_dir_all(&root).unwrap();
        let directory = tempfile::tempdir_in(root).unwrap();
        std::fs::create_dir_all(directory.path().join("materials/dev")).unwrap();
        std::fs::write(
            directory
                .path()
                .join("materials/dev/dev_measuregeneric01.vmt"),
            "LightmappedGeneric { }",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("test.rad"),
            "dev/dev_measuregeneric01 255 0 0 100\n",
        )
        .unwrap();
        let mut game = crate::test_common::empty_game();
        game.mounts.push(source_formats::gamefs::MountedPath {
            ids: vec!["game".into()],
            mount: source_formats::gamefs::Mount::Directory(directory.path().to_owned()),
        });
        let mut vmf = crate::test_common::room();
        vmf.world
            .properties
            .set(MATERIAL_EMISSION_FILE_KEY, "test.rad");
        let control = Control::default();
        let mut report = Report::default();
        let instances =
            crate::instances::prepare_for_compile(&vmf, None, &game, false, &control, &mut report)
                .unwrap();
        let sources = BuildMaterialSources::default();
        let input = preflight(
            &instances,
            &[],
            &game,
            &sources,
            Options::default(),
            &control,
            &mut report,
        )
        .unwrap()
        .unwrap();
        assert_eq!(input.stats.matched_sides, 36);
        assert!(
            input.budget.work() < 2 * crate::materials::MAX_GEOMETRY_MATERIAL_BYTES.div_ceil(64)
        );
        assert!(input.sources.same_capture(&sources));
        assert!(sources.retained("test.rad").unwrap().is_some());
    }

    #[test]
    fn finite_weights_match_independent_near_field_rectangle_and_horizon_references() {
        for size in [16.0, 64.0, 128.0] {
            let h = size * 0.5;
            let bounds = [-h, -h, h, h];
            for z in [0.01, 0.1, 0.5, 1.0, 2.0, 4.0, 8.0, 32.0, 128.0] {
                for (x, y) in [
                    (0.0, 0.0),
                    (h, 0.0),
                    (h, h),
                    (h + 0.25, 0.0),
                    (h / 3.0, -h / 5.0),
                ] {
                    for tilt in [0.0, PI / 6.0, PI / 3.0] {
                        let mut sample = receiver(DVec3::new(x, y, z));
                        sample.normal = DVec3::new(tilt.sin(), 0.0, -tilt.cos());
                        let actual = triangles(bounds, 0.0)
                            .into_iter()
                            .map(|points| weight(points, &sample).unwrap())
                            .sum::<f64>();
                        let expected = rectangle(bounds, sample.point, tilt);
                        assert!(
                            (actual - expected).abs() <= 1e-10 + expected.abs() * 1e-10,
                            "size {size} point {:?} tilt {tilt}: {actual} != {expected}",
                            sample.point
                        );
                    }
                    let mut sample = receiver(DVec3::new(x, y, z));
                    sample.angular = ReceiverAngularMode::IgnoreNormals;
                    let corner = |u: f64, v: f64| (u * v).atan2(z * (u * u + v * v + z * z).sqrt());
                    let expected = super::super::IGNORE_NORMALS_FACTOR
                        * (corner(h - x, h - y) - corner(-h - x, h - y) - corner(h - x, -h - y)
                            + corner(-h - x, -h - y));
                    let actual = triangles(bounds, 0.0)
                        .into_iter()
                        .map(|points| weight(points, &sample).unwrap())
                        .sum::<f64>();
                    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
                }
            }
        }
    }

    #[test]
    fn source_identity_keeps_near_terminal_blockers_and_terrain_and_cancellation() {
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let prepared = emitter([-8.0, -8.0, 8.0, 8.0], -0.00001, SkyQuality::Fast, &budget);
        let own = hull(
            DVec3::new(-8.0, -8.0, -4.0),
            DVec3::new(8.0, 8.0, 0.0),
            true,
        );
        let props =
            ShadowIndex::<super::super::props::PropTriangle>::new(Vec::new(), &mut |work| {
                budget.consume(work)
            })
            .unwrap();
        let terrain =
            ShadowIndex::<Triangle>::new(Vec::new(), &mut |work| budget.consume(work)).unwrap();
        for blocked in [false, true] {
            let mut hulls = vec![own.clone()];
            if blocked {
                hulls.push(hull(
                    DVec3::new(-9.0, -9.0, 0.001),
                    DVec3::new(9.0, 9.0, 0.002),
                    false,
                ));
            }
            let shadows = ShadowIndex::new(hulls, &mut |work| budget.consume(work)).unwrap();
            let color = prepared
                .gather(
                    LightingKind::Ldr,
                    receiver(DVec3::Z * 16.0),
                    Context {
                        shadows: &shadows,
                        terrain: &terrain,
                        props: &props,
                        model: None,
                        skip_owner: None,
                    },
                    &mut budget,
                )
                .unwrap();
            if blocked {
                assert_eq!(color, DVec3::ZERO);
            } else {
                assert!(color.min_element() > 0.0);
            }
        }
        for (z, blocked) in [(0.00001, false), (-0.00001, true)] {
            let source = emitter([-8.0, -8.0, 8.0, 8.0], z, SkyQuality::Fast, &budget);
            let biased = Sample {
                point: DVec3::new(32.0, 0.0, 0.00002),
                origin: DVec3::new(31.5, 0.0, -0.000001),
                normal: DVec3::new(-1.0, 0.0, -0.000042),
                face: None,
                angular: ReceiverAngularMode::Normal,
            };
            let endpoint = DVec3::Z * z;
            assert_eq!(
                own.interval(biased.origin, endpoint).unwrap().is_some(),
                blocked
            );
            let shadows =
                ShadowIndex::new(vec![own.clone()], &mut |work| budget.consume(work)).unwrap();
            assert_eq!(
                source_blocked(
                    &shadows,
                    biased.origin,
                    endpoint,
                    Some(&source.sources[0].boundary),
                    z.abs(),
                    &mut budget,
                )
                .unwrap(),
                blocked
            );
            let color = source
                .gather(
                    LightingKind::Ldr,
                    biased,
                    Context {
                        shadows: &shadows,
                        terrain: &terrain,
                        props: &props,
                        model: None,
                        skip_owner: None,
                    },
                    &mut budget,
                )
                .unwrap();
            if blocked {
                assert_eq!(color, DVec3::ZERO);
            } else {
                assert!(color.min_element() > 0.0);
            }
        }
        let shadows = ShadowIndex::new(vec![own], &mut |work| budget.consume(work)).unwrap();
        let terrain = ShadowIndex::new(
            triangles([-32.0, -32.0, 32.0, 32.0], 8.0)
                .into_iter()
                .map(|points| Triangle::new(points).unwrap())
                .collect(),
            &mut |work| budget.consume(work),
        )
        .unwrap();
        let color = prepared
            .gather(
                LightingKind::Ldr,
                receiver(DVec3::Z * 16.0),
                Context {
                    shadows: &shadows,
                    terrain: &terrain,
                    props: &props,
                    model: None,
                    skip_owner: None,
                },
                &mut budget,
            )
            .unwrap();
        assert_eq!(color, DVec3::ZERO);
        control.cancel().unwrap();
        assert!(
            prepared
                .gather(
                    LightingKind::Ldr,
                    receiver(DVec3::Z * 16.0),
                    Context {
                        shadows: &shadows,
                        terrain: &terrain,
                        props: &props,
                        model: None,
                        skip_owner: None
                    },
                    &mut budget
                )
                .is_err()
        );
    }

    #[test]
    fn sampled_aperture_has_an_independent_finite_area_reference() {
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let prepared = emitter(
            [-64.0, -64.0, 64.0, 64.0],
            0.0,
            SkyQuality::Detailed,
            &budget,
        );
        let shadows =
            ShadowIndex::<Occluder>::new(Vec::new(), &mut |work| budget.consume(work)).unwrap();
        let props =
            ShadowIndex::<super::super::props::PropTriangle>::new(Vec::new(), &mut |work| {
                budget.consume(work)
            })
            .unwrap();
        let blockers = [
            [-128.0, -128.0, -0.5, 128.0],
            [0.5, -128.0, 128.0, 128.0],
            [-0.5, -128.0, 0.5, -0.5],
            [-0.5, 0.5, 0.5, 128.0],
        ]
        .into_iter()
        .flat_map(|bounds| triangles(bounds, 16.0))
        .map(|points| Triangle::new(points).unwrap())
        .collect();
        let terrain = ShadowIndex::new(blockers, &mut |work| budget.consume(work)).unwrap();
        for x in [0.0, 0.25, 0.75] {
            let point = DVec3::new(x, 0.0, 32.0);
            let actual = prepared
                .gather(
                    LightingKind::Ldr,
                    receiver(point),
                    Context {
                        shadows: &shadows,
                        terrain: &terrain,
                        props: &props,
                        model: None,
                        skip_owner: None,
                    },
                    &mut budget,
                )
                .unwrap()
                .x;
            let expected = rectangle([-1.0 - x, -1.0, 1.0 - x, 1.0], point, 0.0);
            assert!(
                (actual - expected).abs() <= 0.0002 * PI + 0.02 * expected,
                "phase {x}: {actual} != {expected}"
            );
        }
    }
}
