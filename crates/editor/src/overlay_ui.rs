use std::{cell::RefCell, rc::Rc};

use glam::{DVec2, DVec3};
use map_core::{overlay::Options, surface::FaceId};
use serde_json::Value;
use slint::ComponentHandle;

use crate::{
    OverlayWindow,
    runtime::{SharedSession, lock},
    session::{Actor, Session, overlay::Request},
    ui::preview_image,
};

#[derive(Default)]
struct Form {
    source: Option<FaceId>,
    token: Option<u64>,
    reviewed: Option<Options>,
    snapshot: String,
    seen: String,
    serial: u64,
}

#[derive(Clone)]
pub struct MaterialTarget {
    snapshot: String,
    serial: u64,
}

pub struct Controller {
    pub window: OverlayWindow,
    form: Rc<RefCell<Form>>,
}

fn number(text: &str) -> Result<f32, String> {
    text.trim()
        .parse::<f32>()
        .ok()
        .filter(|number| number.is_finite())
        .ok_or("Enter finite texture coordinates and fade distances".into())
}

fn fields(window: &OverlayWindow) -> Result<Options, String> {
    let targetname = window.get_targetname();
    Ok(Options {
        material: window.get_material().to_string(),
        targetname: (!targetname.trim().is_empty()).then(|| targetname.to_string()),
        render_order: window
            .get_render_order()
            .try_into()
            .map_err(|_| "Render order must be between 0 and 3")?,
        u: [number(&window.get_u_start())?, number(&window.get_u_end())?],
        v: [number(&window.get_v_start())?, number(&window.get_v_end())?],
        fade_min: number(&window.get_fade_min())?,
        fade_max: number(&window.get_fade_max())?,
    })
}

fn load(window: &OverlayWindow, options: &Value) {
    let text = |value: &Value, fallback: f64| {
        (value.as_f64().unwrap_or(fallback) as f32)
            .to_string()
            .into()
    };
    window.set_material(options["material"].as_str().unwrap_or_default().into());
    window.set_targetname(options["targetname"].as_str().unwrap_or_default().into());
    window.set_render_order(options["render_order"].as_i64().unwrap_or_default() as i32);
    window.set_u_start(text(&options["u"][0], 0.0));
    window.set_u_end(text(&options["u"][1], 1.0));
    window.set_v_start(text(&options["v"][0], 0.0));
    window.set_v_end(text(&options["v"][1], 1.0));
    window.set_fade_min(text(&options["fade_min"], -1.0));
    window.set_fade_max(text(&options["fade_max"], 0.0));
}

fn source(value: &Value) -> Option<FaceId> {
    Some(FaceId {
        solid: value["solid"].as_u64()?.try_into().ok()?,
        side: value["side"].as_u64()?.try_into().ok()?,
    })
}

fn outline(value: &Value) -> Option<String> {
    let points = value
        .as_array()?
        .iter()
        .map(|point| {
            Some(DVec3::new(
                point[0].as_f64()?,
                point[1].as_f64()?,
                point[2].as_f64()?,
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    if points.len() != 4 || points.iter().any(|point| !point.is_finite()) {
        return None;
    }
    let right = (points[1] - points[0]).try_normalize()?;
    let normal = right.cross(points[3] - points[0]).try_normalize()?;
    let up = normal.cross(right);
    let projected = points
        .iter()
        .map(|point| DVec2::new(point.dot(right), point.dot(up)))
        .collect::<Vec<_>>();
    let (mins, maxs) = projected
        .iter()
        .fold((DVec2::MAX, DVec2::MIN), |(mins, maxs), &point| {
            (mins.min(point), maxs.max(point))
        });
    let extent = maxs - mins;
    if extent.min_element() <= 0.0 {
        return None;
    }
    let scale = (288.0 / extent.x).min(148.0 / extent.y);
    let center = (mins + maxs) * 0.5;
    let mut path = String::new();
    for (index, point) in projected.iter().enumerate() {
        let point = (*point - center) * scale;
        path.push_str(&format!(
            "{} {} {} ",
            if index == 0 { "M" } else { "L" },
            160.0 + point.x,
            90.0 - point.y
        ));
    }
    path.push('Z');
    Some(path)
}

fn refresh_material(window: &OverlayWindow, session: &mut Session) {
    match session.material_preview(&window.get_material()) {
        Ok(preview) => {
            window.set_material_image(preview_image(&preview));
            window.set_material_detail(if preview.missing || preview.image.is_none() {
                "Texture preview unavailable".into()
            } else {
                format!("{} × {}", preview.width, preview.height).into()
            });
        }
        Err(_) => {
            window.set_material_image(slint::Image::default());
            window.set_material_detail("Choose an overlay material".into());
        }
    }
}

fn refresh(window: &OverlayWindow, session: &Session, form: &RefCell<Form>) {
    let state = session.get_overlay_state();
    let draft = &state["draft"];
    let snapshot = state["snapshot"].as_str().unwrap_or_default();
    let mut form = form.borrow_mut();
    let context_refreshed = form.seen != snapshot;
    form.seen = snapshot.into();
    window.set_can_review(draft["valid"] == true);
    window.set_can_preview(form.source.is_some() && form.snapshot == snapshot);
    let matches = draft["token"].as_u64() == form.token && draft["owner"] == "you";
    window.set_ready(
        matches
            && draft["valid"] == true
            && form.reviewed.is_some()
            && fields(window).ok().as_ref() == form.reviewed.as_ref(),
    );
    window.set_draft_label(if draft.is_null() {
        "No overlay draft · Fit / Preview leaves the map unchanged".into()
    } else if draft["valid"] != true {
        "The shared draft is stale · reopen Overlay to fit the current face".into()
    } else if matches {
        "Your fitted overlay draft · Apply overlay creates one entity".into()
    } else {
        "A shared draft is available · Review current draft to use it".into()
    });
    let display_source = if matches && draft["valid"] == true {
        &draft["source"]
    } else {
        &state["selected_source"]
    };
    window.set_source_label(if let Some(source) = source(display_source) {
        format!(
            "Source · brush {} · face {} · world / detail quad",
            source.solid, source.side
        )
        .into()
    } else {
        state["selection_error"]
            .as_str()
            .unwrap_or("Select exactly one visible world or detail quad face")
            .into()
    });
    let corners = if matches && draft["valid"] == true {
        &draft["corners"]
    } else {
        &state["selected_source"]["corners"]
    };
    window.set_outline(outline(corners).unwrap_or_default().into());
    if form.snapshot != snapshot {
        window.set_message("Map, selection or shared draft changed. Review current draft, or reopen Overlay to use the selected face.".into());
    } else if context_refreshed && window.get_ready() {
        window.set_message("".into());
    }
}

fn edited(window: &OverlayWindow, session: &mut Session, form: &RefCell<Form>) {
    let mut state = form.borrow_mut();
    state.reviewed = None;
    state.serial += 1;
    drop(state);
    window.set_ready(false);
    window.set_message("Settings changed. Preview settings before applying.".into());
    refresh_material(window, session);
}

fn adopt(window: &OverlayWindow, state: &Value, form: &RefCell<Form>) {
    let draft = &state["draft"];
    load(window, &draft["options"]);
    let mut form = form.borrow_mut();
    form.source = source(&draft["source"]);
    form.token = draft["token"].as_u64();
    form.reviewed = fields(window).ok();
    form.snapshot = state["snapshot"].as_str().unwrap_or_default().into();
    form.serial += 1;
}

fn preview(window: &OverlayWindow, session: &mut Session, form: &RefCell<Form>) {
    let result = (|| {
        let (source, snapshot) = {
            let form = form.borrow();
            (
                form.source
                    .ok_or("Select one world or detail quad face and reopen Overlay")?,
                form.snapshot.clone(),
            )
        };
        session.preview_overlay(
            Actor::User,
            Request {
                source,
                options: fields(window)?,
            },
            &snapshot,
        )
    })();
    match result {
        Ok(state) => {
            adopt(window, &state, form);
            window.set_message(
                "Review the cyan outline and material thumbnail, then Apply overlay.".into(),
            );
        }
        Err(error) => {
            form.borrow_mut().reviewed = None;
            window.set_message(error.into());
        }
    }
    refresh_material(window, session);
    refresh(window, session, form);
}

fn cancel_owned(session: &mut Session, form: &RefCell<Form>) {
    let token = form.borrow_mut().token.take();
    let state = session.get_overlay_state();
    if let Some(token) = token
        && state["draft"]["token"].as_u64() == Some(token)
        && state["draft"]["owner"] == "you"
    {
        let _ = session.cancel_overlay(Actor::User, token);
    }
    form.borrow_mut().reviewed = None;
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = OverlayWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        window.on_edited({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || edited(&window.unwrap(), &mut lock(&shared), &form)
        });
        window.on_preview({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || preview(&window.unwrap(), &mut lock(&shared), &form)
        });
        window.on_fit({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                window.set_u_start("0".into());
                window.set_u_end("1".into());
                window.set_v_start("0".into());
                window.set_v_end("1".into());
                edited(&window, &mut lock(&shared), &form);
                preview(&window, &mut lock(&shared), &form);
            }
        });
        window.on_review({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let state = session.get_overlay_state();
                let Some(token) = state["draft"]["token"].as_u64() else {
                    window.set_message("No current overlay draft to review.".into());
                    return;
                };
                match session.review_overlay(Actor::User, token) {
                    Ok(state) => {
                        adopt(&window, &state, &form);
                        window.set_message("Current draft loaded. Apply overlay confirms it; Cancel discards this reviewed draft.".into());
                    }
                    Err(error) => window.set_message(error.into()),
                }
                refresh_material(&window, &mut session);
                refresh(&window, &session, &form);
            }
        });
        window.on_apply({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let token = {
                    let form = form.borrow();
                    form.token.filter(|_| {
                        form.reviewed.is_some()
                            && fields(&window).ok().as_ref() == form.reviewed.as_ref()
                    })
                };
                let Some(token) = token else {
                    window.set_ready(false);
                    window.set_message(
                        "Fit / Preview or Review current draft before applying.".into(),
                    );
                    return;
                };
                match session.apply_overlay(Actor::User, token) {
                    Ok(_) => {
                        *form.borrow_mut() = Form::default();
                        window.set_message("Overlay created and selected. Undo removes it.".into());
                        refresh(&window, &session, &form);
                        let _ = window.hide();
                    }
                    Err(error) => {
                        window.set_message(error.into());
                        refresh(&window, &session, &form);
                    }
                }
            }
        });
        window.on_cancel({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                cancel_owned(&mut lock(&shared), &form);
                let _ = window.unwrap().hide();
            }
        });
        window.window().on_close_requested({
            let (shared, form) = (shared.clone(), form.clone());
            move || {
                cancel_owned(&mut lock(&shared), &form);
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self { window, form })
    }

    pub fn show(&self, session: &mut Session, material: &str) -> Result<(), slint::PlatformError> {
        let state = session.get_overlay_state();
        let mut form = self.form.borrow_mut();
        form.source = source(&state["selected_source"]);
        form.snapshot = state["snapshot"].as_str().unwrap_or_default().into();
        form.reviewed = None;
        form.serial += 1;
        drop(form);
        load(&self.window, &Value::Null);
        self.window.set_material(material.into());
        self.window.set_message(state["selection_error"].as_str().unwrap_or("Choose a material, then Fit / Preview. Review current draft loads a shared overlay preview.").into());
        refresh_material(&self.window, session);
        refresh(&self.window, session, &self.form);
        self.window.show()
    }

    pub fn refresh(&self, session: &Session) {
        if self.form.borrow().seen != session.overlay_stamp() {
            refresh(&self.window, session, &self.form);
        }
    }

    pub fn material_target(&self, session: &Session) -> MaterialTarget {
        MaterialTarget {
            snapshot: session.overlay_stamp(),
            serial: self.form.borrow().serial,
        }
    }

    pub fn choose_material(
        &self,
        session: &mut Session,
        target: &MaterialTarget,
        material: &str,
    ) -> Result<(), String> {
        if !self.window.window().is_visible()
            || target.snapshot != session.overlay_stamp()
            || target.serial != self.form.borrow().serial
        {
            return Err(
                "Overlay fields, map or draft changed. Open the material picker again.".into(),
            );
        }
        self.window.set_material(material.into());
        edited(&self.window, session, &self.form);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use map_core::{BrushGeometry, ObjectId};
    use std::sync::{Arc, Mutex};

    #[test]
    fn overlay_controls_guard_fields_material_choices_and_shared_draft_ownership() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (_assets, game) = crate::test_assets::game();
        let mut session = Session::new(Ok(game));
        let ObjectId::Solid(solid) = session
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::new(192.0, 128.0, 16.0),
                "test/checker",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let brush = session.document().solid(solid).unwrap();
        let geometry = BrushGeometry::from_solid(brush).unwrap();
        let face = geometry
            .faces
            .iter()
            .find(|face| face.plane.normal.z > 0.9)
            .unwrap();
        let source = FaceId {
            solid,
            side: brush.sides[face.side].id,
        };
        session
            .select_faces(&[source], crate::session::SelectMode::Replace)
            .unwrap();
        let original = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.show(&mut lock(&shared), "test/checker").unwrap();
        assert!(controller.window.get_can_preview());
        assert!(!controller.window.get_outline().is_empty());
        assert!(controller.window.get_material_image().size().width > 0);
        controller.window.set_targetname("floor_wayfinding".into());
        controller.window.set_render_order(1);
        controller.window.set_u_start("2".into());
        controller.window.invoke_fit();
        assert!(
            controller.window.get_ready(),
            "{}",
            controller.window.get_message()
        );
        assert_eq!(controller.window.get_u_start(), "0");
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
        let before_competing = lock(&shared).get_overlay_state();
        lock(&shared)
            .edit_faces(
                Actor::Agent,
                &[source],
                map_core::surface::SurfaceEdit {
                    shift_u: Some(12.125),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_ready());
        let blocked = lock(&shared).get_overlay_state();
        assert_ne!(blocked["snapshot"], before_competing["snapshot"]);
        assert_eq!(
            blocked["draft"]["token"],
            before_competing["draft"]["token"]
        );
        assert_eq!(blocked["draft"]["valid"], false);
        lock(&shared).cancel_face_preview();
        controller.refresh(&lock(&shared));
        assert!(controller.window.get_ready());
        assert!(controller.window.get_message().is_empty());
        assert_eq!(lock(&shared).get_overlay_state(), before_competing);
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
        controller
            .window
            .window()
            .set_size(slint::PhysicalSize::new(860, 940));
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "overlay-authoring");
        for label in [
            "Browse materials…",
            "Review current draft",
            "Fit / Preview",
            "Preview settings",
            "Apply overlay",
            "Cancel",
        ] {
            crate::ui::tests::assert_dialog_button_visible(&controller.window, label);
        }

        let owned = lock(&shared).get_overlay_state()["draft"]["token"]
            .as_u64()
            .unwrap();
        let material_target = controller.material_target(&lock(&shared));
        controller.window.set_u_end("0".into());
        controller.window.invoke_edited();
        controller.window.invoke_preview();
        assert!(!controller.window.get_ready());
        assert_eq!(lock(&shared).get_overlay_state()["draft"]["token"], owned);
        controller.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert!(
            controller
                .choose_material(&mut lock(&shared), &material_target, "test/other")
                .is_err()
        );
        assert_eq!(controller.window.get_material(), "test/checker");

        controller.window.set_u_end("1".into());
        controller.window.invoke_edited();
        let material_target = controller.material_target(&lock(&shared));
        controller
            .choose_material(&mut lock(&shared), &material_target, "test/checker")
            .unwrap();
        controller.window.invoke_preview();
        assert!(controller.window.get_ready());
        let foreign = {
            let mut session = lock(&shared);
            let snapshot = session.overlay_stamp();
            session
                .preview_overlay(
                    Actor::Agent,
                    Request {
                        source,
                        options: Options {
                            material: "test/checker".into(),
                            targetname: Some("shared_wall_mark".into()),
                            u: [1.0, 0.0],
                            ..Options::default()
                        },
                    },
                    &snapshot,
                )
                .unwrap()["draft"]["token"]
                .as_u64()
                .unwrap()
        };
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_ready());
        controller.window.invoke_cancel();
        assert_eq!(lock(&shared).get_overlay_state()["draft"]["token"], foreign);
        assert_eq!(lock(&shared).get_overlay_state()["draft"]["owner"], "agent");

        controller.show(&mut lock(&shared), "test/checker").unwrap();
        controller.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        controller.window.invoke_review();
        assert!(controller.window.get_ready());
        assert_eq!(controller.window.get_targetname(), "shared_wall_mark");
        assert_eq!(controller.window.get_u_start(), "1");
        assert_eq!(controller.window.get_u_end(), "0");
        let reviewed = lock(&shared).get_overlay_state();
        assert_ne!(reviewed["draft"]["token"], foreign);
        assert_eq!(reviewed["draft"]["owner"], "you");
        controller.window.invoke_apply();
        let applied = lock(&shared).document().vmf().clone();
        assert_eq!(applied.world.solids, original.world.solids);
        let overlay = applied
            .entities
            .iter()
            .find(|entity| entity.classname() == Some("info_overlay"))
            .unwrap();
        assert_eq!(
            overlay.properties.get("targetname"),
            Some("shared_wall_mark")
        );
        assert_eq!(overlay.properties.get("StartU"), Some("1"));
        assert_eq!(overlay.properties.get("EndU"), Some("0"));
        assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        lock(&shared).redo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &applied);
        lock(&shared).undo(Actor::Agent).unwrap();

        lock(&shared)
            .select_faces(&[source], crate::session::SelectMode::Replace)
            .unwrap();
        controller.show(&mut lock(&shared), "test/checker").unwrap();
        controller.window.invoke_fit();
        assert!(controller.window.get_ready());
        lock(&shared)
            .select_faces(&[], crate::session::SelectMode::Replace)
            .unwrap();
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_ready());
        controller.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        controller.window.invoke_cancel();
        assert!(lock(&shared).get_overlay_state()["draft"].is_null());
        assert_eq!(lock(&shared).document().vmf(), &original);

        for selected in [Vec::new(), vec![ObjectId::Solid(solid)]] {
            {
                let mut session = lock(&shared);
                session
                    .select(&selected, crate::session::SelectMode::Replace)
                    .unwrap();
                let snapshot = session.overlay_stamp();
                session
                    .preview_overlay(
                        Actor::Agent,
                        Request {
                            source,
                            options: Options {
                                material: "test/checker".into(),
                                ..Options::default()
                            },
                        },
                        &snapshot,
                    )
                    .unwrap();
                assert!(!session.face_mode());
                assert!(session.selected_faces().is_empty());
            }
            let before_open = lock(&shared).get_overlay_state();
            let app = crate::AppWindow::new().unwrap();
            app.set_can_apply_material(false);
            app.set_has_faces(false);
            let review = Rc::new(Controller::new(&shared).unwrap());
            app.on_create_overlay({
                let (review, shared) = (review.clone(), shared.clone());
                move || review.show(&mut lock(&shared), "test/checker").unwrap()
            });
            app.show().unwrap();
            app.window().set_size(slint::PhysicalSize::new(1600, 1100));
            app.window().take_snapshot().unwrap();
            crate::ui::tests::assert_dialog_button_visible(&app, "Overlay…");
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, "Overlay…")
                .next()
                .unwrap()
                .invoke_accessible_default_action();
            assert!(review.window.window().is_visible());
            assert!(!review.window.get_can_preview());
            assert!(review.window.get_can_review());
            let opened = lock(&shared).get_overlay_state();
            assert_eq!(opened["snapshot"], before_open["snapshot"]);
            assert_eq!(opened["draft"]["token"], before_open["draft"]["token"]);
            assert_eq!(opened["draft"]["valid"], true);
            assert_eq!(lock(&shared).selected_ids(), selected);
            review.window.invoke_review();
            assert!(review.window.get_ready());
            assert_eq!(lock(&shared).selected_ids(), selected);
            review.window.invoke_apply();
            assert_eq!(
                lock(&shared).document().vmf().entities.len(),
                original.entities.len() + 1
            );
            assert_eq!(
                lock(&shared).document().vmf().world.solids,
                original.world.solids
            );
            lock(&shared).undo(Actor::Agent).unwrap();
            assert_eq!(lock(&shared).document().vmf(), &original);
            app.hide().unwrap();
        }
    }
}
