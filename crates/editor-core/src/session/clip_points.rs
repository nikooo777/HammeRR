use glam::DVec3;
use map_core::clip::Keep;
use map_core::{Plane, query};
use render::Camera;
use serde_json::{Value, json};

use super::{Actor, Pointer, Session};

#[derive(Clone)]
pub struct Draft {
    pub points: Vec<DVec3>,
    pub keep: Keep,
    pub material: Option<String>,
    pub snap: bool,
}

impl Session {
    pub fn clip_points(&self) -> Option<&Draft> {
        self.clip_points.as_ref()
    }

    pub fn clip_point_stamp(&self) -> String {
        format!(
            "{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.clip_epoch
        )
    }

    pub fn clip_points_report(&self) -> Option<Value> {
        self.clip_points.as_ref().map(|draft| json!({
            "points":draft.points.iter().map(|p|p.to_array()).collect::<Vec<_>>(),
            "snap":draft.snap,
            "keep":match draft.keep { Keep::Front => "front", Keep::Back => "back", Keep::Both => "both" },
            "material":draft.material,
        }))
    }

    pub fn set_clip_points(
        &mut self,
        actor: Actor,
        mut draft: Draft,
        stamp: &str,
    ) -> Result<(), String> {
        if stamp != self.clip_point_stamp() {
            return Err("Cut, view or map changed; inspect the clipping state again".into());
        }
        if !self.can_clip_selection() {
            return Err("Select brushes or brush entities before choosing cut points".into());
        }
        if draft.points.len() > 3
            || draft
                .points
                .iter()
                .any(|p| !p.is_finite() || p.abs().max_element() > 65536.0)
        {
            return Err("Choose up to three finite points within ±65536 world units".into());
        }
        if draft.snap {
            for point in &mut draft.points {
                if !self.clip_points.as_ref().is_some_and(|previous| {
                    previous.snap == draft.snap && previous.points.contains(point)
                }) {
                    *point = self.spatial.snap_point(*point);
                }
            }
        }
        for (index, point) in draft.points.iter().enumerate() {
            if draft.points[..index]
                .iter()
                .any(|p| p.distance(*point) < 0.002)
            {
                return Err("Cut points must be distinct after snapping".into());
            }
        }
        if draft.points.len() == 3 {
            let plane = Plane::from_points([draft.points[0], draft.points[1], draft.points[2]])
                .ok_or("The three cut points must not lie on one line")?;
            self.clip_brushes(
                actor,
                &self.selected_ids(),
                plane,
                draft.keep,
                draft.material.clone(),
                true,
            )?;
        } else {
            let camera = self.camera;
            self.set_clip_tool(actor, true)?;
            if camera != self.camera {
                self.set_camera(camera);
            }
            self.cancel_clip_preview();
        }
        self.clip_drag = None;
        self.clip_points = Some(draft);
        self.clip_epoch += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    pub fn pick_clip_point(
        &mut self,
        actor: Actor,
        camera: Camera,
        pointer: Pointer,
        stamp: &str,
    ) -> Result<DVec3, String> {
        if stamp != self.clip_point_stamp() {
            return Err("Cut, view or map changed; inspect the clipping state again".into());
        }
        pointer.validate()?;
        let mut draft = self
            .clip_points
            .clone()
            .ok_or("Start three-point clipping first")?;
        if draft.points.len() == 3 {
            return Err("Three points are already set; remove a point or start again".into());
        }
        let ndc = pointer.ndc();
        let (origin, direction) = camera.ray(
            ndc.x as f32,
            ndc.y as f32,
            pointer.size.0 as f32 / pointer.size.1 as f32,
        );
        let hidden = self.hidden_brushes_for(false);
        let hit = query::pick_face(
            &self.document,
            &query::Ray {
                origin: origin.as_dvec3(),
                direction: direction.as_dvec3(),
            },
            |solid| !hidden.contains(&solid.id),
        )
        .ok_or("Choose a visible brush surface for the cut point")?;
        draft.points.push(hit.point);
        self.set_clip_points(actor, draft, stamp)?;
        Ok(*self.clip_points.as_ref().unwrap().points.last().unwrap())
    }

    pub fn append_clip_points(&self, mesh: &mut render::SceneMesh) {
        if !self.working_selection_visible() {
            return;
        }
        let Some(draft) = &self.clip_points else {
            return;
        };
        let mut segments = Vec::new();
        for point in &draft.points {
            for axis in DVec3::AXES {
                segments.push([*point - axis * 3.0, *point + axis * 3.0]);
            }
        }
        segments.extend(draft.points.windows(2).map(|pair| [pair[0], pair[1]]));
        if draft.points.len() == 3 {
            segments.push([draft.points[2], draft.points[0]]);
        }
        for segment in segments {
            for point in segment {
                mesh.lines.push(render::scene::LineVertex {
                    position: point.as_vec3().to_array(),
                    color: [255, 190, 65, 255],
                });
            }
        }
    }
}
