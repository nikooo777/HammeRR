use glam::DVec3;
use source_formats::vmf::Solid;

use crate::surface::FaceId;
use crate::{BrushGeometry, Document, ObjectId, Plane, query};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    Contained,
    Intersecting,
}

pub struct Region {
    pub planes: Vec<Plane>,
    pub seed: DVec3,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Selection {
    pub objects: Vec<ObjectId>,
    pub faces: Vec<FaceId>,
}

impl Region {
    pub fn from_bounds(mins: DVec3, maxs: DVec3) -> Result<Self, &'static str> {
        if !mins.is_finite()
            || !maxs.is_finite()
            || !mins.cmplt(maxs).all()
            || !(maxs - mins).is_finite()
        {
            return Err("Volume needs finite corners and positive width, height and depth");
        }
        let mut planes = Vec::with_capacity(6);
        for axis in 0..3 {
            let mut normal = DVec3::ZERO;
            normal[axis] = 1.0;
            planes.push(Plane {
                normal,
                dist: maxs[axis],
            });
            planes.push(Plane {
                normal: -normal,
                dist: -mins[axis],
            });
        }
        Ok(Self {
            planes,
            seed: mins + (maxs - mins) * 0.5,
        })
    }

    fn contains(&self, point: DVec3) -> bool {
        self.planes
            .iter()
            .all(|plane| plane.distance_to(point) <= crate::plane::ON_EPSILON)
    }

    pub fn polygon_matches(&self, vertices: &[DVec3], rule: Match) -> bool {
        if rule == Match::Contained {
            return !vertices.is_empty() && vertices.iter().all(|point| self.contains(*point));
        }
        let mut winding = vertices.to_vec();
        for plane in &self.planes {
            winding = plane.clip_behind(&winding);
            if winding.is_empty() {
                return false;
            }
        }
        !winding.is_empty()
    }

    fn brush_matches(&self, solid: &Solid, shape: &BrushGeometry, rule: Match) -> bool {
        if rule == Match::Contained {
            return shape
                .faces
                .iter()
                .all(|face| self.polygon_matches(&face.vertices, rule));
        }
        shape
            .faces
            .iter()
            .any(|face| self.polygon_matches(&face.vertices, rule))
            || solid.sides.iter().all(|side| {
                Plane::from_points(side.plane.0)
                    .is_some_and(|plane| plane.distance_to(self.seed) <= crate::plane::ON_EPSILON)
            })
    }

    fn box_matches(&self, mins: DVec3, maxs: DVec3, rule: Match) -> bool {
        let corners: Vec<_> = (0..8)
            .map(|index| {
                DVec3::new(
                    if index & 1 == 0 { mins.x } else { maxs.x },
                    if index & 2 == 0 { mins.y } else { maxs.y },
                    if index & 4 == 0 { mins.z } else { maxs.z },
                )
            })
            .collect();
        if rule == Match::Contained {
            return corners.iter().all(|point| self.contains(*point));
        }
        if self.seed.cmpge(mins).all() && self.seed.cmple(maxs).all() {
            return true;
        }
        [
            [0, 1, 3, 2],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 3, 7, 6],
            [0, 2, 6, 4],
            [1, 3, 7, 5],
        ]
        .iter()
        .any(|face| self.polygon_matches(&face.map(|index| corners[index]), rule))
    }

    pub fn select(
        &self,
        document: &Document,
        rule: Match,
        faces: bool,
        include: impl Fn(&Solid) -> bool,
    ) -> Selection {
        let mut result = Selection::default();
        for id in document.objects() {
            let solids: Vec<_> = match id {
                ObjectId::Solid(id) => document.solid(id).into_iter().collect(),
                ObjectId::Entity(id) => {
                    let entity = document.entity(id).unwrap();
                    if entity.hidden {
                        continue;
                    }
                    if entity.solids.is_empty() {
                        if !faces
                            && query::entity_bounds(entity)
                                .is_some_and(|(mins, maxs)| self.box_matches(mins, maxs, rule))
                        {
                            result.objects.push(ObjectId::Entity(id));
                        }
                        continue;
                    }
                    entity.solids.iter().collect()
                }
            };
            let mut matches = Vec::new();
            for solid in solids {
                if solid.hidden || !include(solid) {
                    continue;
                }
                let Ok(shape) = BrushGeometry::from_solid(solid) else {
                    continue;
                };
                if solid.sides.iter().any(|side| side.displacement.is_some()) {
                    let surfaces = shape
                        .faces
                        .iter()
                        .filter_map(|face| {
                            let info = solid.sides[face.side].displacement.as_ref()?;
                            let mesh = crate::DisplacementMesh::build(face, info).ok()?;
                            mesh.positions
                                .iter()
                                .all(|point| point.is_finite())
                                .then_some((face.side, mesh))
                        })
                        .collect::<Vec<_>>();
                    let surface_matches = |mesh: &crate::DisplacementMesh| {
                        if rule == Match::Contained {
                            mesh.positions.iter().all(|point| self.contains(*point))
                        } else {
                            mesh.triangles().any(|indices| {
                                self.polygon_matches(
                                    &indices.map(|index| mesh.positions[index]),
                                    rule,
                                )
                            })
                        }
                    };
                    if faces {
                        result.faces.extend(
                            surfaces
                                .iter()
                                .filter(|(_, mesh)| surface_matches(mesh))
                                .map(|(side, _)| FaceId {
                                    solid: solid.id,
                                    side: solid.sides[*side].id,
                                }),
                        );
                    } else {
                        matches.push(
                            !surfaces.is_empty()
                                && if rule == Match::Contained {
                                    surfaces.len()
                                        == solid
                                            .sides
                                            .iter()
                                            .filter(|side| side.displacement.is_some())
                                            .count()
                                        && surfaces.iter().all(|(_, mesh)| surface_matches(mesh))
                                } else {
                                    surfaces.iter().any(|(_, mesh)| surface_matches(mesh))
                                },
                        );
                    }
                    continue;
                }
                if faces {
                    for face in &shape.faces {
                        if self.polygon_matches(&face.vertices, rule) {
                            result.faces.push(FaceId {
                                solid: solid.id,
                                side: solid.sides[face.side].id,
                            });
                        }
                    }
                } else {
                    matches.push(self.brush_matches(solid, &shape, rule));
                }
            }
            if !matches.is_empty()
                && if rule == Match::Contained {
                    matches.iter().all(|matched| *matched)
                } else {
                    matches.iter().any(|matched| *matched)
                }
            {
                result.objects.push(id);
            }
        }
        result.objects.sort();
        result.faces.sort();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit;

    fn region(mins: DVec3, maxs: DVec3) -> Region {
        Region::from_bounds(mins, maxs).unwrap()
    }

    #[test]
    fn invalid_volume_bounds_are_rejected() {
        for (mins, maxs) in [
            (DVec3::ZERO, DVec3::ZERO),
            (DVec3::ONE, DVec3::ZERO),
            (DVec3::splat(f64::NAN), DVec3::ONE),
            (DVec3::ZERO, DVec3::splat(f64::INFINITY)),
            (DVec3::splat(-f64::MAX), DVec3::splat(f64::MAX)),
        ] {
            assert!(Region::from_bounds(mins, maxs).is_err());
        }
    }

    #[test]
    fn intersections_use_geometry_and_include_a_region_inside_a_brush() {
        let mut document = Document::empty();
        let id = document
            .edit("box", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(-32.0, -32.0, -16.0),
                    DVec3::new(32.0, 32.0, 16.0),
                    "test",
                )
            })
            .unwrap();
        document
            .edit("rotate", |tx| {
                edit::transform(
                    tx,
                    &[id],
                    edit::Transform {
                        rotation: DVec3::new(0.0, 0.0, 45.0),
                        ..edit::Transform::default()
                    },
                )
            })
            .unwrap();
        let outside = region(DVec3::new(35.0, 35.0, -8.0), DVec3::new(40.0, 40.0, 8.0));
        assert!(
            outside
                .select(&document, Match::Intersecting, false, |_| true)
                .objects
                .is_empty()
        );
        let inside = region(DVec3::splat(-1.0), DVec3::splat(1.0));
        assert_eq!(
            inside
                .select(&document, Match::Intersecting, false, |_| true)
                .objects,
            vec![id]
        );
        assert!(
            inside
                .select(&document, Match::Contained, false, |_| true)
                .objects
                .is_empty()
        );
        let all = region(DVec3::splat(-64.0), DVec3::splat(64.0));
        assert_eq!(
            all.select(&document, Match::Contained, false, |_| true)
                .objects,
            vec![id]
        );
    }

    #[test]
    fn brush_entity_containment_hidden_members_and_faces_follow_the_rule() {
        let mut document = Document::empty();
        let (a, b) = document
            .edit("boxes", |tx| {
                let a = edit::create_block(tx, DVec3::ZERO, DVec3::splat(16.0), "test")?;
                let b = edit::create_block(tx, DVec3::splat(64.0), DVec3::splat(80.0), "test")?;
                Ok((a, b))
            })
            .unwrap();
        let ids = [a, b].map(|id| match id {
            ObjectId::Solid(id) => id,
            _ => unreachable!(),
        });
        let entity = document
            .edit("tie", |tx| {
                edit::tie_to_entity(tx, &ids, "func_detail", &[])
            })
            .unwrap();
        let area = region(DVec3::splat(-1.0), DVec3::splat(17.0));
        assert_eq!(
            area.select(&document, Match::Intersecting, false, |_| true)
                .objects,
            vec![entity]
        );
        assert!(
            area.select(&document, Match::Contained, false, |_| true)
                .objects
                .is_empty()
        );
        assert_eq!(
            area.select(&document, Match::Contained, true, |_| true)
                .faces
                .len(),
            6
        );
        assert!(
            area.select(&document, Match::Intersecting, false, |_| false)
                .objects
                .is_empty()
        );
        let ObjectId::Entity(number) = entity else {
            unreachable!()
        };
        document
            .edit("hide", |tx| {
                tx.entity_mut(number)?.solids[1].hidden = true;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            area.select(&document, Match::Contained, false, |_| true)
                .objects,
            vec![entity]
        );
        document
            .edit("hide owner", |tx| {
                tx.entity_mut(number)?.hidden = true;
                Ok(())
            })
            .unwrap();
        assert!(
            area.select(&document, Match::Intersecting, true, |_| true)
                .faces
                .is_empty()
        );
    }
    #[test]
    fn terrain_regions_match_actual_sheets_and_discard_parent_volumes_and_faces() {
        let mut document = Document::empty();
        let ObjectId::Solid(id) = document
            .edit("parent", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(-64.0, -64.0, 0.0),
                    DVec3::new(64.0, 64.0, 16.0),
                    "test",
                )
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
        let face = FaceId { solid: id, side };
        document
            .edit("terrain", |tx| {
                crate::displacement::edit::create(tx, &[face], 2, 0)
            })
            .unwrap();
        document
            .edit("raise", |tx| {
                tx.solid_mut(id)?
                    .sides
                    .iter_mut()
                    .find(|side| side.id == face.side)
                    .unwrap()
                    .displacement
                    .as_mut()
                    .unwrap()
                    .distances = Some(vec![vec![64.0; 5]; 5]);
                Ok(())
            })
            .unwrap();
        let source = document.vmf().clone();
        let sheet = region(DVec3::new(-65.0, -65.0, 79.0), DVec3::new(65.0, 65.0, 81.0));
        let parent = region(DVec3::new(-65.0, -65.0, -1.0), DVec3::new(65.0, 65.0, 17.0));
        for rule in [Match::Contained, Match::Intersecting] {
            assert_eq!(
                sheet.select(&document, rule, false, |_| true).objects,
                [ObjectId::Solid(id)]
            );
            assert_eq!(sheet.select(&document, rule, true, |_| true).faces, [face]);
            assert!(
                parent
                    .select(&document, rule, false, |_| true)
                    .objects
                    .is_empty()
            );
            assert!(
                parent
                    .select(&document, rule, true, |_| true)
                    .faces
                    .is_empty()
            );
        }
        assert_eq!(document.vmf(), &source);
        document
            .edit("peak", |tx| {
                tx.solid_mut(id)?
                    .sides
                    .iter_mut()
                    .find(|side| side.id == face.side)
                    .unwrap()
                    .displacement
                    .as_mut()
                    .unwrap()
                    .distances
                    .as_mut()
                    .unwrap()[2][2] = 128.0;
                Ok(())
            })
            .unwrap();
        let low = region(
            DVec3::new(-65.0, -65.0, 79.0),
            DVec3::new(65.0, 65.0, 100.0),
        );
        assert!(
            low.select(&document, Match::Contained, false, |_| true)
                .objects
                .is_empty()
        );
        assert!(
            low.select(&document, Match::Contained, true, |_| true)
                .faces
                .is_empty()
        );
        let peak = region(DVec3::new(-1.0, -1.0, 143.0), DVec3::new(1.0, 1.0, 145.0));
        assert_eq!(
            peak.select(&document, Match::Intersecting, true, |_| true)
                .faces,
            [face]
        );
    }
}
