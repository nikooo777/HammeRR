use map_core::query::Ray;
use render::Camera;

use crate::{
    session::{Actor, Pointer, Session},
    viewport::CLICK_SLOP,
};

struct Press {
    pointer: Pointer,
    camera: Camera,
    generation: u64,
    revision: u64,
    scene: u64,
    view: u64,
    filters: u64,
    dragged: bool,
}

#[derive(Default)]
pub(super) struct Gesture {
    press: Option<Press>,
    awaiting_release: bool,
}

impl Gesture {
    pub fn begin(
        &mut self,
        session: &Session,
        camera: Camera,
        pointer: Pointer,
        control: bool,
        alt: bool,
    ) -> Result<bool, String> {
        *self = Self::default();
        if !alt
            || control
            || session.face_mode()
            || session.component_picking().is_some()
            || !session.component_selection().is_empty()
            || session.paint_enabled()
            || session.clip_tool()
            || session.spatial().draw_block
            || session.partition_preview_token().is_some()
        {
            return Ok(false);
        }
        self.awaiting_release = true;
        pointer.validate()?;
        if pointer.position.min_element() < 0.0
            || pointer.position.x >= f64::from(pointer.size.0)
            || pointer.position.y >= f64::from(pointer.size.1)
        {
            return Err("Click inside the viewport to cycle overlapping objects".into());
        }
        self.press = Some(Press {
            pointer,
            camera,
            generation: session.document_generation(),
            revision: session.document().revision(),
            scene: session.scene_serial(),
            view: session.view_serial(),
            filters: session.selection_filter_revision(),
            dragged: false,
        });
        Ok(true)
    }

    pub fn active(&self) -> bool {
        self.awaiting_release
    }

    pub fn moved(&mut self, pointer: Pointer) -> bool {
        if let Some(press) = &mut self.press {
            press.dragged |= pointer.validate().is_err()
                || pointer.size != press.pointer.size
                || pointer.position.distance(press.pointer.position) > f64::from(CLICK_SLOP);
        }
        self.active()
    }

    pub fn cancel(&mut self) {
        self.press = None;
    }

    pub fn release(
        &mut self,
        session: &mut Session,
        camera: Camera,
        pointer: Pointer,
    ) -> Result<Option<String>, String> {
        self.awaiting_release = false;
        let Some(press) = self.press.take() else {
            return Ok(None);
        };
        if press.dragged
            || pointer.validate().is_err()
            || pointer.position.min_element() < 0.0
            || pointer.position.x >= f64::from(pointer.size.0)
            || pointer.position.y >= f64::from(pointer.size.1)
            || pointer.size != press.pointer.size
            || pointer.position.distance(press.pointer.position) > f64::from(CLICK_SLOP)
            || camera != press.camera
            || session.camera() != press.camera
            || session.document_generation() != press.generation
            || session.document().revision() != press.revision
            || session.scene_serial() != press.scene
            || session.view_serial() != press.view
            || session.selection_filter_revision() != press.filters
        {
            return Ok(None);
        }
        let ndc = pointer.ndc();
        let (origin, direction) = camera.ray(
            ndc.x as f32,
            ndc.y as f32,
            pointer.size.0 as f32 / pointer.size.1 as f32,
        );
        let report = session.cycle_object_hit(
            Actor::User,
            &Ray {
                origin: origin.as_dvec3(),
                direction: direction.as_dvec3(),
            },
        )?;
        let message = report["message"]
            .as_str()
            .ok_or("Missing object hit feedback")?;
        Ok(Some(message.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AppWindow,
        runtime::lock,
        session::{SelectMode, TransformTool},
    };
    use glam::{DVec2, DVec3, Vec3};
    use render::{Projection, handles::Mode};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        sync::{Arc, Mutex},
    };

    #[test]
    fn filter_revision_cancels_an_alt_press_even_when_flags_return_without_view_changes() {
        let mut session = Session::new(Err("no assets".into()));
        session
            .create_block(
                Actor::User,
                DVec3::splat(-16.0),
                DVec3::splat(16.0),
                "test",
                false,
            )
            .unwrap();
        let camera = Camera::planar(Projection::Top, Vec3::ZERO, 256.0);
        session.set_camera(camera);
        let pointer = Pointer {
            position: DVec2::splat(256.0),
            size: (512, 512),
        };
        let mut gesture = Gesture::default();
        assert!(
            gesture
                .begin(&session, camera, pointer, false, true)
                .unwrap()
        );
        let flags = session.selection_filters();
        let serials = (session.scene_serial(), session.view_serial());
        session
            .set_selection_filters(
                Actor::Agent,
                crate::session::selection_filters::SelectionFilters {
                    world_brushes: false,
                    ..flags
                },
            )
            .unwrap();
        session.set_selection_filters(Actor::User, flags).unwrap();
        assert_eq!((session.scene_serial(), session.view_serial()), serials);
        assert!(
            gesture
                .release(&mut session, camera, pointer)
                .unwrap()
                .is_none()
        );
        assert!(!gesture.active());
    }

    #[test]
    fn temporary_hide_and_show_consume_an_old_alt_press_without_cycling() {
        let mut session = Session::new(Err("no assets".into()));
        let id = session
            .create_block(
                Actor::User,
                DVec3::splat(-16.0),
                DVec3::splat(16.0),
                "test",
                false,
            )
            .unwrap();
        let camera = Camera::planar(Projection::Top, Vec3::ZERO, 256.0);
        session.set_camera(camera);
        let pointer = Pointer {
            position: DVec2::splat(256.0),
            size: (512, 512),
        };
        let mut gesture = Gesture::default();
        assert!(
            gesture
                .begin(&session, camera, pointer, false, true)
                .unwrap()
        );
        session
            .set_editor_visibility(
                Actor::Agent,
                crate::session::editor_visibility::Action::Hide(vec![id]),
                &session.working_region_stamp(),
            )
            .unwrap();
        session
            .set_editor_visibility(
                Actor::Agent,
                crate::session::editor_visibility::Action::ShowAll,
                &session.working_region_stamp(),
            )
            .unwrap();
        assert!(
            gesture
                .release(&mut session, camera, pointer)
                .unwrap()
                .is_none()
        );
        assert!(!gesture.active());
        assert_eq!(session.selected_ids(), [id]);
    }

    #[test]
    fn alt_click_cycles_on_release_and_consumes_cancelled_or_stale_gestures() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut session = Session::new(Err("No assets needed".into()));
        let back = session
            .create_block(
                Actor::Agent,
                DVec3::new(-32.0, -32.0, 0.0),
                DVec3::new(32.0, 32.0, 32.0),
                "brick/test",
                false,
            )
            .unwrap();
        let front = session
            .create_block(
                Actor::Agent,
                DVec3::new(-32.0, -32.0, 64.0),
                DVec3::new(32.0, 32.0, 96.0),
                "brick/test",
                false,
            )
            .unwrap();
        session.select(&[front], SelectMode::Replace).unwrap();
        session
            .set_transform_tool(
                Actor::User,
                TransformTool {
                    mode: Some(Mode::Move),
                    ..Default::default()
                },
            )
            .unwrap();
        let camera = Camera::planar(Projection::Top, Vec3::ZERO, 256.0);
        session.publish_camera(camera);
        let pointer = Pointer {
            position: DVec2::splat(256.0),
            size: (512, 512),
        };
        assert!(session.pick_transform_handle(pointer).is_some());
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let shared = Arc::new(Mutex::new(session));
        let gesture = Rc::new(RefCell::new(Gesture::default()));
        let normal_events = Rc::new(Cell::new(0));
        let size = Rc::new(Cell::new((512, 512)));
        let app = AppWindow::new().unwrap();
        app.on_mouse_button({
            let (shared, gesture, normal_events, size) = (
                shared.clone(),
                gesture.clone(),
                normal_events.clone(),
                size.clone(),
            );
            move |button, down, x, y, control, alt| {
                if button != 0 {
                    gesture.borrow_mut().cancel();
                    return;
                }
                let mut session = lock(&shared);
                let pointer = Pointer {
                    position: DVec2::new(f64::from(x), f64::from(y)),
                    size: size.get(),
                };
                let camera = session.camera();
                if !down && gesture.borrow().active() {
                    gesture
                        .borrow_mut()
                        .release(&mut session, camera, pointer)
                        .unwrap();
                    return;
                }
                if down
                    && gesture
                        .borrow_mut()
                        .begin(&session, camera, pointer, control, alt)
                        .unwrap()
                {
                    return;
                }
                normal_events.set(normal_events.get() + 1);
            }
        });
        app.on_mouse_moved({
            let (gesture, size) = (gesture.clone(), size.clone());
            move |x, y| {
                gesture.borrow_mut().moved(Pointer {
                    position: DVec2::new(f64::from(x), f64::from(y)),
                    size: size.get(),
                });
            }
        });
        app.on_viewport_focus_lost({
            let gesture = gesture.clone();
            move || gesture.borrow_mut().cancel()
        });
        for expected in [back, front] {
            app.invoke_mouse_button(0, true, 256.0, 256.0, false, true);
            assert_ne!(lock(&shared).selected_ids(), vec![expected]);
            assert!(lock(&shared).transform_drag().is_none());
            assert!(lock(&shared).area_preview().is_none());
            app.invoke_mouse_button(0, false, 256.0, 256.0, false, false);
            assert_eq!(lock(&shared).selected_ids(), vec![expected]);
            assert!(!gesture.borrow().active());
        }
        assert_eq!(normal_events.get(), 0);
        for case in 0..6 {
            app.invoke_mouse_button(0, true, 256.0, 256.0, false, true);
            match case {
                0 => {
                    app.invoke_mouse_moved(280.0, 256.0);
                    app.invoke_mouse_moved(256.0, 256.0);
                }
                1 => app.invoke_viewport_focus_lost(),
                2 => size.set((1024, 512)),
                3 => {
                    let mut session = lock(&shared);
                    session.select(&[back], SelectMode::Replace).unwrap();
                    session.select(&[front], SelectMode::Replace).unwrap();
                }
                4 => {
                    let mut changed = camera;
                    changed.position.x += 8.0;
                    lock(&shared).publish_camera(changed);
                }
                5 => {
                    let map_core::ObjectId::Solid(solid) = front else {
                        unreachable!()
                    };
                    lock(&shared)
                        .edit(Actor::Agent, "change source", |tx| {
                            tx.solid_mut(solid)?.sides[0].material = "brick/other".into();
                            Ok(())
                        })
                        .unwrap();
                }
                _ => unreachable!(),
            }
            app.invoke_mouse_button(0, false, 256.0, 256.0, false, false);
            assert_eq!(lock(&shared).selected_ids(), vec![front], "case {case}");
            assert!(!gesture.borrow().active());
            assert_eq!(normal_events.get(), 0);
            if case == 5 {
                lock(&shared).undo(Actor::Agent).unwrap();
            }
            size.set((512, 512));
            lock(&shared).publish_camera(camera);
        }
        app.invoke_mouse_button(0, true, 8.0, 8.0, false, true);
        app.invoke_mouse_button(0, false, 8.0, 8.0, false, true);
        assert_eq!(lock(&shared).selected_ids(), vec![front]);
        assert_eq!(normal_events.get(), 0);
        assert_eq!(lock(&shared).document().vmf(), &source);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);

        for (control, alt) in [(false, false), (true, false), (true, true)] {
            app.invoke_mouse_button(0, true, 256.0, 256.0, control, alt);
            app.invoke_mouse_button(0, false, 256.0, 256.0, control, alt);
        }
        assert_eq!(normal_events.get(), 6);
        lock(&shared).set_face_mode(true);
        app.invoke_mouse_button(0, true, 256.0, 256.0, false, true);
        app.invoke_mouse_button(0, false, 256.0, 256.0, false, true);
        assert_eq!(normal_events.get(), 8);
        assert!(!gesture.borrow().active());
        {
            let mut session = lock(&shared);
            session.set_face_mode(false);
            session.set_paint_tool(Actor::User, true, 12.0).unwrap();
        }
        app.invoke_mouse_button(0, true, 256.0, 256.0, false, true);
        app.invoke_mouse_button(0, false, 256.0, 256.0, false, true);
        assert_eq!(normal_events.get(), 10);
        assert!(lock(&shared).paint_enabled());
        {
            let mut session = lock(&shared);
            session.set_paint_tool(Actor::User, false, 12.0).unwrap();
            session.select(&[front], SelectMode::Replace).unwrap();
            let map_core::ObjectId::Solid(solid) = front else {
                unreachable!()
            };
            let stamp = session.component_stamp();
            session
                .set_component_picking(
                    Some(crate::session::component_picking::Kind::Vertex),
                    &[solid],
                    &stamp,
                )
                .unwrap();
        }
        app.invoke_mouse_button(0, true, 256.0, 256.0, false, true);
        app.invoke_mouse_button(0, false, 256.0, 256.0, false, true);
        assert_eq!(normal_events.get(), 12);
        assert!(lock(&shared).component_picking().is_some());
        assert_eq!(lock(&shared).document().vmf(), &source);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
    }
}
