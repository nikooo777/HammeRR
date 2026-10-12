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
        let draft = self.checked_partition(token)?.clone();
        let visible = self.partition_top_visibility(&draft);
        let mut hidden = self.hidden_tool_brushes();
        for (piece, shown) in &visible {
            if *shown {
                hidden.remove(&piece.solid);
            }
        }
        let faces = if self.view.tool_textures {
            Vec::new()
        } else {
            self.render_document()
                .vmf()
                .solids()
                .flat_map(|solid| {
                    solid
                        .sides
                        .iter()
                        .map(move |side| (solid.id, side.id, side.material.clone()))
                })
                .collect::<Vec<_>>()
        };
        let hidden_faces = faces
            .into_iter()
            .filter_map(|(solid, side, name)| {
                let material = self.materials.load(&self.game, &name);
                self.materials.get(material).tool.then_some((solid, side))
            })
            .collect::<BTreeSet<_>>();
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
        let Some(hit) = query::pick_face_filtered(
            self.render_document(),
            &ray,
            |solid| !hidden.contains(&solid.id),
            |solid, side, point| {
                if let Some(part) = draft
                    .partitions
                    .iter()
                    .find(|part| part.source.solid == solid.id && part.source.side == side.id)
                {
                    let positions = part.positions(draft.rise);
                    let depth =
                        (point - camera.position.as_dvec3()).dot(camera.forward().as_dvec3());
                    let half = if camera.projection == Projection::Perspective {
                        (f64::from(camera.fov_y_degrees).to_radians() * 0.5).tan() * depth
                    } else {
                        f64::from(camera.ortho_height) * 0.5
                    };
                    let tolerance = PICK_EPSILON.max(half / f64::from(pointer.size.1));
                    part.loops.iter().any(|piece| {
                        visible.get(&PartitionRef {
                            solid: solid.id,
                            partition: piece.id,
                        }) == Some(&true)
                            && contains(
                                &piece
                                    .vertices
                                    .iter()
                                    .map(|index| positions[*index as usize])
                                    .collect::<Vec<_>>(),
                                point,
                                tolerance,
                            )
                    })
                } else {
                    !hidden_faces.contains(&(solid.id, side.id))
                }
            },
        ) else {
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
        if let Some(piece) = self
            .checked_component_preview(token)?
            .extrusion
            .as_ref()
            .and_then(|draft| {
                draft
                    .plan
                    .pieces
                    .iter()
                    .find(|piece| hit.solid == Some(piece.solid.id))
            })
        {
            return Ok(vec![PartitionPick {
                partition: PartitionRef {
                    solid: piece.source.solid,
                    partition: piece.partition,
                },
                position: hit.point,
                depth,
            }]);
        }
        let half = if camera.projection == Projection::Perspective {
            (f64::from(camera.fov_y_degrees).to_radians() * 0.5).tan() * depth
        } else {
            f64::from(camera.ortho_height) * 0.5
        };
        let tolerance = PICK_EPSILON.max(half / f64::from(pointer.size.1));
        let mut candidates = Vec::new();
        for partition in &draft.partitions {
            if hit.solid != Some(partition.source.solid) {
                continue;
            }
            let positions = partition.positions(draft.rise);
            for piece in &partition.loops {
                let points = piece
                    .vertices
                    .iter()
                    .map(|index| positions[*index as usize])
                    .collect::<Vec<_>>();
                if visible.get(&PartitionRef {
                    solid: partition.source.solid,
                    partition: piece.id,
                }) != Some(&true)
                    || !contains(&points, hit.point, tolerance)
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

fn contains(points: &[DVec3], point: DVec3, tolerance: f64) -> bool {
    let Some(plane) = Plane::from_points([points[0], points[1], points[2]]) else {
        return false;
    };
    plane.distance_to(point).abs() <= tolerance
        && (0..points.len()).all(|index| {
            let edge = points[(index + 1) % points.len()] - points[index];
            edge.cross(point - points[index]).dot(plane.normal) <= tolerance * edge.length()
        })
}
