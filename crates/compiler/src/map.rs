//! The VMF converted into compiler brushes, sides and entities.

use std::collections::HashMap;

use glam::DVec3;
use map_core::Plane;
use map_core::brush::BrushGeometry;
use source_formats::bsp::{self, contents};
use source_formats::gamefs::GameFs;
use source_formats::vmf::{Entity, Side, Solid, Vmf};

use crate::materials::{Materials, PropSourceMode, Requirements};
use crate::planes::PlaneSet;
use crate::winding::Winding;
use crate::{Control, Diagnostic, Report};

#[path = "bevels.rs"]
mod bevels;

#[path = "support.rs"]
mod support;

#[path = "origins.rs"]
pub(crate) mod origins;

#[path = "cubemaps.rs"]
mod cubemaps;

#[path = "occluders.rs"]
pub(crate) mod occluders;

#[path = "terrain.rs"]
pub(crate) mod terrain;
pub use terrain::Terrain;

pub const DEFAULT_LIGHTMAP_SCALE: f64 = 16.0;
pub use map_core::checks::SOURCE_COORDINATE_LIMIT as MAX_COMPILE_COORD;

pub(crate) fn within_compile_bounds(point: DVec3) -> bool {
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

pub fn unresolved_instance_class(classname: &str) -> bool {
    ["func_instance", "func_instance_parms"]
        .iter()
        .any(|name| name.eq_ignore_ascii_case(classname))
}

pub fn unresolved_instance_diagnostic(entity: &Entity) -> Option<Diagnostic> {
    let classname = entity.classname()?;
    (!entity.hidden && unresolved_instance_class(classname)).then(|| {
        Diagnostic::error(
            format!(
                "{classname} requires referenced-map instance expansion, which is not implemented; resolve it into ordinary map content or hide/remove the helper before building"
            ),
            Some(format!("entity:{}", entity.id)),
            entity.origin().filter(|point| within_compile_bounds(*point)),
        )
    })
}

pub fn unsupported_entity_class(classname: &str) -> bool {
    unresolved_instance_class(classname)
}

#[derive(Debug, Clone)]
pub struct MapSide {
    pub source_id: Option<u32>,
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
    /// Brushes whose planes split the world tree.
    pub fn structural(&self) -> bool {
        self.contents & contents::DETAIL == 0
            && self.contents
                & (contents::SOLID
                    | contents::WINDOW
                    | contents::WATER
                    | contents::SLIME
                    | contents::AREAPORTAL)
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
    pub(crate) water_bottom_texinfos: HashMap<usize, usize>,
    pub(crate) water_current: Option<[f32; 3]>,
    pub(crate) water_damping: Option<f32>,
    pub(crate) water_analogs: Vec<crate::water_analog::Carrier>,
    pub brushes: Vec<MapBrush>,
    /// Entity 0 is worldspawn.
    pub entities: Vec<MapEntity>,
    pub cubemaps: Vec<bsp::Cubemap>,
    pub(crate) occluders: Vec<occluders::Definition>,
    occluder_csg: occluders::CsgBudget,
    pub terrains: Vec<Terrain>,
    pub(crate) area_portals: Vec<crate::areas::PortalBrush>,
    pub(crate) overlays: crate::overlays::Plan,
    pub(crate) hints: Vec<crate::hints::Hint>,
    pub(crate) visclusters: Vec<crate::visclusters::Definition>,
    pub(crate) viscluster_attempts: usize,
    pub(crate) viscluster_work: usize,
    brush_side_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootSide {
    pub owner: map_core::ObjectId,
    pub solid: u32,
    pub side: u32,
}

#[derive(Debug)]
pub struct UnsupportedBrush {
    pub owner: map_core::ObjectId,
    pub solid: u32,
    pub bounds: Option<(DVec3, DVec3)>,
}

/// Flat world/detail conversion, not a complete region or unsupported-content inventory.
/// A resolver must classify omitted geometry from the original VMF before admitting this topology.
pub struct PreparedWorld<'a> {
    pub map: Map<'a>,
    pub source_sides: std::collections::BTreeMap<u32, RootSide>,
    pub displacements: Vec<UnsupportedBrush>,
}

impl<'a> Map<'a> {
    fn empty(fs: &'a GameFs, requirements: Requirements) -> Self {
        Self {
            planes: PlaneSet::default(),
            materials: Materials::with_requirements(fs, requirements),
            texinfos: TexInfos::default(),
            texdata: Vec::new(),
            water_bottom_texinfos: HashMap::new(),
            water_current: None,
            water_damping: None,
            water_analogs: Vec::new(),
            brushes: Vec::new(),
            entities: Vec::new(),
            cubemaps: Vec::new(),
            occluders: Vec::new(),
            occluder_csg: occluders::CsgBudget::default(),
            terrains: Vec::new(),
            area_portals: Vec::new(),
            overlays: crate::overlays::Plan::default(),
            hints: Vec::new(),
            visclusters: Vec::new(),
            viscluster_attempts: 0,
            viscluster_work: 0,
            brush_side_count: 0,
        }
    }

    fn add_world(&mut self, vmf: &Vmf) {
        let mut properties: Vec<(String, String)> = vmf
            .world
            .properties
            .iter()
            .filter(|(key, _)| {
                !key.eq_ignore_ascii_case(crate::GENERATED_DETAIL_LIGHTING_TINT_KEY)
                    && !key.eq_ignore_ascii_case(crate::WATER_CURRENT_VELOCITY_KEY)
                    && !key.eq_ignore_ascii_case(crate::WATER_DAMPING_KEY)
            })
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect();
        if !properties.iter().any(|(key, _)| key == "classname") {
            properties.insert(0, ("classname".into(), "worldspawn".into()));
        }
        self.entities.push(MapEntity {
            id: vmf.world.id,
            classname: "worldspawn".into(),
            properties,
            brushes: Vec::new(),
            origin: None,
        });
    }

    /// Converts flat world/func_detail brushes; other entity geometry requires separate admission.
    pub fn prepare_world_geometry_controlled(
        vmf: &Vmf,
        fs: &'a GameFs,
        report: &mut Report,
        control: &Control,
    ) -> Result<PreparedWorld<'a>, String> {
        control.checkpoint()?;
        vmf.validate_ids().map_err(|error| error.to_string())?;
        let mut map = Self::empty(fs, Requirements::GeometrySemantics);
        map.add_world(vmf);
        let mut source_sides = std::collections::BTreeMap::new();
        let mut displacements = Vec::new();
        let sources = vmf
            .world
            .solids
            .iter()
            .map(|solid| (map_core::ObjectId::Solid(solid.id), solid, 0))
            .chain(
                vmf.entities
                    .iter()
                    .filter(|entity| !entity.hidden && entity.classname() == Some("func_detail"))
                    .flat_map(|entity| {
                        entity.solids.iter().map(move |solid| {
                            (
                                map_core::ObjectId::Entity(entity.id),
                                solid,
                                contents::DETAIL,
                            )
                        })
                    }),
            );
        for (owner, solid, extra_contents) in sources.filter(|(_, solid, _)| !solid.hidden) {
            control.checkpoint()?;
            if solid.sides.iter().any(|side| side.displacement.is_some()) {
                if origins::checked_geometry(solid, report).is_some() {
                    displacements.push(UnsupportedBrush {
                        owner,
                        solid: solid.id,
                        bounds: None,
                    });
                }
                continue;
            }
            map.add_brush(solid, 0, extra_contents, report, control)?;
            if report.has_errors() {
                return Err(
                    "World geometry preparation reported invalid structural geometry".into(),
                );
            }
            for side in &solid.sides {
                source_sides.insert(
                    side.id,
                    RootSide {
                        owner,
                        solid: solid.id,
                        side: side.id,
                    },
                );
            }
        }
        if report.has_errors() {
            return Err("World geometry preparation reported invalid structural geometry".into());
        }
        Ok(PreparedWorld {
            map,
            source_sides,
            displacements,
        })
    }

    pub fn load(vmf: &Vmf, fs: &'a GameFs, report: &mut Report) -> Result<Self, String> {
        Self::load_controlled(vmf, fs, report, &Control::default())
    }

    pub fn load_controlled(
        vmf: &Vmf,
        fs: &'a GameFs,
        report: &mut Report,
        control: &Control,
    ) -> Result<Self, String> {
        Self::load_inner(vmf, fs, report, control, None, None)
    }

    pub(crate) fn load_with_prop_sources_controlled(
        vmf: &Vmf,
        fs: &'a GameFs,
        mode: PropSourceMode,
        report: &mut Report,
        control: &Control,
    ) -> Result<Self, String> {
        Self::load_inner(vmf, fs, report, control, Some(mode), None)
    }

    pub(crate) fn load_with_material_sources_controlled(
        vmf: &Vmf,
        fs: &'a GameFs,
        report: &mut Report,
        control: &Control,
        prop_sources: Option<PropSourceMode>,
        material_sources: &crate::asset_sources::BuildMaterialSources,
    ) -> Result<Self, String> {
        Self::load_inner(
            vmf,
            fs,
            report,
            control,
            prop_sources,
            Some(material_sources),
        )
    }

    fn load_inner(
        vmf: &Vmf,
        fs: &'a GameFs,
        report: &mut Report,
        control: &Control,
        prop_sources: Option<PropSourceMode>,
        material_sources: Option<&crate::asset_sources::BuildMaterialSources>,
    ) -> Result<Self, String> {
        control.checkpoint()?;
        let mut map = Self::empty(fs, Requirements::Compile);
        if let Some(sources) = material_sources {
            map.materials = Materials::with_build_sources(fs, Requirements::Compile, sources);
        }
        if let Some(mode) = prop_sources {
            map.materials.track_prop_lighting_sources(mode)?;
        }
        if let Err(error) = vmf.validate_ids() {
            report
                .diagnostics
                .push(Diagnostic::error(error.to_string(), None, None));
            return Ok(map);
        }
        map.add_world(vmf);

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
            if let Some(diagnostic) = unresolved_instance_diagnostic(entity) {
                report.diagnostics.push(diagnostic);
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
            if classname.eq_ignore_ascii_case("func_viscluster") {
                crate::visclusters::load(&mut map, entity, report, control)?;
                continue;
            }
            if classname.eq_ignore_ascii_case("func_occluder") {
                map.add_occluder(entity, report, control)?;
                continue;
            }
            if classname.eq_ignore_ascii_case("info_overlay")
                || classname.eq_ignore_ascii_case("info_overlay_transition")
            {
                continue;
            }
            if classname.eq_ignore_ascii_case("prop_static")
                || classname.eq_ignore_ascii_case("info_lighting")
                || classname.eq_ignore_ascii_case("prop_detail")
                || classname.eq_ignore_ascii_case("detail_prop")
                || classname.eq_ignore_ascii_case("prop_detail_sprite")
            {
                continue;
            }
            if CONSUMED.contains(&classname.as_str()) {
                for solid in entity.solids.iter().filter(|s| !s.hidden) {
                    control.checkpoint()?;
                    map.add_brush(solid, 0, contents::DETAIL, report, control)?;
                }
                continue;
            }
            if classname.eq_ignore_ascii_case("func_areaportal")
                || classname.eq_ignore_ascii_case("func_areaportalwindow")
            {
                map.add_areaportal(entity, report, control)?;
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
            let water_analog = crate::water_analog::declared(entity);
            if water_analog && !crate::water_analog::admit_source(entity, report) {
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
            if water_analog {
                crate::water_analog::admit_carrier(&mut map, index, report);
            }
        }
        crate::areas::windows::validate(&map, report, control)?;
        crate::water::prepare_materials(&mut map, report, control)?;
        crate::overlays::load(vmf, &mut map, report, control)?;
        if !map.cubemaps.is_empty() {
            report.diagnostics.push(Diagnostic::warning(
                "Cubemap sample records compiled; enable Generate reflections in Build settings for reflection material binding and texture capture. Sample records alone contain no captured textures",
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
            return self.add_terrain(solid, &geometry, entity, extra_contents, report, control);
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
            let material = self.materials.load(&side.material)?;
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
            let texinfo = self.texinfo(side, material, origin)?;
            sides.push(MapSide {
                source_id: Some(side.id),
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
            if let Err(error) =
                crate::hints::collect(self, solid.id, entity, extra_contents, &sides)
            {
                report
                    .diagnostics
                    .push(Diagnostic::error(error, Some(object), Some(world_center)));
            }
            self.brush_side_count += sides.len();
            return Ok(());
        }
        if solid_sides && brush_contents & !(contents::SOLID | contents::DETAIL) != 0 {
            // A mix like nodraw + clip; the special contents win.
            brush_contents &= !contents::SOLID;
            if brush_contents & (contents::WINDOW | contents::TRANSLUCENT) != 0 {
                brush_contents |= contents::WINDOW;
            }
        }
        if extra_contents & contents::AREAPORTAL != 0 {
            brush_contents = contents::AREAPORTAL;
            for side in &mut sides {
                side.visible = false;
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
        if brush_contents & contents::AREAPORTAL != 0 && extra_contents & contents::AREAPORTAL == 0
        {
            report.diagnostics.push(Diagnostic::warning(
                "areaportal material requires func_areaportal; world brush left out",
                Some(object),
                Some(world_center),
            ));
            return Ok(());
        }
        brush_contents |= extra_contents;

        if extra_contents & contents::AREAPORTAL == 0
            && let Err(error) = bevels::add(
                &mut sides,
                &mut self.planes,
                &geometry,
                side_budget,
                control,
            )
        {
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

    fn add_areaportal(
        &mut self,
        entity: &Entity,
        report: &mut Report,
        control: &Control,
    ) -> Result<(), String> {
        let solids = entity
            .solids
            .iter()
            .filter(|solid| !solid.hidden)
            .collect::<Vec<_>>();
        if solids.is_empty() {
            return Ok(());
        }
        if solids.len() != 1
            || self.area_portals.len() >= (source_formats::bsp::MAX_AREA_PORTALS - 1) / 2
        {
            report.diagnostics.push(Diagnostic::error(
                "Areaportals require one visible convex brush, with at most 511 portals in a map",
                Some(format!("entity:{}", entity.id)),
                entity.origin(),
            ));
            return Ok(());
        }
        let index = self.entities.len();
        let mut properties = entity
            .properties
            .iter()
            .filter(|(key, _)| {
                !key.eq_ignore_ascii_case("model")
                    && !key.eq_ignore_ascii_case("portalnumber")
                    && !key.eq_ignore_ascii_case("PortalVersion")
                    && !key.eq_ignore_ascii_case("classname")
            })
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect::<Vec<_>>();
        properties.push(("hammerid".into(), entity.id.to_string()));
        properties.push(("PortalVersion".into(), "1".into()));
        let window = entity
            .classname()
            .is_some_and(|name| name.eq_ignore_ascii_case("func_areaportalwindow"));
        let classname = if window {
            "func_areaportalwindow"
        } else {
            "func_areaportal"
        };
        properties.push(("classname".into(), classname.into()));
        if window {
            crate::areas::windows::normalize(&mut properties, entity.id, report);
        }
        for connection in entity.connections.iter().flatten() {
            properties.push((connection.output.clone(), connection.value.clone()));
        }
        self.entities.push(MapEntity {
            id: entity.id,
            classname: classname.into(),
            properties,
            brushes: Vec::new(),
            origin: None,
        });
        let brush = self.brushes.len();
        self.add_brush(solids[0], 0, contents::AREAPORTAL, report, control)?;
        if self.brushes.len() != brush {
            self.area_portals.push(crate::areas::PortalBrush {
                entity: index,
                brush,
            });
        } else if !report.has_errors() {
            report.diagnostics.push(Diagnostic::error("Areaportal brush could not be compiled; use a convex volume without hint, origin or displacement tooling", Some(format!("entity:{}", entity.id)), entity.origin()));
        }
        Ok(())
    }

    fn texinfo(&mut self, side: &Side, material: usize, origin: DVec3) -> Result<usize, String> {
        let flags = self.materials.get(material).surface;
        let texdata = self.texture_data(material);
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
        let info = bsp::TexInfo {
            texture_vecs: [vec(&side.u_axis), vec(&side.v_axis)],
            lightmap_vecs: [lightmap_vec(&side.u_axis), lightmap_vec(&side.v_axis)],
            flags,
            texdata: texdata as i32,
        };
        if info
            .texture_vecs
            .iter()
            .chain(&info.lightmap_vecs)
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(format!(
                "side:{}: texture/lightmap projections must fit finite native floats",
                side.id
            ));
        }
        Ok(self.texinfos.add(info))
    }

    pub(crate) fn texture_data(&mut self, material: usize) -> usize {
        match self.texdata.iter().position(|&m| m == material) {
            Some(i) => i,
            None => {
                self.texdata.push(material);
                self.texdata.len() - 1
            }
        }
    }

    pub(crate) fn model_bounds(&self, entity: usize) -> (DVec3, DVec3) {
        let mut bounds = self.entities[entity].brushes.iter().fold(
            (DVec3::MAX, DVec3::MIN),
            |(lo, hi), &index| {
                (
                    lo.min(self.brushes[index].mins),
                    hi.max(self.brushes[index].maxs),
                )
            },
        );
        if entity == 0 {
            for point in self
                .terrains
                .iter()
                .flat_map(|terrain| &terrain.mesh.positions)
            {
                bounds = (bounds.0.min(*point), bounds.1.max(*point));
            }
        }
        bounds
    }
}

#[cfg(test)]
mod world_preparation_tests {
    use super::*;
    use map_core::{Document, ObjectId, edit};
    use source_formats::gamefs::{Mount, MountedPath};

    #[test]
    fn flat_preparation_preserves_source_owners_and_normal_compile_order() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("materials/dev")).unwrap();
        std::fs::write(
            directory
                .path()
                .join("materials/dev/dev_measuregeneric01.vmt"),
            "LightmappedGeneric {}",
        )
        .unwrap();
        let mut game = crate::test_common::empty_game();
        game.mounts.push(MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().to_owned()),
        });
        let mut document = Document::new(crate::test_common::room());
        let mut roots = Vec::new();
        for classname in ["func_brush", "func_detail"] {
            let pair = document
                .edit("brush owner", |tx| {
                    let ObjectId::Solid(solid) = edit::create_block(
                        tx,
                        DVec3::splat(32.0),
                        DVec3::splat(48.0),
                        "dev/dev_measuregeneric01",
                    )?
                    else {
                        unreachable!()
                    };
                    let ObjectId::Entity(entity) =
                        edit::tie_to_entity(tx, &[solid], classname, &[])?
                    else {
                        unreachable!()
                    };
                    Ok((solid, entity))
                })
                .unwrap();
            roots.push(pair);
        }
        let vmf = document.vmf();
        let mut report = Report::default();
        let compile = Map::load(vmf, &game, &mut report).unwrap();
        assert!(!report.has_errors(), "{report:?}");
        let expected = vmf
            .world
            .solids
            .iter()
            .map(|solid| solid.id)
            .chain(roots.iter().map(|(solid, _)| *solid))
            .collect::<Vec<_>>();
        assert_eq!(
            compile
                .brushes
                .iter()
                .map(|brush| brush.id)
                .collect::<Vec<_>>(),
            expected
        );
        let prepared = Map::prepare_world_geometry_controlled(
            vmf,
            &game,
            &mut Report::default(),
            &Control::default(),
        )
        .unwrap();
        assert_eq!(prepared.map.entities.len(), 1);
        assert!(prepared.displacements.is_empty());
        let detail = prepared
            .map
            .brushes
            .iter()
            .find(|brush| brush.id == roots[1].0)
            .unwrap();
        assert_ne!(detail.contents & contents::DETAIL, 0);
        assert!(!detail.structural());
        assert!(
            !prepared
                .map
                .brushes
                .iter()
                .any(|brush| brush.id == roots[0].0)
        );
        for side in &detail.sides {
            if let Some(id) = side.source_id {
                assert_eq!(
                    prepared.source_sides[&id].owner,
                    ObjectId::Entity(roots[1].1)
                );
            }
        }
        for solid in &vmf.world.solids {
            for side in &solid.sides {
                assert_eq!(
                    prepared.source_sides[&side.id].owner,
                    ObjectId::Solid(solid.id)
                );
            }
        }
        assert!(
            prepared
                .map
                .materials
                .iter()
                .all(|info| info.metrics == crate::materials::MetricState::Deferred)
        );
    }
}
