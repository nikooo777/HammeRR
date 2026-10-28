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
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) token: u64,
    pub(super) powers: BTreeMap<FaceId, u32>,
    selected: BTreeSet<TriangleRef>,
    pub(super) vertices: Option<BTreeSet<super::terrain_vertices::VertexRef>>,
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
        Ok(self.install_terrain_highlight(actor, powers, selected, None))
    }

    pub(super) fn install_terrain_highlight(
        &mut self,
        actor: Actor,
        powers: BTreeMap<FaceId, u32>,
        selected: BTreeSet<TriangleRef>,
        vertices: Option<BTreeSet<super::terrain_vertices::VertexRef>>,
    ) -> u64 {
        self.terrain_triangle_serial = self.terrain_triangle_serial.wrapping_add(1);
        let token = self.terrain_triangle_serial;
        self.terrain_triangle_highlight = Some(Highlight {
            generation: self.document_generation(),
            revision: self.document.revision(),
            token,
            powers,
            selected,
            vertices,
        });
        self.scene_serial += 1;
        self.log(actor, "highlighted terrain selection");
        token
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

    pub(super) fn terrain_highlight_stale(&self, highlight: &Highlight) -> bool {
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
            None => json!({"token":null,"stale":false,"kind":null,"triangles":[],"vertices":[]}),
            Some(highlight) => {
                json!({"token":highlight.token.to_string(),"stale":self.terrain_highlight_stale(highlight),"kind":if highlight.vertices.is_some() {"vertices"} else {"triangles"},"triangles":highlight.selected.iter().map(|selected|json!({"solid":selected.face.solid,"side":selected.face.side,"triangle":selected.triangle})).collect::<Vec<_>>(),"vertices":highlight.vertices.iter().flatten().map(|selected|json!({"solid":selected.face.solid,"side":selected.face.side,"vertex":selected.vertex})).collect::<Vec<_>>()})
            }
        }
    }

    pub(super) fn hidden_terrain_tool_faces(&mut self) -> HashSet<FaceId> {
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
        if self.display_visibility_filtered() {
            let document = self.render_document();
            hidden.extend(
                document
                    .vmf()
                    .solids()
                    .filter(|solid| !self.working_solid_visible(document, solid.id))
                    .flat_map(|solid| {
                        solid.sides.iter().map(move |side| FaceId {
                            solid: solid.id,
                            side: side.id,
                        })
                    }),
            );
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
        let grids = self.terrain_surface_grids(faces, true)?;
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
        for row in self.terrain_surface_triangles(&faces, true)? {
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
        if self
            .terrain_triangle_highlight
            .as_ref()
            .is_some_and(|highlight| highlight.vertices.is_some())
        {
            self.append_terrain_vertex_guides(mesh);
            return;
        }
        let Some(highlight) = &self.terrain_triangle_highlight else {
            return;
        };
        if self.terrain_highlight_stale(highlight) || highlight.selected.is_empty() {
            return;
        }
        let selected = highlight.selected.clone();
        let faces = highlight.powers.keys().copied().collect::<Vec<_>>();
        let hidden = self.hidden_terrain_tool_faces();
        let Ok(rows) = self.terrain_surface_triangles(&faces, true) else {
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
        let count = self
            .faces
            .iter()
            .map(|face| face.vertices.len())
            .sum::<usize>();
        format!(
            "{count} {} on {} surface{}",
            if count == 1 { "vertex" } else { "vertices" },
            self.faces.len(),
            if self.faces.len() == 1 { "" } else { "s" }
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
            .ok_or("Select terrain triangles or vertices before using a vertex mask")?;
        if token.is_some_and(|token| token != highlight.token) {
            return Err("Terrain vertex mask was replaced; inspect the current selection".into());
        }
        if self.terrain_highlight_stale(highlight) {
            return Err("Terrain vertex mask is stale; select terrain components again".into());
        }
        let faces = faces.iter().copied().collect::<BTreeSet<_>>();
        if let Some(vertices) = &highlight.vertices {
            let mut selected = BTreeMap::<FaceId, Vec<usize>>::new();
            for vertex in vertices {
                if faces.contains(&vertex.face) {
                    selected.entry(vertex.face).or_default().push(vertex.vertex);
                }
            }
            if selected.is_empty() {
                return Err("No highlighted vertices on these terrain faces".into());
            }
            return Ok(VertexMask {
                token: highlight.token,
                faces: selected
                    .into_iter()
                    .map(|(face, vertices)| VertexMaskFace {
                        face,
                        vertices,
                        triangles: 0,
                    })
                    .collect(),
            });
        }
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
                | map_core::displacement::Operation::Stroke { .. }
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
        self.geometry
            .face_preview
            .as_mut()
            .unwrap()
            .terrain_mask_token = Some(token);
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

pub struct VolumeSelection {
    pub token: u64,
    pub targets: Vec<(FaceId, Vec<usize>)>,
}

impl Session {
    pub fn select_terrain_triangles_in_volume(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        token: u64,
    ) -> Result<VolumeSelection, String> {
        let targets = self.terrain_triangles_in_volume(faces, token)?;
        let token = self.highlight_terrain_triangles(actor, &targets)?;
        Ok(VolumeSelection { token, targets })
    }

    pub fn terrain_triangles_in_volume(
        &mut self,
        faces: &[FaceId],
        token: u64,
    ) -> Result<Vec<(FaceId, Vec<usize>)>, String> {
        let spec = self.checked_volume(token)?.spec;
        let region = map_core::region::Region::from_bounds(spec.mins, spec.maxs)?;
        let rows = self.terrain_surface_triangles(faces, true)?;
        let hidden = self.hidden_terrain_tool_faces();
        let mut targets = Vec::new();
        for row in rows {
            if self.component_hidden(row.face.solid, &HashSet::new()) || hidden.contains(&row.face)
            {
                continue;
            }
            let triangles = row
                .triangles
                .iter()
                .filter(|triangle| region.polygon_matches(&triangle.positions, spec.rule))
                .map(|triangle| triangle.id)
                .collect::<Vec<_>>();
            if !triangles.is_empty() {
                targets.push((row.face, triangles));
            }
        }
        Ok(targets)
    }
}
