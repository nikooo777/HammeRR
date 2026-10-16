use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use source_formats::bsp::PakEntry;
use source_formats::gamefs::GameFs;
use source_formats::vmf::Vmf;

use super::material::{Graph, ensure_default_unmounted, material_name, sample_texture};
use super::*;
use crate::map::Map;
use crate::{Control, Report};

pub(crate) struct Plan {
    pub manifest: CubemapAssetManifest,
    pub entries: Vec<PakEntry>,
    eligible_faces: BTreeSet<u32>,
}

struct Side {
    id: u32,
    texinfo: usize,
    center: DVec3,
    normal: DVec3,
}

pub fn inspect_cubemap_bindings(
    vmf: &Vmf,
    fs: &GameFs,
    query: &CubemapBindingQuery,
    control: &Control,
) -> Result<CubemapBindingReport, String> {
    if query.limit > 1024 || query.query.as_ref().is_some_and(|text| text.len() > 256) {
        return Err("Cubemap binding query exceeds its limit".into());
    }
    let requested: BTreeSet<_> = query.face_ids.iter().copied().collect();
    if requested.len() > 1024 || requested.len() != query.face_ids.len() || requested.contains(&0) {
        return Err(
            "Cubemap inspection face IDs must be distinct positive IDs, at most 1024".into(),
        );
    }
    let mut report = Report::default();
    let context = CompileContext::new(query.namespace.as_deref().unwrap_or("untitled"))?;
    let prepared = crate::cordons::prepare(vmf, fs, &mut report, control)?;
    let included: BTreeSet<_> = prepared
        .vmf
        .entities
        .iter()
        .map(|entity| entity.id)
        .collect();
    let helpers: Vec<_> = vmf
        .entities
        .iter()
        .filter(|entity| {
            entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("env_cubemap"))
        })
        .take(4097)
        .map(|entity| CubemapHelperStatus {
            helper_id: entity.id,
            hidden: entity.hidden,
            excluded: !included.contains(&entity.id),
            origin: entity.origin().map(|origin| origin.to_array()),
        })
        .collect();
    if helpers.len() > 4096 {
        return Err("Cubemap helper inspection limit exceeded".into());
    }
    if query
        .helper_id
        .is_some_and(|id| !helpers.iter().any(|helper| helper.helper_id == id))
    {
        return Err("Requested cubemap helper is missing".into());
    }
    let mut map = Map::load_controlled(prepared.vmf.as_ref(), fs, &mut report, control)?;
    if report.has_errors() {
        return Err(report
            .diagnostics
            .iter()
            .filter(|item| item.level == crate::Level::Error)
            .map(|item| item.message.as_str())
            .collect::<Vec<_>>()
            .join("; "));
    }
    if map.cubemaps.is_empty() {
        if !requested.is_empty() {
            return Err(
                "Explicit cubemap face inspection requires a visible included helper".into(),
            );
        }
        return Ok(CubemapBindingReport {
            namespace: context.namespace().into(),
            helpers,
            samples: Vec::new(),
            total: 0,
            assignments: Vec::new(),
        });
    }
    let plan = prepare(
        prepared.vmf.as_ref(),
        fs,
        &mut map,
        &context,
        false,
        false,
        control,
    )?;
    if !requested.is_subset(&plan.eligible_faces) {
        return Err("Requested cubemap face is missing, hidden, outside the compile region or does not produce a visible compiled side".into());
    }
    let needle = query.query.as_deref().unwrap_or("").to_ascii_lowercase();
    let filtered: Vec<_> = plan
        .manifest
        .assignments
        .into_iter()
        .filter(|assignment| {
            (requested.is_empty() || requested.contains(&assignment.face_id))
                && query
                    .helper_id
                    .is_none_or(|helper| helper == assignment.helper_id)
                && (needle.is_empty()
                    || assignment.material.contains(&needle)
                    || assignment.face_id.to_string().contains(&needle))
        })
        .collect();
    let total = filtered.len();
    let limit = if query.limit == 0 { 256 } else { query.limit };
    Ok(CubemapBindingReport {
        namespace: context.namespace().into(),
        helpers,
        samples: plan.manifest.samples,
        total,
        assignments: filtered
            .into_iter()
            .skip(query.offset)
            .take(limit)
            .collect(),
    })
}

pub(crate) fn capture_preflight(vmf: &Vmf) -> Result<(), String> {
    native_runtime::preflight_vmf(vmf).map_err(|error| error.to_string())
}

pub(crate) fn prepare(
    vmf: &Vmf,
    fs: &GameFs,
    map: &mut Map<'_>,
    context: &CompileContext,
    capture_scene: bool,
    hdr: bool,
    control: &Control,
) -> Result<Plan, String> {
    let helpers: Vec<_> = vmf
        .entities
        .iter()
        .filter(|entity| {
            !entity.hidden
                && entity
                    .classname()
                    .is_some_and(|name| name.eq_ignore_ascii_case("env_cubemap"))
        })
        .collect();
    if helpers.is_empty() || helpers.len() > MAX_SAMPLES || helpers.len() != map.cubemaps.len() {
        return Err("Generate reflections requires 1–32 valid visible env_cubemap helpers".into());
    }
    if capture_scene {
        ensure_default_unmounted(fs, context.namespace())?;
        if hdr {
            super::material::ensure_hdr_default_unmounted(fs, context.namespace())?;
        }
    }
    let mut positions = BTreeSet::new();
    let mut samples = Vec::with_capacity(helpers.len());
    let mut hdr_samples = Vec::new();
    for (entity, cube) in helpers.iter().zip(&map.cubemaps) {
        control.checkpoint()?;
        let edge = if cube.size == 0 {
            128
        } else {
            1u16.checked_shl(u32::from(cube.size - 1))
                .ok_or("Invalid cubemap size")?
        };
        if edge > MAX_CUBE_EDGE {
            return Err(format!(
                "entity:{}: native reflection capture currently supports cube edges up to {MAX_CUBE_EDGE}; size 256 requires a larger verified viewport",
                entity.id
            ));
        }
        if !positions.insert(cube.origin) {
            return Err(format!(
                "entity:{}: cubemap helpers collide after native integer origin quantization at {:?}",
                entity.id, cube.origin
            ));
        }
        let bootstrap = super::manifest::bootstrap_cube(edge)?;
        let sha256 = hash(&bootstrap);
        let texture = sample_texture(context.namespace(), cube.origin);
        if fs
            .read_limited(&texture, 1)
            .map_err(|error| {
                format!("Generated reflection sample path collision: {texture}: {error}")
            })?
            .is_some()
        {
            return Err(format!(
                "Generated reflection sample collides with mounted content: {texture}"
            ));
        }
        samples.push(CubemapSampleAsset {
            helper_id: entity.id,
            origin: cube.origin,
            size_enum: cube.size,
            edge,
            texture,
            bootstrap_sha256: sha256.clone(),
            sha256,
        });
        if hdr {
            let mut sample = samples.last().unwrap().clone();
            sample.texture = super::material::hdr_sample_texture(context.namespace(), cube.origin);
            if fs
                .read_limited(&sample.texture, 1)
                .map_err(|error| {
                    format!(
                        "Generated HDR reflection sample path collision: {}: {error}",
                        sample.texture
                    )
                })?
                .is_some()
            {
                return Err(format!(
                    "Generated HDR reflection sample collides with mounted content: {}",
                    sample.texture
                ));
            }
            sample.bootstrap_sha256 = hash(&super::hdr::bootstrap_cube(edge)?);
            sample.sha256 = sample.bootstrap_sha256.clone();
            hdr_samples.push(sample);
        }
    }
    let mut sides = BTreeMap::new();
    for brush in &map.brushes {
        let origin = map.entities[brush.entity].origin.unwrap_or(DVec3::ZERO);
        for side in &brush.sides {
            if !side.visible || side.bevel {
                continue;
            }
            let (Some(id), Some(winding)) = (side.source_id, &side.winding) else {
                continue;
            };
            if winding.0.is_empty() {
                return Err(format!(
                    "side:{id}: cubemap assignment needs a finite source winding"
                ));
            }
            let center = winding.0.iter().copied().sum::<DVec3>() / winding.0.len() as f64 + origin;
            let normal = map.planes.get(side.plane).normal;
            if sides
                .insert(
                    id,
                    Side {
                        id,
                        texinfo: side.texinfo,
                        center,
                        normal,
                    },
                )
                .is_some()
            {
                return Err(format!("side:{id}: cubemap source provenance is ambiguous"));
            }
        }
    }
    for terrain in &map.terrains {
        let center = terrain.base.iter().copied().sum::<DVec3>() / 4.0;
        if sides
            .insert(
                terrain.side,
                Side {
                    id: terrain.side,
                    texinfo: terrain.texinfo,
                    center,
                    normal: map.planes.get(terrain.plane).normal,
                },
            )
            .is_some()
        {
            return Err(format!(
                "side:{}: terrain cubemap source provenance is ambiguous",
                terrain.side
            ));
        }
    }
    let mut manual = BTreeMap::new();
    for (sample, entity) in helpers.iter().enumerate() {
        let mut own = BTreeSet::new();
        for value in entity
            .properties
            .get("sides")
            .unwrap_or("")
            .split_whitespace()
        {
            let id = value.parse::<u32>().map_err(|_| {
                format!(
                    "entity:{}: cubemap sides must contain positive face IDs",
                    entity.id
                )
            })?;
            if id == 0 || !own.insert(id) || manual.insert(id, sample).is_some() {
                return Err(format!(
                    "entity:{}: side:{id} has duplicate cubemap ownership",
                    entity.id
                ));
            }
            if !sides.contains_key(&id) {
                return Err(format!(
                    "entity:{}: side:{id} is missing, hidden, outside the compile region or does not produce a visible compiled side",
                    entity.id
                ));
            }
        }
    }
    if sides
        .len()
        .checked_mul(samples.len())
        .is_none_or(|work| work > MAX_ASSIGNMENT_WORK)
    {
        return Err("Cubemap assignment comparison budget exceeded".into());
    }
    let mut graph = Graph::new(fs, control);
    if capture_scene {
        graph.record_scene(vmf)?;
    }
    let mut assignments = Vec::new();
    let mut replacements = BTreeMap::new();
    let mut alias_materials = BTreeMap::new();
    for side in sides.values() {
        control.checkpoint()?;
        if !side.center.is_finite() || !side.normal.is_finite() {
            return Err(format!(
                "side:{}: nonfinite cubemap source geometry",
                side.id
            ));
        }
        let original = map.texinfos.infos[side.texinfo];
        let material_index = map.texdata[original.texdata as usize];
        let material = material_name(&map.materials.get(material_index).name)?;
        let reflective = graph.needs_alias(&material)?;
        if !reflective && !manual.contains_key(&side.id) {
            continue;
        }
        let sample = if let Some(&sample) = manual.get(&side.id) {
            sample
        } else {
            nearest(side, &samples)
        };
        let alias = graph.generate(&material, sample, &samples, context.namespace())?;
        assignments.push(CubemapAssignment {
            face_id: side.id,
            helper_id: samples[sample].helper_id,
            manual: manual.contains_key(&side.id),
            material: material.clone(),
            alias: alias.clone(),
        });
        if alias == material {
            continue;
        }
        let texinfo = alias_texinfo(
            map,
            side.texinfo,
            material_index,
            &alias,
            &mut alias_materials,
        )?;
        replacements.insert(side.id, texinfo);
        if let Some(&bottom) = map.water_bottom_texinfos.get(&side.texinfo) {
            let bottom_material = map.texdata[map.texinfos.infos[bottom].texdata as usize];
            let bottom_name = material_name(&map.materials.get(bottom_material).name)?;
            let bottom_alias =
                graph.generate(&bottom_name, sample, &samples, context.namespace())?;
            let new_bottom = if bottom_alias == bottom_name {
                bottom
            } else {
                alias_texinfo(
                    map,
                    bottom,
                    bottom_material,
                    &bottom_alias,
                    &mut alias_materials,
                )?
            };
            map.water_bottom_texinfos.insert(texinfo, new_bottom);
        }
    }
    for brush in &mut map.brushes {
        for side in &mut brush.sides {
            if let Some(texinfo) = side.source_id.and_then(|id| replacements.get(&id)) {
                side.texinfo = *texinfo;
            }
        }
    }
    for terrain in &mut map.terrains {
        if let Some(&texinfo) = replacements.get(&terrain.side) {
            terrain.texinfo = texinfo;
        }
    }
    let aliases = graph.aliases.values().cloned().collect();
    let sources = graph
        .sources
        .iter()
        .map(|(path, sha256)| CubemapSourceAsset {
            path: path.clone(),
            sha256: sha256.clone(),
        })
        .collect();
    let missing_sources = graph.missing_sources.iter().cloned().collect();
    let mut entries = graph.into_entries();
    for sample in &samples {
        entries.push(PakEntry {
            name: sample.texture.clone(),
            data: super::manifest::bootstrap_cube(sample.edge)?,
        });
    }
    for sample in &hdr_samples {
        entries.push(PakEntry {
            name: sample.texture.clone(),
            data: super::hdr::bootstrap_cube(sample.edge)?,
        });
    }
    Ok(Plan {
        eligible_faces: sides.keys().copied().collect(),
        manifest: CubemapAssetManifest {
            version: if hdr { 2 } else { 1 },
            phase: CubemapAssetPhase::Pending,
            logical_namespace: context.namespace().into(),
            namespace: context.namespace().into(),
            channel: if hdr { "ldr_hdr" } else { "ldr" }.into(),
            samples,
            hdr_samples,
            assignments,
            aliases,
            sources,
            missing_sources,
            default_texture: None,
            hdr_default_texture: None,
            texinfo_bindings: Vec::new(),
            face_bindings: Vec::new(),
        },
        entries,
    })
}

fn nearest(side: &Side, samples: &[CubemapSampleAsset]) -> usize {
    let mut front = None;
    let mut any = (0, f64::INFINITY);
    for (index, sample) in samples.iter().enumerate() {
        let delta = DVec3::from_array(sample.origin.map(f64::from)) - side.center;
        let distance = delta.length_squared();
        if distance < any.1 {
            any = (index, distance);
        }
        if delta.dot(side.normal) >= 0.0 && front.is_none_or(|(_, best)| distance < best) {
            front = Some((index, distance));
        }
    }
    front.unwrap_or(any).0
}

fn alias_texinfo(
    map: &mut Map<'_>,
    source: usize,
    material: usize,
    alias: &str,
    cache: &mut BTreeMap<String, usize>,
) -> Result<usize, String> {
    let index = if let Some(&index) = cache.get(alias) {
        index
    } else {
        let alias_material = map.materials.add_alias(material, alias)?;
        map.texdata.push(alias_material);
        let index = map.texdata.len() - 1;
        cache.insert(alias.to_owned(), index);
        index
    };
    let mut info = map.texinfos.infos[source];
    info.texdata = i32::try_from(index).map_err(|_| "Reflection texdata index overflow")?;
    let index = map.texinfos.add(info);
    if index > i16::MAX as usize {
        return Err("Reflection texinfo index limit exceeded".into());
    }
    Ok(index)
}
