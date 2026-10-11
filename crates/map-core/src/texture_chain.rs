use std::collections::{BTreeMap, BTreeSet, VecDeque};

use glam::{DVec2, DVec3};

use crate::surface::{self, FaceId, PaintMode, SurfaceEdit, SurfaceSample};
use crate::{Document, EditError};

pub const MAX_FACES: usize = 256;
const SEAM_EPSILON: f64 = 0.001;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub parent: FaceId,
    pub face: FaceId,
}

#[derive(Debug)]
pub struct Chain {
    pub edits: Vec<(FaceId, SurfaceEdit)>,
    pub links: Vec<Link>,
    pub source_in_targets: bool,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn uv(sample: &SurfaceSample, point: DVec3) -> DVec2 {
    DVec2::new(
        point.dot(sample.projection.u.axis) / sample.projection.u.scale
            + sample.projection.u.offset,
        point.dot(sample.projection.v.axis) / sample.projection.v.scale
            + sample.projection.v.offset,
    )
}

pub fn wrap(
    document: &Document,
    faces: &[FaceId],
    source: &SurfaceSample,
) -> Result<Chain, EditError> {
    let mut targets = faces.iter().copied().collect::<BTreeSet<_>>();
    let source_in_targets = targets.remove(&source.face);
    if targets.is_empty() {
        return Err(invalid(
            "Select connected target faces besides the sampled source",
        ));
    }
    if targets.len() > MAX_FACES {
        return Err(invalid(format!(
            "Connected wrapping supports at most {MAX_FACES} target faces"
        )));
    }
    if source.displaced {
        return Err(invalid(
            "Connected wrapping does not support displacement faces",
        ));
    }
    let mut pending = BTreeMap::new();
    for face in targets {
        let target = surface::sample(document, face)?;
        if target.displaced {
            return Err(invalid(
                "Connected wrapping does not support displacement faces",
            ));
        }
        pending.insert(face, target);
    }
    let mut assigned = BTreeMap::from([(source.face, source.clone())]);
    let mut queue = VecDeque::from([source.face]);
    let mut edits = Vec::new();
    let mut links = Vec::new();
    while let Some(parent_id) = queue.pop_front() {
        let parent = assigned[&parent_id].clone();
        let neighbors = pending
            .iter()
            .filter_map(|(face, target)| {
                surface::shared_edge(&parent.vertices, &target.vertices).map(|_| *face)
            })
            .collect::<Vec<_>>();
        for face in neighbors {
            let edit = surface::paint_edit(document, face, &parent, PaintMode::Wrap)?;
            let mut target = pending.remove(&face).unwrap();
            target.material = parent.material.clone();
            target.projection = edit.projection.as_ref().unwrap().clone();
            assigned.insert(face, target);
            edits.push((face, edit));
            links.push(Link {
                parent: parent_id,
                face,
            });
            queue.push_back(face);
        }
    }
    if !pending.is_empty() {
        let first = pending.keys().next().unwrap();
        return Err(invalid(format!(
            "{} target faces are disconnected from the sample through the selected faces (first: brush {}, face {}). Select the intermediate faces.",
            pending.len(),
            first.solid,
            first.side
        )));
    }
    let samples = assigned.values().collect::<Vec<_>>();
    for (index, first) in samples.iter().enumerate() {
        for second in &samples[index + 1..] {
            let Some((anchor, direction)) = surface::shared_edge(&first.vertices, &second.vertices)
            else {
                continue;
            };
            for point in [anchor, anchor + direction] {
                let a = uv(first, point);
                let b = uv(second, point);
                if !a.is_finite() || !b.is_finite() {
                    return Err(invalid(
                        "Sampled texture projection has invalid scales or coordinates",
                    ));
                }
                if (a - b).abs().max_element() > SEAM_EPSILON {
                    return Err(invalid(format!(
                        "Connected faces form an incompatible texture loop between brush {} face {} and brush {} face {}. Leave a seam by excluding a face, then wrap the remaining strip.",
                        first.face.solid, first.face.side, second.face.solid, second.face.side
                    )));
                }
            }
        }
    }
    Ok(Chain {
        edits,
        links,
        source_in_targets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ObjectId, Plane, edit, texel_uv};

    fn block(document: &mut Document, mins: DVec3) -> u32 {
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(tx, mins, mins + DVec3::splat(64.0), "brick/test")
            })
            .unwrap()
        else {
            unreachable!()
        };
        id
    }

    fn face(document: &Document, solid: u32, normal: DVec3) -> FaceId {
        FaceId {
            solid,
            side: document
                .solid(solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| {
                    Plane::from_points(side.plane.0)
                        .unwrap()
                        .normal
                        .abs_diff_eq(normal, 1e-8)
                })
                .unwrap()
                .id,
        }
    }

    fn apply(document: &mut Document, chain: &Chain) {
        document
            .edit("wrap connected faces", |tx| {
                chain.edits.iter().try_for_each(|(face, patch)| {
                    surface::edit_faces(tx, &[*face], patch).map(|_| ())
                })
            })
            .unwrap();
    }

    #[test]
    fn folds_through_intermediate_faces_and_preserves_every_shared_seam() {
        let mut document = Document::empty();
        let solid = block(&mut document, DVec3::ZERO);
        let source = face(&document, solid, DVec3::Z);
        let front = face(&document, solid, DVec3::NEG_Y);
        let bottom = face(&document, solid, DVec3::NEG_Z);
        document
            .edit("sample mapping", |tx| {
                surface::edit_faces(
                    tx,
                    &[source],
                    &SurfaceEdit {
                        shift_u: Some(13.5),
                        shift_v: Some(-9.0),
                        rotation: Some(23.0),
                        scale_u: Some(-0.5),
                        scale_v: Some(0.25),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let before = document.vmf().clone();
        let sample = surface::sample(&document, source).unwrap();
        let chain = wrap(&document, &[bottom, source, front, front], &sample).unwrap();
        assert!(chain.source_in_targets);
        assert_eq!(
            chain.links,
            vec![
                Link {
                    parent: source,
                    face: front
                },
                Link {
                    parent: front,
                    face: bottom
                }
            ]
        );
        assert_eq!(document.vmf(), &before);
        assert_eq!(
            wrap(&document, &[front, bottom], &sample).unwrap().links,
            chain.links
        );
        let history = document.undo_labels().count();
        apply(&mut document, &chain);
        assert_eq!(document.undo_labels().count(), history + 1);
        for link in &chain.links {
            let parent = surface::sample(&document, link.parent).unwrap();
            let child = surface::sample(&document, link.face).unwrap();
            let (anchor, direction) =
                surface::shared_edge(&parent.vertices, &child.vertices).unwrap();
            for point in [anchor - direction * 20.0, anchor, anchor + direction * 20.0] {
                assert!(
                    texel_uv(surface::side(&document, link.parent).unwrap(), point).abs_diff_eq(
                        texel_uv(surface::side(&document, link.face).unwrap(), point),
                        1e-7
                    )
                );
            }
        }
        document.undo().unwrap();
        assert_eq!(document.vmf(), &before);
    }

    #[test]
    fn accepts_consistent_planar_cycles_and_rejects_disconnected_or_closed_box_loops() {
        let mut document = Document::empty();
        let mut faces = Vec::new();
        for (x, y) in [(0.0, 0.0), (64.0, 0.0), (0.0, 64.0), (64.0, 64.0)] {
            let id = block(&mut document, DVec3::new(x, y, 0.0));
            faces.push(face(&document, id, DVec3::Z));
        }
        let sample = surface::sample(&document, faces[0]).unwrap();
        assert_eq!(wrap(&document, &faces, &sample).unwrap().links.len(), 3);
        let first = faces[0].solid;
        let closed = document
            .solid(first)
            .unwrap()
            .sides
            .iter()
            .map(|side| FaceId {
                solid: first,
                side: side.id,
            })
            .collect::<Vec<_>>();
        assert!(
            wrap(&document, &closed, &sample)
                .unwrap_err()
                .to_string()
                .contains("loop")
        );
        let distant = block(&mut document, DVec3::splat(256.0));
        assert!(
            wrap(&document, &[face(&document, distant, DVec3::Z)], &sample)
                .unwrap_err()
                .to_string()
                .contains("disconnected")
        );
        let mut displaced = sample.clone();
        displaced.displaced = true;
        assert!(wrap(&document, &faces, &displaced).is_err());
        let too_many = (0..MAX_FACES + 1)
            .map(|side| FaceId {
                solid: distant,
                side: 1000 + side as u32,
            })
            .collect::<Vec<_>>();
        assert!(
            wrap(&document, &too_many, &sample)
                .unwrap_err()
                .to_string()
                .contains("at most")
        );
    }
}
