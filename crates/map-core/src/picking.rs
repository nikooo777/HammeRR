use glam::DVec3;
use source_formats::vmf::Solid;

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
    order: usize,
}

impl Primitive {
    fn hit(&self, ray: &Ray) -> Option<Hit> {
        let (mut enter, mut exit) = self.bounds.entry(ray)?;
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
        let mut primitives = Vec::new();
        let mut add = |order, object, solid: &Solid| {
            if solid.hidden || !include_solid(solid) {
                return;
            }
            let Ok(geometry) = BrushGeometry::from_solid(solid) else {
                return;
            };
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
                order,
            });
        };
        for (order, solid) in document.vmf().world.solids.iter().enumerate() {
            add(order, ObjectId::Solid(solid.id), solid);
        }
        let mut order = document.vmf().world.solids.len();
        for entity in &document.vmf().entities {
            if !entity.hidden {
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
                    && let Some((mins, maxs)) = entity_bounds(entity)
                {
                    primitives.push(Primitive {
                        bounds: Bounds { mins, maxs },
                        object: ObjectId::Entity(entity.id),
                        solid: None,
                        planes: Vec::new(),
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
}
