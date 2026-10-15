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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Actor;
    use glam::Vec3;

    fn block(session: &mut Session, offset: DVec3) -> u32 {
        let ObjectId::Solid(id) = session
            .create_block(
                Actor::Agent,
                offset,
                offset + DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        id
    }

    fn fixture() -> (Session, u32) {
        let mut session = Session::new(Err("no assets".into()));
        let id = block(&mut session, DVec3::ZERO);
        (session, id)
    }

    fn pointer(camera: Camera, position: DVec3) -> Pointer {
        Pointer {
            position: render::handles::project(&camera, position.as_vec3(), (1200, 800))
                .unwrap()
                .as_dvec2(),
            size: (1200, 800),
        }
    }

    #[test]
    fn corners_and_edges_match_render_projection_in_every_view() {
        let (mut session, id) = fixture();
        let original = session.document.vmf().clone();
        let stamp = session.component_stamp();
        let shape = session.component_topology(&[id]).unwrap().remove(0);
        for projection in [
            Projection::Perspective,
            Projection::Top,
            Projection::Front,
            Projection::Side,
        ] {
            let camera = if projection == Projection::Perspective {
                Camera::looking_at(Vec3::new(-300.0, -400.0, 350.0), Vec3::splat(32.0))
            } else {
                Camera::planar(projection, Vec3::splat(32.0), 256.0)
            };
            for vertex in &shape.vertices {
                let picks = session
                    .pick_components(
                        camera,
                        pointer(camera, vertex.position),
                        Kind::Vertex,
                        &[id],
                        1.0,
                    )
                    .unwrap();
                assert!(
                    picks
                        .candidates
                        .iter()
                        .any(|candidate| candidate.component.sides == vertex.sides),
                    "{projection:?}"
                );
                assert!(picks.candidates[0].distance < 0.01);
            }
            for edge in &shape.edges {
                let [a, b] = edge.vertices.map(|index| shape.vertices[index].position);
                let picks = session
                    .pick_components(
                        camera,
                        pointer(camera, (a + b) * 0.5),
                        Kind::Edge,
                        &[id],
                        1.0,
                    )
                    .unwrap();
                let hit = picks
                    .candidates
                    .iter()
                    .find(|candidate| candidate.component.sides == edge.sides)
                    .unwrap();
                assert!(hit.distance < 0.01);
                assert!(project(camera, hit.position, (1200, 800)).distance(hit.screen) < 1e-5);
            }
        }
        assert_eq!(session.component_stamp(), stamp);
        assert_eq!(session.document.vmf(), &original);
    }

    #[test]
    fn picking_scope_survives_component_changes_and_invalid_settings_but_not_external_edits() {
        let (mut session, a) = fixture();
        let b = block(&mut session, DVec3::X * 128.0);
        let ids = [a, b];
        session
            .set_component_picking(Some(Kind::Vertex), &ids, &session.component_stamp())
            .unwrap();
        let shape = session.component_topology(&[a]).unwrap().remove(0);
        let components = shape
            .vertices
            .iter()
            .map(|vertex| Component {
                solid: a,
                sides: vertex.sides.clone(),
            })
            .collect::<Vec<_>>();
        let history = session.document.undo_labels().count();
        session
            .select_components(&components, SelectMode::Replace, &session.component_stamp())
            .unwrap();
        assert_eq!(session.selected_ids(), vec![ObjectId::Solid(a)]);
        assert_eq!(session.component_solids(), ids);
        let token = session
            .preview_component_move(DVec3::Z * 4.0, false, false, &session.component_stamp())
            .unwrap();
        let stamp = session.component_stamp();
        assert!(
            session
                .set_component_picking(Some(Kind::Face), &[u32::MAX], &stamp)
                .is_err()
        );
        assert!(session.set_component_picking(None, &[], "stale").is_err());
        assert_eq!(session.component_stamp(), stamp);
        assert_eq!(
            session.geometry.components.preview.as_ref().unwrap().token,
            token
        );
        session
            .select_components(&[], SelectMode::Replace, &stamp)
            .unwrap();
        assert_eq!(session.component_solids(), ids);
        assert_eq!(session.document.undo_labels().count(), history);
        session
            .select(&[ObjectId::Solid(a)], SelectMode::Replace)
            .unwrap();
        assert!(session.component_picking().is_none());
        session
            .set_component_picking(Some(Kind::Edge), &ids, &session.component_stamp())
            .unwrap();
        session
            .edit(Actor::Agent, "source change", |tx| {
                tx.solid_mut(a)?.sides[0].material = "brick/other".into();
                Ok(())
            })
            .unwrap();
        assert!(session.component_picking().is_none());
    }

    #[test]
    fn previews_move_query_points_and_cancel_restores_them() {
        let (mut session, id) = fixture();
        let shape = session.component_topology(&[id]).unwrap().remove(0);
        let components = shape
            .vertices
            .iter()
            .map(|vertex| Component {
                solid: id,
                sides: vertex.sides.clone(),
            })
            .collect::<Vec<_>>();
        session
            .select_components(&components, SelectMode::Replace, &session.component_stamp())
            .unwrap();
        let camera = Camera::planar(Projection::Top, Vec3::splat(32.0), 512.0);
        let moved = pointer(camera, shape.vertices[0].position + DVec3::X * 128.0);
        assert_eq!(
            session
                .pick_components(camera, moved, Kind::Vertex, &[id], 1.0)
                .unwrap()
                .total,
            0
        );
        let token = session
            .preview_component_move(DVec3::X * 128.0, false, false, &session.component_stamp())
            .unwrap();
        assert!(
            session
                .pick_components(camera, moved, Kind::Vertex, &[id], 1.0)
                .unwrap()
                .total
                > 0
        );
        session.cancel_component_move(token).unwrap();
        assert_eq!(
            session
                .pick_components(camera, moved, Kind::Vertex, &[id], 1.0)
                .unwrap()
                .total,
            0
        );
    }

    #[test]
    fn guides_and_queries_exclude_hidden_brushes_and_owners_but_allow_xray_corners() {
        let (mut session, back) = fixture();
        let front = block(&mut session, DVec3::Z * 128.0);
        let camera = Camera::planar(Projection::Top, Vec3::splat(32.0), 512.0);
        let center = pointer(camera, DVec3::splat(32.0));
        assert_eq!(
            session
                .pick_components(camera, center, Kind::Face, &[back], 10.0)
                .unwrap()
                .total,
            0
        );
        let corner = pointer(camera, DVec3::ZERO);
        let picks = session
            .pick_components(camera, corner, Kind::Vertex, &[back, front], 1.0)
            .unwrap();
        assert_eq!(picks.total, 4);
        assert_eq!(picks.candidates[0].component.solid, front);
        assert!(
            picks
                .candidates
                .windows(2)
                .all(|pair| pair[0].depth <= pair[1].depth)
        );
        let owner = session
            .tie_to_entity(
                Actor::Agent,
                &[ObjectId::Solid(front)],
                "func_detail",
                Default::default(),
            )
            .unwrap();
        session
            .set_visibility(Actor::User, "hide", &[owner])
            .unwrap();
        session
            .set_component_picking(Some(Kind::Vertex), &[front], &session.component_stamp())
            .unwrap();
        let mut mesh = render::SceneMesh::build(
            &source_formats::vmf::Vmf::parse("world { id 1 }").unwrap(),
            &session.game,
            &mut session.materials,
            &Default::default(),
        );
        session.append_component_guides(&mut mesh);
        assert!(mesh.lines.is_empty());
        assert_eq!(
            session
                .pick_components(camera, corner, Kind::Vertex, &[front], 1.0)
                .unwrap()
                .total,
            0
        );
        let hit = session
            .pick_components(camera, center, Kind::Face, &[back], 10.0)
            .unwrap();
        assert_eq!(hit.candidates[0].component.solid, back);
        session
            .set_visibility(Actor::User, "hide", &[ObjectId::Solid(back)])
            .unwrap();
        assert_eq!(
            session
                .pick_components(camera, corner, Kind::Edge, &[back], 1.0)
                .unwrap()
                .total,
            0
        );
    }

    #[test]
    fn tool_visibility_rebuilds_guides_and_reactivating_transform_exits_picking() {
        let (mut session, id) = fixture();
        let assets = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(assets.path().join("materials/tools")).unwrap();
        std::fs::write(
            assets.path().join("materials/tools/toolsnodraw.vmt"),
            "UnlitGeneric { %compilenodraw 1 }",
        )
        .unwrap();
        let gameinfo = assets.path().join("gameinfo.txt");
        std::fs::write(
            &gameinfo,
            "GameInfo { FileSystem { SearchPaths { Game |gameinfo_path|. } } }",
        )
        .unwrap();
        session.game =
            std::sync::Arc::new(source_formats::gamefs::GameFs::from_gameinfo(&gameinfo).unwrap());
        session
            .edit(Actor::Agent, "tool material", |tx| {
                for side in &mut tx.solid_mut(id)?.sides {
                    side.material = "tools/toolsnodraw".into();
                }
                Ok(())
            })
            .unwrap();
        let camera = Camera::planar(Projection::Top, Vec3::splat(32.0), 256.0);
        let corner = pointer(camera, DVec3::ZERO);
        session
            .set_component_picking(Some(Kind::Edge), &[id], &session.component_stamp())
            .unwrap();
        let mut view = session.view();
        view.tool_textures = true;
        session.set_view(view);
        assert!(!session.build_scene().lines.is_empty());
        assert!(
            session
                .pick_components(camera, corner, Kind::Edge, &[id], 1.0)
                .unwrap()
                .total
                > 0
        );
        let stamp = session.component_stamp();
        view.tool_textures = false;
        session.set_view(view);
        assert_ne!(stamp, session.component_stamp());
        assert!(session.build_scene().lines.is_empty());
        for kind in [Kind::Vertex, Kind::Edge, Kind::Face] {
            assert_eq!(
                session
                    .pick_components(camera, corner, kind, &[id], 1.0)
                    .unwrap()
                    .total,
                0
            );
        }
        view.tool_textures = true;
        session.set_view(view);
        assert!(!session.build_scene().lines.is_empty());
        session
            .set_component_picking(None, &[], &session.component_stamp())
            .unwrap();
        let mut tool = session.transform_tool();
        tool.mode = Some(render::handles::Mode::Move);
        session.set_transform_tool(Actor::Agent, tool).unwrap();
        session
            .set_component_picking(Some(Kind::Edge), &[id], &session.component_stamp())
            .unwrap();
        session.set_transform_tool(Actor::Agent, tool).unwrap();
        assert!(session.component_picking().is_none());
        session
            .set_component_picking(Some(Kind::Edge), &[id], &session.component_stamp())
            .unwrap();
        tool.mode = None;
        session.set_transform_tool(Actor::Agent, tool).unwrap();
        assert!(session.component_picking().is_none());
        session
            .set_component_picking(Some(Kind::Edge), &[id], &session.component_stamp())
            .unwrap();
        session.set_face_mode(false);
        assert!(session.component_picking().is_none());
    }

    #[test]
    fn near_plane_clips_edges_and_perspective_hit_positions_project_back_to_pixels() {
        let (mut session, id) = fixture();
        let camera =
            Camera::looking_at(Vec3::new(32.0, -10.0, 10.0), Vec3::new(100.0, -10.0, 10.0));
        let shape = session.component_topology(&[id]).unwrap().remove(0);
        let edge = shape
            .edges
            .iter()
            .find(|edge| {
                edge.vertices.iter().all(|index| {
                    let p = shape.vertices[*index].position;
                    p.y == 0.0 && p.z == 0.0
                })
            })
            .unwrap();
        let target = DVec3::new(60.0, 0.0, 0.0);
        let picks = session
            .pick_components(camera, pointer(camera, target), Kind::Edge, &[id], 1.0)
            .unwrap();
        let hit = picks
            .candidates
            .iter()
            .find(|candidate| candidate.component.sides == edge.sides)
            .unwrap();
        assert!(hit.position.distance(target) < 0.01);
        assert!(
            picks
                .candidates
                .iter()
                .all(|hit| hit.depth >= f64::from(Camera::NEAR) - 1e-8)
        );
        assert!(clip_edge(camera, DVec3::ZERO, DVec3::X * 30.0).is_none());
        let clipped = clip_edge(camera, DVec3::ZERO, DVec3::X * 64.0).unwrap();
        assert!((depth(camera, clipped[0]) - f64::from(Camera::NEAR)).abs() < 1e-8);
    }

    #[test]
    fn query_limits_and_ties_are_bounded_and_deterministic() {
        let (mut session, first) = fixture();
        let mut ids = vec![first];
        for _ in 1..16 {
            ids.push(block(&mut session, DVec3::ZERO));
        }
        let camera = Camera::planar(Projection::Top, Vec3::splat(32.0), 8192.0);
        let p = pointer(camera, DVec3::splat(32.0));
        let hits = session
            .pick_components(camera, p, Kind::Vertex, &ids, 32.0)
            .unwrap();
        assert_eq!(hits.total, 128);
        assert_eq!(hits.candidates.len(), MAX_CANDIDATES);
        ids.reverse();
        let again = session
            .pick_components(camera, p, Kind::Vertex, &ids, 32.0)
            .unwrap();
        assert_eq!(
            hits.candidates
                .iter()
                .map(|hit| &hit.component)
                .collect::<Vec<_>>(),
            again
                .candidates
                .iter()
                .map(|hit| &hit.component)
                .collect::<Vec<_>>()
        );
        for radius in [0.0, 33.0, f64::NAN] {
            assert!(
                session
                    .pick_components(camera, p, Kind::Edge, &ids, radius)
                    .is_err()
            );
        }
        let outside = Pointer {
            position: DVec2::splat(-1.0),
            ..p
        };
        assert!(
            session
                .pick_components(camera, outside, Kind::Face, &ids, 10.0)
                .is_err()
        );
    }
}
