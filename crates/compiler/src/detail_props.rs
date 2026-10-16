use std::collections::BTreeMap;

use glam::{DQuat, DVec3, EulerRot};
use source_formats::bsp::{Bsp, DetailProp, DetailProps, Leaf, Model, Node, Plane, contents};
use source_formats::gamefs::{GameFs, asset_relative};
use source_formats::vmf::{Entity, Vmf};

use crate::{
    Control, Diagnostic, Report, emit,
    map::{MAX_COMPILE_COORD, Map},
    static_props,
};

mod assets;
mod scatter;
mod shapes;
mod sprites;

const MAX_PROPS: usize = 16_384;
const MAX_LEAF_WORK: usize = 4_000_000;

#[derive(Default)]
pub(crate) struct Stats {
    pub props: usize,
    pub models: usize,
    pub sprites: usize,
    pub model_centers: Vec<DVec3>,
}

fn class(entity: &Entity) -> bool {
    entity.classname().is_some_and(|name| {
        ["prop_detail", "detail_prop", "prop_detail_sprite"]
            .iter()
            .any(|class| name.eq_ignore_ascii_case(class))
    })
}

fn record(entity: &Entity) -> Result<DetailProp, String> {
    if entity.solids.iter().any(|solid| !solid.hidden) {
        return Err("Detail model props are point entities; remove their brush geometry".into());
    }
    if entity.connections.iter().flatten().next().is_some() {
        return Err(
            "Detail model props are client-only and cannot carry entity I/O connections".into(),
        );
    }
    for key in [
        "parentname",
        "globalname",
        "target",
        "vscripts",
        "thinkfunction",
    ] {
        if entity
            .properties
            .get(key)
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Err(format!(
                "Detail model props are client-only and cannot honor {key}; remove the runtime behavior"
            ));
        }
    }
    for (key, default) in [
        ("skin", 0.0),
        ("body", 0.0),
        ("modelscale", 1.0),
        ("scale", 1.0),
        ("detailOrientation", 0.0),
        ("renderamt", 255.0),
        ("rendermode", 0.0),
        ("renderfx", 0.0),
        ("spawnflags", 0.0),
        ("solid", 0.0),
        ("disableshadows", 0.0),
        ("fademindist", -1.0),
        ("fademaxdist", 0.0),
        ("fadescale", 1.0),
    ] {
        if let Some(raw) = entity.properties.get(key) {
            let value = raw.parse::<f64>().ok().filter(|value| value.is_finite());
            if value != Some(default) {
                return Err(format!(
                    "Detail model props require {key}={default}; this native model record does not support that override"
                ));
            }
        }
    }
    if source_formats::render_color::rgb(entity.properties.get("rendercolor"))
        .map_err(|error| error.to_string())?
        != [255; 3]
    {
        return Err(
            "Tinted detail model props are unsupported; rendercolor must be 255 255 255".into(),
        );
    }
    if entity
        .properties
        .get("lightingorigin")
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err("Detail model props do not support named lighting origins yet".into());
    }
    let origin = if entity.properties.get("origin").is_some() {
        entity
            .origin()
            .ok_or("Detail model prop origin must contain three finite coordinates")?
    } else {
        DVec3::ZERO
    };
    if !origin.is_finite() || origin.abs().max_element() > MAX_COMPILE_COORD {
        return Err("Detail model prop origin must fit finite Source coordinates".into());
    }
    let angles = static_props::angles(entity)?;
    let origin = origin.as_vec3();
    let angles = angles.as_vec3();
    Ok(DetailProp {
        origin: origin.to_array(),
        angles: angles.to_array(),
        ..Default::default()
    })
}

struct Builder<'a> {
    fs: &'a GameFs,
    nodes: Vec<Node>,
    planes: Vec<Plane>,
    leaves: Vec<Leaf>,
    head: i32,
    props: DetailProps,
    models: BTreeMap<String, (u16, assets::ModelBounds)>,
    sprites: BTreeMap<[u32; 8], u16>,
    sprite_material: String,
    sprite_atlas_checked: bool,
    validator: assets::ModelValidator,
    work: usize,
}

impl<'a> Builder<'a> {
    fn new(fs: &'a GameFs, bsp: &Bsp, vmf: &Vmf) -> Result<Self, String> {
        Ok(Self {
            fs,
            nodes: bsp.records::<Node>().map_err(|error| error.to_string())?,
            planes: bsp.records::<Plane>().map_err(|error| error.to_string())?,
            leaves: bsp.records::<Leaf>().map_err(|error| error.to_string())?,
            head: bsp
                .records::<Model>()
                .map_err(|error| error.to_string())?
                .first()
                .ok_or("Detail model props require a world model")?
                .head_node,
            props: DetailProps::default(),
            models: BTreeMap::new(),
            sprites: BTreeMap::new(),
            sprite_material: vmf
                .world
                .properties
                .get("detailmaterial")
                .filter(|value| !value.is_empty())
                .unwrap_or("detail/detailsprites")
                .to_owned(),
            sprite_atlas_checked: false,
            validator: assets::ModelValidator::default(),
            work: 0,
        })
    }

    fn model(
        &mut self,
        raw: &str,
        control: &Control,
    ) -> Result<(u16, assets::ModelBounds), String> {
        let path = asset_relative(raw).map_err(|error| error.to_string())?;
        if !path.starts_with("models/") || !path.ends_with(".mdl") {
            return Err("Detail model must name a models/*.mdl asset".into());
        }
        if let Some(&entry) = self.models.get(&path) {
            return Ok(entry);
        }
        if self.models.len() >= source_formats::bsp::MAX_DETAIL_PROP_MODELS {
            return Err("Detail model dictionary exceeds the 1024-model build budget".into());
        }
        let name = DetailProps::model_record(&path).map_err(|error| error.to_string())?;
        let bounds = self.validator.bounds(self.fs, &path, control)?;
        let entry = (self.props.models.len() as u16, bounds);
        self.models.insert(path, entry);
        self.props.models.push(name);
        Ok(entry)
    }

    fn add(
        &mut self,
        path: &str,
        mut prop: DetailProp,
        leaf_point: DVec3,
        control: &Control,
    ) -> Result<(), String> {
        if self.props.props.len() >= MAX_PROPS {
            return Err("Detail model props exceed the 16,384-object build budget".into());
        }
        let (model, bounds) = self.model(path, control)?;
        let origin = DVec3::from_array(prop.origin.map(f64::from));
        let angles = DVec3::from_array(prop.angles.map(f64::from));
        if !origin.is_finite()
            || origin.abs().max_element() > MAX_COMPILE_COORD
            || !angles.is_finite()
            || !leaf_point.is_finite()
        {
            return Err("Detail model transforms must fit finite Source coordinates".into());
        }
        let radians = angles.map(f64::to_radians);
        let rotation = DQuat::from_euler(EulerRot::ZYX, radians.y, radians.x, radians.z);
        bounds.check_transform(origin, rotation)?;
        prop.model = model;
        self.add_record(prop, leaf_point, control)
    }

    fn add_sprite(
        &mut self,
        sprite: source_formats::bsp::DetailSprite,
        mut prop: DetailProp,
        leaf_point: DVec3,
        control: &Control,
    ) -> Result<(), String> {
        sprites::check_prop_bounds(sprite, &prop)?;
        if !self.sprite_atlas_checked {
            self.validator
                .sprite_atlas(self.fs, &self.sprite_material, control)?;
            self.sprite_atlas_checked = true;
        }
        let key = sprites::key(sprite);
        let index = if let Some(&index) = self.sprites.get(&key) {
            index
        } else {
            if self.sprites.len() >= sprites::MAX_SPRITES {
                return Err("Detail sprite dictionary exceeds its 4096-entry build budget".into());
            }
            let index = self.props.sprites.len() as u16;
            self.sprites.insert(key, index);
            self.props.sprites.push(sprite);
            index
        };
        prop.model = index;
        self.add_record(prop, leaf_point, control)
    }

    fn add_record(
        &mut self,
        mut prop: DetailProp,
        leaf_point: DVec3,
        control: &Control,
    ) -> Result<(), String> {
        if self.props.props.len() >= MAX_PROPS {
            return Err("Detail model props exceed the 16,384-object build budget".into());
        }
        if !leaf_point.is_finite()
            || prop
                .origin
                .into_iter()
                .chain(prop.angles)
                .any(|value| !value.is_finite())
        {
            return Err("Detail prop transforms and leaf samples must remain finite".into());
        }
        let mut cursor = self.head;
        while cursor >= 0 {
            control.checkpoint()?;
            self.work += 1;
            if self.work > MAX_LEAF_WORK {
                return Err("Detail-prop leaf traversal exceeds four million steps".into());
            }
            let node = self
                .nodes
                .get(cursor as usize)
                .ok_or("Invalid detail-prop BSP node")?;
            let plane = self
                .planes
                .get(node.plane as usize)
                .ok_or("Invalid detail-prop BSP plane")?;
            let distance = DVec3::from_array(plane.normal.map(f64::from)).dot(leaf_point)
                - f64::from(plane.dist);
            cursor = node.children[usize::from(distance < 0.0)];
        }
        let leaf =
            usize::try_from(-i64::from(cursor) - 1).map_err(|_| "Invalid detail-prop leaf")?;
        let info = self
            .leaves
            .get(leaf)
            .ok_or("Invalid detail-prop BSP leaf")?;
        if info.contents & contents::SOLID != 0 || info.cluster < 0 {
            return Err("Detail model origin must lie in an open visible world leaf".into());
        }
        prop.leaf = u16::try_from(leaf)
            .map_err(|_| "Detail model leaf exceeds the native unsigned short")?;
        self.props.props.push(prop);
        Ok(())
    }
}

pub(crate) fn bake(
    vmf: &Vmf,
    fs: &GameFs,
    map: &Map,
    models: &[emit::Model],
    bsp: &mut Bsp,
    report: &mut Report,
    control: &Control,
) -> Result<Stats, String> {
    let entities = vmf
        .entities
        .iter()
        .filter(|entity| !entity.hidden && class(entity))
        .take(MAX_PROPS + 1)
        .collect::<Vec<_>>();
    if entities.is_empty() && !scatter::requested(map, models) {
        return Ok(Stats::default());
    }
    if entities.len() > MAX_PROPS {
        return Err("Detail model props exceed the 16,384-object build budget".into());
    }
    let mut builder = Builder::new(fs, bsp, vmf)?;
    for entity in entities {
        control.checkpoint()?;
        let result: Result<(), String> = (|| {
            if entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("prop_detail_sprite"))
            {
                return Err("Explicit prop_detail_sprite compilation is unsupported; use a model-only prop_detail".into());
            }
            let prop = record(entity)?;
            let point = DVec3::from_array(prop.origin.map(f64::from));
            builder.add(
                entity.properties.get("model").unwrap_or(""),
                prop,
                point,
                control,
            )
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
    scatter::bake(vmf, map, models, &mut builder, report, control)?;
    if report.has_errors() {
        return Ok(Stats::default());
    }
    if !builder.props.props.is_empty() {
        builder.props.props.sort_by_key(|prop| prop.leaf);
        bsp.set_detail_props(&builder.props)
            .map_err(|error| error.to_string())?;
        report.diagnostics.push(Diagnostic::warning("Detail props are non-colliding client-only models, plain sprites and static cross/tri shapes; Fullbright uses unbaked white lighting, while lit builds bake direct, ambient and style colors; authored sway is unsupported", None, None));
    }
    let mut model_centers = vec![DVec3::ZERO; builder.props.models.len()];
    for &(index, bounds) in builder.models.values() {
        control.checkpoint()?;
        model_centers[usize::from(index)] = bounds.lighting_center();
    }
    Ok(Stats {
        props: builder.props.props.len(),
        models: builder.props.models.len(),
        sprites: builder.props.sprites.len(),
        model_centers,
    })
}
