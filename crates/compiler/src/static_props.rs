use std::collections::BTreeMap;

#[cfg(test)]
use glam::EulerRot;
use glam::{DQuat, DVec3};
use source_formats::bsp::{self, Bsp, Leaf, Model, Node, Plane, StaticProp, StaticProps, contents};
use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::mdl::ModelHeader;
use source_formats::vmf::{Entity, Vmf, parse_vec3};

use crate::{Control, Diagnostic, Report, map::MAX_COMPILE_COORD};

const MAX_LEAF_WORK: usize = 16_000_000;
const LEAF_BIAS: f64 = 0.01;

mod leaf_box;

#[derive(Default)]
pub(crate) struct Stats {
    pub props: usize,
    pub models: usize,
    pub leaf_refs: usize,
    pub sources: Vec<usize>,
    pub source_guard: Option<crate::asset_sources::AssetSourceGuard>,
    pub source_owner: Option<crate::asset_sources::BuildMaterialSources>,
}

struct ModelAsset {
    index: u16,
    header: ModelHeader,
    collision: Option<bool>,
}

fn number(entity: &Entity, key: &str, default: f32) -> Result<f32, String> {
    entity.properties.get(key).map_or(Ok(default), |raw| {
        raw.parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("{key} must be a finite number"))
    })
}

fn integer(entity: &Entity, key: &str, default: i32) -> Result<i32, String> {
    entity.properties.get(key).map_or(Ok(default), |raw| {
        raw.parse().map_err(|_| format!("{key} must be an integer"))
    })
}

pub(crate) struct Visibility {
    fade_min: f32,
    fade_max: f32,
    fade_scale: f32,
    flags: u32,
    dx_levels: [u16; 2],
}

pub(crate) fn visibility(entity: &Entity) -> Result<Visibility, String> {
    let mut fade_min = number(entity, "fademindist", -1.0)?;
    let fade_max = number(entity, "fademaxdist", 0.0)?;
    let fade_scale = number(entity, "fadescale", 1.0)?;
    if fade_scale < 0.0 || fade_max < 0.0 {
        return Err("fade scale and end distance must be nonnegative".into());
    }
    let mut flags = 0;
    if fade_max > 0.0 {
        flags |= 1;
        if fade_min < 0.0 {
            fade_min = fade_max;
        }
    } else {
        fade_min = 0.0;
    }
    match integer(entity, "screenspacefade", 0)? {
        0 => {}
        1 => flags |= 32,
        _ => return Err("screenspacefade must be 0 or 1".into()),
    }
    let mut dx_levels = [0; 2];
    for (index, key) in ["mindxlevel", "maxdxlevel"].into_iter().enumerate() {
        dx_levels[index] = u16::try_from(integer(entity, key, 0)?)
            .map_err(|_| format!("{key} must fit an unsigned native short"))?;
    }
    if dx_levels[1] != 0 && dx_levels[0] > dx_levels[1] {
        return Err("minimum DX level exceeds maximum DX level".into());
    }
    Ok(Visibility {
        fade_min,
        fade_max,
        fade_scale,
        flags,
        dx_levels,
    })
}

pub(crate) fn angles(entity: &Entity) -> Result<DVec3, String> {
    let value = if let Some(raw) = entity.properties.get("angles") {
        parse_vec3(raw).ok_or("angles must contain Pitch Yaw Roll")?
    } else if entity.properties.get("angle").is_some() {
        match number(entity, "angle", 0.0)? {
            -1.0 => DVec3::new(-90.0, 0.0, 0.0),
            -2.0 => DVec3::new(90.0, 0.0, 0.0),
            yaw => DVec3::new(0.0, f64::from(yaw), 0.0),
        }
    } else {
        DVec3::ZERO
    };
    if !value.is_finite() || value.to_array().iter().any(|v| !(*v as f32).is_finite()) {
        return Err("angles must fit finite native floats".into());
    }
    Ok(value.map(|angle| angle % 360.0))
}

pub(crate) fn bounds(
    header: ModelHeader,
    origin: DVec3,
    rotation: DQuat,
) -> Result<(DVec3, DVec3), String> {
    let mins = DVec3::from_array(header.render_mins.map(f64::from))
        .min(DVec3::from_array(header.hull_mins.map(f64::from)));
    let maxs = DVec3::from_array(header.render_maxs.map(f64::from))
        .max(DVec3::from_array(header.hull_maxs.map(f64::from)));
    let mut low = DVec3::splat(f64::INFINITY);
    let mut high = DVec3::splat(f64::NEG_INFINITY);
    for corner in 0..8 {
        let local = DVec3::from_array(std::array::from_fn(|axis| {
            if corner & (1 << axis) == 0 {
                mins[axis]
            } else {
                maxs[axis]
            }
        }));
        let point = origin + rotation * local;
        if !point.is_finite() || point.abs().max_element() > MAX_COMPILE_COORD {
            return Err(format!(
                "transformed model bounds must fit ±{MAX_COMPILE_COORD} units"
            ));
        }
        low = low.min(point);
        high = high.max(point);
    }
    Ok((
        low - DVec3::splat(LEAF_BIAS),
        high + DVec3::splat(LEAF_BIAS),
    ))
}

struct Space {
    nodes: Vec<Node>,
    planes: Vec<Plane>,
    leaves: Vec<Leaf>,
    head: i32,
    work: usize,
    clipped_points: usize,
}

impl Space {
    fn new(bsp: &Bsp) -> Result<Self, String> {
        let models = bsp.records::<Model>().map_err(|e| e.to_string())?;
        Ok(Self {
            nodes: bsp.records().map_err(|e| e.to_string())?,
            planes: bsp.records().map_err(|e| e.to_string())?,
            leaves: bsp.records().map_err(|e| e.to_string())?,
            head: models.first().ok_or("world model is missing")?.head_node,
            work: 0,
            clipped_points: 0,
        })
    }

    fn intersect(&mut self, bounds: (DVec3, DVec3), control: &Control) -> Result<Vec<u16>, String> {
        let center = (bounds.0 + bounds.1) * 0.5;
        let radius = (bounds.1 - bounds.0) * 0.5;
        let mut pending = vec![self.head];
        let mut leaves = Vec::new();
        while let Some(index) = pending.pop() {
            control.checkpoint()?;
            self.work += 1;
            if self.work > MAX_LEAF_WORK {
                return Err("Static-prop leaf membership exceeded its work budget".into());
            }
            if index < 0 {
                let index = (-i64::from(index) - 1) as usize;
                let leaf = self
                    .leaves
                    .get(index)
                    .ok_or("invalid static-prop leaf reference")?;
                if leaf.contents & contents::SOLID == 0 {
                    leaves.push(
                        u16::try_from(index)
                            .map_err(|_| "Static-prop leaf index exceeds the native limit")?,
                    );
                }
                continue;
            }
            let node = self
                .nodes
                .get(index as usize)
                .ok_or("invalid static-prop node reference")?;
            let plane = self
                .planes
                .get(node.plane as usize)
                .ok_or("invalid static-prop plane reference")?;
            let normal = DVec3::from_array(plane.normal.map(f64::from));
            let distance = normal.dot(center) - f64::from(plane.dist);
            let extent = normal.abs().dot(radius);
            if distance + extent >= 0.0 {
                pending.push(node.children[0]);
            }
            if distance - extent <= 0.0 {
                pending.push(node.children[1]);
            }
        }
        leaves.sort_unstable();
        leaves.dedup();
        if leaves.is_empty() {
            return Err("model bounds do not touch any open world leaves".into());
        }
        Ok(leaves)
    }
}

fn record(
    entity: &Entity,
    header: ModelHeader,
    lighting: &crate::lighting_origins::Origins<'_>,
    binding: Option<&crate::lighting_origins::Binding>,
) -> Result<StaticProp, String> {
    if entity.solids.iter().any(|solid| !solid.hidden) {
        return Err("prop_static is a point entity; remove its brush geometry".into());
    }
    let origin = entity.origin().unwrap_or(DVec3::ZERO);
    let angles = angles(entity)?;
    let solid = integer(entity, "solid", 6)?;
    if ![0, 2, 6].contains(&solid) {
        return Err("solid must be 0 (none), 2 (bounding box) or 6 (VPhysics)".into());
    }
    let skin = integer(entity, "skin", 0)?;
    if skin < 0 || skin as usize >= header.skins {
        return Err(format!(
            "skin must be from 0 to {} for this model",
            header.skins - 1
        ));
    }
    let visibility = visibility(entity)?;
    let mut prop = StaticProp {
        origin: origin.to_array().map(|v| v as f32),
        angles: angles.to_array().map(|v| v as f32),
        solid: solid as u8,
        skin,
        fade_min: visibility.fade_min,
        fade_max: visibility.fade_max,
        fade_scale: visibility.fade_scale,
        flags: visibility.flags,
        dx_levels: visibility.dx_levels,
        ..Default::default()
    };
    for (key, bit) in [
        ("ignorenormals", 8),
        ("disableshadows", 16),
        ("disablevertexlighting", 64),
        ("disableselfshadowing", 128),
    ] {
        match integer(entity, key, 0)? {
            0 => {}
            1 => prop.flags |= bit,
            _ => return Err(format!("{key} must be 0 or 1")),
        }
    }
    if number(entity, "modelscale", 1.0)? != 1.0 {
        return Err("Static-prop output does not support modelscale other than 1; use a compatible dynamic model entity for scale".into());
    }
    if integer(entity, "generatelightmaps", 0)? != 0 {
        return Err("Per-texel static-prop lightmaps are not supported yet".into());
    }
    if integer(entity, "body", 0)? != 0 {
        return Err("Static props require body 0; use prop_dynamic for bodygroups".into());
    }
    if source_formats::render_color::rgb(entity.properties.get("rendercolor"))
        .map_err(|error| error.to_string())?
        != [255; 3]
    {
        return Err("Tinted static props are not supported by the current static-prop output; use prop_dynamic for model tint".into());
    }
    let appearance = source_formats::render_color::model_appearance(
        entity.properties.get("rendermode"),
        entity.properties.get("renderamt"),
        entity.properties.get("renderfx"),
    )?;
    if appearance.mode != source_formats::render_color::NORMAL {
        return Err("Static-prop output does not support authored render modes; use prop_dynamic for model opacity".into());
    }
    if let Some(point) =
        lighting.resolve_with_binding(entity.properties.get("lightingorigin"), binding)?
    {
        prop.flags |= 2;
        prop.lighting_origin = point.to_array().map(|v| v as f32);
    }
    Ok(prop)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn bake(
    vmf: &Vmf,
    fs: &GameFs,
    bsp: &mut Bsp,
    report: &mut Report,
    control: &Control,
    mut source_reads: Option<&mut crate::lighting::asset_tracking::SourceReads>,
    instance_metadata: &crate::instances::StaticPropMetadata<'_>,
    retained_sources: Option<&crate::asset_sources::BuildMaterialSources>,
) -> Result<Stats, String> {
    let mut props = StaticProps {
        version: 6,
        models: Vec::new(),
        leaves: Vec::new(),
        props: Vec::new(),
    };
    let mut models = BTreeMap::<String, ModelAsset>::new();
    let mut sources = Vec::new();
    let entities: Vec<_> = vmf
        .entities
        .iter()
        .filter(|entity| {
            !entity.hidden
                && entity
                    .classname()
                    .is_some_and(|class| class.eq_ignore_ascii_case("prop_static"))
        })
        .take(bsp::MAX_STATIC_PROPS + 1)
        .collect();
    if entities.len() > bsp::MAX_STATIC_PROPS {
        return Err("Static-prop count exceeds the native limit".into());
    }
    if entities.is_empty() {
        return Ok(Stats::default());
    }
    let source_owner = retained_sources
        .cloned()
        .unwrap_or_else(crate::asset_sources::BuildMaterialSources::without_retained_payloads);
    let maximum_asset_bytes = retained_sources.map(|_| bsp::PakLimits::default().max_entry_bytes);
    let lighting = crate::lighting_origins::Origins::new(
        vmf,
        entities
            .iter()
            .copied()
            .filter_map(|entity| entity.properties.get("lightingorigin")),
        control,
    )?;
    let mut space = Space::new(bsp)?;
    for entity in entities {
        control.checkpoint()?;
        let result = (|| {
            if props.props.len() >= bsp::MAX_STATIC_PROPS {
                return Err("Static-prop count exceeds the native limit".into());
            }
            let path = asset_relative(entity.properties.get("model").unwrap_or(""))
                .map_err(|e| e.to_string())?;
            if !path.starts_with("models/") || !path.ends_with(".mdl") {
                return Err("model must name a models/*.mdl asset".into());
            }
            if !models.contains_key(&path) {
                if models.len() >= bsp::MAX_STATIC_PROP_MODELS {
                    return Err("Static-prop model dictionary exceeds the native limit".into());
                }
                let name = StaticProps::model_record(&path).map_err(|e| e.to_string())?;
                let maximum = maximum_asset_bytes
                    .unwrap_or(source_formats::mdl::MAX_MODEL_BYTES)
                    .min(source_formats::mdl::MAX_MODEL_BYTES);
                let maximum = match source_reads.as_deref() {
                    Some(reads) => reads
                        .read_limit(&path, maximum)
                        .map_err(|error| error.to_string())?,
                    None => maximum,
                };
                let data = source_owner
                    .read_asset(fs, &path, maximum)
                    .map_err(|e| e.to_string())?;
                if let Some(reads) = &mut source_reads {
                    reads
                        .record(&path, data.as_ref().map(|asset| asset.bytes()))
                        .map_err(|error| error.to_string())?;
                }
                let data = data.ok_or_else(|| format!("model {path} was not found"))?;
                let header =
                    ModelHeader::parse(data.bytes()).map_err(|e| format!("{path}: {e}"))?;
                if !header.is_static_prop() {
                    return Err(format!("{path} was not built with $staticprop"));
                }
                let index = props.models.len() as u16;
                props.models.push(name);
                models.insert(
                    path.clone(),
                    ModelAsset {
                        index,
                        header,
                        collision: None,
                    },
                );
            }
            let asset = models.get_mut(&path).unwrap();
            if let Some(poses) = instance_metadata
                .poses
                .and_then(|poses| poses.get(&entity.id))
            {
                for (index, pose) in poses.iter().enumerate() {
                    control.checkpoint()?;
                    pose.validate_model_bounds(asset.header).map_err(|error| {
                        let stage = if index == 0 {
                            "source pose".to_owned()
                        } else {
                            format!("intermediate pose {index}")
                        };
                        format!("Static child prop {stage}: {error}")
                    })?;
                }
            }
            let binding = instance_metadata
                .lighting_origins
                .and_then(|bindings| bindings.get(&entity.id));
            let mut prop = record(entity, asset.header, &lighting, binding)?;
            if prop.solid == 6 {
                if asset.collision.is_none() {
                    let phy_path = format!("{}.phy", &path[..path.len() - 4]);
                    asset.collision = Some(
                        match source_owner
                            .read_asset(
                                fs,
                                &phy_path,
                                maximum_asset_bytes
                                    .unwrap_or(bsp::MAX_PHYSICS_BYTES)
                                    .min(bsp::MAX_PHYSICS_BYTES),
                            )
                            .map_err(|e| format!("{phy_path}: {e}"))?
                        {
                            Some(bytes) => {
                                source_formats::mdl::validate_physics(
                                    bytes.bytes(),
                                    asset.header.checksum,
                                )
                                .map_err(|e| format!("{phy_path}: {e}"))?;
                                true
                            }
                            None => false,
                        },
                    );
                }
                if asset.collision != Some(true) {
                    return Err(format!(
                        "{path} needs its .phy collision asset for VPhysics"
                    ));
                }
            }
            let transform = prop.transform().map_err(|error| error.to_string())?;
            let leaves = space.prop_leaves(
                asset.header,
                transform.translation,
                DQuat::from_mat3(&transform.matrix3),
                control,
            )?;
            if props.leaves.len() + leaves.len() > bsp::MAX_STATIC_PROP_LEAVES {
                return Err("Static-prop leaf table exceeds the native limit".into());
            }
            prop.model = asset.index;
            prop.first_leaf = props.leaves.len() as u16;
            prop.leaf_count = leaves.len() as u16;
            props.leaves.extend(leaves);
            props.props.push(prop);
            sources.push(entity.id as usize);
            Ok(())
        })();
        if let Err(message) = result {
            control.checkpoint()?;
            report.diagnostics.push(Diagnostic::error(
                message,
                Some(format!("entity:{}", entity.id)),
                entity.origin(),
            ));
        }
    }
    if report.has_errors() {
        return Ok(Stats::default());
    }
    let source_guard = source_owner
        .source_guard(fs, control)
        .map_err(|error| error.to_string())?;
    if !props.props.is_empty() {
        report.diagnostics.push(Diagnostic::warning("Static props use model assets and conservative header-box leaf membership, with rotated boxes clipped through world leaves; exact model-hull leaf clipping is unfinished", None, None));
    }
    bsp.set_static_props(&props).map_err(|e| e.to_string())?;
    Ok(Stats {
        props: props.props.len(),
        models: props.models.len(),
        leaf_refs: props.leaves.len(),
        sources,
        source_guard: Some(source_guard),
        source_owner: Some(source_owner),
    })
}

#[cfg(test)]
mod source_tests;
