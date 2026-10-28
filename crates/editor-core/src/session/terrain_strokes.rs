use glam::DVec3;
use map_core::{
    displacement::{
        Operation,
        stroke::{self, Path, Spec},
    },
    query::Ray,
    surface::{self, FaceId, SurfaceEdit},
};
use serde_json::{Value, json};

use super::{Actor, Session, Snapshot};

#[derive(Default)]
pub struct State {
    serial: u64,
    stroke: Option<Stroke>,
}

struct Stroke {
    token: u64,
    snapshot: Snapshot,
    faces: Vec<FaceId>,
    spec: Spec,
    path: Path,
    mask_token: Option<u64>,
    preview_token: Option<u64>,
    vertices: usize,
    axis: DVec3,
    changed: usize,
}

impl Session {
    pub(super) fn terrain_stroke_active(&self) -> bool {
        self.terrain_strokes.stroke.is_some()
    }

    pub(super) fn working_region_end_terrain_stroke(&mut self) {
        self.terrain_strokes.stroke = None;
    }
    pub fn check_terrain_stroke_token(&self, token: u64) -> Result<(), String> {
        self.checked_terrain_stroke(token).map(|_| ())
    }
    fn checked_terrain_stroke(&self, token: u64) -> Result<&Stroke, String> {
        let stroke = self
            .terrain_strokes
            .stroke
            .as_ref()
            .filter(|stroke| stroke.token == token)
            .ok_or("Terrain stroke was closed or replaced")?;
        if !self.current_snapshot(&stroke.snapshot) {
            return Err("Map changed; start a new terrain stroke".into());
        }
        if self.face_preview_token() != stroke.preview_token {
            return Err("Terrain stroke preview was replaced; cancel or start a new stroke".into());
        }
        if let Some(token) = stroke.mask_token {
            self.terrain_vertex_mask(&stroke.faces, Some(token))?;
        }
        Ok(stroke)
    }

    pub fn begin_terrain_stroke(
        &mut self,
        actor: Actor,
        faces: &[FaceId],
        operation: Operation,
        mask_token: Option<u64>,
    ) -> Result<u64, String> {
        if !std::ptr::eq(self.render_document(), &self.document) {
            return Err(
                "Apply or cancel the active geometry preview before starting a terrain stroke"
                    .into(),
            );
        }
        let spec = Spec::from_operation(&operation).map_err(|error| error.to_string())?;
        let grids = self.terrain_grids(faces, false)?;
        let mask = mask_token
            .map(|token| self.terrain_vertex_mask(faces, Some(token)))
            .transpose()?;
        let mut probe = self.document.preview_snapshot();
        probe
            .edit("check terrain stroke", |tx| {
                if let Some(mask) = &mask {
                    for face in &mask.faces {
                        surface::edit_faces(
                            tx,
                            &[face.face],
                            &SurfaceEdit {
                                terrain: Some(Operation::Masked {
                                    operation: Box::new(operation.clone()),
                                    vertices: face.vertices.clone(),
                                }),
                                ..Default::default()
                            },
                        )?;
                    }
                } else {
                    surface::edit_faces(
                        tx,
                        faces,
                        &SurfaceEdit {
                            terrain: Some(operation.clone()),
                            ..Default::default()
                        },
                    )?;
                }
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        self.terrain_strokes.serial = self.terrain_strokes.serial.wrapping_add(1);
        let token = self.terrain_strokes.serial;
        self.terrain_strokes.stroke = Some(Stroke {
            token,
            snapshot: Snapshot {
                generation: self.document_generation(),
                revision: self.document.revision(),
            },
            faces: grids.iter().map(|grid| grid.face).collect(),
            vertices: grids.iter().map(|grid| grid.mesh.positions.len()).sum(),
            axis: match spec.brush.axis {
                map_core::displacement::brush::Axis::WorldZ => DVec3::Z,
                map_core::displacement::brush::Axis::FaceNormal => {
                    let mesh = &grids[0].mesh;
                    (mesh.base_positions[1] - mesh.base_positions[0])
                        .cross(mesh.base_positions[mesh.size] - mesh.base_positions[0])
                        .normalize_or_zero()
                }
            },
            spec,
            changed: 0,
            path: Path::default(),
            mask_token,
            preview_token: None,
        });
        self.scene_serial += 1;
        self.log(actor, "started terrain brush stroke");
        Ok(token)
    }

    pub fn update_terrain_stroke(
        &mut self,
        actor: Actor,
        token: u64,
        points: &[DVec3],
    ) -> Result<usize, String> {
        let stroke = self.checked_terrain_stroke(token)?;
        let path = stroke
            .path
            .append(points, stroke.spec.spacing(), stroke.axis)
            .map_err(|error| error.to_string())?;
        if path.points.len() * stroke.vertices > stroke::MAX_WORK {
            return Err("Terrain stroke exceeds its two-million vertex/sample work limit".into());
        }
        if path.points.len() == stroke.path.points.len() && stroke.preview_token.is_some() {
            let changed = stroke.changed;
            self.terrain_strokes.stroke.as_mut().unwrap().path = path;
            self.scene_serial += 1;
            return Ok(changed);
        }
        let faces = stroke.faces.clone();
        let mask_token = stroke.mask_token;
        let operation = Operation::Stroke {
            spec: stroke.spec,
            points: path.points.clone(),
        };
        let changed = if let Some(mask) = mask_token {
            self.preview_terrain_masked_edit(actor, &faces, operation, mask)?
        } else {
            self.preview_terrain_edit(actor, &faces, operation)?
        };
        let preview_token = self.face_preview_token();
        let stroke = self.terrain_strokes.stroke.as_mut().unwrap();
        stroke.path = path;
        stroke.preview_token = preview_token;
        stroke.changed = changed;
        Ok(changed)
    }

    pub fn update_terrain_stroke_at_ray(
        &mut self,
        actor: Actor,
        token: u64,
        ray: &Ray,
    ) -> Result<usize, String> {
        let faces = self.checked_terrain_stroke(token)?.faces.clone();
        let Some(hit) = self.pick_terrain_triangle(ray, &faces)? else {
            self.interrupt_terrain_stroke(token)?;
            return Err(
                "No visible selected terrain; the next hit starts a new stroke segment".into(),
            );
        };
        let point = hit.point;
        self.update_terrain_stroke(actor, token, &[point])
    }

    pub fn interrupt_terrain_stroke(&mut self, token: u64) -> Result<(), String> {
        self.checked_terrain_stroke(token)?;
        self.terrain_strokes
            .stroke
            .as_mut()
            .unwrap()
            .path
            .interrupt();
        self.scene_serial += 1;
        Ok(())
    }

    pub fn finish_terrain_stroke(&mut self, actor: Actor, token: u64) -> Result<u64, String> {
        let stroke = self.checked_terrain_stroke(token)?;
        let preview = stroke
            .preview_token
            .ok_or("Draw on terrain before finishing the stroke")?;
        self.terrain_strokes.stroke = None;
        self.scene_serial += 1;
        self.log(actor, "finished terrain brush stroke preview");
        Ok(preview)
    }

    pub fn cancel_terrain_stroke(&mut self, token: u64) -> Result<(), String> {
        let stroke = self
            .terrain_strokes
            .stroke
            .as_ref()
            .filter(|stroke| stroke.token == token)
            .ok_or("Terrain stroke was closed or replaced")?;
        let preview = stroke.preview_token;
        if preview.is_some() && preview == self.face_preview_token() {
            self.cancel_face_preview();
        }
        self.terrain_strokes.stroke = None;
        self.scene_serial += 1;
        Ok(())
    }

    pub fn terrain_stroke_json(&self) -> Value {
        match &self.terrain_strokes.stroke {
            None => json!({"handle":null,"token":null,"active":false}),
            Some(stroke) => {
                json!({"handle":self.terrain_stroke_handle(),"token":stroke.token.to_string(),"active":true,"stale":self.checked_terrain_stroke(stroke.token).is_err(),"mode":stroke.spec.name(),"radius":stroke.spec.brush.radius,"spacing":stroke.spec.spacing(),"sampling_normal":stroke.axis.to_array(),"samples":stroke.path.points.len(),"input_points":stroke.path.input_points,"cursor":stroke.path.cursor.map(|point|point.to_array()),"preview_token":stroke.preview_token.map(|token|token.to_string()),"mask_token":stroke.mask_token.map(|token|token.to_string()),"max_samples":stroke::MAX_DABS,"max_input_points":stroke::MAX_INPUT_POINTS,"max_update_points":stroke::MAX_UPDATE_POINTS,"max_work":stroke::MAX_WORK})
            }
        }
    }

    pub fn terrain_stroke_handle(&self) -> Value {
        Self::edit_handle(
            "terrain_stroke",
            self.terrain_strokes.stroke.as_ref().and_then(|stroke| {
                self.checked_terrain_stroke(stroke.token)
                    .is_ok()
                    .then_some(stroke.token)
            }),
        )
    }

    pub(super) fn append_terrain_stroke_guides(&self, mesh: &mut render::SceneMesh) {
        let Some(stroke) = &self.terrain_strokes.stroke else {
            return;
        };
        if self.checked_terrain_stroke(stroke.token).is_err() {
            return;
        }
        let Some(point) = stroke.path.cursor else {
            return;
        };
        if stroke
            .faces
            .iter()
            .any(|face| !self.working_solid_visible(self.render_document(), face.solid))
        {
            return;
        }
        let normal = stroke.axis;
        let u = normal
            .cross(if normal.z.abs() > 0.9 {
                DVec3::Y
            } else {
                DVec3::Z
            })
            .normalize_or_zero();
        let v = normal.cross(u);
        let segments = 64;
        for index in 0..segments {
            for endpoint in [index, index + 1] {
                let angle = std::f64::consts::TAU * endpoint as f64 / segments as f64;
                let p = point
                    + normal * 0.15
                    + (u * angle.cos() + v * angle.sin()) * stroke.spec.brush.radius;
                mesh.lines.push(render::scene::LineVertex {
                    position: p.as_vec3().to_array(),
                    color: [240, 245, 255, 255],
                });
            }
        }
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.stroke
            .as_ref()
            .map(|stroke| serde_json::json!([self.serial, stroke.token]))
    }
}
