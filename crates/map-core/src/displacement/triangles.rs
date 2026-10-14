use std::collections::BTreeSet;

use glam::DVec3;
use source_formats::{bsp::DisplacementInfo, vmf::DispInfo};

use super::DisplacementMesh;
use crate::EditError;

pub const WALKABLE: u16 = 1;
pub const FORCE_WALKABLE: u16 = 2;
pub const WALKABLE_VALUE: u16 = 4;
pub const BUILDABLE: u16 = 8;
pub const FORCE_BUILDABLE: u16 = 16;
pub const BUILDABLE_VALUE: u16 = 32;
pub const REMOVE: u16 = 64;
pub const KNOWN_TAGS: u16 = 127;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Automatic,
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Settings {
    pub walkable: Option<Choice>,
    pub buildable: Option<Choice>,
}

#[derive(Debug, Clone)]
pub struct Triangle {
    pub id: usize,
    pub indices: [usize; 3],
    pub positions: [DVec3; 3],
    pub centroid: DVec3,
    pub source_tags: Option<u16>,
    pub walkable: bool,
    pub buildable: bool,
    pub walkable_override: Option<bool>,
    pub buildable_override: Option<bool>,
    pub removed: bool,
    pub unknown_tags: u16,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

pub fn indices(power: u32) -> Result<Vec<[usize; 3]>, EditError> {
    let power = i32::try_from(power).map_err(|_| invalid("Unsupported terrain power"))?;
    DisplacementInfo {
        power,
        ..Default::default()
    }
    .triangle_indices()
    .map_err(|error| invalid(error.to_string()))
}

pub fn source_tags(info: &DispInfo) -> Result<Option<Vec<u16>>, EditError> {
    if !(2..=4).contains(&info.power) {
        return Err(invalid("Terrain power must be 2, 3 or 4"));
    }
    let Some(rows) = &info.triangle_tags else {
        return Ok(None);
    };
    let cells = 1 << info.power;
    if rows.len() != cells || rows.iter().any(|row| row.len() != cells * 2) {
        return Err(invalid("Terrain triangle tag rows do not match the grid"));
    }
    rows.iter()
        .flatten()
        .map(|&tag| {
            u16::try_from(tag)
                .map_err(|_| invalid("Terrain triangle tags must fit unsigned shorts"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn automatic(mesh: &DisplacementMesh, [a, b, c]: [usize; 3]) -> bool {
    (mesh.positions[c] - mesh.positions[a])
        .cross(mesh.positions[b] - mesh.positions[a])
        .normalize_or_zero()
        .z
        >= 0.7
}

fn override_value(tag: u16, force: u16, value: u16) -> Option<bool> {
    (tag & force != 0).then_some(tag & value != 0)
}

pub fn inspect(info: &DispInfo, mesh: &DisplacementMesh) -> Result<Vec<Triangle>, EditError> {
    let tags = source_tags(info)?;
    Ok(indices(info.power)?
        .into_iter()
        .enumerate()
        .map(|(id, indices)| {
            let source_tags = tags.as_ref().map(|tags| tags[id]);
            let tag = source_tags.unwrap_or_else(|| {
                if automatic(mesh, indices) {
                    WALKABLE | BUILDABLE
                } else {
                    0
                }
            });
            let walkable_override = override_value(tag, FORCE_WALKABLE, WALKABLE_VALUE);
            let buildable_override = override_value(tag, FORCE_BUILDABLE, BUILDABLE_VALUE);
            let positions = indices.map(|index| mesh.positions[index]);
            Triangle {
                id,
                indices,
                positions,
                centroid: positions.into_iter().sum::<DVec3>() / 3.0,
                source_tags,
                walkable: walkable_override.unwrap_or(tag & WALKABLE != 0),
                buildable: buildable_override.unwrap_or(tag & BUILDABLE != 0),
                walkable_override,
                buildable_override,
                removed: tag & REMOVE != 0,
                unknown_tags: tag & !KNOWN_TAGS,
            }
        })
        .collect())
}

pub fn native_tags(info: &DispInfo, mesh: &DisplacementMesh) -> Result<Vec<u16>, EditError> {
    inspect(info, mesh)?
        .into_iter()
        .map(|triangle| {
            if triangle.removed {
                return Err(invalid("Terrain triangle removal is not supported yet"));
            }
            if triangle.unknown_tags != 0 {
                return Err(invalid("Terrain triangle tags contain unsupported bits"));
            }
            Ok(1 | if triangle.walkable { 2 } else { 0 } | if triangle.buildable { 4 } else { 0 })
        })
        .collect()
}

pub fn edit(
    info: &DispInfo,
    mesh: &DisplacementMesh,
    selected: &[usize],
    settings: Settings,
) -> Result<DispInfo, EditError> {
    let indices = indices(info.power)?;
    let selected = selected.iter().copied().collect::<BTreeSet<_>>();
    if selected.is_empty() || selected.iter().any(|&id| id >= indices.len()) {
        return Err(invalid("Select valid native terrain triangle IDs"));
    }
    if settings.walkable.is_none() && settings.buildable.is_none() {
        return Err(invalid("Choose walkability or buildability to change"));
    }
    let mut tags = source_tags(info)?.unwrap_or_else(|| {
        indices
            .iter()
            .map(|&indices| {
                if automatic(mesh, indices) {
                    WALKABLE | BUILDABLE
                } else {
                    0
                }
            })
            .collect()
    });
    let original = tags.clone();
    for id in selected {
        for (choice, cached, force, value) in [
            (settings.walkable, WALKABLE, FORCE_WALKABLE, WALKABLE_VALUE),
            (
                settings.buildable,
                BUILDABLE,
                FORCE_BUILDABLE,
                BUILDABLE_VALUE,
            ),
        ] {
            if let Some(choice) = choice {
                tags[id] &= !(force | value);
                match choice {
                    Choice::Automatic => {
                        tags[id] &= !cached;
                        if automatic(mesh, indices[id]) {
                            tags[id] |= cached;
                        }
                    }
                    Choice::Enabled => tags[id] |= force | value,
                    Choice::Disabled => tags[id] |= force,
                }
            }
        }
    }
    let cells = mesh.size - 1;
    let mut result = info.clone();
    let rows = tags
        .chunks(cells * 2)
        .map(|row| row.iter().map(|&tag| i32::from(tag)).collect())
        .collect();
    if info.triangle_tags.is_some() || tags != original {
        result.triangle_tags = Some(rows);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, ObjectId, edit, surface::FaceId};

    fn fixture() -> (DispInfo, DisplacementMesh) {
        let mut document = Document::empty();
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "brick/test")
            })
            .unwrap()
        else {
            unreachable!()
        };
        let side = document
            .solid(id)
            .unwrap()
            .sides
            .iter()
            .find(|side| crate::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        document
            .edit("terrain", |tx| {
                super::super::edit::create(tx, &[FaceId { solid: id, side }], 2, 0)
            })
            .unwrap();
        let solid = document.solid(id).unwrap();
        let face = crate::BrushGeometry::from_solid(solid)
            .unwrap()
            .faces
            .into_iter()
            .find(|face| solid.sides[face.side].id == side)
            .unwrap();
        let info = solid.sides[face.side].displacement.clone().unwrap();
        let mesh = DisplacementMesh::build(&face, &info).unwrap();
        (info, mesh)
    }

    #[test]
    fn source_override_bits_collapse_to_native_classification_without_false_removal() {
        let (mut info, mesh) = fixture();
        for (source, expected) in [
            (0, 1),
            (1, 3),
            (8, 5),
            (9, 7),
            (7, 3),
            (3, 1),
            (48, 5),
            (24, 1),
            (32, 1),
        ] {
            info.triangle_tags = Some(vec![vec![source; 8]; 4]);
            assert_eq!(
                native_tags(&info, &mesh).unwrap(),
                vec![expected; 32],
                "source {source}"
            );
        }
        info.triangle_tags = Some(vec![vec![64; 8]; 4]);
        assert!(
            inspect(&info, &mesh)
                .unwrap()
                .iter()
                .all(|triangle| triangle.removed)
        );
        assert!(native_tags(&info, &mesh).is_err());
        info.triangle_tags = Some(vec![vec![128; 8]; 4]);
        assert_eq!(inspect(&info, &mesh).unwrap()[0].unknown_tags, 128);
        assert!(native_tags(&info, &mesh).is_err());
    }

    #[test]
    fn subset_edits_preserve_geometry_other_tags_and_exact_automatic_no_ops() {
        let (mut info, mesh) = fixture();
        assert_eq!(
            edit(
                &info,
                &mesh,
                &[0, 31],
                Settings {
                    walkable: Some(Choice::Automatic),
                    buildable: None
                }
            )
            .unwrap(),
            info
        );
        info.triangle_tags = Some(vec![vec![i32::from(128 | REMOVE | BUILDABLE); 8]; 4]);
        let changed = edit(
            &info,
            &mesh,
            &[0, 0, 31],
            Settings {
                walkable: Some(Choice::Disabled),
                buildable: None,
            },
        )
        .unwrap();
        let triangles = inspect(&changed, &mesh).unwrap();
        assert_eq!(triangles[0].indices, [0, 5, 6]);
        assert_eq!(triangles[0].walkable_override, Some(false));
        assert!(triangles[0].removed && triangles[0].buildable);
        assert_eq!(triangles[0].unknown_tags, 128);
        assert_eq!(triangles[1].source_tags, Some(128 | REMOVE | BUILDABLE));
        let mut restored = changed.clone();
        restored.triangle_tags = info.triangle_tags.clone();
        assert_eq!(restored, info);
        assert!(
            edit(
                &info,
                &mesh,
                &[32],
                Settings {
                    walkable: Some(Choice::Enabled),
                    buildable: None
                }
            )
            .is_err()
        );
        info.triangle_tags.as_mut().unwrap()[0].pop();
        assert!(source_tags(&info).is_err());
    }

    #[test]
    fn native_triangle_rays_match_centers_at_all_powers_and_reject_backfaces() {
        for power in 2..=4 {
            let size = (1usize << power) + 1;
            for rotation in 0..4 {
                let positions = (0..size)
                    .flat_map(|row| {
                        (0..size).map(move |col| {
                            let point =
                                DVec3::new(col as f64, row as f64, ((row * col) % 3) as f64 * 0.2);
                            match rotation {
                                0 => point,
                                1 => DVec3::new(-point.y, point.x, point.z),
                                2 => DVec3::new(-point.x, -point.y, point.z),
                                _ => DVec3::new(point.y, -point.x, point.z),
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                let mesh = DisplacementMesh {
                    size,
                    base_positions: positions.clone(),
                    alphas: vec![0.0; positions.len()],
                    positions,
                };
                for (id, indices) in indices(power).unwrap().into_iter().enumerate() {
                    let center = indices
                        .map(|index| mesh.positions[index])
                        .into_iter()
                        .sum::<DVec3>()
                        / 3.0;
                    let ray = crate::query::Ray {
                        origin: center + DVec3::Z * 64.0,
                        direction: DVec3::NEG_Z,
                    };
                    let hit = crate::query::pick_displacement_triangle(&ray, &mesh).unwrap();
                    assert_eq!(hit.0, id);
                    assert!((hit.1 - 64.0).abs() < 1e-9);
                }
                assert!(
                    crate::query::pick_displacement_triangle(
                        &crate::query::Ray {
                            origin: mesh.positions[0] - DVec3::Z * 64.0,
                            direction: DVec3::Z
                        },
                        &mesh
                    )
                    .is_none()
                );
            }
        }
    }
}
