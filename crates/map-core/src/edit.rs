//! Editing operations. Each runs inside a [`Transaction`], so callers group
//! them into undoable steps with [`crate::Document::edit`].

use glam::DVec3;
use source_formats::vmf::{Entity, Plane, Side, Solid, UvAxis, format_vec3, parse_vec3};

use crate::document::{EditError, Object, ObjectId, Transaction};

pub use crate::transform::{Transform, transform, transform_each};

/// Hammer's default texture scale for new faces.
pub const DEFAULT_TEXTURE_SCALE: f64 = 0.25;
pub const DEFAULT_LIGHTMAP_SCALE: u32 = 16;

/// A face, or all faces of an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceTarget {
    Object(ObjectId),
    Side { solid: u32, side: u32 },
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

/// World-aligned texture axes for a face normal, as Hammer assigns them:
/// floors and ceilings map X/-Y, walls map their horizontal axis and -Z.
pub fn world_texture_axes(normal: DVec3) -> (DVec3, DVec3) {
    let a = normal.abs();
    if a.z >= a.x && a.z >= a.y {
        (DVec3::X, DVec3::NEG_Y)
    } else if a.x >= a.y {
        (DVec3::Y, DVec3::NEG_Z)
    } else {
        (DVec3::X, DVec3::NEG_Z)
    }
}

/// A new side on the plane through `points` (VMF winding), with
/// world-aligned texturing.
pub fn new_side(
    tx: &mut Transaction,
    points: [DVec3; 3],
    material: &str,
) -> Result<Side, EditError> {
    source_formats::keyvalues::validate_string(material)
        .map_err(|error| invalid(error.to_string()))?;
    let plane =
        crate::Plane::from_points(points).ok_or_else(|| invalid("collinear plane points"))?;
    let (u, v) = world_texture_axes(plane.normal);
    let axis = |axis| UvAxis {
        axis,
        offset: 0.0,
        scale: DEFAULT_TEXTURE_SCALE,
    };
    Ok(Side {
        id: tx.new_side_id()?,
        plane: Plane(points),
        material: material.to_ascii_uppercase(),
        u_axis: axis(u),
        v_axis: axis(v),
        rotation: 0.0,
        lightmap_scale: DEFAULT_LIGHTMAP_SCALE,
        smoothing_groups: Some(0),
        displacement: None,
        extra_properties: Default::default(),
        extra: Vec::new(),
    })
}

/// An axis-aligned box brush with every face using `material`.
pub fn block_solid(
    tx: &mut Transaction,
    mins: DVec3,
    maxs: DVec3,
    material: &str,
) -> Result<Solid, EditError> {
    if !(mins.cmplt(maxs).all()) {
        return Err(invalid(format!(
            "block mins {mins} must be below maxs {maxs} on every axis"
        )));
    }
    if material.trim().is_empty() {
        return Err(invalid("material must not be empty"));
    }
    let id = tx.new_object_id()?;
    let mut sides = Vec::with_capacity(6);
    for normal in [
        DVec3::Z,
        DVec3::NEG_Z,
        DVec3::NEG_X,
        DVec3::X,
        DVec3::Y,
        DVec3::NEG_Y,
    ] {
        // Pick in-plane directions a, b with a × b = normal and start from the
        // corner that keeps p1 + a and p1 + b on the box, so all three points
        // are box corners.
        let a = if normal.z != 0.0 {
            DVec3::NEG_X * normal.z
        } else {
            DVec3::Z
        };
        let b = normal.cross(a);
        let corner = |dir: DVec3, axis: usize| {
            if dir[axis] > 0.0 {
                mins[axis]
            } else {
                maxs[axis]
            }
        };
        let mut p1 = DVec3::ZERO;
        for axis in 0..3 {
            p1[axis] = if normal[axis] > 0.0 {
                maxs[axis]
            } else if normal[axis] < 0.0 {
                mins[axis]
            } else if a[axis] != 0.0 {
                corner(a, axis)
            } else {
                corner(b, axis)
            };
        }
        let size = maxs - mins;
        let p0 = p1 + a * size;
        let p2 = p1 + b * size;
        sides.push(new_side(tx, [p0, p1, p2], material)?);
    }
    Ok(Solid {
        id,
        sides,
        editor: Some(default_editor(None)),
        hidden: false,
        extra_properties: Default::default(),
        extra: Vec::new(),
    })
}

/// The `editor` block Hammer writes for new objects.
fn default_editor(color: Option<&str>) -> source_formats::vmf::Properties {
    let color = color.map_or_else(|| random_brush_color().to_string(), str::to_owned);
    source_formats::vmf::Properties(vec![
        ("color".into(), color),
        ("visgroupshown".into(), "1".into()),
        ("visgroupautoshown".into(), "1".into()),
    ])
}

fn random_brush_color() -> source_formats::vmf::Color {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let channel = |shift: u32| 100 + ((seed >> shift) % 156) as u8;
    source_formats::vmf::Color([channel(0), channel(8), channel(16)])
}

pub fn create_block(
    tx: &mut Transaction,
    mins: DVec3,
    maxs: DVec3,
    material: &str,
) -> Result<ObjectId, EditError> {
    let solid = block_solid(tx, mins, maxs, material)?;
    tx.add_solid(solid)
}

/// Creates a point entity, or a brush entity when `solids` are given.
pub fn create_entity(
    tx: &mut Transaction,
    classname: &str,
    origin: Option<DVec3>,
    properties: &[(String, String)],
    solids: Vec<Solid>,
) -> Result<ObjectId, EditError> {
    if classname.trim().is_empty() {
        return Err(invalid("classname must not be empty"));
    }
    source_formats::keyvalues::validate_string(classname)
        .map_err(|error| invalid(error.to_string()))?;
    for (key, value) in properties {
        source_formats::keyvalues::validate_string(key)
            .map_err(|error| invalid(error.to_string()))?;
        source_formats::keyvalues::validate_string(value)
            .map_err(|error| invalid(error.to_string()))?;
    }
    let mut entity = Entity::new(tx.new_object_id()?, classname);
    for (key, value) in properties {
        if key.eq_ignore_ascii_case("classname") {
            continue;
        }
        entity.properties.set(key.clone(), value.clone());
    }
    if let Some(origin) = origin {
        entity.properties.set("origin", format_vec3(origin));
    }
    entity.solids = solids;
    entity.editor = Some(default_editor(Some("220 30 220")));
    tx.add_entity(entity)
}

/// Sets (`Some`) or removes (`None`) entity keys. `classname` can be
/// changed but not removed.
pub fn set_properties(
    tx: &mut Transaction,
    entity: u32,
    changes: &[(String, Option<String>)],
) -> Result<(), EditError> {
    for (key, value) in changes {
        source_formats::keyvalues::validate_string(key)
            .map_err(|error| invalid(error.to_string()))?;
        if let Some(value) = value {
            source_formats::keyvalues::validate_string(value)
                .map_err(|error| invalid(error.to_string()))?;
        }
    }
    let entity = tx.entity_mut(entity)?;
    for (key, value) in changes {
        match value {
            Some(value) => entity.properties.set(key.clone(), value.clone()),
            None if key.eq_ignore_ascii_case("classname") => {
                return Err(invalid("classname cannot be removed"));
            }
            None => {
                entity.properties.remove(key);
            }
        }
    }
    Ok(())
}

pub fn delete(tx: &mut Transaction, ids: &[ObjectId]) -> Result<(), EditError> {
    for &id in ids {
        tx.remove(id)?;
    }
    Ok(())
}

/// Moves a brush. With `texture_lock`, texture offsets compensate so the
/// textures stay fixed to the brush.
pub fn translate_solid(solid: &mut Solid, delta: DVec3, texture_lock: bool) {
    for side in &mut solid.sides {
        for p in &mut side.plane.0 {
            *p += delta;
        }
        if texture_lock {
            for axis in [&mut side.u_axis, &mut side.v_axis] {
                let scale = if axis.scale == 0.0 { 1.0 } else { axis.scale };
                axis.offset -= delta.dot(axis.axis) / scale;
            }
        }
        if let Some(disp) = &mut side.displacement {
            disp.start_position += delta;
        }
    }
}

pub fn translate_entity(entity: &mut Entity, delta: DVec3, texture_lock: bool) {
    if let Some(origin) = entity.origin() {
        entity.properties.set("origin", format_vec3(origin + delta));
    }
    for solid in &mut entity.solids {
        translate_solid(solid, delta, texture_lock);
    }
}

pub fn translate(
    tx: &mut Transaction,
    ids: &[ObjectId],
    delta: DVec3,
    texture_lock: bool,
) -> Result<(), EditError> {
    for &id in ids {
        match id {
            ObjectId::Solid(id) => translate_solid(tx.solid_mut(id)?, delta, texture_lock),
            ObjectId::Entity(id) => translate_entity(tx.entity_mut(id)?, delta, texture_lock),
        }
    }
    Ok(())
}

fn renumber_solid(
    tx: &mut Transaction,
    solid: &mut Solid,
    mapping: &mut Vec<(ObjectId, ObjectId)>,
) -> Result<(), EditError> {
    let old = solid.id;
    solid.id = tx.new_object_id()?;
    mapping.push((ObjectId::Solid(old), ObjectId::Solid(solid.id)));
    for side in &mut solid.sides {
        side.id = tx.new_side_id()?;
    }
    Ok(())
}

/// Copies objects with fresh ids, offset by `offset`. Returns the copies in
/// the same order.
pub fn duplicate(
    tx: &mut Transaction,
    ids: &[ObjectId],
    offset: DVec3,
) -> Result<Vec<ObjectId>, EditError> {
    let mut copies = Vec::with_capacity(ids.len());
    let mut mapping = Vec::new();
    for &id in ids {
        let copy = match id {
            ObjectId::Solid(id) => {
                let mut solid = tx
                    .document()
                    .solid(id)
                    .ok_or(EditError::NotFound(ObjectId::Solid(id)))?
                    .clone();
                renumber_solid(tx, &mut solid, &mut mapping)?;
                translate_solid(&mut solid, offset, true);
                tx.add_solid(solid)?
            }
            ObjectId::Entity(id) => {
                let mut entity = tx
                    .document()
                    .entity(id)
                    .ok_or(EditError::NotFound(ObjectId::Entity(id)))?
                    .clone();
                entity.id = tx.new_object_id()?;
                mapping.push((ObjectId::Entity(id), ObjectId::Entity(entity.id)));
                for solid in &mut entity.solids {
                    renumber_solid(tx, solid, &mut mapping)?;
                }
                translate_entity(&mut entity, offset, true);
                tx.add_entity(entity)?
            }
        };
        copies.push(copy);
    }
    crate::context::copy_notes(tx, &mapping)?;
    Ok(copies)
}

/// Applies `f` to every brush of an object.
fn for_each_solid(
    tx: &mut Transaction,
    id: ObjectId,
    mut f: impl FnMut(&mut Solid) -> Result<(), EditError>,
) -> Result<(), EditError> {
    match id {
        ObjectId::Solid(id) => f(tx.solid_mut(id)?),
        ObjectId::Entity(id) => tx.entity_mut(id)?.solids.iter_mut().try_for_each(f),
    }
}

/// Applies a material to faces. Returns the number of faces changed.
pub fn set_material(
    tx: &mut Transaction,
    targets: &[FaceTarget],
    material: &str,
) -> Result<usize, EditError> {
    source_formats::keyvalues::validate_string(material)
        .map_err(|error| invalid(error.to_string()))?;
    if material.trim().is_empty() {
        return Err(invalid("material must not be empty"));
    }
    let material = material.to_ascii_uppercase();
    let mut changed = 0;
    for &target in targets {
        match target {
            FaceTarget::Object(id) => for_each_solid(tx, id, |solid| {
                for side in &mut solid.sides {
                    changed += usize::from(side.material != material);
                    side.material.clone_from(&material);
                }
                Ok(())
            })?,
            FaceTarget::Side { solid, side } => {
                let owner = tx
                    .document()
                    .owner_of_solid(solid)
                    .ok_or(EditError::NotFound(ObjectId::Solid(solid)))?;
                for_each_solid(tx, owner, |s| {
                    if s.id != solid {
                        return Ok(());
                    }
                    let face = s
                        .sides
                        .iter_mut()
                        .find(|f| f.id == side)
                        .ok_or_else(|| invalid(format!("solid {solid} has no side {side}")))?;
                    changed += usize::from(face.material != material);
                    face.material.clone_from(&material);
                    Ok(())
                })?;
            }
        }
    }
    Ok(changed)
}

/// Moves world brushes into a new brush entity (Hammer's "Tie to Entity").
pub fn tie_to_entity(
    tx: &mut Transaction,
    solids: &[u32],
    classname: &str,
    properties: &[(String, String)],
) -> Result<ObjectId, EditError> {
    if solids.is_empty() {
        return Err(invalid("no brushes to tie"));
    }
    let mut taken = Vec::with_capacity(solids.len());
    for &id in solids {
        match tx.remove(ObjectId::Solid(id))? {
            Object::Solid(solid) => taken.push(solid),
            Object::Entity(_) => unreachable!("removing a solid id yields a solid"),
        }
    }
    create_entity(tx, classname, None, properties, taken)
}

/// Moves a brush entity's brushes back into the world and deletes the
/// entity. Returns the brushes.
pub fn move_to_world(tx: &mut Transaction, entity: u32) -> Result<Vec<ObjectId>, EditError> {
    let Object::Entity(entity) = tx.remove(ObjectId::Entity(entity))? else {
        unreachable!("removing an entity id yields an entity");
    };
    if entity.solids.is_empty() {
        return Err(invalid(format!("entity {} has no brushes", entity.id)));
    }
    entity.solids.into_iter().map(|s| tx.add_solid(s)).collect()
}

pub fn entity_origin(entity: &Entity) -> Option<DVec3> {
    entity.properties.get("origin").and_then(parse_vec3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrushGeometry, Document};

    #[test]
    fn unrepresentable_edits_roll_back_without_consuming_ids_or_history() {
        let mut doc = Document::empty();
        let before = doc.vmf().clone();
        for (classname, properties) in [
            ("bad\"class", vec![]),
            ("audit", vec![("message".into(), "hello \"world\"".into())]),
            ("audit", vec![("bad\"key".into(), "value".into())]),
        ] {
            assert!(
                doc.edit("invalid entity", |tx| create_entity(
                    tx,
                    classname,
                    None,
                    &properties,
                    Vec::new()
                ))
                .is_err()
            );
            assert_eq!(doc.vmf(), &before);
            assert_eq!(doc.revision(), 0);
        }
        assert!(
            doc.edit("invalid material", |tx| create_block(
                tx,
                DVec3::ZERO,
                DVec3::splat(32.0),
                "bad\"material"
            ))
            .is_err()
        );
        assert_eq!(doc.vmf(), &before);
        let id = doc
            .edit("valid entity", |tx| {
                create_entity(tx, "audit", None, &[], Vec::new())
            })
            .unwrap();
        assert_eq!(id, ObjectId::Entity(2));
        assert_eq!(doc.revision(), 1);
        let before = doc.vmf().clone();
        assert!(
            doc.edit("invalid update", |tx| set_properties(
                tx,
                2,
                &[("message".into(), Some("hello \"world\"".into()))]
            ))
            .is_err()
        );
        assert!(
            doc.edit("invalid group", |tx| crate::groups::apply(
                tx,
                &crate::groups::Edit {
                    group: None,
                    name: "bad\"group".into(),
                    parent: None,
                    members: vec![crate::groups::Member::Object(id)],
                    ungroup: false
                }
            ))
            .is_err()
        );
        assert_eq!(doc.vmf(), &before);
        assert_eq!(doc.revision(), 1);
        doc.vmf().try_write(&Default::default()).unwrap();
        doc.undo().unwrap();
        assert_eq!(doc.vmf().entities.len(), 0);
        assert!(doc.undo().is_none());
    }

    #[test]
    fn blocks_have_outward_planes_and_exact_bounds() {
        let mut doc = Document::empty();
        let (mins, maxs) = (DVec3::new(-64.0, 0.0, 0.0), DVec3::new(64.0, 32.0, 128.0));
        let id = doc
            .edit("block", |tx| {
                create_block(tx, mins, maxs, "dev/dev_measuregeneric01")
            })
            .unwrap();
        let ObjectId::Solid(id) = id else { panic!() };
        let solid = doc.solid(id).unwrap();
        let brush = BrushGeometry::from_solid(solid).unwrap();
        assert_eq!(brush.faces.len(), 6);
        assert_eq!((brush.mins, brush.maxs), (mins, maxs));
        for side in &solid.sides {
            let plane = crate::Plane::from_points(side.plane.0).unwrap();
            let center = (mins + maxs) * 0.5;
            assert!(plane.distance_to(center) < 0.0, "normal must point out");
            for p in side.plane.0 {
                let on_corner = (0..3).all(|i| p[i] == mins[i] || p[i] == maxs[i]);
                assert!(on_corner, "{p} is not a box corner");
            }
        }
    }

    #[test]
    fn translate_with_texture_lock_keeps_texture_coordinates() {
        let mut doc = Document::empty();
        let id = doc
            .edit("block", |tx| {
                create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "a")
            })
            .unwrap();
        let ObjectId::Solid(sid) = id else { panic!() };
        let point = DVec3::new(64.0, 10.0, 20.0);
        let side_uv = |doc: &Document, p| crate::texel_uv(&doc.solid(sid).unwrap().sides[3], p);
        let before = side_uv(&doc, point);
        let delta = DVec3::new(16.0, 8.0, -4.0);
        doc.edit("move", |tx| translate(tx, &[id], delta, true))
            .unwrap();
        assert!((side_uv(&doc, point + delta) - before).length() < 1e-9);
    }

    #[test]
    fn duplicate_assigns_fresh_ids() {
        let mut doc = Document::empty();
        let id = doc
            .edit("block", |tx| {
                create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "a")
            })
            .unwrap();
        let copies = doc
            .edit("dup", |tx| duplicate(tx, &[id], DVec3::X * 64.0))
            .unwrap();
        let ObjectId::Solid(copy) = copies[0] else {
            panic!()
        };
        let (a, b) = (
            doc.vmf().world.solids[0].clone(),
            doc.solid(copy).unwrap().clone(),
        );
        assert_ne!(a.id, b.id);
        assert!(a.sides.iter().all(|s| b.sides.iter().all(|t| t.id != s.id)));
        assert_eq!(b.sides[0].plane.0[1].x, a.sides[0].plane.0[1].x + 64.0);
    }

    #[test]
    fn tie_and_move_back_to_world() {
        let mut doc = Document::empty();
        let id = doc
            .edit("block", |tx| {
                create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "a")
            })
            .unwrap();
        let ObjectId::Solid(sid) = id else { panic!() };
        let entity = doc
            .edit("tie", |tx| tie_to_entity(tx, &[sid], "func_detail", &[]))
            .unwrap();
        assert!(doc.vmf().world.solids.is_empty());
        let ObjectId::Entity(eid) = entity else {
            panic!()
        };
        assert_eq!(doc.entity(eid).unwrap().solids[0].id, sid);
        assert_eq!(doc.owner_of_solid(sid), Some(entity));

        let solids = doc.edit("untie", |tx| move_to_world(tx, eid)).unwrap();
        assert_eq!(solids, [id]);
        assert!(doc.vmf().entities.is_empty());
    }

    #[test]
    fn set_material_on_one_face_of_a_brush_entity() {
        let mut doc = Document::empty();
        let id = doc
            .edit("block", |tx| {
                create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "a")
            })
            .unwrap();
        let ObjectId::Solid(sid) = id else { panic!() };
        doc.edit("tie", |tx| tie_to_entity(tx, &[sid], "func_detail", &[]))
            .unwrap();
        let side = doc.vmf().entities[0].solids[0].sides[2].id;
        let changed = doc
            .edit("paint", |tx| {
                set_material(tx, &[FaceTarget::Side { solid: sid, side }], "b")
            })
            .unwrap();
        assert_eq!(changed, 1);
        let materials: Vec<&str> = doc.vmf().entities[0].solids[0]
            .sides
            .iter()
            .map(|s| s.material.as_str())
            .collect();
        assert_eq!(materials, ["A", "A", "B", "A", "A", "A"]);
    }
}
