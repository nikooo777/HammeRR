use std::collections::BTreeSet;

use glam::DVec3;
use source_formats::vmf::{Side, Solid, UvAxis};

use crate::edit::{DEFAULT_LIGHTMAP_SCALE, DEFAULT_TEXTURE_SCALE, world_texture_axes};
use crate::plane::ON_EPSILON;
use crate::{BrushGeometry, EditError, ObjectId, Plane, Transaction};

const PLANE_POINT_SPAN: f64 = 64.0;
const MAX_COORDINATE: f64 = 65_536.0;
const MIN_VOLUME: f64 = 1e-6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    Front,
    Back,
    Both,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cut {
    pub source: u32,
    pub solids: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClipResult {
    pub cuts: Vec<Cut>,
    pub objects: Vec<ObjectId>,
    pub unchanged: Vec<u32>,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

pub fn normalized_plane(plane: Plane) -> std::result::Result<Plane, EditError> {
    let length = plane.normal.length();
    if !plane.normal.is_finite() || !plane.dist.is_finite() || !length.is_finite() || length < 1e-8
    {
        return Err(invalid(
            "Cut plane needs a finite, nonzero normal and finite distance",
        ));
    }
    let length = if (length - 1.0).abs() < 1e-12 {
        1.0
    } else {
        length
    };
    let plane = Plane {
        normal: plane.normal / length,
        dist: plane.dist / length,
    };
    if !plane.dist.is_finite() || plane.dist.abs() > MAX_COORDINATE {
        return Err(invalid(
            "Cut plane is outside the supported coordinate range",
        ));
    }
    Ok(plane)
}

pub fn plane_points(plane: Plane) -> [DVec3; 3] {
    let center = plane.normal * plane.dist;
    let u = plane.normal.any_orthonormal_vector() * PLANE_POINT_SPAN;
    let v = plane.normal.cross(u);
    [center + u, center, center + v]
}

fn geometry(solid: &Solid) -> std::result::Result<BrushGeometry, EditError> {
    if solid
        .sides
        .iter()
        .any(|side| side.plane.0.iter().any(|point| !point.is_finite()))
    {
        return Err(invalid(format!(
            "Brush {} has nonfinite plane points",
            solid.id
        )));
    }
    let geometry = BrushGeometry::from_solid(solid)
        .map_err(|error| invalid(format!("Brush {}: {error}", solid.id)))?;
    let center = geometry.center();
    let volume: f64 = geometry
        .faces
        .iter()
        .map(|face| {
            let a = face.vertices[0] - center;
            face.vertices[1..]
                .windows(2)
                .map(|pair| a.dot((pair[0] - center).cross(pair[1] - center)) / 6.0)
                .sum::<f64>()
        })
        .sum();
    if !volume.is_finite()
        || volume.abs() <= MIN_VOLUME
        || geometry
            .faces
            .iter()
            .flat_map(|face| &face.vertices)
            .any(|point| !point.is_finite() || point.abs().max_element() > MAX_COORDINATE)
    {
        return Err(invalid(format!(
            "Brush {} has empty, unbounded or invalid geometry",
            solid.id
        )));
    }
    Ok(geometry)
}

fn half(solid: &Solid, plane: Plane, material: &str) -> std::result::Result<Solid, EditError> {
    let mut part = solid.clone();
    let (u, v) = world_texture_axes(plane.normal);
    part.sides.push(Side {
        id: 0,
        plane: source_formats::vmf::Plane(plane_points(plane)),
        material: material.to_ascii_uppercase(),
        u_axis: UvAxis {
            axis: u,
            offset: 0.0,
            scale: DEFAULT_TEXTURE_SCALE,
        },
        v_axis: UvAxis {
            axis: v,
            offset: 0.0,
            scale: DEFAULT_TEXTURE_SCALE,
        },
        rotation: 0.0,
        lightmap_scale: DEFAULT_LIGHTMAP_SCALE,
        smoothing_groups: Some(0),
        displacement: None,
        extra_properties: Default::default(),
        extra: Vec::new(),
    });
    let shape = geometry(&part)?;
    let active: BTreeSet<_> = shape.faces.iter().map(|face| face.side).collect();
    part.sides = part
        .sides
        .into_iter()
        .enumerate()
        .filter_map(|(index, side)| active.contains(&index).then_some(side))
        .collect();
    if part.sides.last().is_none_or(|side| {
        side.id != 0 || side.plane != source_formats::vmf::Plane(plane_points(plane))
    }) {
        return Err(invalid("Cut would produce a degenerate cap face"));
    }
    geometry(&part)?;
    Ok(part)
}

struct PlannedCut {
    owner: ObjectId,
    source: u32,
    parts: Vec<Solid>,
}

pub fn clip(
    tx: &mut Transaction,
    ids: &[ObjectId],
    plane: Plane,
    keep: Keep,
    material: Option<&str>,
) -> std::result::Result<ClipResult, EditError> {
    let plane = normalized_plane(plane)?;
    if ids.is_empty() {
        return Err(invalid("Select at least one brush or brush entity to clip"));
    }
    if material.is_some_and(|name| name.trim().is_empty()) {
        return Err(invalid("Cut-face material must not be empty"));
    }
    let mut planned = Vec::new();
    let mut unchanged = Vec::new();
    for owner in ids.iter().copied().collect::<BTreeSet<_>>() {
        let solids: Vec<_> = match owner {
            ObjectId::Solid(id) => vec![tx.document().solid(id).ok_or(EditError::NotFound(owner))?],
            ObjectId::Entity(id) => {
                let entity = tx.document().entity(id).ok_or(EditError::NotFound(owner))?;
                if entity.solids.is_empty() {
                    return Err(invalid(format!(
                        "{owner} is a point entity; select brushes to clip"
                    )));
                }
                entity.solids.iter().collect()
            }
        };
        for solid in solids {
            let shape = geometry(solid)?;
            let mut front = false;
            let mut back = false;
            for point in shape.faces.iter().flat_map(|face| &face.vertices) {
                let distance = plane.distance_to(*point);
                front |= distance > ON_EPSILON;
                back |= distance < -ON_EPSILON;
            }
            if !front || !back {
                unchanged.push(solid.id);
                continue;
            }
            if solid.sides.iter().any(|side| side.displacement.is_some()) {
                return Err(invalid(format!(
                    "Brush {} has a displacement; remove it before clipping",
                    solid.id
                )));
            }
            let material = material.map(str::trim).unwrap_or(&solid.sides[0].material);
            let mut parts = Vec::new();
            if matches!(keep, Keep::Back | Keep::Both) {
                parts.push(half(solid, plane, material)?);
            }
            if matches!(keep, Keep::Front | Keep::Both) {
                parts.push(half(
                    solid,
                    Plane {
                        normal: -plane.normal,
                        dist: -plane.dist,
                    },
                    material,
                )?);
            }
            planned.push(PlannedCut {
                owner,
                source: solid.id,
                parts,
            });
        }
    }
    if planned.is_empty() {
        return Err(invalid(
            "Cut plane does not cross any selected brush; nothing changed",
        ));
    }
    let mut result = ClipResult {
        cuts: Vec::new(),
        objects: Vec::new(),
        unchanged,
    };
    for mut cut in planned {
        for (index, part) in cut.parts.iter_mut().enumerate() {
            if index == 0 {
                part.sides.last_mut().unwrap().id = tx.new_side_id();
            } else {
                part.id = tx.new_object_id();
                for side in &mut part.sides {
                    side.id = tx.new_side_id();
                }
            }
        }
        result.cuts.push(Cut {
            source: cut.source,
            solids: cut.parts.iter().map(|solid| solid.id).collect(),
        });
        match cut.owner {
            ObjectId::Solid(id) => {
                *tx.solid_mut(id)? = cut.parts.remove(0);
                result.objects.push(cut.owner);
                for part in cut.parts {
                    result.objects.push(tx.add_solid(part)?);
                }
            }
            ObjectId::Entity(id) => {
                let entity = tx.entity_mut(id)?;
                let index = entity
                    .solids
                    .iter()
                    .position(|solid| solid.id == cut.source)
                    .unwrap();
                entity.solids.splice(index..=index, cut.parts);
                result.objects.push(cut.owner);
            }
        }
    }
    result.objects.sort();
    result.objects.dedup();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, edit, texel_uv};
    use source_formats::keyvalues::Node;

    fn fixture() -> (Document, ObjectId) {
        let mut doc = Document::empty();
        let id = doc
            .edit("box", |tx| {
                edit::create_block(
                    tx,
                    DVec3::splat(-32.0),
                    DVec3::splat(32.0),
                    "brick/brickwall001a",
                )
            })
            .unwrap();
        (doc, id)
    }

    fn split(doc: &mut Document, id: ObjectId, keep: Keep, plane: Plane) -> ClipResult {
        doc.edit("clip", |tx| {
            clip(tx, &[id, id], plane, keep, Some("tools/toolsnodraw"))
        })
        .unwrap()
    }

    #[test]
    fn halves_preserve_uvs_metadata_ids_and_round_trip_with_undo() {
        for keep in [Keep::Front, Keep::Back, Keep::Both] {
            let (mut doc, id) = fixture();
            let ObjectId::Solid(number) = id else {
                unreachable!()
            };
            doc.edit("metadata", |tx| {
                let solid = tx.solid_mut(number)?;
                solid
                    .extra
                    .push(Node::block("custom", vec![Node::string("key", "value")]));
                solid.sides[0].u_axis.offset = 17.5;
                solid.sides[0].rotation = 22.5;
                Ok(())
            })
            .unwrap();
            let before = doc.vmf().clone();
            let original = doc.solid(number).unwrap().clone();
            let result = split(
                &mut doc,
                id,
                keep,
                Plane {
                    normal: DVec3::X,
                    dist: 0.0,
                },
            );
            assert_eq!(result.cuts.len(), 1);
            assert_eq!(result.objects.len(), if keep == Keep::Both { 2 } else { 1 });
            let mut sides = BTreeSet::new();
            for solid in &doc.vmf().world.solids {
                assert_eq!(solid.extra, original.extra);
                let shape = geometry(solid).unwrap();
                for face in shape.faces {
                    let side = &solid.sides[face.side];
                    assert!(sides.insert(side.id));
                    if let Some(source) = original
                        .sides
                        .iter()
                        .find(|source| source.plane == side.plane)
                    {
                        for point in face.vertices {
                            assert!(
                                texel_uv(source, point).abs_diff_eq(texel_uv(side, point), 1e-8)
                            );
                        }
                        if solid.id == number {
                            assert_eq!(side, source);
                        }
                    } else {
                        assert_eq!(side.material, "TOOLS/TOOLSNODRAW");
                    }
                }
                if keep == Keep::Front {
                    assert_eq!(shape.mins.x, 0.0);
                }
                if keep == Keep::Back {
                    assert_eq!(shape.maxs.x, 0.0);
                }
            }
            let after = doc.vmf().clone();
            assert_eq!(
                source_formats::vmf::Vmf::parse(&after.write(&Default::default())).unwrap(),
                after
            );
            assert_eq!(doc.undo().unwrap().label, "clip");
            assert_eq!(doc.vmf(), &before);
            doc.redo().unwrap();
            assert_eq!(doc.vmf(), &after);
        }
    }

    #[test]
    fn oblique_cuts_through_vertices_and_edges_produce_closed_pieces() {
        for normal in [
            DVec3::new(1.0, 1.0, 0.0),
            DVec3::new(1.0, 1.0, 1.0),
            DVec3::new(0.0, 1.0, 2.0),
        ] {
            let (mut doc, id) = fixture();
            split(&mut doc, id, Keep::Both, Plane { normal, dist: 0.0 });
            for solid in &doc.vmf().world.solids {
                let shape = geometry(solid).unwrap();
                for face in &shape.faces {
                    for pair in face
                        .vertices
                        .iter()
                        .zip(face.vertices.iter().cycle().skip(1))
                        .take(face.vertices.len())
                    {
                        let matching = shape
                            .faces
                            .iter()
                            .filter(|other| other.side != face.side)
                            .flat_map(|other| {
                                other
                                    .vertices
                                    .iter()
                                    .zip(other.vertices.iter().cycle().skip(1))
                                    .take(other.vertices.len())
                            })
                            .filter(|(a, b)| {
                                a.abs_diff_eq(*pair.1, ON_EPSILON)
                                    && b.abs_diff_eq(*pair.0, ON_EPSILON)
                            })
                            .count();
                        assert_eq!(matching, 1, "each edge has one reversed neighbor");
                    }
                }
            }
        }
    }

    #[test]
    fn brush_entities_keep_properties_membership_and_unaffected_brushes() {
        let (mut doc, id) = fixture();
        let far = doc
            .edit("far", |tx| {
                edit::create_block(
                    tx,
                    DVec3::splat(64.0),
                    DVec3::splat(96.0),
                    "brick/brickwall001a",
                )
            })
            .unwrap();
        let numbers: Vec<_> = [id, far]
            .into_iter()
            .map(|id| match id {
                ObjectId::Solid(number) => number,
                _ => unreachable!(),
            })
            .collect();
        let entity = doc
            .edit("tie", |tx| {
                edit::tie_to_entity(
                    tx,
                    &numbers,
                    "func_detail",
                    &[("targetname".into(), "detail".into())],
                )
            })
            .unwrap();
        let before = doc.vmf().clone();
        let result = split(
            &mut doc,
            entity,
            Keep::Both,
            Plane {
                normal: DVec3::X,
                dist: 0.0,
            },
        );
        let ObjectId::Entity(number) = entity else {
            unreachable!()
        };
        let brush_entity = doc.entity(number).unwrap();
        assert_eq!(brush_entity.properties, before.entities[0].properties);
        assert_eq!(brush_entity.solids.len(), 3);
        assert_eq!(brush_entity.solids[2], before.entities[0].solids[1]);
        assert_eq!(result.objects, vec![entity]);
        assert_eq!(result.unchanged, vec![before.entities[0].solids[1].id]);
        doc.undo();
        assert_eq!(doc.vmf(), &before);
    }

    #[test]
    fn scaled_planes_keep_their_distance_and_normalization_is_stable() {
        let plane = normalized_plane(Plane {
            normal: DVec3::new(3.0, 4.0, 5.0),
            dist: 7.0,
        })
        .unwrap();
        assert!((plane.normal.length() - 1.0).abs() < 1e-12);
        assert!((plane.dist - 7.0 / 50.0_f64.sqrt()).abs() < 1e-12);
        assert_eq!(normalized_plane(plane).unwrap(), plane);
        let restored = Plane::from_points(plane_points(plane)).unwrap();
        assert!(restored.normal.abs_diff_eq(plane.normal, 1e-12));
        assert!((restored.dist - plane.dist).abs() < 1e-12);
    }

    #[test]
    fn displacement_and_unbounded_batches_fail_before_mutation_or_id_allocation() {
        let (mut doc, first) = fixture();
        let second = doc
            .edit("second", |tx| {
                edit::create_block(tx, DVec3::splat(-16.0), DVec3::splat(16.0), "brick/test")
            })
            .unwrap();
        for displacement in [true, false] {
            let mut vmf = doc.vmf().clone();
            let solid = &mut vmf.world.solids[1];
            if displacement {
                solid.sides[0].displacement = Some(source_formats::vmf::DispInfo {
                    power: 2,
                    start_position: DVec3::splat(-16.0),
                    flags: None,
                    elevation: 0.0,
                    subdivided: false,
                    normals: None,
                    distances: None,
                    offsets: None,
                    offset_normals: None,
                    alphas: None,
                    triangle_tags: None,
                    allowed_verts: None,
                    extra_properties: Default::default(),
                    extra: vec![Node::string("custom", "preserved")],
                });
            } else {
                solid.sides.pop();
            }
            let mut candidate = Document::new(vmf.clone());
            let baseline = candidate.clone();
            let error = candidate
                .edit("clip", |tx| {
                    clip(
                        tx,
                        &[first, second],
                        Plane {
                            normal: DVec3::X,
                            dist: 0.0,
                        },
                        Keep::Both,
                        None,
                    )
                })
                .unwrap_err();
            assert!(
                error.to_string().contains(if displacement {
                    "displacement"
                } else {
                    "unbounded"
                }),
                "{error}"
            );
            assert_eq!(candidate.vmf(), &vmf);
            assert_eq!(candidate.revision(), 0);
            assert_eq!(candidate.undo_labels().count(), 0);
            let mut control = baseline;
            let next = candidate.edit("id", |tx| Ok(tx.new_side_id())).unwrap();
            let expected = control.edit("id", |tx| Ok(tx.new_side_id())).unwrap();
            assert_eq!(next, expected);
        }
    }

    #[test]
    fn invalid_planes_nonintersections_and_bad_batches_leave_document_unchanged() {
        let (mut doc, id) = fixture();
        let before = doc.vmf().clone();
        let revision = doc.revision();
        for plane in [
            Plane {
                normal: DVec3::ZERO,
                dist: 0.0,
            },
            Plane {
                normal: DVec3::X,
                dist: f64::NAN,
            },
            Plane {
                normal: DVec3::X,
                dist: 32.0,
            },
            Plane {
                normal: DVec3::X,
                dist: 100.0,
            },
        ] {
            assert!(
                doc.edit("clip", |tx| clip(tx, &[id], plane, Keep::Both, None))
                    .is_err()
            );
        }
        assert!(
            doc.edit("clip", |tx| clip(
                tx,
                &[id, ObjectId::Solid(999)],
                Plane {
                    normal: DVec3::X,
                    dist: 0.0
                },
                Keep::Both,
                None
            ))
            .is_err()
        );
        assert_eq!(doc.vmf(), &before);
        assert_eq!(doc.revision(), revision);
    }
}
