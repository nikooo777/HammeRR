use super::*;
use crate::session::Pointer;
use map_core::{
    Plane,
    query::{self, Ray},
};
use render::{Camera, Projection};

const PICK_EPSILON: f64 = 0.0001;

#[derive(Debug)]
pub struct PartitionPick {
    pub partition: PartitionRef,
    pub position: DVec3,
    pub depth: f64,
}

impl Session {
    pub fn pick_component_partitions(
        &mut self,
        token: u64,
        camera: Camera,
        pointer: Pointer,
    ) -> Result<Vec<PartitionPick>, String> {
        pointer.validate()?;
        if pointer.position.min_element() < 0.0
            || pointer.position.x >= f64::from(pointer.size.0)
            || pointer.position.y >= f64::from(pointer.size.1)
        {
            return Err("Pick inside the viewport".into());
        }
        if !camera.position.is_finite()
            || !camera.yaw_degrees.is_finite()
            || !camera.pitch_degrees.is_finite()
            || !camera.fov_y_degrees.is_finite()
            || !(1.0..179.0).contains(&camera.fov_y_degrees)
            || !camera.ortho_height.is_finite()
            || camera.ortho_height <= 0.0
        {
            return Err("Partition picking needs a valid camera".into());
        }
        self.checked_partition(token)?;
        let hidden = self.hidden_tool_brushes();
        let ndc = pointer.ndc();
        let (origin, direction) = camera.ray(
            ndc.x as f32,
            ndc.y as f32,
            pointer.size.0 as f32 / pointer.size.1 as f32,
        );
        let ray = Ray {
            origin: origin.as_dvec3(),
            direction: direction.as_dvec3(),
        };
        let Some(hit) = query::pick_face(self.render_document(), &ray, |solid| {
            !hidden.contains(&solid.id)
        }) else {
            return Ok(Vec::new());
        };
        let depth = (hit.point - camera.position.as_dvec3()).dot(camera.forward().as_dvec3());
        let far = if camera.projection == Projection::Perspective {
            f64::INFINITY
        } else {
            f64::from(Camera::ORTHO_DISTANCE) * 2.0
        };
        if depth < f64::from(Camera::NEAR) || depth > far {
            return Ok(Vec::new());
        }
        let half = if camera.projection == Projection::Perspective {
            (f64::from(camera.fov_y_degrees).to_radians() * 0.5).tan() * depth
        } else {
            f64::from(camera.ortho_height) * 0.5
        };
        let tolerance = PICK_EPSILON.max(half / f64::from(pointer.size.1));
        let draft = self.checked_partition(token)?;
        let mut candidates = Vec::new();
        for partition in &draft.partitions {
            if hit.solid != Some(partition.source.solid) {
                continue;
            }
            let positions = partition.positions(draft.rise);
            for piece in &partition.loops {
                let points = piece.vertices.map(|index| positions[index as usize]);
                let plane = Plane::from_points([points[0], points[1], points[2]]).unwrap();
                if plane.distance_to(hit.point).abs() > tolerance
                    || (0..4).any(|index| {
                        let edge = points[(index + 1) % 4] - points[index];
                        edge.cross(hit.point - points[index]).dot(plane.normal)
                            > tolerance * edge.length()
                    })
                {
                    continue;
                }
                candidates.push(PartitionPick {
                    partition: PartitionRef {
                        solid: partition.source.solid,
                        partition: piece.id,
                    },
                    position: hit.point,
                    depth,
                });
            }
        }
        candidates.sort_by(|a, b| {
            a.depth
                .total_cmp(&b.depth)
                .then(a.partition.cmp(&b.partition))
        });
        Ok(candidates)
    }
}
