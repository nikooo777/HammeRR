//! The VMF converted into compiler brushes, sides and entities.

use std::collections::HashMap;

use glam::DVec3;
use map_core::brush::BrushGeometry;
use source_formats::bsp::{self, contents};
use source_formats::gamefs::GameFs;
use source_formats::vmf::{Side, Solid, Vmf};

use crate::materials::Materials;
use crate::planes::PlaneSet;
use crate::winding::Winding;
use crate::{Diagnostic, Report};

pub const DEFAULT_LIGHTMAP_SCALE: f64 = 16.0;

/// Entities that the compiler consumes instead of passing to the game.
const CONSUMED: &[&str] = &["func_detail"];
/// Entities whose compile support doesn't exist yet; dropped with a warning.
const UNSUPPORTED: &[&str] = &[
    "prop_static",
    "info_overlay",
    "info_overlay_transition",
    "env_cubemap",
    "func_viscluster",
    "func_occluder",
    "func_areaportal",
    "func_areaportalwindow",
    "func_instance",
    "info_lighting",
];

pub fn unsupported_entity_class(classname: &str) -> bool {
    UNSUPPORTED
        .iter()
        .any(|name| name.eq_ignore_ascii_case(classname))
}

#[derive(Debug, Clone)]
pub struct MapSide {
    /// Outward-facing plane.
    pub plane: usize,
    pub material: usize,
    pub texinfo: usize,
    /// The side's polygon; `None` for bevels.
    pub winding: Option<Winding>,
    pub bevel: bool,
    /// Whether this side becomes a visible face.
    pub visible: bool,
}

#[derive(Debug, Clone)]
pub struct MapBrush {
    /// VMF solid id.
    pub id: u32,
    pub entity: usize,
    pub contents: i32,
    pub sides: Vec<MapSide>,
    pub mins: DVec3,
    pub maxs: DVec3,
}

impl MapBrush {
    /// Brushes that split the world tree and seal the map.
    pub fn structural(&self) -> bool {
        self.contents & contents::DETAIL == 0
            && self.contents
                & (contents::SOLID | contents::WINDOW | contents::WATER | contents::SLIME)
                != 0
    }

    pub fn points(&self) -> impl Iterator<Item = DVec3> + '_ {
        self.sides
            .iter()
            .filter_map(|s| s.winding.as_ref())
            .flat_map(|w| w.0.iter().copied())
    }
}

#[derive(Debug, Clone)]
pub struct MapEntity {
    pub id: u32,
    pub classname: String,
    /// Key/values and outputs, in VMF order.
    pub properties: Vec<(String, String)>,
    pub brushes: Vec<usize>,
    pub origin: Option<DVec3>,
}

#[derive(Debug, Default)]
pub struct TexInfos {
    pub infos: Vec<bsp::TexInfo>,
    by_bits: HashMap<[u32; 18], usize>,
}

impl TexInfos {
    pub fn add(&mut self, info: bsp::TexInfo) -> usize {
        let bits: [u32; 18] = bytemuck_bits(&info);
        *self.by_bits.entry(bits).or_insert_with(|| {
            self.infos.push(info);
            self.infos.len() - 1
        })
    }
}

fn bytemuck_bits(info: &bsp::TexInfo) -> [u32; 18] {
    let mut bits = [0u32; 18];
    let floats = info
        .texture_vecs
        .iter()
        .chain(&info.lightmap_vecs)
        .flatten();
    for (slot, value) in bits.iter_mut().zip(floats) {
        *slot = value.to_bits();
    }
    bits[16] = info.flags as u32;
    bits[17] = info.texdata as u32;
    bits
}

pub struct Map<'a> {
    pub planes: PlaneSet,
    pub materials: Materials<'a>,
    pub texinfos: TexInfos,
    /// Material index of each texdata entry.
    pub texdata: Vec<usize>,
    pub brushes: Vec<MapBrush>,
    /// Entity 0 is worldspawn.
    pub entities: Vec<MapEntity>,
}

impl<'a> Map<'a> {
    pub fn load(vmf: &Vmf, fs: &'a GameFs, report: &mut Report) -> Self {
        let mut map = Self {
            planes: PlaneSet::default(),
            materials: Materials::new(fs),
            texinfos: TexInfos::default(),
            texdata: Vec::new(),
            brushes: Vec::new(),
            entities: Vec::new(),
        };
        let mut world_properties: Vec<(String, String)> = vmf
            .world
            .properties
            .iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        if !world_properties.iter().any(|(k, _)| k == "classname") {
            world_properties.insert(0, ("classname".into(), "worldspawn".into()));
        }
        map.entities.push(MapEntity {
            id: vmf.world.id,
            classname: "worldspawn".into(),
            properties: world_properties,
            brushes: Vec::new(),
            origin: None,
        });

        let mut hidden = 0;
        for solid in &vmf.world.solids {
            if solid.hidden {
                hidden += 1;
                continue;
            }
            map.add_brush(solid, 0, 0, report);
        }

        for entity in &vmf.entities {
            if entity.hidden {
                hidden += 1;
                continue;
            }
            let classname = entity.classname().unwrap_or("").to_owned();
            let object = format!("entity:{}", entity.id);
            if CONSUMED.contains(&classname.as_str()) {
                for solid in entity.solids.iter().filter(|s| !s.hidden) {
                    map.add_brush(solid, 0, contents::DETAIL, report);
                }
                continue;
            }
            if unsupported_entity_class(&classname) {
                report.diagnostics.push(Diagnostic::warning(
                    format!("{classname} is not compiled yet and was left out"),
                    Some(object),
                    entity.origin(),
                ));
                continue;
            }
            let index = map.entities.len();
            let mut properties: Vec<(String, String)> = entity
                .properties
                .iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect();
            properties.push(("hammerid".into(), entity.id.to_string()));
            for connection in entity.connections.iter().flatten() {
                properties.push((connection.output.clone(), connection.value.clone()));
            }
            map.entities.push(MapEntity {
                id: entity.id,
                classname,
                properties,
                brushes: Vec::new(),
                origin: entity.origin(),
            });
            for solid in entity.solids.iter().filter(|s| !s.hidden) {
                map.add_brush(solid, index, 0, report);
            }
            map.resolve_brush_entity(index, report);
        }
        if hidden > 0 {
            report.diagnostics.push(Diagnostic::info(
                format!("{hidden} hidden objects were not compiled"),
                None,
                None,
            ));
        }
        map
    }

    fn add_brush(
        &mut self,
        solid: &Solid,
        entity: usize,
        extra_contents: i32,
        report: &mut Report,
    ) {
        let object = format!("solid:{}", solid.id);
        if solid.sides.iter().any(|s| s.displacement.is_some()) {
            report.diagnostics.push(Diagnostic::warning(
                "displacements are not compiled yet; brush left out",
                Some(object),
                None,
            ));
            return;
        }
        let geometry = match BrushGeometry::from_solid(solid) {
            Ok(geometry) => geometry,
            Err(error) => {
                report.diagnostics.push(Diagnostic::error(
                    format!("invalid brush: {error}"),
                    Some(object),
                    None,
                ));
                return;
            }
        };

        let mut sides = Vec::with_capacity(geometry.faces.len() + 6);
        let mut brush_contents = 0;
        let mut solid_sides = false;
        let mut hint = false;
        for face in &geometry.faces {
            let side = &solid.sides[face.side];
            let material = self.materials.load(&side.material);
            let info = self.materials.get(material);
            brush_contents |= info.contents;
            solid_sides |= info.contents == contents::SOLID;
            hint |= info.hint || info.skip;
            let visible = info.draws;
            let plane = self.planes.find(face.plane.normal, face.plane.dist);
            let texinfo = self.texinfo(side, material);
            sides.push(MapSide {
                plane,
                material,
                texinfo,
                winding: Some(Winding(face.vertices.clone())),
                bevel: false,
                visible,
            });
        }
        if hint {
            return;
        }
        if solid_sides && brush_contents != contents::SOLID {
            // A mix like nodraw + clip; the special contents win.
            brush_contents &= !contents::SOLID;
            if brush_contents & (contents::WINDOW | contents::TRANSLUCENT) != 0 {
                brush_contents |= contents::WINDOW;
            }
        }
        if brush_contents & contents::ORIGIN != 0 {
            report.diagnostics.push(Diagnostic::warning(
                "origin brushes are not supported yet; brush left out",
                Some(object),
                Some(geometry.center()),
            ));
            return;
        }
        if brush_contents == 0 {
            return;
        }
        brush_contents |= extra_contents;

        for axis in 0..3 {
            for sign in [1.0, -1.0] {
                let mut normal = DVec3::ZERO;
                normal[axis] = sign;
                if sides
                    .iter()
                    .any(|s| self.planes.get(s.plane).normal == normal)
                {
                    continue;
                }
                let dist = if sign > 0.0 {
                    geometry.maxs[axis]
                } else {
                    -geometry.mins[axis]
                };
                let plane = self.planes.find(normal, dist);
                let first = &sides[0];
                sides.push(MapSide {
                    plane,
                    material: first.material,
                    texinfo: first.texinfo,
                    winding: None,
                    bevel: true,
                    visible: false,
                });
            }
        }

        let index = self.brushes.len();
        self.brushes.push(MapBrush {
            id: solid.id,
            entity,
            contents: brush_contents,
            sides,
            mins: geometry.mins,
            maxs: geometry.maxs,
        });
        self.entities[entity].brushes.push(index);
    }

    fn texinfo(&mut self, side: &Side, material: usize) -> usize {
        let info = self.materials.get(material);
        let texdata = match self.texdata.iter().position(|&m| m == material) {
            Some(i) => i,
            None => {
                self.texdata.push(material);
                self.texdata.len() - 1
            }
        };
        let lightmap_scale = if side.lightmap_scale == 0 {
            DEFAULT_LIGHTMAP_SCALE
        } else {
            side.lightmap_scale as f64
        };
        let vec = |axis: &source_formats::vmf::UvAxis| {
            let scale = if axis.scale == 0.0 { 1.0 } else { axis.scale };
            let a = axis.axis / scale;
            [a.x as f32, a.y as f32, a.z as f32, axis.offset as f32]
        };
        let lightmap_vec = |axis: &source_formats::vmf::UvAxis| {
            let a = axis.axis.normalize_or_zero() / lightmap_scale;
            [a.x as f32, a.y as f32, a.z as f32, 0.0]
        };
        self.texinfos.add(bsp::TexInfo {
            texture_vecs: [vec(&side.u_axis), vec(&side.v_axis)],
            lightmap_vecs: [lightmap_vec(&side.u_axis), lightmap_vec(&side.v_axis)],
            flags: info.surface,
            texdata: texdata as i32,
        })
    }

    /// Brush entities keep their brushes in world coordinates, so a stray
    /// `origin` key would offset the model in game.
    fn resolve_brush_entity(&mut self, index: usize, report: &mut Report) {
        let entity = &mut self.entities[index];
        if entity.brushes.is_empty() {
            return;
        }
        let had_origin = entity
            .properties
            .iter()
            .any(|(k, v)| k == "origin" && !v.trim().is_empty());
        entity.properties.retain(|(k, _)| k != "origin");
        if had_origin && entity.origin.is_some_and(|o| o.length() > 0.01) {
            report.diagnostics.push(Diagnostic::info(
                format!(
                    "{}: origin key ignored for a brush entity without an origin brush",
                    entity.classname
                ),
                Some(format!("entity:{}", entity.id)),
                entity.origin,
            ));
        }
        entity.origin = None;
    }
}
