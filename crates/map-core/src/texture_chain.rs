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
