use glam::DVec3;
use source_formats::vmf::{Entity, Solid};

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
        self.select_with_entity_bounds(document, rule, faces, include, query::entity_bounds)
    }

    pub fn select_with_entity_bounds(
        &self,
        document: &Document,
        rule: Match,
        faces: bool,
        include: impl Fn(&Solid) -> bool,
        entity_bounds: impl Fn(&Entity) -> Option<(DVec3, DVec3)>,
    ) -> Selection {
        self.select_with_entity_bounds_and_solids(
            document,
            rule,
            faces,
            false,
            include,
            entity_bounds,
        )
    }

    pub fn select_with_entity_bounds_and_solids(
        &self,
        document: &Document,
        rule: Match,
        faces: bool,
        individual_solids: bool,
        include: impl Fn(&Solid) -> bool,
        entity_bounds: impl Fn(&Entity) -> Option<(DVec3, DVec3)>,
    ) -> Selection {
        let mut result = Selection::default();
        let terrain = crate::displacement::surface::prepare(document.vmf());
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
                            && entity_bounds(entity)
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
                            let mesh = terrain.get(&FaceId {
                                solid: solid.id,
                                side: solid.sides[face.side].id,
                            })?;
                            Some((face.side, mesh))
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
                        let matched = !surfaces.is_empty()
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
                            };
                        if individual_solids {
                            if matched {
                                result.objects.push(ObjectId::Solid(solid.id));
                            }
                        } else {
                            matches.push(matched);
                        }
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
                    let matched = self.brush_matches(solid, &shape, rule);
                    if individual_solids {
                        if matched {
                            result.objects.push(ObjectId::Solid(solid.id));
                        }
                    } else {
                        matches.push(matched);
                    }
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
