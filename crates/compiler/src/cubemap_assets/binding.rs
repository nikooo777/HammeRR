use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use source_formats::bsp::PakEntry;
use source_formats::gamefs::GameFs;
use source_formats::vmf::{Entity, Vmf};

use super::material::{Graph, ensure_default_unmounted, material_name, sample_texture};
use super::*;
use crate::brush_model_pose::{Pose, initial_offset, initial_rotation};
use crate::map::Map;
use crate::{Control, Diagnostic, Report};

const POSE_PROPERTY_SCANS: usize = 3;

pub(crate) fn manual_side_ids(entity: &Entity) -> impl Iterator<Item = Result<u32, String>> + '_ {
    entity
        .properties
        .get("sides")
        .unwrap_or("")
        .split_whitespace()
        .map(move |value| {
            value.parse::<u32>().map_err(|_| {
                format!(
                    "entity:{}: cubemap sides must contain positive face IDs",
                    entity.id
                )
            })
        })
}

pub(crate) fn validate_instance_root_bindings(vmf: &Vmf, control: &Control) -> Result<(), String> {
    control.checkpoint()?;
    if vmf
        .object_ids()
        .take(crate::instances::MAX_OBJECTS + 1)
        .count()
        > crate::instances::MAX_OBJECTS
    {
        return Err("Instance root cubemap validation exceeds the object limit".into());
    }
    let mut sides = BTreeSet::new();
    for (index, id) in vmf.side_ids().enumerate() {
        control.checkpoint()?;
        if index >= crate::instances::MAX_SIDES {
            return Err("Instance root cubemap validation exceeds the side limit".into());
        }
        sides.insert(id);
    }
    let mut work = 0;
    for entity in &vmf.entities {
        control.checkpoint()?;
        if entity.hidden
            || !entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("env_cubemap"))
        {
            continue;
        }
        for id in manual_side_ids(entity) {
            control.checkpoint()?;
            work += 1;
            if work > MAX_ASSIGNMENT_WORK {
                return Err("Cubemap assignment comparison budget exceeded".into());
            }
            let id = id?;
            if !sides.contains(&id) {
                return Err(format!(
                    "entity:{}: side:{id} is missing from the original root map; generated instance sides cannot satisfy root cubemap bindings",
                    entity.id
                ));
            }
        }
    }
    Ok(())
}

pub(crate) struct Plan {
    pub manifest: CubemapAssetManifest,
    pub entries: Vec<PakEntry>,
    pub(super) eligible_faces: BTreeSet<u32>,
}

#[derive(Default)]
pub(crate) struct PrepareOptions {
    pub capture_scene: bool,
    pub hdr: bool,
}

struct Side {
    id: u32,
    entity: usize,
    texinfo: usize,
    local_center: DVec3,
    normal: DVec3,
}

pub(crate) fn capture_preflight(vmf: &Vmf) -> Result<(), String> {
    native_runtime::preflight_vmf(vmf).map_err(|error| error.to_string())
}

fn helper_error(report: &mut Report, entity: &Entity, message: &str) {
    report.diagnostics.push(Diagnostic::error(
        message,
        Some(format!("entity:{}", entity.id)),
        entity.origin(),
    ));
}

pub(crate) fn prepare(
    vmf: &Vmf,
    fs: &GameFs,
    map: &mut Map<'_>,
    context: &CompileContext,
    options: PrepareOptions,
    report: &mut Report,
    control: &Control,
) -> Result<Plan, String> {
    let PrepareOptions { capture_scene, hdr } = options;
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
        let message = "Generate reflections requires 1–32 valid visible env_cubemap helpers";
        for helper in helpers.iter().skip(MAX_SAMPLES) {
            control.checkpoint()?;
            helper_error(report, helper, message);
        }
        return Err(message.into());
    }
    if capture_scene {
        ensure_default_unmounted(fs, context.namespace())?;
        if hdr {
            super::material::ensure_hdr_default_unmounted(fs, context.namespace())?;
        }
    }
    let mut positions = BTreeMap::new();
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
            let message = format!(
                "entity:{}: native reflection capture supports cube edges up to {MAX_CUBE_EDGE}",
                entity.id
            );
            helper_error(report, entity, &message);
            return Err(message);
        }
        if let Some(previous) = positions.insert(cube.origin, *entity) {
            let message = format!(
                "entity:{}: cubemap helpers collide after native integer origin quantization at {:?}",
                entity.id, cube.origin
            );
            helper_error(report, previous, &message);
            helper_error(report, entity, &message);
            return Err(message);
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
        control.checkpoint()?;
        for side in &brush.sides {
            control.checkpoint()?;
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
            let local_center = winding.0.iter().copied().sum::<DVec3>() / winding.0.len() as f64;
            let normal = map.planes.get(side.plane).normal;
            if sides
                .insert(
                    id,
                    Side {
                        id,
                        entity: brush.entity,
                        texinfo: side.texinfo,
                        local_center,
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
        control.checkpoint()?;
        let center = terrain.base.iter().copied().sum::<DVec3>() / 4.0;
        if sides
            .insert(
                terrain.side,
                Side {
                    id: terrain.side,
                    entity: 0,
                    texinfo: terrain.texinfo,
                    local_center: center,
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
        for id in manual_side_ids(entity) {
            control.checkpoint()?;
            let id = match id {
                Ok(id) => id,
                Err(message) => {
                    helper_error(report, entity, &message);
                    return Err(message);
                }
            };
            let previous = manual.get(&id).copied();
            if id == 0 || !own.insert(id) || manual.insert(id, sample).is_some() {
                let message = format!(
                    "entity:{}: side:{id} has duplicate cubemap ownership",
                    entity.id
                );
                if let Some(previous) = previous
                    && previous != sample
                {
                    helper_error(report, helpers[previous], &message);
                }
                helper_error(report, entity, &message);
                return Err(message);
            }
            if !sides.contains_key(&id) {
                let message = format!(
                    "entity:{}: side:{id} is missing, hidden, outside the compile region or does not produce a visible compiled side",
                    entity.id
                );
                helper_error(report, entity, &message);
                return Err(message);
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
    let mut model_poses: BTreeMap<usize, Pose> = BTreeMap::new();
    let mut pose_work = 0;
    for side in sides.values() {
        control.checkpoint()?;
        let center = side.local_center + map.entities[side.entity].origin.unwrap_or(DVec3::ZERO);
        if !center.is_finite() || !side.normal.is_finite() {
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
            let (center, normal) = if side.entity == 0 {
                (center, side.normal)
            } else {
                let pose = if let Some(pose) = model_poses.get(&side.entity) {
                    *pose
                } else {
                    let pose = checked_pose(map, side.entity, control, &mut pose_work)
                        .map_err(|error| format!(
                            "entity:{}: side:{}: Unsupported initial pose for automatic cubemap assignment: {error}",
                            map.entities[side.entity].id, side.id,
                        ))?;
                    model_poses.insert(side.entity, pose);
                    pose
                };
                if pose.rotation.is_some() {
                    consume_pose_work(control, &mut pose_work, 2)?;
                    (pose.point(side.local_center), pose.normal(side.normal))
                } else {
                    (center, side.normal)
                }
            };
            if !center.is_finite() || !normal.is_finite() {
                return Err(format!(
                    "side:{}: nonfinite posed cubemap source geometry",
                    side.id
                ));
            }
            nearest(center, normal, &samples)
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

fn consume_pose_work(control: &Control, work: &mut usize, amount: usize) -> Result<(), String> {
    control.checkpoint()?;
    *work = work
        .checked_add(amount)
        .ok_or("Cubemap initial-pose work count overflow")?;
    if *work > MAX_ASSIGNMENT_WORK {
        return Err("Cubemap initial-pose work budget exceeded".into());
    }
    Ok(())
}

fn checked_pose(
    map: &Map<'_>,
    entity: usize,
    control: &Control,
    work: &mut usize,
) -> Result<Pose, String> {
    let source = &map.entities[entity];
    for (key, value) in &source.properties {
        let amount = key
            .len()
            .max(1)
            .checked_add(value.len())
            .and_then(|amount| amount.checked_mul(POSE_PROPERTY_SCANS))
            .ok_or("Cubemap initial-pose work count overflow")?;
        consume_pose_work(control, work, amount)?;
    }
    let rotation = initial_rotation(&source.classname, &source.properties)?;
    let pose = Pose {
        offset: if rotation.is_some() {
            initial_offset(&source.properties)?
        } else {
            source.origin.unwrap_or(DVec3::ZERO)
        },
        rotation,
    };
    if rotation.is_some() {
        consume_pose_work(control, work, source.brushes.len())?;
        let bounds = map.model_bounds(entity);
        consume_pose_work(control, work, 16)?;
        pose.raw_and_stored_bounds(bounds.0, bounds.1)?;
    }
    Ok(pose)
}

fn nearest(center: DVec3, normal: DVec3, samples: &[CubemapSampleAsset]) -> usize {
    let mut front = None;
    let mut any = (0, f64::INFINITY);
    for (index, sample) in samples.iter().enumerate() {
        let delta = DVec3::from_array(sample.origin.map(f64::from)) - center;
        let distance = delta.length_squared();
        if distance < any.1 {
            any = (index, distance);
        }
        if delta.dot(normal) >= 0.0 && front.is_none_or(|(_, best)| distance < best) {
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
