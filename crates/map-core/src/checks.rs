use std::collections::BTreeMap;

use glam::DVec3;
use source_formats::vmf::{Properties, Side, Solid, Vmf};

use crate::surface::FaceId;
use crate::{BrushGeometry, DisplacementMesh, ObjectId, Plane};

pub const SOURCE_COORDINATE_LIMIT: f64 = 16_384.0;
const MAX_CHECKED_BRUSH_SIDES: usize = 256;
const MAX_ISSUES: usize = 10_000;
const ALPHA_ROUNDING_TOLERANCE: f64 = 0.01;
const DISPLACEMENT_CORNER_TOLERANCE: f64 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    ResetProjection(FaceId),
    ResetLightmapScale(FaceId),
}

#[derive(Debug, Clone, Default)]
pub struct Location {
    pub object: Option<ObjectId>,
    pub face: Option<FaceId>,
    pub property: Option<String>,
    pub connection: Option<usize>,
    pub position: Option<DVec3>,
    pub hidden: bool,
    pub ambiguous: bool,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    pub location: Location,
    pub fix: Option<Fix>,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub issues: Vec<Issue>,
    pub omitted: usize,
}

impl Report {
    pub fn add(
        &mut self,
        severity: Severity,
        code: &'static str,
        location: &Location,
        message: impl Into<String>,
        fix: Option<Fix>,
    ) {
        if self.issues.len() == MAX_ISSUES {
            self.omitted += 1;
            return;
        }
        self.issues.push(Issue {
            code,
            severity,
            message: message.into(),
            location: location.clone(),
            fix,
        });
    }
}

fn count_ids(vmf: &Vmf) -> (BTreeMap<u32, usize>, BTreeMap<u32, usize>) {
    let mut objects = BTreeMap::new();
    let mut faces = BTreeMap::new();
    for id in std::iter::once(vmf.world.id)
        .chain(vmf.world.groups.iter().map(|group| group.id))
        .chain(vmf.entities.iter().map(|entity| entity.id))
        .chain(vmf.solids().map(|solid| solid.id))
    {
        *objects.entry(id).or_default() += 1;
    }
    for side in vmf.solids().flat_map(|solid| &solid.sides) {
        *faces.entry(side.id).or_default() += 1;
    }
    (objects, faces)
}

#[derive(Clone, Copy)]
enum IdKind {
    World,
    Group,
    Brush,
    Face,
    Entity,
}

impl IdKind {
    fn label(self) -> &'static str {
        match self {
            Self::World => "World",
            Self::Group => "Group",
            Self::Brush => "Brush",
            Self::Face => "Face",
            Self::Entity => "Entity",
        }
    }
}

fn check_id(report: &mut Report, id: u32, count: usize, kind: IdKind, location: &Location) {
    let label = kind.label();
    if id == 0 && !matches!(kind, IdKind::World) {
        report.add(
            Severity::Error,
            "zero_id",
            location,
            format!("{label} has ID 0; assign a unique positive ID before editing."),
            None,
        );
    }
    if count > 1 {
        report.add(
            Severity::Error,
            "duplicate_id",
            location,
            format!("{label} ID {id} is used {count} times; editing by ID is ambiguous."),
            None,
        );
    }
}

pub fn check_properties(report: &mut Report, properties: &Properties, location: &Location) {
    let mut keys = BTreeMap::new();
    for (key, _) in properties.iter() {
        *keys.entry(key.to_ascii_lowercase()).or_insert(0) += 1;
    }
    for (key, count) in keys {
        if count > 1 {
            let mut location = location.clone();
            location.property = Some(key.clone());
            report.add(Severity::Warning, "duplicate_property", &location, format!("{key} is stored {count} times. Inspect the values before changing this property."), None);
        }
    }
}

fn independent_corner(point: DVec3, planes: &[Plane]) -> bool {
    let touching: Vec<_> = planes
        .iter()
        .filter(|plane| plane.distance_to(point).abs() < crate::plane::ON_EPSILON * 4.0)
        .map(|plane| plane.normal)
        .collect();
    for (i, a) in touching.iter().enumerate() {
        for (j, b) in touching.iter().enumerate().skip(i + 1) {
            let cross = a.cross(*b);
            if touching
                .iter()
                .skip(j + 1)
                .any(|c| cross.dot(*c).abs() > 1e-8)
            {
                return true;
            }
        }
    }
    false
}

pub fn projection_valid(side: &Side) -> bool {
    let Some(plane) = Plane::from_points(side.plane.0) else {
        return false;
    };
    let u = side.u_axis;
    let v = side.v_axis;
    if !u.axis.is_finite()
        || !v.axis.is_finite()
        || !u.offset.is_finite()
        || !v.offset.is_finite()
        || !u.scale.is_finite()
        || !v.scale.is_finite()
        || u.scale.abs() < 1e-8
        || v.scale.abs() < 1e-8
        || !side.rotation.is_finite()
    {
        return false;
    }
    let area = u.axis.cross(v.axis).dot(plane.normal).abs();
    area >= 1e-12 && area >= 1e-8 * u.axis.length() * v.axis.length()
}

fn check_brush(
    report: &mut Report,
    solid: &Solid,
    mut location: Location,
    object_ids: &BTreeMap<u32, usize>,
    side_ids: &BTreeMap<u32, usize>,
) {
    location.ambiguous |= object_ids[&solid.id] > 1 || solid.id == 0;
    check_id(
        report,
        solid.id,
        object_ids[&solid.id],
        IdKind::Brush,
        &location,
    );
    let planes = solid
        .sides
        .iter()
        .map(|side| Plane::from_points(side.plane.0))
        .collect::<Option<Vec<_>>>();
    let geometry = if solid.sides.len() > MAX_CHECKED_BRUSH_SIDES {
        report.add(Severity::Warning, "geometry_check_limit", &location, format!("Brush has {} sides. Geometry checking is limited to {MAX_CHECKED_BRUSH_SIDES} sides per brush; simplify it or inspect it separately.", solid.sides.len()), None);
        None
    } else {
        match BrushGeometry::from_solid(solid) {
            Ok(geometry) => {
                if !geometry.mins.is_finite()
                    || !geometry.maxs.is_finite()
                    || planes.as_ref().is_none_or(|planes| {
                        geometry
                            .faces
                            .iter()
                            .flat_map(|face| &face.vertices)
                            .any(|point| !independent_corner(*point, planes))
                    })
                {
                    report.add(Severity::Error, "open_brush", &location, "Brush planes do not form a closed finite solid. Check missing or reversed faces.", None);
                } else {
                    location.position = Some(geometry.center());
                    if geometry.mins.abs().max(geometry.maxs.abs()).max_element()
                        > SOURCE_COORDINATE_LIMIT
                    {
                        report.add(Severity::Warning, "world_bounds", &location, "Brush extends beyond the Source SDK reference coordinate range of ±16384 units.", None);
                    }
                    if geometry.faces.len() != solid.sides.len() {
                        report.add(Severity::Warning, "redundant_faces", &location, format!("{} stored sides produce {} visible faces; inspect duplicate or redundant planes.", solid.sides.len(), geometry.faces.len()), None);
                    }
                }
                Some(geometry)
            }
            Err(error) => {
                report.add(
                    Severity::Error,
                    "invalid_brush",
                    &location,
                    format!("Invalid brush: {error}."),
                    None,
                );
                None
            }
        }
    };
    for (index, side) in solid.sides.iter().enumerate() {
        let id = FaceId {
            solid: solid.id,
            side: side.id,
        };
        let mut face_location = location.clone();
        face_location.face = Some(id);
        face_location.path = format!("{}/side[{index}]", location.path);
        face_location.ambiguous |= side.id == 0 || side_ids[&side.id] > 1;
        let face = geometry
            .as_ref()
            .and_then(|geometry| geometry.faces.iter().find(|face| face.side == index));
        if let Some(face) = face {
            let center = face.vertices.iter().copied().sum::<DVec3>() / face.vertices.len() as f64;
            if center.is_finite() {
                face_location.position = Some(center);
            }
        }
        check_id(
            report,
            side.id,
            side_ids[&side.id],
            IdKind::Face,
            &face_location,
        );
        if side.material.trim().is_empty() {
            report.add(
                Severity::Error,
                "empty_material",
                &face_location,
                "Face has no material. Choose a material before compiling.",
                None,
            );
        }
        if !projection_valid(side) {
            let fix = (!face_location.ambiguous && Plane::from_points(side.plane.0).is_some())
                .then_some(Fix::ResetProjection(id));
            report.add(Severity::Error, "invalid_projection", &face_location, "Texture projection has invalid values or collapses on this face. Resetting uses world-aligned axes, 0.25 scale and zero shift/rotation.", fix);
        }
        if side.lightmap_scale == 0 {
            let fix = (!face_location.ambiguous).then_some(Fix::ResetLightmapScale(id));
            report.add(
                Severity::Error,
                "zero_lightmap_scale",
                &face_location,
                "Lightmap scale is zero. Resetting uses 16 world units per lightmap sample.",
                fix,
            );
        }
        if let Some(displacement) = &side.displacement {
            match face.map(|face| DisplacementMesh::build(face, displacement)) {
                Some(Ok(mesh)) => {
                    if !displacement.start_position.is_finite()
                        || !displacement.elevation.is_finite()
                        || mesh.positions.iter().any(|point| !point.is_finite())
                        || mesh.alphas.iter().any(|alpha| !alpha.is_finite())
                    {
                        report.add(
                            Severity::Error,
                            "invalid_displacement",
                            &face_location,
                            "Displacement has nonfinite positions or blend weights.",
                            None,
                        );
                    }
                    if mesh.alphas.iter().any(|alpha| {
                        !(-ALPHA_ROUNDING_TOLERANCE..=255.0 + ALPHA_ROUNDING_TOLERANCE)
                            .contains(alpha)
                    }) {
                        report.add(Severity::Warning,"displacement_blend_range",&face_location,"Terrain blend weights are outside the usual 0–255 range. Inspect the material transition; imported values are preserved.",None);
                    }
                    if let Some(face) = face {
                        let distance = face
                            .vertices
                            .iter()
                            .map(|point| point.distance(displacement.start_position))
                            .fold(f64::INFINITY, f64::min);
                        if distance > DISPLACEMENT_CORNER_TOLERANCE {
                            report.add(Severity::Warning,"displacement_start",&face_location,format!("Displacement start position is {distance:.3} units from the nearest base-face corner."),None);
                        }
                    }
                }
                Some(Err(error)) => report.add(
                    Severity::Error,
                    "invalid_displacement",
                    &face_location,
                    format!("Invalid displacement: {error}."),
                    None,
                ),
                None if solid.sides.len() > MAX_CHECKED_BRUSH_SIDES => {}
                None => report.add(
                    Severity::Error,
                    "invalid_displacement",
                    &face_location,
                    "Displacement has no valid base face.",
                    None,
                ),
            }
        }
    }
}

pub fn check(vmf: &Vmf) -> Report {
    let mut report = Report::default();
    let (objects, faces) = count_ids(vmf);
    let world = Location {
        path: "world".into(),
        ambiguous: objects[&vmf.world.id] > 1,
        ..Default::default()
    };
    if objects.contains_key(&u32::MAX) || faces.contains_key(&u32::MAX) {
        report.add(Severity::Warning,"id_space_exhausted",&world,"The highest available object or face ID is already used. Operations that allocate those IDs will fail until the map is renumbered.",None);
    }
    check_id(
        &mut report,
        vmf.world.id,
        objects[&vmf.world.id],
        IdKind::World,
        &world,
    );
    check_properties(&mut report, &vmf.world.properties, &world);
    if vmf
        .world
        .properties
        .get("classname")
        .is_none_or(|class| !class.eq_ignore_ascii_case("worldspawn"))
    {
        report.add(
            Severity::Error,
            "world_class",
            &world,
            "World classname must be worldspawn.",
            None,
        );
    }
    for (index, group) in vmf.world.groups.iter().enumerate() {
        check_id(
            &mut report,
            group.id,
            objects[&group.id],
            IdKind::Group,
            &Location {
                path: format!("world/group[{index}]"),
                ..Default::default()
            },
        );
    }
    for (index, solid) in vmf.world.solids.iter().enumerate() {
        check_brush(
            &mut report,
            solid,
            Location {
                object: Some(ObjectId::Solid(solid.id)),
                hidden: solid.hidden,
                path: format!("world/solid[{index}]"),
                ..Default::default()
            },
            &objects,
            &faces,
        );
    }
    for (index, entity) in vmf.entities.iter().enumerate() {
        let mut location = Location {
            object: Some(ObjectId::Entity(entity.id)),
            hidden: entity.hidden,
            ambiguous: entity.id == 0 || objects[&entity.id] > 1,
            path: format!("entity[{index}]"),
            position: entity.origin().filter(|origin| origin.is_finite()),
            ..Default::default()
        };
        check_id(
            &mut report,
            entity.id,
            objects[&entity.id],
            IdKind::Entity,
            &location,
        );
        check_properties(&mut report, &entity.properties, &location);
        if entity.classname().is_none_or(|name| name.trim().is_empty()) {
            report.add(
                Severity::Error,
                "missing_class",
                &location,
                "Entity has no classname.",
                None,
            );
        }
        if let Some(origin) = entity.properties.get("origin") {
            location.property = Some("origin".into());
            match source_formats::vmf::parse_vec3(origin) {
                Some(point) if point.is_finite() => {
                    if point.abs().max_element() > SOURCE_COORDINATE_LIMIT {
                        report.add(Severity::Warning, "world_bounds", &location, "Entity origin is outside the Source SDK reference coordinate range of ±16384 units.", None);
                    }
                }
                _ => report.add(
                    Severity::Error,
                    "invalid_origin",
                    &location,
                    "Entity origin must contain three finite coordinates.",
                    None,
                ),
            }
            location.property = None;
        }
        for (index, solid) in entity.solids.iter().enumerate() {
            let mut brush_location = location.clone();
            brush_location.path = format!("{}/solid[{index}]", location.path);
            brush_location.hidden |= solid.hidden;
            check_brush(&mut report, solid, brush_location, &objects, &faces);
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, edit};

    fn fixture() -> Vmf {
        let mut doc = Document::empty();
        doc.edit("box", |tx| {
            edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "brick/test")
        })
        .unwrap();
        doc.vmf().clone()
    }

    #[test]
    fn sdk_world_id_zero_is_valid_but_entity_id_zero_is_not() {
        let mut vmf = fixture();
        vmf.world.id = 0;
        assert!(check(&vmf).issues.is_empty());
        vmf.entities
            .push(source_formats::vmf::Entity::new(0, "info_target"));
        assert!(
            check(&vmf)
                .issues
                .iter()
                .any(|issue| issue.code == "zero_id"
                    && issue.location.object == Some(ObjectId::Entity(0)))
        );
    }

    #[test]
    fn valid_rotated_brushes_pass_and_missing_face_is_an_error() {
        let mut vmf = fixture();
        assert!(check(&vmf).issues.is_empty());
        let turn = glam::DQuat::from_euler(glam::EulerRot::XYZ, 0.2, 0.5, 0.1);
        for side in &mut vmf.world.solids[0].sides {
            for point in &mut side.plane.0 {
                *point = turn * *point;
            }
            side.u_axis.axis = turn * side.u_axis.axis;
            side.v_axis.axis = turn * side.v_axis.axis;
        }
        assert!(check(&vmf).issues.is_empty(), "{:?}", check(&vmf).issues);
        vmf.world.solids[0].sides.pop();
        assert!(
            check(&vmf)
                .issues
                .iter()
                .any(|issue| issue.code == "open_brush")
        );
    }

    #[test]
    fn malformed_brushes_and_nonfinite_projection_are_reported_without_mutation() {
        let mut vmf = fixture();
        let side = &mut vmf.world.solids[0].sides[0];
        side.u_axis.scale = 0.0;
        side.lightmap_scale = 0;
        side.material.clear();
        let before = vmf.clone();
        let report = check(&vmf);
        assert_eq!(vmf, before);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == "empty_material")
        );
        assert_eq!(
            report
                .issues
                .iter()
                .filter(|issue| issue.fix.is_some())
                .count(),
            2
        );
        vmf.world.solids[0].sides[1].plane.0 = [DVec3::ZERO; 3];
        assert!(
            check(&vmf)
                .issues
                .iter()
                .any(|issue| issue.code == "invalid_brush")
        );
        vmf.world.solids[0].sides[0].u_axis.scale = f64::INFINITY;
        assert!(
            check(&vmf)
                .issues
                .iter()
                .any(|issue| issue.code == "invalid_projection")
        );
    }

    #[test]
    fn duplicate_ids_have_paths_and_disable_repairs_and_navigation() {
        let mut vmf = fixture();
        vmf.world.solids[0].sides[0].lightmap_scale = 0;
        vmf.world.solids.push(vmf.world.solids[0].clone());
        let report = check(&vmf);
        let duplicate: Vec<_> = report
            .issues
            .iter()
            .filter(|issue| issue.code == "duplicate_id")
            .collect();
        assert_eq!(duplicate.len(), 14);
        assert!(duplicate.iter().all(|issue| issue.location.ambiguous));
        assert!(report.issues.iter().all(|issue| issue.fix.is_none()));
        assert!(
            duplicate
                .iter()
                .any(|issue| issue.location.path.starts_with("world/solid[1]"))
        );
    }

    #[test]
    fn hidden_content_and_custom_keys_are_preserved_and_checked() {
        let mut vmf = fixture();
        let mut entity = source_formats::vmf::Entity::new(100, "custom_class");
        entity.hidden = true;
        entity.properties.set("custom_setting", "anything");
        entity.properties.set("origin", "not coordinates");
        vmf.entities.push(entity);
        let report = check(&vmf);
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].code, "invalid_origin");
        assert!(report.issues[0].location.hidden);
        assert_eq!(
            report.issues[0].location.property.as_deref(),
            Some("origin")
        );
    }

    #[test]
    fn projection_checks_allow_mirrored_axes_but_reject_collapsed_mapping() {
        let mut vmf = fixture();
        let side = &mut vmf.world.solids[0].sides[0];
        side.u_axis.scale = -0.25;
        assert!(projection_valid(side));
        side.v_axis.axis = side.u_axis.axis;
        assert!(!projection_valid(side));
    }
}
