use std::collections::{BTreeMap, BTreeSet, HashSet};

use glam::DVec3;
use map_core::{displacement::edit::TerrainGrid, query::Ray, surface::FaceId};
use serde_json::{Value, json};

use super::{Actor, Session, terrain_triangles::VolumeSelection};

pub const MAX_SET_FACES: usize = 16;
pub const MAX_SET_VERTICES: usize = MAX_SET_FACES * map_core::displacement::MAX_GRID_VERTICES;
pub const MAX_HEIGHT_CSV_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexSource {
    pub generation: u64,
    pub revision: u64,
}

impl VertexSource {
    pub fn json(self) -> Value {
        json!({"generation":self.generation.to_string(),"revision":self.revision.to_string()})
    }
}

#[derive(Debug, Clone)]
pub struct VertexSetFace {
    pub face: FaceId,
    pub vertices: Vec<map_core::displacement::VertexSet>,
}

pub fn parse_height_csv(text: &str) -> Result<Vec<VertexSetFace>, String> {
    if text.len() > MAX_HEIGHT_CSV_BYTES {
        return Err("Terrain height CSV exceeds 512 KiB".into());
    }
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("solid,side,vertex,z") {
        return Err("Terrain height CSV must begin with solid,side,vertex,z".into());
    }
    let mut faces = BTreeMap::<FaceId, Vec<map_core::displacement::VertexSet>>::new();
    let mut selected = BTreeSet::new();
    for (line, row) in lines.enumerate() {
        if selected.len() == MAX_SET_VERTICES {
            return Err("Set at most 4624 terrain vertices per preview".into());
        }
        let error = || {
            format!(
                "Invalid terrain height CSV row {}: expected solid,side,vertex,z with finite Z within ±65536",
                line + 2
            )
        };
        let mut fields = row.split(',').map(str::trim);
        let solid = fields
            .next()
            .ok_or_else(error)?
            .parse()
            .map_err(|_| error())?;
        let side = fields
            .next()
            .ok_or_else(error)?
            .parse()
            .map_err(|_| error())?;
        let vertex = fields
            .next()
            .ok_or_else(error)?
            .parse()
            .map_err(|_| error())?;
        let z: f64 = fields
            .next()
            .ok_or_else(error)?
            .parse()
            .map_err(|_| error())?;
        if fields.next().is_some()
            || !z.is_finite()
            || z.abs() > map_core::primitive::MAX_COORDINATE
        {
            return Err(error());
        }
        let face = FaceId { solid, side };
        if !selected.insert((face, vertex)) {
            return Err(format!(
                "Terrain height CSV repeats a vertex at row {}",
                line + 2
            ));
        }
        if (!faces.contains_key(&face) && faces.len() == MAX_SET_FACES)
            || faces
                .get(&face)
                .is_some_and(|vertices| vertices.len() == map_core::displacement::MAX_GRID_VERTICES)
        {
            return Err("Set 1–16 terrain faces and at most 289 vertices per face".into());
        }
        faces
            .entry(face)
            .or_default()
            .push(map_core::displacement::VertexSet {
                vertex,
                value: map_core::displacement::VertexValue::WorldZ(z),
            });
    }
    if selected.is_empty() {
        return Err("Terrain height CSV must contain at least one vertex row".into());
    }
    Ok(faces
        .into_iter()
        .map(|(face, vertices)| VertexSetFace { face, vertices })
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VertexRef {
    pub face: FaceId,
    pub vertex: usize,
}

pub struct Hit {
    pub power: u32,
    pub vertex: VertexRef,
    pub position: DVec3,
    pub point: DVec3,
    pub distance: f64,
}

impl Session {
    pub fn terrain_vertex_source(&self) -> VertexSource {
        VertexSource {
            generation: self.document_generation(),
            revision: self.document.revision(),
        }
    }

    pub fn preview_terrain_vertex_set(
        &mut self,
        actor: Actor,
        targets: &[VertexSetFace],
        source: VertexSource,
    ) -> Result<usize, String> {
        if source != self.terrain_vertex_source() {
            return Err(
                "Terrain vertex source is stale; inspect the current native vertices again".into(),
            );
        }
        if targets.is_empty() || targets.len() > MAX_SET_FACES {
            return Err("Set vertices on 1–16 terrain faces".into());
        }
        let mut faces = BTreeSet::new();
        for target in targets {
            if !faces.insert(target.face) {
                return Err("Use one absolute vertex group per terrain face".into());
            }
            if target.vertices.is_empty()
                || target.vertices.len() > map_core::displacement::MAX_GRID_VERTICES
            {
                return Err("Set 1–289 native vertices per terrain face".into());
            }
        }
        self.terrain_grids(&faces.into_iter().collect::<Vec<_>>(), false)?;
        let highlight = self
            .terrain_triangle_highlight
            .as_ref()
            .filter(|highlight| !self.terrain_highlight_stale(highlight))
            .map(|highlight| highlight.token);
        let edits = targets
            .iter()
            .map(|target| {
                (
                    target.face,
                    map_core::surface::SurfaceEdit {
                        terrain: Some(map_core::displacement::Operation::VertexSet {
                            vertices: target.vertices.clone(),
                        }),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let changed = self.preview_or_apply_faces(actor, edits, true)?;
        self.geometry
            .face_preview
            .as_mut()
            .unwrap()
            .terrain_mask_token = highlight;
        Ok(changed)
    }

    pub fn preview_terrain_vertex_move(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        offset: DVec3,
        token: u64,
    ) -> Result<usize, String> {
        let mask = self.terrain_vertex_mask(faces, Some(token))?;
        let edits = mask
            .faces
            .into_iter()
            .map(|face| {
                (
                    face.face,
                    map_core::surface::SurfaceEdit {
                        terrain: Some(map_core::displacement::Operation::VertexMove {
                            vertices: face.vertices,
                            offset,
                        }),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let changed = self.preview_or_apply_faces(actor, edits, true)?;
        self.geometry
            .face_preview
            .as_mut()
            .unwrap()
            .terrain_mask_token = Some(token);
        Ok(changed)
    }

    pub fn terrain_vertices_json(&self, faces: &[FaceId], preview: bool) -> Result<Value, String> {
        let grids = self.terrain_grids(faces, preview)?;
        Ok(
            json!({"source":if preview {Value::Null} else {self.terrain_vertex_source().json()},"faces":grids.iter().map(|grid|json!({
            "solid":grid.face.solid,"side":grid.face.side,"power":grid.power,"size":grid.mesh.size,
            "vertices":grid.mesh.positions.iter().enumerate().map(|(id,point)|json!({
                "id":id,"row":id/grid.mesh.size,"column":id%grid.mesh.size,
                "position":point.to_array(),"base_position":grid.mesh.base_positions[id].to_array(),
                "alpha":grid.mesh.alphas[id]
            })).collect::<Vec<_>>()
        })).collect::<Vec<_>>()}),
        )
    }

    fn terrain_vertex_targets(
        &self,
        targets: &[(FaceId, Vec<usize>)],
    ) -> Result<(Vec<TerrainGrid>, BTreeSet<VertexRef>), String> {
        if targets.is_empty() {
            return Ok((Vec::new(), BTreeSet::new()));
        }
        let faces = targets.iter().map(|(face, _)| *face).collect::<Vec<_>>();
        if faces.iter().collect::<BTreeSet<_>>().len() != faces.len() {
            return Err("Use one vertex selection per terrain face".into());
        }
        let grids = self.terrain_grids(&faces, false)?;
        let counts = grids
            .iter()
            .map(|grid| (grid.face, grid.mesh.positions.len()))
            .collect::<BTreeMap<_, _>>();
        let mut selected = BTreeSet::new();
        for (face, vertices) in targets {
            if vertices.iter().any(|&vertex| vertex >= counts[face]) {
                return Err("Select valid row-major terrain vertex IDs".into());
            }
            selected.extend(vertices.iter().map(|&vertex| VertexRef {
                face: *face,
                vertex,
            }));
        }
        Ok((grids, selected))
    }

    pub fn highlight_terrain_vertices(
        &mut self,
        actor: Actor,
        targets: &[(FaceId, Vec<usize>)],
    ) -> Result<u64, String> {
        let (grids, selected) = self.terrain_vertex_targets(targets)?;
        let powers = grids.iter().map(|grid| (grid.face, grid.power)).collect();
        Ok(self.install_terrain_highlight(actor, powers, BTreeSet::new(), Some(selected)))
    }

    pub fn pick_terrain_vertex(
        &mut self,
        ray: &Ray,
        faces: &[FaceId],
    ) -> Result<Option<Hit>, String> {
        let Some(hit) = self.pick_terrain_triangle(ray, faces)? else {
            return Ok(None);
        };
        let grids = self.terrain_surface_grids(&[hit.triangle.face], true)?;
        let grid = &grids[0];
        let indices = map_core::displacement::triangles::indices(grid.power)
            .map_err(|error| error.to_string())?[hit.triangle.triangle];
        let vertex = indices
            .into_iter()
            .min_by(|&a, &b| {
                grid.mesh.positions[a]
                    .distance_squared(hit.point)
                    .total_cmp(&grid.mesh.positions[b].distance_squared(hit.point))
                    .then(a.cmp(&b))
            })
            .unwrap();
        Ok(Some(Hit {
            power: hit.power,
            vertex: VertexRef {
                face: hit.triangle.face,
                vertex,
            },
            position: grid.mesh.positions[vertex],
            point: hit.point,
            distance: hit.distance,
        }))
    }

    pub fn frame_terrain_vertices(
        &mut self,
        actor: Actor,
        targets: &[(FaceId, Vec<usize>)],
        aspect: f32,
    ) -> Result<(), String> {
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("Viewport aspect must be positive and finite".into());
        }
        let (grids, selected) = self.terrain_vertex_targets(targets)?;
        if selected.is_empty() {
            return Err("Select terrain vertices to frame".into());
        }
        let powers = grids
            .iter()
            .map(|grid| (grid.face, grid.power))
            .collect::<BTreeMap<_, _>>();
        let faces = grids.iter().map(|grid| grid.face).collect::<Vec<_>>();
        let grids = self.terrain_surface_grids(&faces, true)?;
        if grids
            .iter()
            .any(|grid| powers.get(&grid.face) != Some(&grid.power))
        {
            return Err("Finish the grid-resolution preview before framing vertices".into());
        }
        let hidden = self.hidden_terrain_tool_faces();
        let mut bounds = None;
        for grid in grids {
            if self.component_hidden(grid.face.solid, &HashSet::new())
                || hidden.contains(&grid.face)
            {
                continue;
            }
            for vertex in selected.iter().filter(|vertex| vertex.face == grid.face) {
                let point = grid.mesh.positions[vertex.vertex];
                bounds = Some(bounds.map_or((point, point), |(lo, hi): (DVec3, DVec3)| {
                    (lo.min(point), hi.max(point))
                }));
            }
        }
        let (lo, hi) = bounds.ok_or("No visible terrain vertices to frame")?;
        self.set_camera(self.camera.frame_bounds(lo.as_vec3(), hi.as_vec3(), aspect));
        self.log(actor, "framed terrain vertices");
        Ok(())
    }

    pub(super) fn append_terrain_vertex_guides(&mut self, mesh: &mut render::SceneMesh) {
        let Some(highlight) = &self.terrain_triangle_highlight else {
            return;
        };
        if self.terrain_highlight_stale(highlight) {
            return;
        }
        let Some(vertices) = &highlight.vertices else {
            return;
        };
        let vertices = vertices.clone();
        let faces = highlight.powers.keys().copied().collect::<Vec<_>>();
        if vertices.is_empty() {
            return;
        }
        let hidden = self.hidden_terrain_tool_faces();
        let Ok(grids) = self.terrain_surface_grids(&faces, true) else {
            return;
        };
        for grid in grids {
            if self.component_hidden(grid.face.solid, &HashSet::new())
                || hidden.contains(&grid.face)
            {
                continue;
            }
            for vertex in vertices.iter().filter(|vertex| vertex.face == grid.face) {
                let point = grid.mesh.positions[vertex.vertex];
                for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                    for sign in [-1.0, 1.0] {
                        mesh.lines.push(render::scene::LineVertex {
                            position: (point + axis * sign * 1.75).as_vec3().to_array(),
                            color: [85, 225, 255, 255],
                        });
                    }
                }
            }
        }
    }

    pub fn terrain_vertices_in_volume(
        &mut self,
        faces: &[FaceId],
        token: u64,
    ) -> Result<Vec<(FaceId, Vec<usize>)>, String> {
        let spec = self.checked_volume(token)?.spec;
        let region = map_core::region::Region::from_bounds(spec.mins, spec.maxs)?;
        let grids = self.terrain_surface_grids(faces, true)?;
        let hidden = self.hidden_terrain_tool_faces();
        let mut targets = Vec::new();
        for grid in grids {
            if self.component_hidden(grid.face.solid, &HashSet::new())
                || hidden.contains(&grid.face)
            {
                continue;
            }
            let ids = grid
                .mesh
                .positions
                .iter()
                .enumerate()
                .filter_map(|(id, point)| {
                    region
                        .polygon_matches(&[*point], map_core::region::Match::Contained)
                        .then_some(id)
                })
                .collect::<Vec<_>>();
            if !ids.is_empty() {
                targets.push((grid.face, ids));
            }
        }
        Ok(targets)
    }

    pub fn select_terrain_vertices_in_volume(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        token: u64,
    ) -> Result<VolumeSelection, String> {
        let targets = self.terrain_vertices_in_volume(faces, token)?;
        let token = self.highlight_terrain_vertices(actor, &targets)?;
        Ok(VolumeSelection { token, targets })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SelectMode, volume::Spec};
    use map_core::{
        ObjectId,
        displacement::{
            Operation,
            brush::{Axis, Blend, Brush, Sculpt},
        },
    };

    fn fixture(power: u32, corner: u32) -> (Session, FaceId) {
        let mut session = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(solid) = session
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::new(64.0, 64.0, 16.0),
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
            .create_terrain(Actor::Agent, &[face], power, corner)
            .unwrap();
        session.select_faces(&[face], SelectMode::Replace).unwrap();
        (session, face)
    }

    #[test]
    fn absolute_vertex_batches_are_exact_atomic_and_preserve_highlights_through_commit() {
        use map_core::displacement::{VertexSet, VertexValue};
        let (mut session, face) = fixture(4, 3);
        let ObjectId::Solid(second) = session
            .create_block(
                Actor::User,
                DVec3::new(96.0, 0.0, 0.0),
                DVec3::new(160.0, 64.0, 16.0),
                "test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let second = FaceId {
            solid: second,
            side: session
                .document()
                .solid(second)
                .unwrap()
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id,
        };
        session
            .create_terrain(Actor::User, &[second], 2, 1)
            .unwrap();
        let source = session.terrain_vertex_source();
        let original = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let before = session
            .terrain_grids(&[face], false)
            .unwrap()
            .remove(0)
            .mesh;
        let highlight = session
            .highlight_terrain_vertices(Actor::User, &[(face, vec![7])])
            .unwrap();
        let guides = session.terrain_triangle_highlight_json();
        let targets = vec![VertexSetFace {
            face,
            vertices: before
                .positions
                .iter()
                .enumerate()
                .map(|(vertex, point)| VertexSet {
                    vertex,
                    value: if vertex % 2 == 0 {
                        VertexValue::WorldZ(vertex as f64 * 17.25 - 2000.0)
                    } else {
                        VertexValue::Position(point.with_z(vertex as f64 * 17.25 - 2000.0))
                    },
                })
                .collect(),
        }];
        assert_eq!(
            session.terrain_vertices_json(&[face], false).unwrap()["source"],
            source.json()
        );
        assert!(session.terrain_vertices_json(&[face], true).unwrap()["source"].is_null());
        assert_eq!(
            session
                .preview_terrain_vertex_set(Actor::Agent, &targets, source)
                .unwrap(),
            1
        );
        let draft = session.terrain_grids(&[face], true).unwrap().remove(0).mesh;
        for (vertex, point) in draft.positions.iter().enumerate() {
            assert_eq!(
                *point,
                before.positions[vertex].with_z(vertex as f64 * 17.25 - 2000.0)
            );
        }
        let preview = session.face_preview_token();
        let preview_map = session.render_document().vmf().clone();
        for invalid in [
            vec![
                targets[0].clone(),
                VertexSetFace {
                    face: second,
                    vertices: vec![VertexSet {
                        vertex: 25,
                        value: VertexValue::WorldZ(1.0),
                    }],
                },
            ],
            vec![
                targets[0].clone(),
                VertexSetFace {
                    face: FaceId {
                        solid: face.solid,
                        side: u32::MAX,
                    },
                    vertices: vec![VertexSet {
                        vertex: 0,
                        value: VertexValue::WorldZ(1.0),
                    }],
                },
            ],
            vec![VertexSetFace {
                face,
                vertices: vec![
                    VertexSet {
                        vertex: 0,
                        value: VertexValue::WorldZ(25.0),
                    },
                    VertexSet {
                        vertex: 289,
                        value: VertexValue::WorldZ(1.0),
                    },
                ],
            }],
            vec![VertexSetFace {
                face,
                vertices: vec![
                    VertexSet {
                        vertex: 0,
                        value: VertexValue::WorldZ(25.0),
                    },
                    VertexSet {
                        vertex: 1,
                        value: VertexValue::WorldZ(f64::INFINITY),
                    },
                ],
            }],
            vec![targets[0].clone(), targets[0].clone()],
            vec![VertexSetFace {
                face,
                vertices: vec![targets[0].vertices[0]; 290],
            }],
        ] {
            assert!(
                session
                    .preview_terrain_vertex_set(Actor::Agent, &invalid, source)
                    .is_err()
            );
            assert_eq!(session.face_preview_token(), preview);
            assert_eq!(session.render_document().vmf(), &preview_map);
            assert_eq!(session.document().vmf(), &original);
            assert_eq!(session.terrain_triangle_highlight_json(), guides);
            assert_eq!(session.document().undo_labels().count(), history);
        }
        for stale in [
            VertexSource {
                generation: source.generation + 1,
                ..source
            },
            VertexSource {
                revision: source.revision + 1,
                ..source
            },
        ] {
            assert!(
                session
                    .preview_terrain_vertex_set(Actor::Agent, &targets, stale)
                    .is_err()
            );
            assert_eq!(session.face_preview_token(), preview);
            assert_eq!(session.terrain_triangle_highlight_json(), guides);
        }
        session.cancel_face_preview();
        assert_eq!(session.document().vmf(), &original);
        assert_eq!(session.terrain_triangle_highlight_json(), guides);
        session
            .preview_terrain_vertex_set(Actor::User, &targets, source)
            .unwrap();
        session.commit_face_preview(Actor::User).unwrap();
        assert_eq!(session.document().vmf(), &preview_map);
        assert_eq!(session.document().undo_labels().count(), history + 1);
        assert_eq!(
            session
                .terrain_vertex_mask(&[face], Some(highlight))
                .unwrap()
                .faces[0]
                .vertices,
            [7]
        );
        assert_eq!(session.terrain_triangle_highlight_json(), guides);
        assert!(
            session
                .preview_terrain_vertex_set(Actor::Agent, &targets, source)
                .is_err()
        );
        session.undo(Actor::User).unwrap();
        assert_eq!(session.document().vmf(), &original);
        assert!(
            session
                .preview_terrain_vertex_set(Actor::Agent, &targets, source)
                .is_err()
        );
        assert!(
            session
                .terrain_vertex_mask(&[face], Some(highlight))
                .is_err()
        );
        session.redo(Actor::User).unwrap();
        assert_eq!(session.document().vmf(), &preview_map);
    }

    #[test]
    fn height_csv_is_strict_bounded_and_preserves_native_sample_identity() {
        let (mut session, face) = fixture(2, 1);
        let source = session.terrain_vertex_source();
        let original = session
            .terrain_grids(&[face], false)
            .unwrap()
            .remove(0)
            .mesh;
        let csv = format!(
            "solid,side,vertex,z\n{},{},7,-12.5\n{},{},0,8191.25\n",
            face.solid, face.side, face.solid, face.side
        );
        let targets = parse_height_csv(&csv).unwrap();
        session
            .preview_terrain_vertex_set(Actor::User, &targets, source)
            .unwrap();
        let draft = session.terrain_grids(&[face], true).unwrap().remove(0).mesh;
        assert_eq!(draft.positions[7], original.positions[7].with_z(-12.5));
        assert_eq!(draft.positions[0], original.positions[0].with_z(8191.25));
        let preview = session.face_preview_token();
        for invalid in [
            "solid,side,vertex,z\n".to_owned(),
            csv.replace(",7,", ",7.5,"),
            csv.replace("-12.5", "NaN"),
            csv.replace("-12.5", "65537"),
            csv.replace("-12.5", ""),
            csv.replace("-12.5", "1,2"),
            csv.replace(",0,", ",7,"),
            format!("{csv}\n"),
            "x".repeat(MAX_HEIGHT_CSV_BYTES + 1),
        ] {
            assert!(parse_height_csv(&invalid).is_err());
            assert_eq!(session.face_preview_token(), preview);
            assert_eq!(session.terrain_grids(&[face], true).unwrap()[0].mesh, draft);
        }
    }

    #[test]
    fn absolute_height_batches_keep_explicit_matching_borders_across_rotated_native_grids() {
        use map_core::displacement::{VertexSet, VertexValue};
        let (mut session, first) = fixture(2, 0);
        let ObjectId::Solid(solid) = session
            .create_block(
                Actor::User,
                DVec3::new(64.0, 0.0, 0.0),
                DVec3::new(128.0, 64.0, 16.0),
                "test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let second = FaceId {
            solid,
            side: session
                .document()
                .solid(solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id,
        };
        session
            .create_terrain(Actor::User, &[second], 2, 2)
            .unwrap();
        let source = session.terrain_vertex_source();
        let grids = session.terrain_grids(&[first, second], false).unwrap();
        let targets = grids
            .iter()
            .map(|grid| VertexSetFace {
                face: grid.face,
                vertices: grid
                    .mesh
                    .positions
                    .iter()
                    .enumerate()
                    .map(|(vertex, point)| VertexSet {
                        vertex,
                        value: VertexValue::WorldZ(8192.0 + point.x * 0.25 + point.y * 0.5),
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        session
            .preview_terrain_vertex_set(Actor::Agent, &targets, source)
            .unwrap();
        let raw = session.terrain_grids(&[first, second], true).unwrap();
        let displayed = session
            .terrain_surface_grids(&[first, second], true)
            .unwrap();
        for (raw, displayed) in raw.iter().zip(&displayed) {
            assert_eq!(raw.mesh, displayed.mesh);
        }
        let mut shared = 0;
        for point in raw[0].mesh.positions.iter().filter(|point| point.x == 64.0) {
            assert!(raw[1].mesh.positions.contains(point));
            shared += 1;
        }
        assert_eq!(shared, 5);
    }

    #[test]
    fn terrain_vertex_subsets_share_single_sample_writes_rotations_and_owned_guards() {
        for power in 2..=4 {
            for corner in 0..4 {
                for actor in [Actor::User, Actor::Agent] {
                    let (mut session, face) = fixture(power, corner);
                    let source = session.document().vmf().clone();
                    let before = session
                        .terrain_grids(&[face], false)
                        .unwrap()
                        .remove(0)
                        .mesh;
                    let id = before.size + 2;
                    let point = before.positions[id];
                    let token = session
                        .highlight_terrain_vertices(actor, &[(face, vec![id, id])])
                        .unwrap();
                    let state = session.terrain_triangle_highlight_json();
                    assert_eq!(state["kind"], "vertices");
                    assert_eq!(
                        session
                            .terrain_vertex_mask(&[face], Some(token))
                            .unwrap()
                            .faces[0]
                            .vertices,
                        [id]
                    );
                    assert!(
                        session
                            .highlight_terrain_vertices(
                                actor,
                                &[(face, vec![before.positions.len()])]
                            )
                            .is_err()
                    );
                    assert_eq!(session.terrain_triangle_highlight_json(), state);
                    assert_eq!(
                        session
                            .build_scene()
                            .lines
                            .iter()
                            .filter(|line| line.color == [85, 225, 255, 255])
                            .count(),
                        6
                    );
                    let hit = session
                        .pick_terrain_vertex(
                            &Ray {
                                origin: point + DVec3::new(0.01, 0.01, 128.0),
                                direction: -DVec3::Z * 3.0,
                            },
                            &[face],
                        )
                        .unwrap()
                        .unwrap();
                    assert_eq!(hit.vertex.vertex, id);
                    assert_eq!(hit.position, point);
                    session
                        .frame_terrain_vertices(actor, &[(face, vec![id])], 1.5)
                        .unwrap();
                    assert!(session.camera().position.is_finite());
                    let brush = Brush {
                        center: point,
                        radius: 8192.0,
                        axis: Axis::WorldZ,
                    };
                    session
                        .preview_terrain_masked_edit(
                            actor,
                            &[face],
                            Operation::Sculpt {
                                brush,
                                operation: Sculpt::Raise(16.0),
                            },
                            token,
                        )
                        .unwrap();
                    let draft = session.terrain_grids(&[face], true).unwrap().remove(0).mesh;
                    for (index, (a, b)) in before.positions.iter().zip(&draft.positions).enumerate()
                    {
                        assert_eq!(
                            *b,
                            if index == id {
                                *a + DVec3::Z * 16.0
                            } else {
                                *a
                            }
                        );
                    }
                    assert_eq!(session.document().vmf(), &source);
                    session.commit_face_preview(actor).unwrap();
                    assert_eq!(
                        session
                            .terrain_vertex_mask(&[face], Some(token))
                            .unwrap()
                            .faces[0]
                            .vertices,
                        [id]
                    );
                    session
                        .preview_terrain_masked_edit(
                            actor,
                            &[face],
                            Operation::Blend {
                                brush,
                                operation: Blend::Fill(200.0),
                            },
                            token,
                        )
                        .unwrap();
                    let draft_token = session.face_preview_token();
                    let replaced = session
                        .highlight_terrain_triangles(actor, &[(face, vec![0])])
                        .unwrap();
                    assert!(
                        session
                            .preview_terrain_masked_edit(
                                actor,
                                &[face],
                                Operation::Blend {
                                    brush,
                                    operation: Blend::Invert
                                },
                                token
                            )
                            .is_err()
                    );
                    assert_eq!(session.face_preview_token(), draft_token);
                    assert!(
                        session
                            .clear_terrain_triangle_highlight(Some(token))
                            .is_err()
                    );
                    assert_eq!(
                        session.terrain_triangle_highlight_json()["token"],
                        replaced.to_string()
                    );
                    session.cancel_face_preview();
                    let token = session
                        .highlight_terrain_vertices(actor, &[(face, vec![id])])
                        .unwrap();
                    session
                        .preview_terrain_resolution(actor, &[face], if power == 4 { 3 } else { 4 })
                        .unwrap();
                    assert!(session.terrain_vertex_mask(&[face], Some(token)).is_err());
                    assert_eq!(session.terrain_triangle_highlight_json()["stale"], true);
                    assert!(
                        session
                            .build_scene()
                            .lines
                            .iter()
                            .all(|line| line.color != [85, 225, 255, 255])
                    );
                    let camera = session.camera();
                    assert!(
                        session
                            .frame_terrain_vertices(actor, &[(face, vec![id])], 1.5)
                            .is_err()
                    );
                    assert_eq!(session.camera(), camera);
                    session.cancel_face_preview();
                    session.undo(actor).unwrap();
                    assert_eq!(session.document().vmf(), &source);
                    assert!(session.terrain_vertex_mask(&[face], Some(token)).is_err());
                }
            }
        }
    }

    #[test]
    fn terrain_vertex_volume_queries_select_points_and_preserve_stale_replacements() {
        let (mut session, face) = fixture(3, 2);
        let source = session.document().vmf().clone();
        let point = session.terrain_grids(&[face], false).unwrap()[0]
            .mesh
            .positions[40];
        let history = session.document().undo_labels().count();
        let spec = Spec {
            mins: point - DVec3::splat(0.1),
            maxs: point + DVec3::splat(0.1),
            rule: map_core::region::Match::Contained,
            mode: SelectMode::Replace,
            faces: true,
        };
        let first = session.preview_volume_selection(spec).unwrap();
        assert_eq!(
            session.terrain_vertices_in_volume(&[face], first).unwrap(),
            [(face, vec![40])]
        );
        let next = session
            .preview_volume_selection(Spec {
                rule: map_core::region::Match::Intersecting,
                ..spec
            })
            .unwrap();
        let selected = session
            .select_terrain_vertices_in_volume(Actor::User, &[face], next)
            .unwrap();
        assert_eq!(selected.targets, [(face, vec![40])]);
        assert_eq!(
            session
                .terrain_vertex_mask(&[face], Some(selected.token))
                .unwrap()
                .faces[0]
                .vertices,
            [40]
        );
        let state = session.terrain_triangle_highlight_json();
        assert!(
            session
                .select_terrain_vertices_in_volume(Actor::Agent, &[face], first)
                .is_err()
        );
        assert_eq!(session.terrain_triangle_highlight_json(), state);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
    }

    #[test]
    fn terrain_vertex_moves_share_exact_world_offsets_masks_history_and_foreign_guards() {
        for power in 2..=4 {
            for corner in 0..4 {
                for actor in [Actor::User, Actor::Agent] {
                    let (mut session, face) = fixture(power, corner);
                    let source = session.document().vmf().clone();
                    let mesh = session
                        .terrain_grids(&[face], false)
                        .unwrap()
                        .remove(0)
                        .mesh;
                    let id = mesh.size + 2;
                    let token = session
                        .highlight_terrain_vertices(actor, &[(face, vec![id])])
                        .unwrap();
                    assert_eq!(
                        session
                            .preview_terrain_vertex_move(actor, &[face], DVec3::ZERO, token)
                            .unwrap(),
                        0
                    );
                    assert_eq!(session.render_document().vmf(), &source);
                    let offset = DVec3::new(1.5, -2.0, 8.0);
                    session
                        .preview_terrain_vertex_move(actor, &[face], offset, token)
                        .unwrap();
                    let draft = session.terrain_grids(&[face], true).unwrap().remove(0).mesh;
                    for (index, (a, b)) in mesh.positions.iter().zip(&draft.positions).enumerate() {
                        assert_eq!(*b, *a + if index == id { offset } else { DVec3::ZERO });
                    }
                    let preview = session.face_preview_token();
                    assert!(
                        session
                            .preview_terrain_vertex_move(actor, &[face], DVec3::X * 4097.0, token)
                            .is_err()
                    );
                    assert_eq!(session.face_preview_token(), preview);
                    assert_eq!(session.document().vmf(), &source);
                    session.commit_face_preview(actor).unwrap();
                    assert!(session.terrain_vertex_mask(&[face], Some(token)).is_ok());
                    session
                        .preview_terrain_vertex_move(actor, &[face], offset, token)
                        .unwrap();
                    let preview = session.face_preview_token();
                    let replaced = session
                        .highlight_terrain_triangles(actor, &[(face, vec![0])])
                        .unwrap();
                    assert!(
                        session
                            .preview_terrain_vertex_move(actor, &[face], offset, token)
                            .is_err()
                    );
                    assert_eq!(session.face_preview_token(), preview);
                    assert_eq!(
                        session.terrain_triangle_highlight_json()["token"],
                        replaced.to_string()
                    );
                    session.cancel_face_preview();
                    session.undo(actor).unwrap();
                    assert_eq!(session.document().vmf(), &source);
                }
            }
        }
    }
}
