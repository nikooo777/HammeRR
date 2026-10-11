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
            draft
                .points
                .iter_mut()
                .for_each(|p| *p = self.spatial.snap_point(*p));
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
        let hidden = self.hidden_tool_brushes();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interaction::Interaction;
    use glam::{DVec2, Vec3};
    use render::Projection;

    fn fixture() -> Session {
        let mut session = Session::new(Err("no assets".into()));
        session
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        session.document.mark_saved();
        session
    }

    fn draft(points: Vec<DVec3>) -> Draft {
        Draft {
            points,
            keep: Keep::Both,
            material: Some("tools/toolsnodraw".into()),
            snap: true,
        }
    }

    #[test]
    fn three_points_preview_exact_cut_reject_bad_planes_and_preserve_history() {
        let mut session = fixture();
        let original = session.document.vmf().clone();
        let camera = session.camera();
        session
            .set_clip_points(Actor::Agent, draft(vec![]), &session.clip_point_stamp())
            .unwrap();
        assert_eq!(session.camera(), camera);
        let points = vec![
            DVec3::new(32.0, 0.0, 0.0),
            DVec3::new(32.0, 64.0, 0.0),
            DVec3::new(32.0, 64.0, 64.0),
        ];
        let stale = session.clip_point_stamp();
        session
            .set_clip_points(Actor::Agent, draft(points.clone()), &stale)
            .unwrap();
        let valid = session.render_document().vmf().clone();
        assert_eq!(session.document.vmf(), &original);
        assert_eq!(session.clip_preview().unwrap().result.cuts.len(), 1);
        assert!(
            session
                .set_clip_points(Actor::Agent, draft(vec![]), &stale)
                .is_err()
        );
        for bad in [
            vec![DVec3::ZERO, DVec3::X],
            vec![DVec3::ZERO, DVec3::X * 32.0, DVec3::X * 64.0],
            vec![DVec3::splat(f64::NAN)],
            vec![DVec3::ZERO; 4],
        ] {
            assert!(
                session
                    .set_clip_points(Actor::Agent, draft(bad), &session.clip_point_stamp())
                    .is_err()
            );
            assert_eq!(session.render_document().vmf(), &valid);
        }
        session
            .set_clip_points(
                Actor::Agent,
                draft(points[..2].to_vec()),
                &session.clip_point_stamp(),
            )
            .unwrap();
        assert!(session.clip_preview().is_none());
        assert_eq!(session.render_document().vmf(), &original);
        session
            .set_clip_points(Actor::Agent, draft(points), &session.clip_point_stamp())
            .unwrap();
        let plan = session.clip_preview().unwrap().result.clone();
        session.commit_clip_preview(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &valid);
        assert_eq!(session.document.vmf().world.solids.len(), 2);
        assert!(session.clip_points().is_none());
        assert_eq!(plan.cuts.len(), 1);
        session.undo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &original);
        session.redo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &valid);
    }

    #[test]
    fn viewport_cut_points_match_surface_picking_and_ignore_stale_clicks() {
        let mut session = fixture();
        let camera = Camera::planar(Projection::Top, Vec3::splat(32.0), 128.0);
        session.set_camera(camera);
        session
            .set_clip_points(Actor::Agent, draft(vec![]), &session.clip_point_stamp())
            .unwrap();
        let mut interaction = Interaction::default();
        let pointer = Pointer {
            position: DVec2::new(400.0, 400.0),
            size: (800, 800),
        };
        interaction.down(&mut session, pointer, false).unwrap();
        interaction.up(&mut session, pointer).unwrap();
        assert_eq!(
            session.clip_points().unwrap().points,
            vec![DVec3::new(32.0, 32.0, 64.0)]
        );
        for action in 0..4 {
            interaction.down(&mut session, pointer, false).unwrap();
            let mut end = pointer;
            match action {
                0 => {
                    let mut c = session.camera();
                    c.position.x += 1.0;
                    session.set_camera(c);
                }
                1 => {
                    end.size.0 += 1;
                }
                2 => {
                    end.position.x += 10.0;
                }
                _ => {
                    interaction.cancel(&mut session);
                }
            }
            interaction.up(&mut session, end).unwrap();
            assert_eq!(session.clip_points().unwrap().points.len(), 1);
        }
        let stamp = session.clip_point_stamp();
        session.cancel_clip_preview();
        assert!(session.clip_points().is_none());
        assert!(
            session
                .pick_clip_point(Actor::Agent, camera, pointer, &stamp)
                .is_err()
        );
    }
}
