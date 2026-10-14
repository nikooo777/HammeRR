use std::collections::{BTreeMap, BTreeSet, HashSet};

use glam::DVec3;
use map_core::{
    query::{self, Ray},
    surface::FaceId,
};
use serde_json::{Value, json};

use super::{Actor, Session};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TriangleRef {
    pub face: FaceId,
    pub triangle: usize,
}

pub struct Hit {
    pub power: u32,
    pub triangle: TriangleRef,
    pub point: DVec3,
    pub distance: f64,
}

pub(super) struct Highlight {
    generation: u64,
    revision: u64,
    token: u64,
    powers: BTreeMap<FaceId, u32>,
    selected: BTreeSet<TriangleRef>,
}

struct Targets {
    powers: BTreeMap<FaceId, u32>,
    selected: BTreeSet<TriangleRef>,
}

impl Session {
    fn validate_terrain_triangle_targets(
        &self,
        targets: &[(FaceId, Vec<usize>)],
    ) -> Result<Targets, String> {
        if targets.is_empty() {
            return Ok(Targets {
                powers: BTreeMap::new(),
                selected: BTreeSet::new(),
            });
        }
        let faces = targets.iter().map(|(face, _)| *face).collect::<Vec<_>>();
        if faces.iter().collect::<BTreeSet<_>>().len() != faces.len() {
            return Err("Use one triangle selection per terrain face".into());
        }
        self.terrain_triangles(&faces, false)?;
        let grids = self.terrain_grids(&faces, false)?;
        let powers = grids
            .iter()
            .map(|grid| (grid.face, grid.power))
            .collect::<BTreeMap<_, _>>();
        let mut selected = BTreeSet::new();
        for (face, ids) in targets {
            let count = 2 * (1usize << powers[face]).pow(2);
            if ids.iter().any(|&id| id >= count) {
                return Err("Select valid native terrain triangle IDs".into());
            }
            selected.extend(ids.iter().map(|&triangle| TriangleRef {
                face: *face,
                triangle,
            }));
        }
        Ok(Targets { powers, selected })
    }

    pub fn highlight_terrain_triangles(
        &mut self,
        actor: Actor,
        targets: &[(FaceId, Vec<usize>)],
    ) -> Result<u64, String> {
        let Targets { powers, selected } = self.validate_terrain_triangle_targets(targets)?;
        self.terrain_triangle_serial = self.terrain_triangle_serial.wrapping_add(1);
        let token = self.terrain_triangle_serial;
        self.terrain_triangle_highlight = Some(Highlight {
            generation: self.document_generation(),
            revision: self.document.revision(),
            token,
            powers,
            selected,
        });
        self.scene_serial += 1;
        self.log(actor, "highlighted terrain triangles");
        Ok(token)
    }

    pub fn clear_terrain_triangle_highlight(&mut self, token: Option<u64>) -> Result<(), String> {
        if let Some(token) = token
            && self
                .terrain_triangle_highlight
                .as_ref()
                .map(|highlight| highlight.token)
                != Some(token)
        {
            return Err("Terrain triangle highlight was replaced".into());
        }
        if self.terrain_triangle_highlight.take().is_some() {
            self.scene_serial += 1;
        }
        Ok(())
    }

    fn terrain_highlight_stale(&self, highlight: &Highlight) -> bool {
        if highlight.generation != self.document_generation()
            || highlight.revision != self.document.revision()
        {
            return true;
        }
        let faces = highlight.powers.keys().copied().collect::<Vec<_>>();
        if faces.is_empty() {
            return false;
        }
        match self.terrain_grids(&faces, true) {
            Ok(grids) => grids
                .iter()
                .any(|grid| highlight.powers.get(&grid.face) != Some(&grid.power)),
            Err(_) => true,
        }
    }

    pub fn terrain_triangle_highlight_json(&self) -> Value {
        match &self.terrain_triangle_highlight {
            None => json!({"token":null,"stale":false,"triangles":[]}),
            Some(highlight) => {
                json!({"token":highlight.token.to_string(),"stale":self.terrain_highlight_stale(highlight),"triangles":highlight.selected.iter().map(|selected|json!({"solid":selected.face.solid,"side":selected.face.side,"triangle":selected.triangle})).collect::<Vec<_>>()})
            }
        }
    }

    fn hidden_terrain_tool_faces(&mut self) -> HashSet<FaceId> {
        let mut hidden = HashSet::new();
        if !self.view.tool_textures {
            let faces = self
                .render_document()
                .vmf()
                .solids()
                .flat_map(|solid| {
                    solid.sides.iter().map(|side| {
                        (
                            FaceId {
                                solid: solid.id,
                                side: side.id,
                            },
                            side.material.clone(),
                        )
                    })
                })
                .collect::<Vec<_>>();
            for (face, material) in faces {
                let id = self.materials.load(&self.game, &material);
                if self.materials.get(id).tool {
                    hidden.insert(face);
                }
            }
        }
        hidden
    }

    pub fn pick_terrain_triangle(
        &mut self,
        ray: &Ray,
        faces: &[FaceId],
    ) -> Result<Option<Hit>, String> {
        if !ray.origin.is_finite()
            || !ray.direction.is_finite()
            || !ray.direction.length_squared().is_finite()
            || ray.direction.length_squared() < 1e-12
        {
            return Err("Terrain picking needs a finite ray with a nonzero direction".into());
        }
        let ray = Ray {
            origin: ray.origin,
            direction: ray.direction.normalize(),
        };
        let grids = self.terrain_grids(faces, true)?;
        let hidden = self.hidden_terrain_tool_faces();
        let hit = query::pick_face_filtered(
            self.render_document(),
            &ray,
            |_| true,
            |solid, side, _| {
                !hidden.contains(&FaceId {
                    solid: solid.id,
                    side: side.id,
                })
            },
        );
        let Some(hit) = hit else { return Ok(None) };
        let (Some(solid), Some(side)) = (hit.solid, hit.side) else {
            return Ok(None);
        };
        let face = FaceId { solid, side };
        let Some(grid) = grids.iter().find(|grid| grid.face == face) else {
            return Ok(None);
        };
        let Some((triangle, distance)) = query::pick_displacement_triangle(&ray, &grid.mesh) else {
            return Ok(None);
        };
        if (distance - hit.distance).abs() > 1e-6 {
            return Ok(None);
        };
        Ok(Some(Hit {
            power: grid.power,
            triangle: TriangleRef { face, triangle },
            point: ray.origin + ray.direction * distance,
            distance,
        }))
    }

    pub fn frame_terrain_triangles(
        &mut self,
        actor: Actor,
        targets: &[(FaceId, Vec<usize>)],
        aspect: f32,
    ) -> Result<(), String> {
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("Viewport aspect must be positive and finite".into());
        }
        let Targets { powers, selected } = self.validate_terrain_triangle_targets(targets)?;
        if selected.is_empty() {
            return Err("Select terrain triangles to frame".into());
        }
        let faces = targets.iter().map(|(face, _)| *face).collect::<Vec<_>>();
        if self
            .terrain_grids(&faces, true)?
            .iter()
            .any(|grid| powers.get(&grid.face) != Some(&grid.power))
        {
            return Err("Finish the grid-resolution preview before framing triangles".into());
        }
        let hidden = self.hidden_terrain_tool_faces();
        let mut bounds = None;
        for row in self.terrain_triangles(&faces, true)? {
            if self.component_hidden(row.face.solid, &HashSet::new()) || hidden.contains(&row.face)
            {
                continue;
            }
            for triangle in &row.triangles {
                if selected.contains(&TriangleRef {
                    face: row.face,
                    triangle: triangle.id,
                }) {
                    for point in triangle.positions {
                        bounds = Some(bounds.map_or((point, point), |(lo, hi): (DVec3, DVec3)| {
                            (lo.min(point), hi.max(point))
                        }));
                    }
                }
            }
        }
        let (lo, hi) = bounds.ok_or("No visible terrain triangles to frame")?;
        self.set_camera(self.camera.frame_bounds(lo.as_vec3(), hi.as_vec3(), aspect));
        self.log(actor, "framed terrain triangles");
        Ok(())
    }

    pub fn append_terrain_triangle_guides(&mut self, mesh: &mut render::SceneMesh) {
        let Some(highlight) = &self.terrain_triangle_highlight else {
            return;
        };
        if self.terrain_highlight_stale(highlight) || highlight.selected.is_empty() {
            return;
        }
        let selected = highlight.selected.clone();
        let faces = highlight.powers.keys().copied().collect::<Vec<_>>();
        let hidden = self.hidden_terrain_tool_faces();
        let Ok(rows) = self.terrain_triangles(&faces, true) else {
            return;
        };
        for row in rows {
            if self.component_hidden(row.face.solid, &HashSet::new()) || hidden.contains(&row.face)
            {
                continue;
            }
            for triangle in &row.triangles {
                if selected.contains(&TriangleRef {
                    face: row.face,
                    triangle: triangle.id,
                }) {
                    let [a, b, c] = triangle.positions;
                    let offset = (c - a).cross(b - a).normalize_or_zero() * 0.05;
                    for point in [a, b, b, c, c, a] {
                        mesh.lines.push(render::scene::LineVertex {
                            position: (point + offset).as_vec3().to_array(),
                            color: [255, 220, 65, 255],
                        });
                    }
                }
            }
        }
    }
}

pub struct VertexMaskFace {
    pub face: FaceId,
    pub vertices: Vec<usize>,
    pub triangles: usize,
}

pub struct VertexMask {
    pub token: u64,
    pub faces: Vec<VertexMaskFace>,
}

impl VertexMask {
    pub fn summary(&self) -> String {
        format!(
            "{} vertices on {} surfaces",
            self.faces
                .iter()
                .map(|face| face.vertices.len())
                .sum::<usize>(),
            self.faces.len()
        )
    }

    pub fn json(&self) -> Value {
        json!({"token":self.token.to_string(),"vertex_count":self.faces.iter().map(|face|face.vertices.len()).sum::<usize>(),"faces":self.faces.iter().map(|face|json!({"solid":face.face.solid,"side":face.face.side,"vertices":face.vertices,"triangle_count":face.triangles})).collect::<Vec<_>>()})
    }
}

impl Session {
    pub fn terrain_vertex_mask(
        &self,
        faces: &[FaceId],
        token: Option<u64>,
    ) -> Result<VertexMask, String> {
        self.terrain_grids(faces, false)?;
        let highlight = self
            .terrain_triangle_highlight
            .as_ref()
            .ok_or("Select terrain triangles before using a vertex mask")?;
        if token.is_some_and(|token| token != highlight.token) {
            return Err("Terrain vertex mask was replaced; inspect the current selection".into());
        }
        if self.terrain_highlight_stale(highlight) {
            return Err("Terrain vertex mask is stale; select triangles again".into());
        }
        let faces = faces.iter().copied().collect::<BTreeSet<_>>();
        let indices = highlight
            .powers
            .iter()
            .filter(|(face, _)| faces.contains(face))
            .map(|(face, power)| {
                map_core::displacement::triangles::indices(*power)
                    .map(|indices| (*face, indices))
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let mut selected = BTreeMap::<FaceId, BTreeSet<usize>>::new();
        let mut counts = BTreeMap::<FaceId, usize>::new();
        for triangle in &highlight.selected {
            if !faces.contains(&triangle.face) {
                continue;
            }
            let vertices = indices[&triangle.face]
                .get(triangle.triangle)
                .ok_or("Terrain triangle IDs changed")?;
            selected.entry(triangle.face).or_default().extend(*vertices);
            *counts.entry(triangle.face).or_default() += 1;
        }
        if selected.is_empty() {
            return Err("No highlighted triangle vertices on these terrain faces".into());
        }
        Ok(VertexMask {
            token: highlight.token,
            faces: selected
                .into_iter()
                .map(|(face, vertices)| VertexMaskFace {
                    face,
                    vertices: vertices.into_iter().collect(),
                    triangles: counts[&face],
                })
                .collect(),
        })
    }

    pub fn preview_terrain_masked_edit(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        operation: map_core::displacement::Operation,
        token: u64,
    ) -> Result<usize, String> {
        if !matches!(
            operation,
            map_core::displacement::Operation::Sculpt { .. }
                | map_core::displacement::Operation::Blend { .. }
        ) {
            return Err("Only sculpting and blending accept a terrain vertex mask".into());
        }
        let mask = self.terrain_vertex_mask(faces, Some(token))?;
        let edits = mask
            .faces
            .into_iter()
            .map(|face| {
                (
                    face.face,
                    map_core::surface::SurfaceEdit {
                        terrain: Some(map_core::displacement::Operation::Masked {
                            operation: Box::new(operation.clone()),
                            vertices: face.vertices,
                        }),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let changed = self.preview_or_apply_faces(actor, edits, true)?;
        self.face_preview.as_mut().unwrap().terrain_mask_token = Some(token);
        Ok(changed)
    }

    pub(super) fn refresh_terrain_mask_after_commit(&mut self, token: u64) {
        let generation = self.document_generation();
        let revision = self.document.revision();
        if let Some(highlight) = self.terrain_triangle_highlight.as_mut()
            && highlight.token == token
        {
            highlight.generation = generation;
            highlight.revision = revision;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use map_core::{
        ObjectId,
        displacement::{
            Operation,
            brush::{Axis, Brush, Sculpt},
        },
    };

    fn fixture() -> (Session, Vec<FaceId>) {
        let mut session = Session::new(Err("no assets needed".into()));
        let mut faces = Vec::new();
        for (index, power) in [2, 3].into_iter().enumerate() {
            let offset = DVec3::X * index as f64 * 96.0;
            let ObjectId::Solid(solid) = session
                .create_block(
                    Actor::Agent,
                    offset,
                    offset + DVec3::new(64.0, 64.0, 16.0),
                    "brick/test",
                    false,
                )
                .unwrap()
            else {
                unreachable!()
            };
            let side = session
                .document()
                .solid(solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id;
            let face = FaceId { solid, side };
            session
                .create_terrain(Actor::Agent, &[face], power, index as u32)
                .unwrap();
            faces.push(face);
        }
        session
            .select_faces(&faces, super::super::SelectMode::Replace)
            .unwrap();
        (session, faces)
    }

    fn guides(session: &mut Session) -> Vec<render::scene::LineVertex> {
        session
            .build_scene()
            .lines
            .into_iter()
            .filter(|line| line.color == [255, 220, 65, 255])
            .collect()
    }

    #[test]
    fn terrain_triangle_guides_follow_drafts_hide_stale_topology_and_guard_replacement() {
        let (mut session, faces) = fixture();
        let original = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let first = session
            .highlight_terrain_triangles(
                Actor::User,
                &[(faces[0], vec![0, 7]), (faces[1], vec![31])],
            )
            .unwrap();
        assert_eq!(guides(&mut session).len(), 18);
        let state = session.terrain_triangle_highlight_json();
        assert!(
            session
                .highlight_terrain_triangles(Actor::Agent, &[(faces[0], vec![32])])
                .is_err()
        );
        assert_eq!(session.terrain_triangle_highlight_json(), state);
        session
            .preview_terrain_edit(
                Actor::Agent,
                &faces,
                Operation::Sculpt {
                    brush: Brush {
                        center: DVec3::new(64.0, 32.0, 16.0),
                        radius: 8192.0,
                        axis: Axis::WorldZ,
                    },
                    operation: Sculpt::Raise(16.0),
                },
            )
            .unwrap();
        assert!(
            guides(&mut session)
                .iter()
                .all(|line| line.position[2] > 31.0)
        );
        assert_eq!(session.terrain_triangle_highlight_json()["stale"], false);
        session.cancel_face_preview();
        session
            .preview_terrain_resolution(Actor::Agent, &[faces[0]], 3)
            .unwrap();
        assert_eq!(session.terrain_triangle_highlight_json()["stale"], true);
        assert!(guides(&mut session).is_empty());
        let camera = session.camera();
        assert!(
            session
                .frame_terrain_triangles(Actor::Agent, &[(faces[0], vec![0])], 1.5)
                .is_err()
        );
        assert_eq!(session.camera(), camera);
        session.cancel_face_preview();
        let replacement = session
            .highlight_terrain_triangles(Actor::Agent, &[(faces[1], vec![7])])
            .unwrap();
        assert!(
            session
                .clear_terrain_triangle_highlight(Some(first))
                .is_err()
        );
        assert_eq!(
            session.terrain_triangle_highlight_json()["token"],
            replacement.to_string()
        );
        session
            .frame_terrain_triangles(Actor::User, &[(faces[1], vec![7])], 1.5)
            .unwrap();
        assert_ne!(session.camera(), camera);
        assert_eq!(session.document().vmf(), &original);
        assert_eq!(session.document().undo_labels().count(), history);
        session
            .clear_terrain_triangle_highlight(Some(replacement))
            .unwrap();
        assert!(guides(&mut session).is_empty());
        session
            .highlight_terrain_triangles(Actor::Agent, &[(faces[0], vec![0])])
            .unwrap();
        session
            .create_block(
                Actor::Agent,
                DVec3::splat(200.0),
                DVec3::splat(216.0),
                "brick/test",
                false,
            )
            .unwrap();
        assert_eq!(session.terrain_triangle_highlight_json()["stale"], true);
        assert!(guides(&mut session).is_empty());
    }

    #[test]
    fn terrain_triangle_picks_share_native_ids_occlusion_tools_and_hidden_owners() {
        let (mut session, faces) = fixture();
        let centroid =
            session.terrain_triangles(&[faces[0]], false).unwrap()[0].triangles[7].centroid;
        let ray = Ray {
            origin: centroid + DVec3::Z * 64.0,
            direction: DVec3::NEG_Z * 2.0,
        };
        let hit = session
            .pick_terrain_triangle(&ray, &faces)
            .unwrap()
            .unwrap();
        assert_eq!(
            hit.triangle,
            TriangleRef {
                face: faces[0],
                triangle: 7
            }
        );
        assert!(hit.point.distance(centroid) < 1e-9 && (hit.distance - 64.0).abs() < 1e-9);
        assert!(
            session
                .pick_terrain_triangle(
                    &Ray {
                        origin: centroid - DVec3::Z * 64.0,
                        direction: DVec3::Z
                    },
                    &faces
                )
                .unwrap()
                .is_none()
        );
        session
            .edit(Actor::Agent, "hide terrain", |tx| {
                tx.solid_mut(faces[0].solid)?.hidden = true;
                Ok(())
            })
            .unwrap();
        assert!(
            session
                .pick_terrain_triangle(&ray, &faces)
                .unwrap()
                .is_none()
        );
        session
            .edit(Actor::Agent, "show terrain", |tx| {
                tx.solid_mut(faces[0].solid)?.hidden = false;
                Ok(())
            })
            .unwrap();
        let (assets, game) = crate::test_assets::game();
        std::fs::create_dir_all(assets.path().join("materials/tools")).unwrap();
        std::fs::write(
            assets.path().join("materials/tools/toolsnodraw.vmt"),
            "UnlitGeneric { %compilenodraw 1 }",
        )
        .unwrap();
        session.game = std::sync::Arc::new(game);
        session
            .edit_faces(
                Actor::Agent,
                &[faces[0]],
                map_core::surface::SurfaceEdit {
                    material: Some("tools/toolsnodraw".into()),
                    ..Default::default()
                },
                false,
                false,
            )
            .unwrap();
        let mut view = session.view();
        view.tool_textures = false;
        session.set_view(view);
        assert!(
            session
                .pick_terrain_triangle(&ray, &faces)
                .unwrap()
                .is_none()
        );
        let mut view = session.view();
        view.tool_textures = true;
        session.set_view(view);
        assert!(
            session
                .pick_terrain_triangle(&ray, &faces)
                .unwrap()
                .is_some()
        );
        session
            .create_block(
                Actor::Agent,
                centroid + DVec3::new(-4.0, -4.0, 8.0),
                centroid + DVec3::new(4.0, 4.0, 16.0),
                "brick/test",
                false,
            )
            .unwrap();
        assert!(
            session
                .pick_terrain_triangle(&ray, &faces)
                .unwrap()
                .is_none()
        );
        assert!(
            session
                .pick_terrain_triangle(
                    &Ray {
                        origin: centroid,
                        direction: DVec3::ZERO
                    },
                    &faces
                )
                .is_err()
        );
    }
    #[test]
    fn terrain_vertex_masks_share_subset_writes_reuse_and_atomic_token_guards() {
        for actor in [Actor::User, Actor::Agent] {
            let (mut session, faces) = fixture();
            let source = session.document().vmf().clone();
            let token = session
                .highlight_terrain_triangles(actor, &[(faces[0], vec![0, 1])])
                .unwrap();
            let mask = session.terrain_vertex_mask(&faces, Some(token)).unwrap();
            assert_eq!(mask.faces.len(), 1);
            assert_eq!(mask.faces[0].vertices, [0, 1, 5, 6]);
            let brush = Brush {
                center: DVec3::new(32.0, 32.0, 16.0),
                radius: 8192.0,
                axis: Axis::WorldZ,
            };
            session
                .preview_terrain_masked_edit(
                    actor,
                    &faces,
                    Operation::Sculpt {
                        brush,
                        operation: Sculpt::Raise(16.0),
                    },
                    token,
                )
                .unwrap();
            let before = session.terrain_grids(&faces, false).unwrap();
            let draft = session.terrain_grids(&faces, true).unwrap();
            for (index, (old, new)) in before[0]
                .mesh
                .positions
                .iter()
                .zip(&draft[0].mesh.positions)
                .enumerate()
            {
                assert_eq!(old != new, mask.faces[0].vertices.contains(&index));
            }
            assert_eq!(before[1].mesh, draft[1].mesh);
            assert_eq!(session.document().vmf(), &source);
            session.commit_face_preview(actor).unwrap();
            assert!(session.terrain_vertex_mask(&faces, Some(token)).is_ok());
            let raised = session.document().vmf().clone();
            session
                .preview_terrain_masked_edit(
                    actor,
                    &faces,
                    Operation::Blend {
                        brush,
                        operation: map_core::displacement::brush::Blend::Fill(200.0),
                    },
                    token,
                )
                .unwrap();
            let values = session.terrain_grids(&faces, true).unwrap();
            for (index, &value) in values[0].mesh.alphas.iter().enumerate() {
                assert_eq!(
                    value,
                    if mask.faces[0].vertices.contains(&index) {
                        200.0
                    } else {
                        0.0
                    }
                );
            }
            let preview = session.face_preview_token();
            session
                .highlight_terrain_triangles(actor, &[(faces[1], vec![0])])
                .unwrap();
            assert!(
                session
                    .preview_terrain_masked_edit(
                        actor,
                        &faces,
                        Operation::Sculpt {
                            brush,
                            operation: Sculpt::Raise(32.0)
                        },
                        token
                    )
                    .is_err()
            );
            assert_eq!(session.face_preview_token(), preview);
            session.cancel_face_preview();
            assert_eq!(session.document().vmf(), &raised);
            session.undo(actor).unwrap();
            assert_eq!(session.document().vmf(), &source);
            assert!(session.terrain_vertex_mask(&faces, None).is_err());
        }
    }
}
