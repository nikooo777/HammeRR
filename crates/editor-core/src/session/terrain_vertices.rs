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
