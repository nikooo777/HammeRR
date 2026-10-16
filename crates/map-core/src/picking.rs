use glam::DVec3;
use source_formats::vmf::{Entity, Solid};

use crate::query::{Hit, Ray, entity_bounds};
use crate::{BrushGeometry, Document, ObjectId, Plane};

const LEAF_SIZE: usize = 8;

#[derive(Clone, Copy)]
struct Bounds {
    mins: DVec3,
    maxs: DVec3,
}

impl Bounds {
    fn entry(self, ray: &Ray) -> Option<(f64, f64)> {
        let (mut enter, mut exit) = (f64::NEG_INFINITY, f64::INFINITY);
        for axis in 0..3 {
            let direction = ray.direction[axis];
            if direction.abs() < 1e-12 {
                if ray.origin[axis] < self.mins[axis] || ray.origin[axis] > self.maxs[axis] {
                    return None;
                }
            } else {
                let a = (self.mins[axis] - ray.origin[axis]) / direction;
                let b = (self.maxs[axis] - ray.origin[axis]) / direction;
                enter = enter.max(a.min(b));
                exit = exit.min(a.max(b));
            }
        }
        (enter <= exit && exit >= 0.0).then_some((enter, exit))
    }
}

struct Primitive {
    bounds: Bounds,
    object: ObjectId,
    solid: Option<u32>,
    planes: Vec<(Plane, u32)>,
    triangles: Vec<(u32, [DVec3; 3])>,
    order: usize,
}

impl Primitive {
    fn hit(&self, ray: &Ray) -> Option<Hit> {
        let (mut enter, mut exit) = self.bounds.entry(ray)?;
        if !self.triangles.is_empty() {
            let (distance, side) = self
                .triangles
                .iter()
                .filter_map(|(side, points)| {
                    crate::query::ray_triangle(ray, *points).map(|distance| (distance, *side))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))?;
            return Some(Hit {
                object: self.object,
                solid: self.solid,
                side: Some(side),
                distance,
                point: ray.origin + ray.direction * distance,
            });
        }
        let mut side = None;
        if self.solid.is_some() {
            enter = f64::NEG_INFINITY;
            exit = f64::INFINITY;
            for (plane, id) in &self.planes {
                let direction = plane.normal.dot(ray.direction);
                let distance = plane.distance_to(ray.origin);
                if direction.abs() < 1e-12 {
                    if distance > 0.0 {
                        return None;
                    }
                } else {
                    let t = -distance / direction;
                    if direction < 0.0 && t > enter {
                        enter = t;
                        side = Some(*id);
                    } else if direction > 0.0 {
                        exit = exit.min(t);
                    }
                }
            }
        }
        (enter >= 0.0 && enter <= exit).then_some(Hit {
            object: self.object,
            solid: self.solid,
            side,
            distance: enter,
            point: ray.origin + ray.direction * enter,
        })
    }
}

struct Node {
    bounds: Bounds,
    start: usize,
    end: usize,
    children: Option<[usize; 2]>,
}

pub struct Picker {
    primitives: Vec<Primitive>,
    nodes: Vec<Node>,
}

impl Picker {
    pub fn new(
        document: &Document,
        include_solid: impl Fn(&Solid) -> bool,
        include_points: bool,
    ) -> Self {
        Self::with_entity_filter(document, include_solid, include_points, |_| true)
    }

    /// Filters point entities and brush owners; children also pass `include_solid`.
    pub fn with_entity_filter(
        document: &Document,
        include_solid: impl Fn(&Solid) -> bool,
        include_points: bool,
        include_entity: impl Fn(&Entity) -> bool,
    ) -> Self {
        Self::with_entity_bounds(
            document,
            include_solid,
            include_points,
            include_entity,
            entity_bounds,
        )
    }

    pub fn with_entity_bounds(
        document: &Document,
        include_solid: impl Fn(&Solid) -> bool,
        include_points: bool,
        include_entity: impl Fn(&Entity) -> bool,
        point_bounds: impl Fn(&Entity) -> Option<(DVec3, DVec3)>,
    ) -> Self {
        let mut primitives = Vec::new();
        let surfaces = crate::displacement::surface::prepare(document.vmf());
        let mut add = |order, object, solid: &Solid| {
            if solid.hidden || !include_solid(solid) {
                return;
            }
            let Ok(geometry) = BrushGeometry::from_solid(solid) else {
                return;
            };
            if solid.sides.iter().any(|side| side.displacement.is_some()) {
                let triangles = geometry
                    .faces
                    .iter()
                    .filter_map(|face| {
                        let side = &solid.sides[face.side];
                        let mesh = surfaces.get(&crate::surface::FaceId {
                            solid: solid.id,
                            side: side.id,
                        })?;
                        Some((side.id, mesh))
                    })
                    .flat_map(|(side, mesh)| {
                        mesh.triangles()
                            .map(|indices| (side, indices.map(|index| mesh.positions[index])))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();
                if triangles.is_empty() {
                    return;
                }
                let bounds = triangles.iter().flat_map(|(_, points)| points).fold(
                    Bounds {
                        mins: DVec3::splat(f64::INFINITY),
                        maxs: DVec3::splat(f64::NEG_INFINITY),
                    },
                    |bounds, point| Bounds {
                        mins: bounds.mins.min(*point),
                        maxs: bounds.maxs.max(*point),
                    },
                );
                primitives.push(Primitive {
                    bounds,
                    object,
                    solid: Some(solid.id),
                    planes: Vec::new(),
                    triangles,
                    order,
                });
                return;
            }
            let planes = solid
                .sides
                .iter()
                .filter_map(|side| Plane::from_points(side.plane.0).map(|plane| (plane, side.id)))
                .collect::<Vec<_>>();
            if planes.len() != solid.sides.len() {
                return;
            }
            primitives.push(Primitive {
                bounds: Bounds {
                    mins: geometry.mins,
                    maxs: geometry.maxs,
                },
                object,
                solid: Some(solid.id),
                planes,
                triangles: Vec::new(),
                order,
            });
        };
        for (order, solid) in document.vmf().world.solids.iter().enumerate() {
            add(order, ObjectId::Solid(solid.id), solid);
        }
        let mut order = document.vmf().world.solids.len();
        for entity in &document.vmf().entities {
            if !entity.hidden && !entity.solids.is_empty() && include_entity(entity) {
                for (index, solid) in entity.solids.iter().enumerate() {
                    add(order + index, ObjectId::Entity(entity.id), solid);
                }
            }
            order += entity.solids.len().max(1);
        }
        if include_points {
            let mut order = document.vmf().world.solids.len();
            for entity in &document.vmf().entities {
                if !entity.hidden
                    && entity.solids.is_empty()
                    && include_entity(entity)
                    && let Some((mins, maxs)) = point_bounds(entity)
                {
                    primitives.push(Primitive {
                        bounds: Bounds { mins, maxs },
                        object: ObjectId::Entity(entity.id),
                        solid: None,
                        planes: Vec::new(),
                        triangles: Vec::new(),
                        order,
                    });
                }
                order += entity.solids.len().max(1);
            }
        }
        let mut picker = Self {
            primitives,
            nodes: Vec::new(),
        };
        if !picker.primitives.is_empty() {
            picker.build(0, picker.primitives.len());
        }
        picker
    }

    fn build(&mut self, start: usize, end: usize) -> usize {
        let bounds = self.primitives[start..end].iter().fold(
            Bounds {
                mins: DVec3::splat(f64::INFINITY),
                maxs: DVec3::splat(f64::NEG_INFINITY),
            },
            |bounds, primitive| Bounds {
                mins: bounds.mins.min(primitive.bounds.mins),
                maxs: bounds.maxs.max(primitive.bounds.maxs),
            },
        );
        let index = self.nodes.len();
        self.nodes.push(Node {
            bounds,
            start,
            end,
            children: None,
        });
        if end - start > LEAF_SIZE {
            let size = bounds.maxs - bounds.mins;
            let axis = if size.x >= size.y && size.x >= size.z {
                0
            } else if size.y >= size.z {
                1
            } else {
                2
            };
            let middle = start + (end - start) / 2;
            self.primitives[start..end].select_nth_unstable_by(middle - start, |a, b| {
                (a.bounds.mins[axis] + a.bounds.maxs[axis])
                    .total_cmp(&(b.bounds.mins[axis] + b.bounds.maxs[axis]))
                    .then_with(|| a.order.cmp(&b.order))
            });
            let left = self.build(start, middle);
            let right = self.build(middle, end);
            self.nodes[index].children = Some([left, right]);
        }
        index
    }

    pub fn pick(&self, ray: &Ray) -> Option<Hit> {
        let direction = ray.direction.try_normalize()?;
        if self.nodes.is_empty() || !ray.origin.is_finite() {
            return None;
        }
        let ray = Ray { direction, ..*ray };
        let mut closest = None;
        self.visit(0, &ray, &mut closest);
        closest.map(|(hit, _)| hit)
    }

    fn visit(&self, index: usize, ray: &Ray, closest: &mut Option<(Hit, usize)>) {
        let node = &self.nodes[index];
        let Some((enter, _)) = node.bounds.entry(ray) else {
            return;
        };
        if closest
            .as_ref()
            .is_some_and(|(hit, _)| enter > hit.distance)
        {
            return;
        }
        if let Some(children) = node.children {
            for child in children {
                self.visit(child, ray, closest);
            }
        } else {
            for primitive in &self.primitives[node.start..node.end] {
                if let Some(hit) = primitive.hit(ray)
                    && closest.as_ref().is_none_or(|(previous, order)| {
                        hit.distance < previous.distance
                            || (hit.distance == previous.distance && primitive.order < *order)
                    })
                {
                    *closest = Some((hit, primitive.order));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{edit, query};

    #[test]
    fn cached_picking_matches_visible_brush_entity_and_face_queries() {
        let mut document = Document::empty();
        document
            .edit("fixture", |tx| {
                for x in 0..16 {
                    for y in 0..16 {
                        edit::create_block(
                            tx,
                            DVec3::new(x as f64 * 80.0, y as f64 * 80.0, 0.0),
                            DVec3::new(x as f64 * 80.0 + 48.0, y as f64 * 80.0 + 48.0, 64.0),
                            "brick/test",
                        )?;
                    }
                }
                edit::create_entity(
                    tx,
                    "light",
                    Some(DVec3::new(24.0, 24.0, 160.0)),
                    &[],
                    Vec::new(),
                )?;
                Ok(())
            })
            .unwrap();
        for points in [true, false] {
            let picker = Picker::new(&document, |_| true, points);
            for x in 0..40 {
                for y in 0..40 {
                    let ray = Ray {
                        origin: DVec3::new(x as f64 * 32.0 + 3.0, y as f64 * 32.0 + 3.0, 256.0),
                        direction: DVec3::NEG_Z,
                    };
                    let expected = if points {
                        query::pick(&document, &ray, |_| true)
                    } else {
                        query::pick_face(&document, &ray, |_| true)
                    };
                    assert_eq!(picker.pick(&ray), expected);
                }
            }
        }
        let picker = Picker::new(&document, |_| false, true);
        assert_eq!(
            picker
                .pick(&Ray {
                    origin: DVec3::new(24.0, 24.0, 256.0),
                    direction: DVec3::NEG_Z
                })
                .unwrap()
                .solid,
            None
        );
        assert!(
            picker
                .pick(&Ray {
                    origin: DVec3::ZERO,
                    direction: DVec3::ZERO
                })
                .is_none()
        );
    }

    #[test]
    fn entity_filter_reveals_allowed_points_owners_and_world_brushes() {
        let mut document = Document::empty();
        let (near, allowed, owner, owned, world) = document
            .edit("fixture", |tx| {
                let near = edit::create_entity(
                    tx,
                    "info_target",
                    Some(DVec3::Z * 192.0),
                    &[],
                    Vec::new(),
                )?;
                let allowed = edit::create_entity(
                    tx,
                    "info_target",
                    Some(DVec3::Z * 144.0),
                    &[],
                    Vec::new(),
                )?;
                let ObjectId::Solid(owned) = edit::create_block(
                    tx,
                    DVec3::new(-16.0, -16.0, 64.0),
                    DVec3::new(16.0, 16.0, 96.0),
                    "brick/test",
                )?
                else {
                    unreachable!()
                };
                let owner = edit::tie_to_entity(tx, &[owned], "func_detail", &[])?;
                let world = edit::create_block(
                    tx,
                    DVec3::new(-16.0, -16.0, 0.0),
                    DVec3::new(16.0, 16.0, 32.0),
                    "brick/test",
                )?;
                Ok((near, allowed, owner, owned, world))
            })
            .unwrap();
        let ray = Ray {
            origin: DVec3::Z * 256.0,
            direction: DVec3::NEG_Z,
        };
        assert_eq!(
            Picker::new(&document, |_| true, true)
                .pick(&ray)
                .unwrap()
                .object,
            near
        );
        let include_entity = |entity: &Entity| ObjectId::Entity(entity.id) != near;
        let filtered = Picker::with_entity_filter(&document, |_| true, true, include_entity);
        assert_eq!(filtered.pick(&ray).unwrap().object, allowed);
        assert_eq!(
            filtered.pick(&ray),
            query::pick_with_entity_filter(&document, &ray, |_| true, include_entity)
        );
        let brushes = Picker::with_entity_filter(
            &document,
            |_| true,
            true,
            |entity| !entity.solids.is_empty(),
        );
        let hit = brushes.pick(&ray).unwrap();
        assert_eq!(hit.object, owner);
        assert_eq!(hit.solid, Some(owned));
        assert!(hit.side.is_some());
        assert_eq!(
            Picker::new(&document, |_| true, false).pick(&ray),
            Some(hit)
        );
        assert_eq!(
            Picker::with_entity_filter(
                &document,
                |_| true,
                false,
                |entity| {
                    assert!(!entity.solids.is_empty());
                    true
                }
            )
            .pick(&ray),
            Some(hit)
        );
        assert_eq!(
            Picker::with_entity_filter(&document, |_| true, true, |_| false)
                .pick(&ray)
                .unwrap()
                .object,
            world
        );
        let world_only =
            Picker::with_entity_filter(&document, |solid| solid.id != owned, false, |_| true);
        assert_eq!(world_only.pick(&ray).unwrap().object, world);
    }

    #[test]
    fn indexed_terrain_matches_direct_queries_for_raised_offset_and_entity_sheets() {
        for power in 2..=4 {
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
                .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id;
            let face = crate::surface::FaceId { solid: id, side };
            document
                .edit("terrain", |tx| {
                    crate::displacement::edit::create(tx, &[face], power, 0)
                })
                .unwrap();
            let size = (1usize << power) + 1;
            document
                .edit("displace", |tx| {
                    let info = tx
                        .solid_mut(id)?
                        .sides
                        .iter_mut()
                        .find(|side| side.id == face.side)
                        .unwrap()
                        .displacement
                        .as_mut()
                        .unwrap();
                    info.distances = Some(
                        (0..size)
                            .map(|row| {
                                (0..size)
                                    .map(|col| 64.0 + ((row * col) % 7) as f64)
                                    .collect()
                            })
                            .collect(),
                    );
                    info.offsets = Some(vec![vec![DVec3::X * 128.0; size]; size]);
                    Ok(())
                })
                .unwrap();
            let source = document.vmf().clone();
            for points in [false, true] {
                let picker = Picker::new(&document, |_| true, points);
                for x in [70.0, 96.0, 128.0, 160.0, 185.0] {
                    for y in [-50.0, -16.0, 0.0, 32.0, 50.0] {
                        let ray = Ray {
                            origin: DVec3::new(x, y, 256.0),
                            direction: DVec3::NEG_Z,
                        };
                        let a = picker.pick(&ray).unwrap();
                        let b = query::pick_face(&document, &ray, |_| true).unwrap();
                        assert_eq!((a.object, a.solid, a.side), (b.object, b.solid, b.side));
                        assert!(a.point.distance(b.point) < 1e-9);
                    }
                }
                assert!(
                    picker
                        .pick(&Ray {
                            origin: DVec3::new(0.0, 0.0, 128.0),
                            direction: DVec3::NEG_Z
                        })
                        .is_none()
                );
                assert!(
                    picker
                        .pick(&Ray {
                            origin: DVec3::new(128.0, 0.0, 64.0),
                            direction: DVec3::NEG_Z
                        })
                        .is_none()
                );
                assert!(
                    picker
                        .pick(&Ray {
                            origin: DVec3::new(128.0, 0.0, 32.0),
                            direction: DVec3::Z
                        })
                        .is_none()
                );
            }
            assert_eq!(document.vmf(), &source);
            let owner = document
                .edit("owner", |tx| {
                    edit::tie_to_entity(tx, &[id], "func_detail", &[])
                })
                .unwrap();
            let picker = Picker::new(&document, |_| true, false);
            assert_eq!(
                picker
                    .pick(&Ray {
                        origin: DVec3::new(128.0, 0.0, 256.0),
                        direction: DVec3::NEG_Z
                    })
                    .unwrap()
                    .object,
                owner
            );
            let ObjectId::Entity(owner) = owner else {
                unreachable!()
            };
            document
                .edit("hide", |tx| {
                    tx.entity_mut(owner)?.hidden = true;
                    Ok(())
                })
                .unwrap();
            assert!(
                Picker::new(&document, |_| true, false)
                    .pick(&Ray {
                        origin: DVec3::new(128.0, 0.0, 256.0),
                        direction: DVec3::NEG_Z
                    })
                    .is_none()
            );
        }
    }
}
