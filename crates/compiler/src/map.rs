//! The VMF converted into compiler brushes, sides and entities.

use std::collections::HashMap;

use glam::DVec3;
use map_core::Plane;
use map_core::brush::BrushGeometry;
use source_formats::bsp::{self, contents};
use source_formats::gamefs::GameFs;
use source_formats::vmf::{Side, Solid, Vmf};

use crate::materials::Materials;
use crate::planes::PlaneSet;
use crate::winding::Winding;
use crate::{Control, Diagnostic, Report};

#[path = "bevels.rs"]
mod bevels;

#[path = "support.rs"]
mod support;

#[path = "origins.rs"]
mod origins;

#[path = "cubemaps.rs"]
mod cubemaps;

pub const DEFAULT_LIGHTMAP_SCALE: f64 = 16.0;
pub use map_core::checks::SOURCE_COORDINATE_LIMIT as MAX_COMPILE_COORD;

fn within_compile_bounds(point: DVec3) -> bool {
    point.is_finite() && point.abs().max_element() <= MAX_COMPILE_COORD
}

fn bounds_error(object: String) -> Diagnostic {
    Diagnostic::error(
        format!(
            "Source compilation requires finite geometry and origins within ±{MAX_COMPILE_COORD} units; move this object inside the compile bounds"
        ),
        Some(object),
        None,
    )
}

/// Entities that the compiler consumes instead of passing to the game.
const CONSUMED: &[&str] = &["func_detail"];
/// Entities whose compile support doesn't exist yet; dropped with a warning.
const UNSUPPORTED: &[&str] = &[
    "prop_static",
    "info_overlay",
    "info_overlay_transition",
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
    pub smoothing_groups: u32,
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
    pub cubemaps: Vec<bsp::Cubemap>,
    brush_side_count: usize,
}

impl<'a> Map<'a> {
    pub fn load(vmf: &Vmf, fs: &'a GameFs, report: &mut Report) -> Self {
        Self::load_controlled(vmf, fs, report, &Control::default()).expect("uncancelled map load")
    }

    pub fn load_controlled(
        vmf: &Vmf,
        fs: &'a GameFs,
        report: &mut Report,
        control: &Control,
    ) -> Result<Self, String> {
        control.checkpoint()?;
        let mut map = Self {
            planes: PlaneSet::default(),
            materials: Materials::new(fs),
            texinfos: TexInfos::default(),
            texdata: Vec::new(),
            brushes: Vec::new(),
            entities: Vec::new(),
            cubemaps: Vec::new(),
            brush_side_count: 0,
        };
        if let Err(error) = vmf.validate_ids() {
            report
                .diagnostics
                .push(Diagnostic::error(error.to_string(), None, None));
            return Ok(map);
        }
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
            control.checkpoint()?;
            if solid.hidden {
                hidden += 1;
                continue;
            }
            map.add_brush(solid, 0, 0, report, control)?;
        }

        for entity in &vmf.entities {
            control.checkpoint()?;
            if entity.hidden {
                hidden += 1;
                continue;
            }
            let classname = entity.classname().unwrap_or("").to_owned();
            let object = format!("entity:{}", entity.id);
            if entity.properties.get("origin").is_some()
                && !entity.origin().is_some_and(within_compile_bounds)
            {
                report.diagnostics.push(bounds_error(object));
                continue;
            }
            if classname.eq_ignore_ascii_case("env_cubemap") {
                map.add_cubemap(entity, report);
                continue;
            }
            if CONSUMED.contains(&classname.as_str()) {
                for solid in entity.solids.iter().filter(|s| !s.hidden) {
                    control.checkpoint()?;
                    map.add_brush(solid, 0, contents::DETAIL, report, control)?;
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
            let origins = map.prepare_origins(entity, report, control)?;
            let index = map.entities.len();
            let mut properties: Vec<(String, String)> = entity
                .properties
                .iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect();
            if !origins.volumes.is_empty()
                && let Some(origin) = origins.position
            {
                properties.retain(|(key, _)| key != "origin");
                properties.push(("origin".into(), source_formats::vmf::format_vec3(origin)));
            }
            properties.push(("hammerid".into(), entity.id.to_string()));
            for connection in entity.connections.iter().flatten() {
                properties.push((connection.output.clone(), connection.value.clone()));
            }
            map.entities.push(MapEntity {
                id: entity.id,
                classname,
                properties,
                brushes: Vec::new(),
                origin: origins.position,
            });
            for solid in entity.solids.iter().filter(|s| !s.hidden) {
                control.checkpoint()?;
                if !origins.volumes.contains(&solid.id) {
                    map.add_brush(solid, index, 0, report, control)?;
                }
            }
            if !origins.volumes.is_empty() {
                let entity = &map.entities[index];
                report.diagnostics.push(if entity.brushes.is_empty() {
                    Diagnostic::error("An origin brush requires compiled brush geometry in the same runtime entity", Some(format!("entity:{}", entity.id)), entity.origin)
                } else {
                    Diagnostic::info("Consumed origin brush; remaining geometry uses the entity pivot", Some(format!("entity:{}", entity.id)), entity.origin)
                });
            }
        }
        if !map.cubemaps.is_empty() {
            report.diagnostics.push(Diagnostic::warning(
                "Cubemap sample records compiled; reflection material binding and texture capture are not implemented yet",
                None,
                None,
            ));
        }
        if hidden > 0 {
            report.diagnostics.push(Diagnostic::info(
                format!("{hidden} hidden objects were not compiled"),
                None,
                None,
            ));
        }
        Ok(map)
    }

    fn add_brush(
        &mut self,
        solid: &Solid,
        entity: usize,
        extra_contents: i32,
        report: &mut Report,
        control: &Control,
    ) -> Result<(), String> {
        let object = format!("solid:{}", solid.id);
        let Some(mut geometry) = origins::checked_geometry(solid, report) else {
            return Ok(());
        };
        if solid.sides.iter().any(|side| side.displacement.is_some()) {
            report.diagnostics.push(Diagnostic::warning(
                "displacements are not compiled yet; brush left out",
                Some(object),
                None,
            ));
            return Ok(());
        }
        if self.origin_material(solid, control)? {
            report.diagnostics.push(Diagnostic::error("Origin brushes must belong to a runtime brush entity; world and func_detail origins are unsupported", Some(object), Some(geometry.center())));
            return Ok(());
        }
        let world_center = geometry.center();
        let origin = self.entities[entity].origin.unwrap_or(DVec3::ZERO);
        if origin != DVec3::ZERO {
            geometry.mins -= origin;
            geometry.maxs -= origin;
            if !within_compile_bounds(geometry.mins) || !within_compile_bounds(geometry.maxs) {
                report.diagnostics.push(Diagnostic::error(format!("Brush model local coordinates exceed ±{MAX_COMPILE_COORD} units; move the entity pivot closer to its brushes"), Some(object), Some(world_center)));
                return Ok(());
            }
            for face in &mut geometry.faces {
                control.checkpoint()?;
                face.plane.dist -= face.plane.normal.dot(origin);
                for point in &mut face.vertices {
                    *point -= origin;
                }
            }
        }

        let side_budget = bevels::MAX_BRUSH_SIDES - self.brush_side_count;
        if geometry.faces.len() > side_budget || self.planes.len() > bevels::MAX_PLANES {
            report.diagnostics.push(Diagnostic::error(
                "BSP collision plane or brush side limit exceeded; reduce map geometry",
                Some(object),
                Some(world_center),
            ));
            return Ok(());
        }
        let mut sides = Vec::with_capacity(geometry.faces.len() + 6);
        let mut brush_contents = 0;
        let mut solid_sides = false;
        let mut hint = false;
        for face in &geometry.faces {
            control.checkpoint()?;
            let side = &solid.sides[face.side];
            let material = self.materials.load(&side.material);
            let info = self.materials.get(material);
            brush_contents |= info.contents;
            solid_sides |= info.contents == contents::SOLID;
            hint |= info.hint;
            if !info.unsupported_flags.is_empty()
                && !sides.iter().any(|side: &MapSide| side.material == material)
            {
                report.diagnostics.push(Diagnostic::warning(
                    format!(
                        "material {} uses unsupported compile flags {}; output may be incomplete",
                        info.name,
                        info.unsupported_flags.join(", ")
                    ),
                    Some(object.clone()),
                    Some(world_center),
                ));
            }
            let visible = info.draws;
            let Some(plane) = support::find_authored(&mut self.planes, &geometry, face, control)?
            else {
                report.diagnostics.push(Diagnostic::error(
                    "cannot preserve authored BSP plane support; simplify this brush",
                    Some(object),
                    Some(world_center),
                ));
                return Ok(());
            };
            if self.planes.len() > bevels::MAX_PLANES {
                report.diagnostics.push(Diagnostic::error(
                    "BSP collision plane limit exceeded; reduce map geometry",
                    Some(object),
                    Some(world_center),
                ));
                return Ok(());
            }
            let texinfo = self.texinfo(side, material, origin);
            sides.push(MapSide {
                plane,
                material,
                texinfo,
                smoothing_groups: side.smoothing_groups.unwrap_or(0),
                winding: Some(Winding(face.vertices.clone())),
                bevel: false,
                visible,
            });
        }
        if hint {
            report.diagnostics.push(Diagnostic::warning(
                "hint visibility splitting is not supported yet; brush left out",
                Some(object),
                Some(world_center),
            ));
            return Ok(());
        }
        if solid_sides && brush_contents & !(contents::SOLID | contents::DETAIL) != 0 {
            // A mix like nodraw + clip; the special contents win.
            brush_contents &= !contents::SOLID;
            if brush_contents & (contents::WINDOW | contents::TRANSLUCENT) != 0 {
                brush_contents |= contents::WINDOW;
            }
        }
        if brush_contents == 0 {
            report.diagnostics.push(Diagnostic::warning(
                "brush has only skip/fog surfaces and no compiled contents; brush left out",
                Some(object),
                Some(world_center),
            ));
            return Ok(());
        }
        if brush_contents & contents::AREAPORTAL != 0 {
            report.diagnostics.push(Diagnostic::warning(
                "areaportal brushes are not supported yet; brush left out",
                Some(object),
                Some(world_center),
            ));
            return Ok(());
        }
        brush_contents |= extra_contents;

        if let Err(error) = bevels::add(
            &mut sides,
            &mut self.planes,
            &geometry,
            side_budget,
            control,
        ) {
            match error {
                bevels::Error::Cancelled(message) => return Err(message),
                bevels::Error::Limit(message) | bevels::Error::Invalid(message) => {
                    report.diagnostics.push(Diagnostic::error(
                        message,
                        Some(object),
                        Some(world_center),
                    ));
                    return Ok(());
                }
            }
        }

        let index = self.brushes.len();
        self.brush_side_count += sides.len();
        self.brushes.push(MapBrush {
            id: solid.id,
            entity,
            contents: brush_contents,
            sides,
            mins: geometry.mins,
            maxs: geometry.maxs,
        });
        self.entities[entity].brushes.push(index);
        Ok(())
    }

    fn texinfo(&mut self, side: &Side, material: usize, origin: DVec3) -> usize {
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
            [
                a.x as f32,
                a.y as f32,
                a.z as f32,
                (axis.offset + origin.dot(a)) as f32,
            ]
        };
        let lightmap_vec = |axis: &source_formats::vmf::UvAxis| {
            let a = axis.axis / lightmap_scale;
            [
                a.x as f32,
                a.y as f32,
                a.z as f32,
                (axis.offset * axis.scale / lightmap_scale + origin.dot(a)) as f32,
            ]
        };
        self.texinfos.add(bsp::TexInfo {
            texture_vecs: [vec(&side.u_axis), vec(&side.v_axis)],
            lightmap_vecs: [lightmap_vec(&side.u_axis), lightmap_vec(&side.v_axis)],
            flags: info.surface,
            texdata: texdata as i32,
        })
    }
}
