use std::collections::BTreeSet;

use source_formats::vmf::{DispInfo, Properties};

use crate::surface::FaceId;
use crate::{BrushGeometry, Document, EditError, ObjectId, Transaction};
use glam::DVec3;

struct Target {
    face: FaceId,
    side: usize,
    corners: [DVec3; 4],
    normal: DVec3,
}

fn targets(document: &Document, faces: &[FaceId]) -> Result<Vec<Target>, EditError> {
    let faces = faces.iter().copied().collect::<BTreeSet<_>>();
    if faces.is_empty() || faces.len() > 16 {
        return Err(EditError::Invalid(
            "Select 1–16 world quad faces for terrain".into(),
        ));
    }
    let mut solids = BTreeSet::new();
    let mut planned = Vec::with_capacity(faces.len());
    for target in faces {
        if !solids.insert(target.solid) {
            return Err(EditError::Invalid(
                "Create terrain on one face per parent brush".into(),
            ));
        }
        if document.owner_of_solid(target.solid) != Some(ObjectId::Solid(target.solid)) {
            return Err(EditError::Invalid(
                "Terrain creation requires world brushes".into(),
            ));
        }
        let solid = document
            .solid(target.solid)
            .ok_or_else(|| EditError::Invalid("Terrain parent brush is missing".into()))?;
        if solid.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(EditError::Invalid(
                "This brush already has terrain; existing terrain is preserved".into(),
            ));
        }
        let matches = solid
            .sides
            .iter()
            .enumerate()
            .filter(|(_, side)| side.id == target.side)
            .collect::<Vec<_>>();
        let [(side_index, _)] = matches.as_slice() else {
            return Err(EditError::Invalid(
                "Terrain face is missing or ambiguous".into(),
            ));
        };
        let geometry = BrushGeometry::from_solid(solid)
            .map_err(|error| EditError::Invalid(error.to_string()))?;
        let face = geometry
            .faces
            .iter()
            .find(|face| face.side == *side_index)
            .ok_or_else(|| EditError::Invalid("Terrain needs an effective quad face".into()))?;
        if face.vertices.len() != 4 {
            return Err(EditError::Invalid(
                "Terrain creation requires four-corner faces".into(),
            ));
        }
        planned.push(Target {
            face: target,
            side: *side_index,
            corners: face.vertices.as_slice().try_into().unwrap(),
            normal: face.plane.normal,
        });
    }
    Ok(planned)
}

pub struct CreationFace {
    pub face: FaceId,
    pub corners: [DVec3; 4],
}

pub fn creation_faces(
    document: &Document,
    faces: &[FaceId],
) -> Result<Vec<CreationFace>, EditError> {
    Ok(targets(document, faces)?
        .into_iter()
        .map(|target| CreationFace {
            face: target.face,
            corners: target.corners,
        })
        .collect())
}

pub fn validate(document: &Document, faces: &[FaceId]) -> Result<(), EditError> {
    targets(document, faces).map(|_| ())
}

pub fn create(
    tx: &mut Transaction<'_>,
    faces: &[FaceId],
    power: u32,
    start_corner: u32,
) -> Result<usize, EditError> {
    if !(2..=4).contains(&power) {
        return Err(EditError::Invalid("Terrain power must be 2, 3 or 4".into()));
    }
    if start_corner > 3 {
        return Err(EditError::Invalid(
            "Terrain start corner must be 0, 1, 2 or 3".into(),
        ));
    }
    let planned = targets(tx.document(), faces)?;
    let count = planned.len();
    let size = (1 << power) + 1;
    for target in planned {
        let info = DispInfo {
            power,
            start_position: target.corners[start_corner as usize],
            flags: Some(0),
            elevation: 0.0,
            subdivided: false,
            normals: Some(vec![vec![target.normal; size]; size]),
            distances: Some(vec![vec![0.0; size]; size]),
            offsets: None,
            offset_normals: None,
            alphas: Some(vec![vec![0.0; size]; size]),
            triangle_tags: None,
            allowed_verts: None,
            extra_properties: Properties::default(),
            extra: Vec::new(),
        };
        tx.solid_mut(target.face.solid)?.sides[target.side].displacement = Some(info);
    }
    Ok(count)
}

fn removal_targets(
    document: &Document,
    faces: &[FaceId],
) -> Result<Vec<(FaceId, usize)>, EditError> {
    let faces = faces.iter().copied().collect::<BTreeSet<_>>();
    if faces.is_empty() || faces.len() > 16 {
        return Err(EditError::Invalid(
            "Select 1–16 terrain faces to remove".into(),
        ));
    }
    let mut planned = Vec::with_capacity(faces.len());
    for target in faces {
        let solid = crate::surface::solid(document, target.solid)
            .ok_or_else(|| EditError::Invalid("Terrain parent brush is missing".into()))?;
        let matches = solid
            .sides
            .iter()
            .enumerate()
            .filter(|(_, side)| side.id == target.side)
            .collect::<Vec<_>>();
        let [(index, side)] = matches.as_slice() else {
            return Err(EditError::Invalid(
                "Terrain face is missing or ambiguous".into(),
            ));
        };
        if side.displacement.is_none() {
            return Err(EditError::Invalid(
                "Every selected face must have terrain".into(),
            ));
        }
        planned.push((target, *index));
    }
    Ok(planned)
}

pub struct TerrainGrid {
    pub face: FaceId,
    pub power: u32,
    pub subdivided: bool,
    pub flags: u32,
    pub collision: super::collision::State,
    pub smoothing_angle: Option<f64>,
    pub smoothing_error: Option<String>,
    pub mesh: super::DisplacementMesh,
}

pub fn inspect(document: &Document, faces: &[FaceId]) -> Result<Vec<TerrainGrid>, EditError> {
    removal_targets(document, faces)?
        .into_iter()
        .map(|(target, index)| {
            let solid = crate::surface::solid(document, target.solid).unwrap();
            let geometry = BrushGeometry::from_solid(solid)
                .map_err(|error| EditError::Invalid(error.to_string()))?;
            let face = geometry
                .faces
                .iter()
                .find(|face| face.side == index)
                .ok_or_else(|| EditError::Invalid("Terrain needs an effective quad face".into()))?;
            let info = solid.sides[index].displacement.as_ref().unwrap();
            let mesh = super::DisplacementMesh::build(face, info)
                .map_err(|error| EditError::Invalid(error.to_string()))?;
            let (smoothing_angle, smoothing_error) = match super::smoothing::angle(info) {
                Ok(angle) => (angle, None),
                Err(error) => (None, Some(error)),
            };
            Ok(TerrainGrid {
                face: target,
                power: info.power,
                subdivided: info.subdivided,
                flags: info.flags.unwrap_or(0),
                collision: super::collision::state(info.flags.unwrap_or(0)),
                smoothing_angle,
                smoothing_error,
                mesh,
            })
        })
        .collect()
}

pub fn validate_removal(document: &Document, faces: &[FaceId]) -> Result<(), EditError> {
    removal_targets(document, faces).map(|_| ())
}

pub fn remove(tx: &mut Transaction<'_>, faces: &[FaceId]) -> Result<usize, EditError> {
    let planned = removal_targets(tx.document(), faces)?;
    let count = planned.len();
    for (target, index) in planned {
        let owner = tx
            .document()
            .owner_of_solid(target.solid)
            .ok_or_else(|| EditError::Invalid("Terrain parent brush is missing".into()))?;
        let solid = match owner {
            ObjectId::Solid(id) => tx.solid_mut(id)?,
            ObjectId::Entity(id) => tx
                .entity_mut(id)?
                .solids
                .iter_mut()
                .find(|solid| solid.id == target.solid)
                .ok_or_else(|| EditError::Invalid("Terrain parent brush is missing".into()))?,
        };
        solid.sides[index].displacement = None;
    }
    Ok(count)
}
