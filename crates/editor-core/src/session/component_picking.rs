use std::collections::BTreeSet;

use glam::{DVec2, DVec3};
use map_core::ObjectId;
use map_core::query::{self, Ray};
use map_core::subobject::{self, Component};
use render::{Camera, Projection};
use serde::{Deserialize, Serialize};

use super::{Pointer, SelectMode, Session};

pub const PICK_RADIUS: f64 = 10.0;
pub const MAX_RADIUS: f64 = 32.0;
pub const MAX_CANDIDATES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Vertex,
    Edge,
    Face,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vertex => "corners",
            Self::Edge => "edges",
            Self::Face => "faces",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Picking {
    pub kind: Kind,
    pub solids: BTreeSet<u32>,
}

impl Picking {
    pub fn hint(&self) -> String {
        format!(
            "Pick {} · {} {} · Ctrl toggles · Esc cancels",
            self.kind.label(),
            self.solids.len(),
            if self.solids.len() == 1 {
                "brush"
            } else {
                "brushes"
            }
        )
    }
}

#[derive(Debug)]
pub struct Candidate {
    pub component: Component,
    pub position: DVec3,
    pub screen: DVec2,
    pub distance: f64,
    pub depth: f64,
}

pub struct Picks {
    pub candidates: Vec<Candidate>,
    pub total: usize,
}

fn depth(camera: Camera, point: DVec3) -> f64 {
    (point - camera.position.as_dvec3()).dot(camera.forward().as_dvec3())
}

fn project(camera: Camera, point: DVec3, size: (u32, u32)) -> DVec2 {
    let relative = point - camera.position.as_dvec3();
    let half = if camera.projection == Projection::Perspective {
        (f64::from(camera.fov_y_degrees).to_radians() * 0.5).tan() * depth(camera, point)
    } else {
        f64::from(camera.ortho_height) * 0.5
    };
    let scale = f64::from(size.1) / (2.0 * half);
    DVec2::new(
        f64::from(size.0) * 0.5 + relative.dot(camera.right().as_dvec3()) * scale,
        f64::from(size.1) * 0.5 - relative.dot(camera.up().as_dvec3()) * scale,
    )
}

fn depth_range(camera: Camera) -> (f64, f64) {
    (
        f64::from(Camera::NEAR),
        if camera.projection == Projection::Perspective {
            f64::INFINITY
        } else {
            f64::from(Camera::ORTHO_DISTANCE) * 2.0
        },
    )
}

fn clip_edge(camera: Camera, mut a: DVec3, mut b: DVec3) -> Option<[DVec3; 2]> {
    let (near, far) = depth_range(camera);
    for (plane, lower) in [(near, true), (far, false)] {
        if !plane.is_finite() {
            continue;
        }
        let da = depth(camera, a);
        let db = depth(camera, b);
        let outside_a = if lower { da < plane } else { da > plane };
        let outside_b = if lower { db < plane } else { db > plane };
        if outside_a && outside_b {
            return None;
        }
        if outside_a != outside_b {
            let point = a.lerp(b, (plane - da) / (db - da));
            if outside_a {
                a = point;
            } else {
                b = point;
            }
        }
    }
    Some([a, b])
}

impl Session {
    pub fn component_picking(&self) -> Option<&Picking> {
        self.geometry.components.picking.as_ref()
    }

    pub fn set_component_picking(
        &mut self,
        kind: Option<Kind>,
        solids: &[u32],
        stamp: &str,
    ) -> Result<(), String> {
        self.check_component_stamp(stamp)?;
        let Some(kind) = kind else {
            self.clear_component_edit();
            return Ok(());
        };
        if solids.is_empty() || solids.len() > subobject::MAX_SOLIDS {
            return Err("Choose 1 to 16 brushes for viewport component selection".into());
        }
        self.component_topology(solids)?;
        let solids = solids.iter().copied().collect::<BTreeSet<_>>();
        let selection = self
            .geometry
            .components
            .selection
            .iter()
            .filter(|component| solids.contains(&component.solid))
            .cloned()
            .collect::<Vec<_>>();
        self.select_components(&selection, SelectMode::Replace, stamp)?;
        self.geometry.components.picking = Some(Picking { kind, solids });
        self.geometry.components.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    pub(super) fn component_hidden(
        &self,
        solid: u32,
        tools: &std::collections::HashSet<u32>,
    ) -> bool {
        let document = self.render_document();
        let Ok((owner, brush)) = subobject::solid(document, solid) else {
            return true;
        };
        brush.hidden
            || !self.working_solid_visible(document, solid)
            || tools.contains(&solid)
            || match owner {
                ObjectId::Entity(id) => document.entity(id).is_none_or(|entity| entity.hidden),
                ObjectId::Solid(_) => false,
            }
    }

    pub fn pick_components(
        &mut self,
        camera: Camera,
        pointer: Pointer,
        kind: Kind,
        solids: &[u32],
        radius: f64,
    ) -> Result<Picks, String> {
        pointer.validate()?;
        if pointer.position.min_element() < 0.0
            || pointer.position.x >= f64::from(pointer.size.0)
            || pointer.position.y >= f64::from(pointer.size.1)
            || !radius.is_finite()
            || !(1.0..=MAX_RADIUS).contains(&radius)
        {
            return Err("Pick inside the viewport with a radius from 1 to 32 pixels".into());
        }
        if !camera.position.is_finite()
            || !camera.yaw_degrees.is_finite()
            || !camera.pitch_degrees.is_finite()
            || !camera.fov_y_degrees.is_finite()
            || !(1.0..179.0).contains(&camera.fov_y_degrees)
            || !camera.ortho_height.is_finite()
            || camera.ortho_height <= 0.0
        {
            return Err("Component picking needs a valid camera".into());
        }
        let component_document = self
            .geometry
            .components
            .preview
            .as_ref()
            .map_or(&self.document, |preview| &preview.document);
        if !std::ptr::eq(self.render_document(), component_document) {
            return Err(
                "Finish the other geometry preview or enter component picking first".into(),
            );
        }
        let shapes = self.component_topology(solids)?;
        let hidden = self.hidden_tool_brushes();
        let mut candidates = Vec::new();
        let (near, far) = depth_range(camera);
        if kind == Kind::Face {
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
            if let Some(hit) = query::pick_face(self.render_document(), &ray, |solid| {
                !hidden.contains(&solid.id)
            }) && let (Some(solid), Some(side)) = (hit.solid, hit.side)
                && solids.contains(&solid)
                && (near..=far).contains(&depth(camera, hit.point))
            {
                candidates.push(Candidate {
                    component: Component {
                        solid,
                        sides: vec![side],
                    },
                    position: hit.point,
                    screen: pointer.position,
                    distance: 0.0,
                    depth: depth(camera, hit.point),
                });
            }
        } else {
            for shape in shapes {
                if self.component_hidden(shape.solid, &hidden) {
                    continue;
                }
                let mut add = |sides: Vec<u32>, position: DVec3, screen: DVec2| {
                    let distance = screen.distance(pointer.position);
                    if distance <= radius {
                        candidates.push(Candidate {
                            component: Component {
                                solid: shape.solid,
                                sides,
                            },
                            position,
                            screen,
                            distance,
                            depth: depth(camera, position),
                        });
                    }
                };
                if kind == Kind::Vertex {
                    for vertex in &shape.vertices {
                        if (near..=far).contains(&depth(camera, vertex.position)) {
                            add(
                                vertex.sides.clone(),
                                vertex.position,
                                project(camera, vertex.position, pointer.size),
                            );
                        }
                    }
                } else {
                    for edge in &shape.edges {
                        let [a, b] = edge.vertices.map(|index| shape.vertices[index].position);
                        let Some([a, b]) = clip_edge(camera, a, b) else {
                            continue;
                        };
                        let pa = project(camera, a, pointer.size);
                        let pb = project(camera, b, pointer.size);
                        let segment = pb - pa;
                        let t = if segment.length_squared() > 1e-12 {
                            ((pointer.position - pa).dot(segment) / segment.length_squared())
                                .clamp(0.0, 1.0)
                        } else if depth(camera, a) <= depth(camera, b) {
                            0.0
                        } else {
                            1.0
                        };
                        let world_t = if camera.projection == Projection::Perspective {
                            t * depth(camera, a)
                                / ((1.0 - t) * depth(camera, b) + t * depth(camera, a))
                        } else {
                            t
                        };
                        add(edge.sides.clone(), a.lerp(b, world_t), pa.lerp(pb, t));
                    }
                }
            }
        }
        candidates.sort_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then(a.depth.total_cmp(&b.depth))
                .then(a.component.cmp(&b.component))
        });
        let total = candidates.len();
        candidates.truncate(MAX_CANDIDATES);
        Ok(Picks { candidates, total })
    }
}
