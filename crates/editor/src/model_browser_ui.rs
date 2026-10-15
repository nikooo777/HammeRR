use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::{ComponentHandle, Model};

use crate::{
    ModelBodygroupRow, ModelBrowserWindow, ModelChoiceRow,
    model_preview::PreviewRenderer,
    runtime::{SharedSession, lock},
    session::{Actor, Session},
};

const PAGE_SIZE: usize = 60;

pub struct Controller {
    pub window: ModelBrowserWindow,
    token: Rc<Cell<Option<u64>>>,
    catalog: Rc<RefCell<Catalog>>,
    preview: Rc<RefCell<Preview>>,
}

#[derive(Default)]
struct Catalog {
    groups: Vec<String>,
    offset: usize,
}

fn refresh_catalog(window: &ModelBrowserWindow, session: &mut Session, state: &mut Catalog) {
    let group = state
        .groups
        .get(window.get_group_index() as usize)
        .map_or("all", String::as_str)
        .to_owned();
    let result = session.browse_models(&window.get_query(), &group, state.offset, PAGE_SIZE);
    let report = match result {
        Ok(report) => report,
        Err(error) => {
            window.set_rows(slint::ModelRc::new(slint::VecModel::from(Vec::<
                ModelChoiceRow,
            >::new(
            ))));
            window.set_can_previous(false);
            window.set_can_next(false);
            window.set_page_info("Model catalog unavailable".into());
            window.set_message(error.into());
            return;
        }
    };
    state.groups = report["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().into())
        .collect();
    window.set_groups(slint::ModelRc::new(slint::VecModel::from(
        report["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| format!("{} ({})", row["label"].as_str().unwrap(), row["count"]).into())
            .collect::<Vec<slint::SharedString>>(),
    )));
    let index = state
        .groups
        .iter()
        .position(|candidate| candidate == &group)
        .unwrap_or(0);
    window.set_group_index(index as i32);
    if state.groups[index] != group {
        state.offset = 0;
        refresh_catalog(window, session, state);
        return;
    }
    window.set_rows(slint::ModelRc::new(slint::VecModel::from(
        report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| ModelChoiceRow {
                path: row["path"].as_str().unwrap().into(),
                name: row["name"].as_str().unwrap().into(),
                folder: row["folder"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("models/")
                    .into(),
            })
            .collect::<Vec<_>>(),
    )));
    let total = report["total"].as_u64().unwrap() as usize;
    window.set_can_previous(state.offset > 0);
    window.set_can_next(state.offset.saturating_add(PAGE_SIZE) < total);
    window.set_page_info(if total == 0 {
        "No models".into()
    } else {
        format!(
            "{}–{} of {}",
            state.offset + 1,
            (state.offset + PAGE_SIZE).min(total),
            total
        )
        .into()
    });
}

#[derive(Default)]
struct Preview {
    renderer: PreviewRenderer,
    seen: Option<(u64, String, usize, u32, u64)>,
}

fn choice_key(
    report: &serde_json::Value,
    generation: u64,
) -> Option<(u64, String, usize, u32, u64)> {
    let asset = &report["asset"];
    (!asset.is_null()).then(|| {
        (
            report["token"].as_u64().unwrap(),
            asset["path"].as_str().unwrap().to_owned(),
            asset["skin"].as_u64().unwrap() as usize,
            asset["body"].as_u64().unwrap() as u32,
            generation,
        )
    })
}

fn fit_window(window: &ModelBrowserWindow, groups: usize) {
    let height = (690.0
        + groups as f32 * 70.0
        + if window.get_show_materials() {
            100.0
        } else {
            0.0
        })
    .min(900.0);
    let size = window.window().size();
    let scale = window.window().scale_factor();
    if size.height as f32 / scale < height {
        window
            .window()
            .set_size(slint::LogicalSize::new(size.width as f32 / scale, height));
    }
}

fn present(
    window: &ModelBrowserWindow,
    session: &Session,
    report: &serde_json::Value,
    preview: &mut Preview,
) {
    let asset = &report["asset"];
    if asset.is_null() {
        return;
    }
    let path = asset["path"].as_str().unwrap();
    let skin = asset["skin"].as_u64().unwrap() as usize;
    let body = asset["body"].as_u64().unwrap() as u32;
    let key = (
        report["token"].as_u64().unwrap(),
        path.to_owned(),
        skin,
        body,
        session.materials().generation(),
    );
    window.set_path(path.into());
    window.set_skin_count(asset["skins"].as_u64().unwrap() as i32);
    window.set_skin(skin as i32);
    window.set_body(body as i32);
    let rows = asset["bodygroups"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|group| group["variants"].as_array().unwrap().len() > 1)
        .map(|group| ModelBodygroupRow {
            index: group["index"].as_u64().unwrap() as i32,
            label: group["label"].as_str().unwrap().into(),
            choices: slint::ModelRc::new(slint::VecModel::from(
                group["variants"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|variant| variant["label"].as_str().unwrap().into())
                    .collect::<Vec<slint::SharedString>>(),
            )),
            selected: group["selected"].as_u64().unwrap() as i32,
        })
        .collect::<Vec<_>>();
    fit_window(window, rows.len());
    window.set_bodygroups(slint::ModelRc::new(slint::VecModel::from(rows)));
    window.set_geometry(if asset["blank"] == true {
        "No model geometry · entity helper".into()
    } else {
        format!(
            "{} triangles · {} vertices · {} meshes",
            asset["triangles"], asset["vertices"], asset["meshes"]
        )
        .into()
    });
    window.set_dimensions(if asset["blank"] == true {
        "Blank body selection".into()
    } else {
        let low = &asset["local_bounds"][0];
        let high = &asset["local_bounds"][1];
        format!(
            "Size · {:.1} × {:.1} × {:.1} units",
            high[0].as_f64().unwrap() - low[0].as_f64().unwrap(),
            high[1].as_f64().unwrap() - low[1].as_f64().unwrap(),
            high[2].as_f64().unwrap() - low[2].as_f64().unwrap()
        )
        .into()
    });
    window.set_materials(
        asset["materials"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            .into(),
    );
    window.set_material_count(asset["materials"].as_array().unwrap().len() as i32);
    window.set_can_apply(true);
    if preview.seen.as_ref() != Some(&key) {
        window.set_has_preview(false);
        preview.seen = Some(key);
        match preview.renderer.render(session, path, skin, body, 640, 400) {
            Ok(pixels) => {
                window.set_preview(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
                    slint::Rgba8Pixel,
                >::clone_from_slice(
                    &pixels, 640, 400
                )));
                window.set_has_preview(true);
            }
            Err(error) => {
                window.set_message(error.into());
                return;
            }
        }
    }
    window.set_message(if let Some(error) = asset["material_error"].as_str() {
        format!("Model materials unavailable: {error}").into()
    } else if asset["blank"]==true {"Blank bodygroups show an editor helper. Use model applies the model, skin and bodygroups together.".into()}else if asset["pose"]=="reference" {"Reference pose; animation sequences are not played. Use model applies model, skin and bodygroups together.".into()}else{"Preview only. Use model applies model, skin and bodygroups together.".into()});
}

fn preview(
    window: &ModelBrowserWindow,
    session: &mut Session,
    token: Option<u64>,
    state: &mut Preview,
) {
    state.seen = None;
    let result = token
        .ok_or("Reopen the model browser".to_owned())
        .and_then(|token| {
            session.choose_model_body(
                token,
                &window.get_path(),
                window.get_skin().max(0) as usize,
                window.get_body().max(0) as u32,
            )
        });
    match result {
        Ok(report) => present(window, session, &report, state),
        Err(error) => {
            window.set_can_apply(false);
            window.set_has_preview(false);
            window.set_geometry("".into());
            window.set_dimensions("".into());
            window.set_materials("".into());
            window.set_material_count(0);
            window.set_bodygroups(Default::default());
            window.set_message(error.into());
        }
    }
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = ModelBrowserWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(960.0, 690.0));
        let token = Rc::new(Cell::new(None));
        let catalog = Rc::new(RefCell::new(Catalog::default()));
        let renderer = Rc::new(RefCell::new(Preview::default()));
        window.on_resize_details({
            let window = window.as_weak();
            move || {
                let window = window.unwrap();
                fit_window(&window, window.get_bodygroups().row_count());
            }
        });
        window.on_search({
            let (window, shared, catalog) = (window.as_weak(), shared.clone(), catalog.clone());
            move || {
                let mut state = catalog.borrow_mut();
                state.offset = 0;
                refresh_catalog(&window.unwrap(), &mut lock(&shared), &mut state);
            }
        });
        window.on_group_changed({
            let window = window.as_weak();
            move || window.unwrap().invoke_search()
        });
        window.on_page({
            let (window, shared, catalog) = (window.as_weak(), shared.clone(), catalog.clone());
            move |direction| {
                let mut state = catalog.borrow_mut();
                state.offset = if direction < 0 {
                    state.offset.saturating_sub(PAGE_SIZE)
                } else {
                    state.offset.saturating_add(PAGE_SIZE)
                };
                refresh_catalog(&window.unwrap(), &mut lock(&shared), &mut state);
            }
        });
        window.on_preview_model({
            let (window, shared, token, renderer) = (
                window.as_weak(),
                shared.clone(),
                token.clone(),
                renderer.clone(),
            );
            move || {
                preview(
                    &window.unwrap(),
                    &mut lock(&shared),
                    token.get(),
                    &mut renderer.borrow_mut(),
                )
            }
        });
        window.on_bodygroup_changed({
            let (window, shared, token, renderer) = (
                window.as_weak(),
                shared.clone(),
                token.clone(),
                renderer.clone(),
            );
            move |group, variant| {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let result = token
                    .get()
                    .ok_or("Reopen the model browser".to_owned())
                    .and_then(|token| {
                        session.choose_model_bodygroup(
                            token,
                            group.max(0) as usize,
                            variant.max(0) as usize,
                        )
                    });
                match result {
                    Ok(report) => present(&window, &session, &report, &mut renderer.borrow_mut()),
                    Err(error) => {
                        window.set_can_apply(false);
                        if let Some(token) = token.get()
                            && let Ok(report) = session.model_choice(token)
                        {
                            present(&window, &session, &report, &mut renderer.borrow_mut());
                        }
                        window.set_message(error.into());
                    }
                }
            }
        });
        window.on_choose({
            let window = window.as_weak();
            move |path| {
                let window = window.unwrap();
                window.set_path(path);
                window.set_skin(0);
                window.set_body(0);
                window.invoke_preview_model();
            }
        });
        window.on_invalidate({
            let window = window.as_weak();
            move || {
                let window = window.unwrap();
                window.set_can_apply(false);
                window.set_has_preview(false);
                window.set_message("Preview the model path before applying it.".into());
            }
        });
        window.on_cancel({
            let (window, shared, token) = (window.as_weak(), shared.clone(), token.clone());
            move || {
                if let Some(token) = token.take() {
                    let _ = lock(&shared).cancel_model_choice(token);
                }
                let _ = window.unwrap().hide();
            }
        });
        window.on_apply({
            let (window, shared, token, renderer) = (
                window.as_weak(),
                shared.clone(),
                token.clone(),
                renderer.clone(),
            );
            move || {
                let window = window.unwrap();
                let result = token
                    .get()
                    .ok_or("Reopen the model browser".to_owned())
                    .and_then(|token| {
                        let mut session = lock(&shared);
                        let report = session.model_choice(token)?;
                        if renderer.borrow().seen.as_ref()
                            != choice_key(&report, session.materials().generation()).as_ref()
                        {
                            present(&window, &session, &report, &mut renderer.borrow_mut());
                            return Ok(None);
                        }
                        session.apply_model_choice(Actor::User, token).map(Some)
                    });
                match result {
                    Ok(Some(_)) => {
                        token.set(None);
                        let _ = window.hide();
                    }
                    Ok(None) => window.set_message(
                        "Model choice changed. Review the preview before Use model.".into(),
                    ),
                    Err(error) => {
                        window.set_can_apply(false);
                        window.set_message(error.into());
                    }
                }
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                window.unwrap().invoke_cancel();
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self {
            window,
            token,
            catalog,
            preview: renderer,
        })
    }

    pub fn initialize(&self, session: &mut Session, id: u32) -> Result<(), String> {
        let report = session.resume_model_choice(id)?;
        let token = report["token"].as_u64().unwrap();
        if let Some(previous) = self.token.get()
            && previous != token
        {
            let _ = session.cancel_model_choice(previous);
        }
        self.token.set(Some(token));
        self.window.set_query("".into());
        self.window.set_group_index(0);
        *self.catalog.borrow_mut() = Catalog::default();
        refresh_catalog(&self.window, session, &mut self.catalog.borrow_mut());
        self.window
            .set_path(report["path"].as_str().unwrap_or_default().into());
        let skin = report["skin"].as_u64().unwrap().min(i32::MAX as u64 - 1) as i32;
        self.window.set_skin_count(skin + 1);
        self.window.set_skin(skin);
        self.window
            .set_body(report["body"].as_u64().unwrap().min(i32::MAX as u64) as i32);
        self.window.set_bodygroups(Default::default());
        self.window.set_can_apply(false);
        self.window.set_has_preview(false);
        self.preview.borrow_mut().seen = None;
        self.window.set_geometry("".into());
        self.window.set_dimensions("".into());
        self.window.set_materials("".into());
        self.window.set_material_count(0);
        self.window.set_show_materials(false);
        self.window
            .set_message("Choose a model. The map stays unchanged until Use model.".into());
        if !report["asset"].is_null() {
            present(
                &self.window,
                session,
                &report,
                &mut self.preview.borrow_mut(),
            );
        }
        Ok(())
    }

    pub fn refresh(&self, session: &Session) {
        if let Some(token) = self.token.get() {
            match session.model_choice(token) {
                Ok(report) => present(
                    &self.window,
                    session,
                    &report,
                    &mut self.preview.borrow_mut(),
                ),
                Err(error) => {
                    self.window.set_can_apply(false);
                    self.window.set_message(error.into());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use map_core::ObjectId;
    use slint::Model;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    #[test]
    fn model_browser_search_skin_preview_apply_and_cancel_share_session_operations() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let Ok(game) = editor_core::startup::mount_game(None) else {
            return;
        };
        let mut session = Session::new(Ok(game));
        let ObjectId::Entity(id) = session
            .create_entity(
                Actor::User,
                "prop_static",
                Some(DVec3::ZERO),
                BTreeMap::from([("model".into(), "".into())]),
            )
            .unwrap()
        else {
            unreachable!()
        };
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.initialize(&mut lock(&shared), id).unwrap();
        controller.window.set_query("oildrum".into());
        controller.window.invoke_search();
        assert!(controller.window.get_rows().row_count() > 0);
        controller
            .window
            .invoke_choose("models/props_c17/oildrum001.mdl".into());
        assert!(
            controller.window.get_can_apply(),
            "{}",
            controller.window.get_message()
        );
        assert!(
            controller.window.get_has_preview(),
            "{}",
            controller.window.get_message()
        );
        let first = controller.window.get_preview().to_rgba8().unwrap();
        controller.window.set_skin(5);
        controller.window.invoke_preview_model();
        assert_ne!(
            first.as_bytes(),
            controller
                .window
                .get_preview()
                .to_rgba8()
                .unwrap()
                .as_bytes()
        );
        assert_eq!(controller.window.get_skin_count(), 6);
        assert_eq!(lock(&shared).document().vmf(), &source);
        controller.window.show().unwrap();
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "model-browser");
        controller.window.invoke_apply();
        assert!(!controller.window.window().is_visible());
        assert_eq!(
            lock(&shared)
                .document()
                .entity(id)
                .unwrap()
                .properties
                .get("skin"),
            Some("5")
        );
        assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
        lock(&shared).undo(Actor::User).unwrap();
        controller.initialize(&mut lock(&shared), id).unwrap();
        controller.window.show().unwrap();
        controller
            .window
            .invoke_choose("models/props_junk/wood_crate001a.mdl".into());
        let replacement = lock(&shared).begin_model_choice(id).unwrap()["token"]
            .as_u64()
            .unwrap();
        controller.window.invoke_cancel();
        assert!(lock(&shared).model_choice(replacement).is_ok());
        assert_eq!(lock(&shared).document().vmf(), &source);
        let ObjectId::Entity(dynamic) = lock(&shared)
            .create_entity(
                Actor::User,
                "prop_dynamic",
                Some(DVec3::ZERO),
                BTreeMap::from([("model".into(), "".into())]),
            )
            .unwrap()
        else {
            unreachable!()
        };
        let before = lock(&shared).document().vmf().clone();
        controller.initialize(&mut lock(&shared), dynamic).unwrap();
        controller.window.set_query("ct_urban".into());
        controller.window.invoke_search();
        let group = controller
            .catalog
            .borrow()
            .groups
            .iter()
            .position(|group| group == "folder:models/player")
            .unwrap();
        controller.window.set_group_index(group as i32);
        controller.window.invoke_group_changed();
        assert_eq!(controller.window.get_rows().row_count(), 1);
        controller
            .window
            .invoke_choose("models/player/ct_urban.mdl".into());
        assert!(
            controller.window.get_can_apply(),
            "{}",
            controller.window.get_message()
        );
        assert!(controller.window.get_has_preview());
        assert!(controller.window.get_message().contains("Reference pose"));
        controller.window.show().unwrap();
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "reference-model-browser");
        controller.window.invoke_cancel();
        assert_eq!(lock(&shared).document().vmf(), &before);
        let token = lock(&shared).begin_model_choice(dynamic).unwrap()["token"]
            .as_u64()
            .unwrap();
        lock(&shared)
            .choose_model_body(token, "models/kleiner.mdl", 0, 0)
            .unwrap();
        controller.initialize(&mut lock(&shared), dynamic).unwrap();
        assert_eq!(controller.token.get(), Some(token));
        assert_eq!(controller.window.get_path(), "models/kleiner.mdl");
        let groups = controller.window.get_bodygroups();
        assert_eq!(groups.row_count(), 1);
        let group = groups.row_data(0).unwrap().index;
        let pixels = controller.window.get_preview().to_rgba8().unwrap();
        assert_eq!(lock(&shared).current_model_choice()["token"], token);
        lock(&shared)
            .choose_model_bodygroup(token, group as usize, 1)
            .unwrap();
        controller.refresh(&lock(&shared));
        assert_eq!(controller.window.get_body(), 1);
        assert_eq!(
            controller
                .window
                .get_bodygroups()
                .row_data(0)
                .unwrap()
                .selected,
            1
        );
        assert_ne!(
            pixels.as_bytes(),
            controller
                .window
                .get_preview()
                .to_rgba8()
                .unwrap()
                .as_bytes()
        );
        controller.window.set_query("kleiner".into());
        controller.window.set_group_index(0);
        controller.window.invoke_search();
        controller.window.show().unwrap();
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "bodygroup-model-browser");
        lock(&shared)
            .choose_model_bodygroup(token, group as usize, 0)
            .unwrap();
        controller.window.invoke_apply();
        assert!(controller.window.window().is_visible());
        assert_eq!(controller.window.get_body(), 0);
        assert_eq!(lock(&shared).document().vmf(), &before);
        controller.window.invoke_bodygroup_changed(group, 1);
        assert_eq!(controller.window.get_body(), 1);
        let history = lock(&shared).document().undo_labels().count();
        controller.window.invoke_apply();
        assert!(!controller.window.window().is_visible());
        assert_eq!(
            lock(&shared)
                .document()
                .entity(dynamic)
                .unwrap()
                .properties
                .get("body"),
            Some("1")
        );
        assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &before);
    }
}
