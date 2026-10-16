use std::collections::BTreeMap;

use glam::{DQuat, DVec3, EulerRot};
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
    lighting: &BTreeMap<&str, Vec<&Entity>>,
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
    let mut prop = StaticProp {
        origin: origin.to_array().map(|v| v as f32),
        angles: angles.to_array().map(|v| v as f32),
        solid: solid as u8,
        skin,
        fade_min: number(entity, "fademindist", -1.0)?,
        fade_max: number(entity, "fademaxdist", 0.0)?,
        fade_scale: number(entity, "fadescale", 1.0)?,
        ..Default::default()
    };
    if prop.fade_scale < 0.0 || prop.fade_max < 0.0 {
        return Err("fade scale and end distance must be nonnegative".into());
    }
    if prop.fade_max > 0.0 {
        prop.flags |= 1;
        if prop.fade_min < 0.0 {
            prop.fade_min = prop.fade_max;
        }
    } else {
        prop.fade_min = 0.0;
    }
    for (key, bit) in [
        ("ignorenormals", 8),
        ("disableshadows", 16),
        ("screenspacefade", 32),
        ("disablevertexlighting", 64),
        ("disableselfshadowing", 128),
    ] {
        match integer(entity, key, 0)? {
            0 => {}
            1 => prop.flags |= bit,
            _ => return Err(format!("{key} must be 0 or 1")),
        }
    }
    for (index, key) in ["mindxlevel", "maxdxlevel"].into_iter().enumerate() {
        prop.dx_levels[index] = u16::try_from(integer(entity, key, 0)?)
            .map_err(|_| format!("{key} must fit an unsigned native short"))?;
    }
    if prop.dx_levels[1] != 0 && prop.dx_levels[0] > prop.dx_levels[1] {
        return Err("minimum DX level exceeds maximum DX level".into());
    }
    if number(entity, "modelscale", 1.0)? != 1.0 || integer(entity, "generatelightmaps", 0)? != 0 {
        return Err(
            "scaled static props and per-texel prop lightmaps are not supported yet".into(),
        );
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
    if let Some(name) = entity
        .properties
        .get("lightingorigin")
        .filter(|name| !name.is_empty())
    {
        let matches = lighting
            .get(name)
            .ok_or("lightingorigin must name one visible info_lighting")?;
        if matches.len() != 1 {
            return Err("lightingorigin resolves to multiple info_lighting helpers".into());
        }
        let helper = matches[0];
        let point = helper
            .origin()
            .filter(|point| point.is_finite() && point.abs().max_element() <= MAX_COMPILE_COORD)
            .ok_or("lighting origin is invalid")?;
        prop.flags |= 2;
        prop.lighting_origin = point.to_array().map(|v| v as f32);
    }
    Ok(prop)
}

pub(crate) fn bake(
    vmf: &Vmf,
    fs: &GameFs,
    bsp: &mut Bsp,
    report: &mut Report,
    control: &Control,
) -> Result<Stats, String> {
    let mut props = StaticProps {
        version: 6,
        models: Vec::new(),
        leaves: Vec::new(),
        props: Vec::new(),
    };
    let mut models = BTreeMap::<String, ModelAsset>::new();
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
    let mut lighting = BTreeMap::<&str, Vec<&Entity>>::new();
    for helper in vmf.entities.iter().filter(|entity| {
        !entity.hidden
            && entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("info_lighting"))
    }) {
        control.checkpoint()?;
        if let Some(name) = helper.properties.get("targetname") {
            lighting.entry(name).or_default().push(helper);
        }
    }
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
                let data = fs
                    .read_limited(&path, source_formats::mdl::MAX_MODEL_BYTES)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("model {path} was not found"))?;
                let header = ModelHeader::parse(&data).map_err(|e| format!("{path}: {e}"))?;
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
            let mut prop = record(entity, asset.header, &lighting)?;
            if prop.solid == 6 {
                if asset.collision.is_none() {
                    let phy_path = format!("{}.phy", &path[..path.len() - 4]);
                    asset.collision =
                        Some(match fs.read(&phy_path).map_err(|e| e.to_string())? {
                            Some(bytes) => {
                                source_formats::mdl::validate_physics(
                                    &bytes,
                                    asset.header.checksum,
                                )
                                .map_err(|e| format!("{phy_path}: {e}"))?;
                                true
                            }
                            None => false,
                        });
                }
                if asset.collision != Some(true) {
                    return Err(format!(
                        "{path} needs its .phy collision asset for VPhysics"
                    ));
                }
            }
            let angles = DVec3::from_array(prop.angles.map(f64::from)).map(f64::to_radians);
            let rotation = DQuat::from_euler(EulerRot::ZYX, angles.y, angles.x, angles.z);
            let leaves = space.prop_leaves(
                asset.header,
                DVec3::from_array(prop.origin.map(f64::from)),
                rotation,
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
    if !props.props.is_empty() {
        report.diagnostics.push(Diagnostic::warning("Static props use model assets and conservative header-box leaf membership, with rotated boxes clipped through world leaves; baked prop lighting and exact model-hull leaf clipping are unfinished", None, None));
    }
    bsp.set_static_props(&props).map_err(|e| e.to_string())?;
    Ok(Stats {
        props: props.props.len(),
        models: props.models.len(),
        leaf_refs: props.leaves.len(),
    })
}
