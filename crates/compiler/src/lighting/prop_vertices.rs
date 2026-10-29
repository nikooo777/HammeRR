use std::collections::BTreeMap;
use std::io;

use glam::{DAffine3, DVec3};
use source_formats::{
    bsp::{Bsp, LightingKind, StaticProp},
    gamefs::{GameFs, asset_relative},
    model::{HardwareLayout, HardwareLimits, Model, Vertex},
    vhv,
    vmf::{Entity, Vmf},
};

use super::bake::{Allocation, Budget};
use super::culling::Candidates;
use super::receivers;
use crate::map::{MAX_COMPILE_COORD, Map};
use crate::prop_lighting_assets::{
    Channel, GeneratedAsset, PropLightingPlan, Receiver, SourceAsset,
};
use crate::{Control, Diagnostic, PropVertexLightingStats, Report};
use crate::{tree::Tree, visibility::Visibility};

mod assets;
pub use assets::load_prop_vertex_material;
mod color;
mod indirect;
mod sources;
pub(super) use sources::{SourceSnapshot, SourceTriangle};
mod channel;
pub(super) use channel::{advance_channel, begin_channel, publish_channel};

const MAX_RECEIVERS: usize = 1024;
const MAX_MODELS: usize = 128;
const MAX_VERTICES: usize = 4_000_000;
const MAX_GENERATED_BYTES: usize = 64 * 1024 * 1024;
const DISABLED: u32 = 0x40;
const IGNORE_NORMALS: u32 = 0x08;
const NO_SELF_SHADOW: u32 = 0x80;

struct ModelData {
    model: Model,
    layout: HardwareLayout,
}

struct BoundReceiver {
    metadata: Receiver,
    prop: StaticProp,
    source: usize,
    model: usize,
}

pub(super) struct Prepared {
    models: Vec<ModelData>,
    receivers: Vec<BoundReceiver>,
    generated: Vec<GeneratedAsset>,
    sources: Vec<SourceAsset>,
    missing: Vec<String>,
    stats: PropVertexLightingStats,
    allocation: Allocation,
    source_allocation: Allocation,
    output_allocation: Allocation,
    generated_bytes: usize,
    reflection: Option<sources::Plan>,
}

pub(super) struct Context<'a> {
    pub map: &'a Map<'a>,
    pub tree: &'a Tree,
    pub visibility: &'a Visibility,
    pub direct: receivers::Context<'a>,
}

pub(super) struct Indirect<'a> {
    pub surfaces: &'a super::radiance::SurfaceIndex,
    pub quality: crate::SkyQuality,
}

pub(super) struct Settings<'a> {
    pub vmf: &'a Vmf,
    pub prop_sources: &'a [usize],
    pub fs: &'a GameFs,
    pub reflect_static_props: bool,
    pub prop_shadows: &'a super::shadows::ShadowIndex<super::props::PropTriangle>,
}

fn source(vmf: &Vmf, id: usize) -> Result<&Entity, String> {
    let mut matches = vmf
        .entities
        .iter()
        .filter(|entity| entity.id as usize == id);
    let entity = matches
        .next()
        .ok_or("Emitted prop source entity is missing")?;
    if matches.next().is_some()
        || entity.hidden
        || !entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("prop_static"))
    {
        return Err(
            "Emitted prop source identity is ambiguous or no longer a visible prop_static".into(),
        );
    }
    Ok(entity)
}

fn admit_model(data: &ModelData) -> Result<(), String> {
    if data.model.body_groups.len() != 1
        || data.model.body_groups[0].variants.len() != 1
        || data.layout.root_lod != 0
    {
        return Err("Baked prop vertices currently require one bodypart, one model variant and authored root LOD 0".into());
    }
    if data.layout.sections.is_empty()
        || data
            .layout
            .sections
            .iter()
            .any(|section| section.vertices.is_empty())
    {
        return Err("Baked prop vertices require nonempty hardware strip groups".into());
    }
    Ok(())
}

fn admit_prop(entity: &Entity, prop: &StaticProp, model: &str) -> Result<(), String> {
    if prop.dx_levels != [0; 2] {
        return Err("Baked prop vertices currently require unrestricted prop DX levels and the DX90 companion".into());
    }
    let path = asset_relative(entity.properties.get("model").unwrap_or(""))
        .map_err(|error| error.to_string())?;
    if path != model {
        return Err("Emitted static-prop ordinal does not match its source model".into());
    }
    for (key, expected) in [("body", 0.0), ("modelscale", 1.0)] {
        if entity
            .properties
            .get(key)
            .is_some_and(|value| value.trim().parse::<f64>().ok() != Some(expected))
        {
            return Err(format!("Baked prop vertices require {key} {expected}"));
        }
    }
    Ok(())
}

pub(super) fn prepare<'a>(
    settings: Settings<'a>,
    mut tracker: super::asset_tracking::Tracker<'a>,
    bsp: &Bsp,
    report: &mut Report,
    budget: &mut Budget<'_>,
) -> Result<Option<Prepared>, String> {
    let Settings {
        vmf,
        prop_sources,
        fs,
        reflect_static_props,
        prop_shadows,
    } = settings;
    let start = budget.work();
    let Some(lump) = bsp.game_lumps.iter().find(|lump| lump.id == *b"sprp") else {
        if !prop_sources.is_empty() {
            return Err("Static-prop source ordinals exist without emitted records".into());
        }
        return Ok(None);
    };
    let _records =
        budget.reserve_bytes(lump.data.len() * 4 + std::mem::size_of_val(prop_sources))?;
    budget.consume(lump.data.len().div_ceil(64) + prop_sources.len())?;
    let props = bsp
        .static_props()
        .map_err(|error| error.to_string())?
        .ok_or("Static-prop records are missing")?;
    if props.props.len() != prop_sources.len() {
        return Err("Static-prop emitted-source ordinals do not match native records".into());
    }
    let count = props
        .props
        .iter()
        .filter(|prop| prop.flags & DISABLED == 0)
        .count();
    if count == 0 {
        return Ok(None);
    }
    if count > MAX_RECEIVERS {
        return Err("Baked prop vertices exceed their 1,024 receiver limit; explicitly disable vertex lighting on other props".into());
    }
    let mut assets = assets::Assets::new(&mut tracker, fs);
    let mut allocation = budget.reserve_bytes(
        count * size_of::<BoundReceiver>()
            + MAX_MODELS * size_of::<ModelData>()
            + count * 2 * size_of::<GeneratedAsset>(),
    )?;
    let mut models = Vec::with_capacity(MAX_MODELS);
    let mut receivers = Vec::with_capacity(count);
    let mut by_model = BTreeMap::new();
    let mut admitted_materials = std::collections::BTreeSet::new();
    let mut source_materials = BTreeMap::new();
    let mut total_vertices = 0usize;
    let mut total_meshes = 0usize;
    for (ordinal, (prop, &source_id)) in props.props.iter().zip(prop_sources).enumerate() {
        budget.consume(1)?;
        if prop.flags & DISABLED != 0 {
            continue;
        }
        budget.consume(vmf.entities.len())?;
        let entity = match source(vmf, source_id) {
            Ok(entity) => entity,
            Err(error) => {
                report.diagnostics.push(Diagnostic::error(
                    error,
                    Some(format!("entity:{source_id}")),
                    Some(DVec3::from_array(prop.origin.map(f64::from))),
                ));
                return Err("Static-prop emitted-source identity failed".into());
            }
        };
        let result = (|| {
            let path = props
                .model_name(usize::from(prop.model))
                .map_err(|error| error.to_string())?;
            admit_prop(entity, prop, path)?;
            let model_index = match by_model.get(path).copied() {
                Some(index) => index,
                None => {
                    if models.len() >= MAX_MODELS {
                        return Err(
                            "Baked prop vertices exceed their 128 unique-model limit".into()
                        );
                    }
                    let stem = path
                        .strip_suffix(".mdl")
                        .ok_or("Static-prop model must end in .mdl")?;
                    let mdl = assets.required(path, budget)?;
                    if mdl.get(377).copied() != Some(0) {
                        return Err(
                            "Baked prop vertices currently require authored MDL root LOD 0".into(),
                        );
                    }
                    let vvd = assets.required(&format!("{stem}.vvd"), budget)?;
                    let vtx = assets.required(&format!("{stem}.dx90.vtx"), budget)?;
                    let (model, layout) = Model::parse_with_hardware(
                        &mdl,
                        &vvd,
                        &vtx,
                        HardwareLimits {
                            sections: crate::prop_lighting_assets::MAX_RECEIVER_MESHES,
                            vertices: MAX_VERTICES - total_vertices,
                            ..HardwareLimits::default()
                        },
                        &mut |charge| {
                            budget.consume(charge.work).map_err(io::Error::other)?;
                            allocation
                                .grow(budget, charge.bytes)
                                .map_err(io::Error::other)
                        },
                    )
                    .map_err(|error| format!("{path}: {error}"))?;
                    let data = ModelData { model, layout };
                    admit_model(&data)?;
                    allocation.grow(budget, path.len() * 2 + 256)?;
                    let index = models.len();
                    models.push(data);
                    by_model.insert(path.to_owned(), index);
                    index
                }
            };
            let data: &ModelData = &models[model_index];
            let skin = data
                .model
                .skins
                .get(prop.skin as usize)
                .ok_or("Baked prop skin is unavailable")?;
            for section in &data.layout.sections {
                budget.consume(1)?;
                let part = data
                    .model
                    .parts
                    .get(section.part)
                    .ok_or("Baked prop section has no MDL mesh")?;
                let material = *skin
                    .get(part.material)
                    .ok_or("Baked prop skin binding is unavailable")?;
                let reflected_material = if reflect_static_props {
                    budget.consume(
                        data.model.parts.len()
                            + source_materials.len().checked_ilog2().unwrap_or(0) as usize
                            + 1,
                    )?;
                    data.model.parts.iter().any(|part| {
                        !part.indices.is_empty() && skin.get(part.material) == Some(&material)
                    })
                } else {
                    false
                };
                let key = (model_index, material);
                if reflected_material && !source_materials.contains_key(&key) {
                    if !admitted_materials.contains(&key) {
                        allocation.grow(budget, 128)?;
                    }
                    allocation.grow(budget, 128 + 16 * size_of::<Allocation>())?;
                    source_materials
                        .insert(key, assets.source_material(&data.model, material, budget)?);
                    admitted_materials.insert(key);
                } else if !admitted_materials.contains(&key) {
                    allocation.grow(budget, 128)?;
                    assets.material(&data.model, material, budget)?;
                    admitted_materials.insert(key);
                }
            }
            total_vertices = total_vertices
                .checked_add(data.layout.vertices.len())
                .filter(|count| *count <= MAX_VERTICES)
                .ok_or(
                    "Baked prop vertices exceed their four-million total hardware-vertex limit",
                )?;
            total_meshes = total_meshes
                .checked_add(data.layout.sections.len())
                .ok_or("Baked prop section count overflow")?;
            allocation.grow(
                budget,
                path.len()
                    + data.layout.sections.len()
                        * size_of::<crate::prop_lighting_assets::MeshLayout>(),
            )?;
            let meshes = data
                .layout
                .sections
                .iter()
                .map(|section| {
                    vhv::MeshLayout {
                        lod: section.lod as u32,
                        vertex_count: section.vertices.len() as u32,
                    }
                    .into()
                })
                .collect();
            receivers.push(BoundReceiver {
                metadata: Receiver {
                    ordinal: ordinal as u32,
                    model: path.to_owned(),
                    checksum: data.layout.checksum as u32,
                    meshes,
                },
                prop: prop.clone(),
                source: source_id,
                model: model_index,
            });
            Ok::<_, String>(())
        })();
        if let Err(error) = result {
            budget.consume(0)?;
            report.diagnostics.push(Diagnostic::error(
                error,
                Some(format!("entity:{}", entity.id)),
                entity.origin(),
            ));
            return Err("Static-prop vertex receiver preparation failed".into());
        }
    }
    let asset_allocations = assets.finish();
    let reflection = if reflect_static_props {
        Some(sources::prepare(
            &models,
            &receivers,
            source_materials,
            prop_shadows,
            asset_allocations,
            report,
            budget,
        )?)
    } else {
        if !asset_allocations.is_empty() {
            return Err(
                "Static-prop receiver-only preparation retained unexpected source assets".into(),
            );
        }
        None
    };
    let reflected_source_props = reflection.as_ref().map_or(0, |plan| plan.source_props);
    let reflected_source_triangles = reflection.as_ref().map_or(0, sources::Plan::triangle_count);
    let reflected_source_vertices = reflection.as_ref().map_or(0, |plan| plan.source_vertices);
    let (sources, missing, source_allocation) = tracker.finish();
    Ok(Some(Prepared {
        models,
        receivers,
        generated: Vec::with_capacity(count * 2),
        sources,
        missing,
        stats: PropVertexLightingStats {
            props: count,
            models: by_model.len(),
            meshes: total_meshes,
            vertices: total_vertices,
            work: budget.work() - start,
            reflected_source_props,
            reflected_source_triangles,
            reflected_source_vertices,
            ..Default::default()
        },
        allocation,
        source_allocation,
        output_allocation: budget.reserve_bytes(0)?,
        generated_bytes: 0,
        reflection,
    }))
}

fn sample(vertex: &Vertex, placement: &DAffine3) -> Result<receivers::Sample, String> {
    let point = placement.transform_point3(DVec3::from_array(vertex.position.map(f64::from)));
    let normal = placement
        .transform_vector3(DVec3::from_array(vertex.normal.map(f64::from)))
        .try_normalize()
        .ok_or("Baked prop vertex normal is invalid")?;
    if !point.is_finite() || point.abs().max_element() > MAX_COMPILE_COORD {
        return Err("Baked prop vertex exceeds finite Source compile coordinates".into());
    }
    Ok(receivers::Sample {
        point,
        normal,
        origin: point,
    })
}

fn validate_point(
    ctx: &receivers::Context<'_>,
    ordinal: u32,
    point: DVec3,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    let buried = ctx.shadows.nearest(
        point,
        point,
        &mut |work| budget.consume(work),
        &mut |hull| {
            if !hull.occupies_world() {
                return Ok(None);
            }
            for plane in &hull.planes {
                let distance = plane.distance_to(point) / plane.normal.length();
                if !distance.is_finite() {
                    return Err("Prop receiver hull distance must be finite".into());
                }
                if distance >= -super::shadows::CONTACT_EPSILON {
                    return Ok(None);
                }
            }
            Ok(Some((0.0, ())))
        },
    )?;
    if buried.is_some() {
        return Err("Baked prop vertex is buried inside a world brush beyond the 1e-6 boundary band; embedded-vertex recovery is unsupported".into());
    }
    let terrain = ctx.terrain.nearest(
        point,
        point,
        &mut |work| budget.consume(work),
        &mut |triangle| Ok(triangle.point_contact(point)?.then_some((0.0, ()))),
    )?;
    if terrain.is_some() {
        return Err("Baked prop vertex touches a terrain triangle within 1e-6 units; triangle support contacts are unsupported".into());
    }
    let props = ctx.props.nearest(
        point,
        point,
        &mut |work| budget.consume(work),
        &mut |triangle| Ok(triangle.point_contact(point, ordinal)?.then_some((0.0, ()))),
    )?;
    if props.is_some() {
        return Err("Baked prop vertex touches another prop triangle within 1e-6 units; triangle support contacts are unsupported".into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn vertex_color(
    index: usize,
    vertex: &Vertex,
    receiver: &BoundReceiver,
    placement: &DAffine3,
    ctx: &Context<'_>,
    indirect: Option<&Indirect<'_>>,
    angular: super::ReceiverAngularMode,
    directions: &[DVec3],
    cone: f64,
    base: Option<DVec3>,
    candidates: &mut Candidates,
    budget: &mut Budget<'_>,
) -> Result<DVec3, String> {
    budget.consume(if base.is_some() {
        4
    } else {
        receivers::STYLE_COUNT + 4
    })?;
    let sample = sample(vertex, placement)?;
    let point = sample.point;
    let normal = sample.normal;
    let result = (|| {
        let skipped_owner =
            (receiver.prop.flags & NO_SELF_SHADOW != 0).then_some(receiver.metadata.ordinal);
        let mut color = if let Some(base) = base {
            base
        } else {
            validate_point(&ctx.direct, receiver.metadata.ordinal, point, budget)?;
            let cluster = candidates.cluster(point, ctx.map, ctx.tree, ctx.visibility, budget)?;
            let colors = receivers::direct_with_reception(
                &ctx.direct,
                candidates,
                cluster,
                sample,
                true,
                receivers::Reception {
                    angular,
                    skip_owner: skipped_owner,
                    styles: receivers::Styles::StaticOnly,
                },
                budget,
            )?;
            if colors.iter().skip(1).any(|color| *color != DVec3::ZERO) {
                return Err("Baked prop vertices cannot store nonzero light styles".into());
            }
            colors[0]
        };
        if let Some(indirect) = indirect {
            color += indirect::gather(
                angular,
                normal,
                directions,
                ctx.direct.ambient_directions,
                &mut |direction| {
                    let mut total = DVec3::ZERO;
                    indirect.surfaces.trace(
                        point,
                        direction,
                        cone,
                        &super::transport::Scene {
                            shadows: ctx.direct.shadows,
                            terrain: ctx.direct.terrain,
                            props: ctx.direct.props,
                            model: None,
                        },
                        skipped_owner,
                        budget,
                        &mut |hit, _| {
                            if !hit.sky && hit.front {
                                total += hit.color * 255.0;
                            }
                            Ok(())
                        },
                    )?;
                    Ok(total)
                },
            )?;
        }
        Ok::<_, String>(color)
    })();
    result.map_err(|error| {
        format!(
            "vertex {index} at ({:.6}, {:.6}, {:.6}): {error}",
            point.x, point.y, point.z,
        )
    })
}

fn encode_output(
    ordinal: u32,
    checksum: u32,
    kind: LightingKind,
    meshes: Vec<vhv::Mesh>,
    generated_bytes: usize,
    output_allocation: &mut Allocation,
    budget: &mut Budget<'_>,
) -> Result<(GeneratedAsset, usize), String> {
    let file = vhv::File { checksum, meshes };
    let limits = vhv::Limits::default();
    let bytes = file
        .encoded_len(limits)
        .map_err(|error| error.to_string())?;
    let total = generated_bytes
        .checked_add(bytes)
        .filter(|bytes| *bytes <= MAX_GENERATED_BYTES)
        .ok_or("Baked prop VHV output exceeds its aggregate 64 MiB limit")?;
    output_allocation.grow(budget, bytes)?;
    budget.consume(bytes.div_ceil(64))?;
    let data = file.to_bytes(limits).map_err(|error| error.to_string())?;
    Ok((
        GeneratedAsset {
            ordinal,
            channel: if kind == LightingKind::Hdr {
                Channel::Hdr
            } else {
                Channel::Ldr
            },
            data,
        },
        total,
    ))
}

pub(super) fn bake_channel(
    prepared: &mut Prepared,
    ctx: &Context<'_>,
    indirect: Option<Indirect<'_>>,
    kind: LightingKind,
    candidates: &mut Candidates,
    report: &mut Report,
    budget: &mut Budget<'_>,
) -> Result<(), String> {
    let start = budget.work();
    let planned = prepared
        .receivers
        .iter()
        .try_fold(0usize, |count, receiver| {
            count
                .checked_add(prepared.models[receiver.model].layout.vertices.len())
                .ok_or("Prop vertex planned sample count overflow")
        })?;
    budget.plan_samples(planned)?;
    budget.samples_planned_complete();
    let _directions = budget.reserve::<DVec3>(
        indirect
            .as_ref()
            .map_or(0, |indirect| indirect.quality.directions()),
    )?;
    let directions = if let Some(indirect) = &indirect {
        budget.consume(indirect.quality.directions())?;
        super::bounce::directions(indirect.quality.directions())
    } else {
        Vec::new()
    };
    let normal_cone = indirect::cone(super::ReceiverAngularMode::Normal, directions.len());
    let sphere_cone = if indirect.is_some() {
        indirect::cone(
            super::ReceiverAngularMode::IgnoreNormals,
            ctx.direct.ambient_directions.len(),
        )
    } else {
        0.0
    };
    for receiver_index in 0..prepared.receivers.len() {
        let receiver = &prepared.receivers[receiver_index];
        budget.consume(1)?;
        let angular = if receiver.prop.flags & IGNORE_NORMALS != 0 {
            super::ReceiverAngularMode::IgnoreNormals
        } else {
            super::ReceiverAngularMode::Normal
        };
        let cone = match angular {
            super::ReceiverAngularMode::Normal => normal_cone,
            super::ReceiverAngularMode::IgnoreNormals => sphere_cone,
        };
        let data = &prepared.models[receiver.model];
        let placement = receiver
            .prop
            .transform()
            .map_err(|error| error.to_string())?;
        let _cache = budget.reserve::<Option<vhv::Bgra8>>(data.model.vertices.len())?;
        let _meshes = budget.reserve::<vhv::Mesh>(data.layout.sections.len())?;
        let _colors = budget.reserve::<vhv::Bgra8>(data.layout.vertices.len())?;
        let mut cached = vec![None; data.model.vertices.len()];
        let mut meshes = Vec::with_capacity(data.layout.sections.len());
        let result = (|| {
            for section in &data.layout.sections {
                let mut colors = Vec::with_capacity(section.vertices.len());
                for &index in &data.layout.vertices[section.vertices.clone()] {
                    budget.consume(1)?;
                    let index = index as usize;
                    let color = match cached[index] {
                        Some(color) => color,
                        None => {
                            let color = color::encode(vertex_color(
                                index,
                                &data.model.vertices[index],
                                receiver,
                                &placement,
                                ctx,
                                indirect.as_ref(),
                                angular,
                                &directions,
                                cone,
                                None,
                                candidates,
                                budget,
                            )?)?;
                            cached[index] = Some(color);
                            color
                        }
                    };
                    colors.push(color);
                    budget.finish_sample();
                }
                meshes.push(vhv::Mesh {
                    layout: vhv::MeshLayout {
                        lod: section.lod as u32,
                        vertex_count: section.vertices.len() as u32,
                    },
                    colors,
                });
            }
            let ordinal = receiver.metadata.ordinal;
            let checksum = data.layout.checksum as u32;
            let (asset, total) = encode_output(
                ordinal,
                checksum,
                kind,
                meshes,
                prepared.generated_bytes,
                &mut prepared.output_allocation,
                budget,
            )?;
            let bytes = asset.data.len();
            prepared.generated.push(asset);
            prepared.generated_bytes = total;
            match kind {
                LightingKind::Ldr => prepared.stats.ldr_bytes += bytes,
                LightingKind::Hdr => prepared.stats.hdr_bytes += bytes,
            }
            Ok::<_, String>(())
        })();
        if let Err(error) = result {
            let receiver = &prepared.receivers[receiver_index];
            budget.consume(0)?;
            report.diagnostics.push(Diagnostic::error(
                error,
                Some(format!("entity:{}", receiver.source)),
                Some(DVec3::from_array(receiver.prop.origin.map(f64::from))),
            ));
            return Err("Static-prop vertex lighting failed".into());
        }
    }
    prepared.stats.work += budget.work() - start;
    Ok(())
}

pub(super) fn finish(
    prepared: Prepared,
    bsp: &mut Bsp,
    dual: bool,
    fs: &GameFs,
    control: &Control,
    budget: &mut Budget<'_>,
) -> Result<(PropLightingPlan, PropVertexLightingStats), String> {
    let Prepared {
        models,
        receivers,
        generated,
        sources,
        missing,
        mut stats,
        allocation,
        source_allocation,
        output_allocation,
        generated_bytes,
        reflection,
    } = prepared;
    drop(reflection);
    drop(models);
    let source_bytes = sources.iter().map(|source| source.bytes).sum::<usize>();
    let _publication = budget.reserve_bytes(
        MAX_GENERATED_BYTES * 3 + generated_bytes * 3 + source_bytes * 2 + 2 * 1024 * 1024,
    )?;
    let start = budget.work();
    budget.consume(
        3 * MAX_GENERATED_BYTES.div_ceil(64)
            + source_bytes.div_ceil(64)
            + generated_bytes.div_ceil(64),
    )?;
    let channels = if dual {
        vec![Channel::Ldr, Channel::Hdr]
    } else {
        vec![Channel::Ldr]
    };
    let receivers = receivers
        .into_iter()
        .map(|receiver| receiver.metadata)
        .collect();
    let plan = PropLightingPlan::new(
        bsp, channels, receivers, generated, sources, missing, fs, control,
    )
    .map_err(|error| error.to_string())?;
    plan.attach(bsp, control)
        .map_err(|error| error.to_string())?;
    stats.asset_bytes = plan.entries().iter().map(|entry| entry.data.len()).sum();
    stats.work += budget.work() - start;
    drop(allocation);
    drop(source_allocation);
    drop(output_allocation);
    Ok((plan, stats))
}
