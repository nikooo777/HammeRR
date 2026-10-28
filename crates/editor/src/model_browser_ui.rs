use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::{ComponentHandle, Model};

use crate::{
    ModelBodygroupRow, ModelBrowserWindow, ModelChoiceRow,
    model_preview::PreviewRenderer,
    runtime::{DocumentAccess, DocumentContinuation},
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
    sources: Vec<String>,
    offset: usize,
}

fn refresh_catalog(window: &ModelBrowserWindow, session: &mut Session, state: &mut Catalog) {
    let group = state
        .groups
        .get(window.get_group_index() as usize)
        .map_or("all", String::as_str)
        .to_owned();
    let source = state
        .sources
        .get(window.get_source_index() as usize)
        .map_or("all", String::as_str)
        .to_owned();
    let result = session.browse_models_filtered(
        &window.get_query(),
        &group,
        state.offset,
        PAGE_SIZE,
        Some(&source),
    );
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
    let mut source_labels = vec![
        "All sources".into(),
        "Directories".into(),
        "VPK archives".into(),
        "Unavailable".into(),
    ];
    state.sources = vec![
        "all".into(),
        "directory".into(),
        "vpk".into(),
        "unavailable".into(),
    ];
    for row in report["sources"].as_array().unwrap() {
        state.sources.push(row["id"].as_str().unwrap().into());
        source_labels.push(
            format!(
                "{} · {}",
                row["kind"].as_str().unwrap(),
                row["mount"].as_str().unwrap()
            )
            .into(),
        );
    }
    window.set_source_labels(slint::ModelRc::new(slint::VecModel::from(source_labels)));
    window.set_source_index(
        state
            .sources
            .iter()
            .position(|candidate| candidate == &source)
            .unwrap_or(0) as i32,
    );
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
                source: row["source"]["mount"].as_str().map_or_else(
                    || "Current MDL source unavailable".into(),
                    |mount| {
                        format!(
                            "Current MDL: {} · {mount}",
                            row["source"]["kind"].as_str().unwrap()
                        )
                        .into()
                    },
                ),
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
    let height = (750.0
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
    window.set_provenance(asset["sources"].as_array().map_or_else(
        || "Cached model origins unavailable".into(),
        |sources| {
            format!(
                "Captured at model load; current files unverified\n{}",
                sources
                    .iter()
                    .map(|source| format!(
                        "{}: {} · {}",
                        source["path"].as_str().unwrap(),
                        source["source"]["kind"].as_str().unwrap(),
                        source["source"]["mount"].as_str().unwrap()
                    ))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
            .into()
        },
    ));
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
            window.set_provenance("".into());
            window.set_materials("".into());
            window.set_material_count(0);
            window.set_bodygroups(Default::default());
            window.set_message(error.into());
        }
    }
}

fn callback(
    window: &slint::Weak<ModelBrowserWindow>,
    continuation: &DocumentContinuation,
    body: impl FnOnce(ModelBrowserWindow, &mut Session),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = continuation.upgrade().and_then(|access| {
        access.with_session(|session| {
            body(window.clone_strong(), session);
            Ok(())
        })
    });
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let continuation = access.downgrade();
        let window = ModelBrowserWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(960.0, 750.0));
        let token = Rc::new(Cell::new(None));
        let catalog = Rc::new(RefCell::new(Catalog::default()));
        let renderer = Rc::new(RefCell::new(Preview::default()));
        window.on_resize_details({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    fit_window(&window, window.get_bodygroups().row_count());
                }
            }
        });
        window.on_search({
            let (window, continuation, catalog) =
                (window.as_weak(), continuation.clone(), catalog.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    let mut state = catalog.borrow_mut();
                    state.offset = 0;
                    refresh_catalog(&window, session, &mut state);
                })
            }
        });
        window.on_group_changed({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_search();
                }
            }
        });
        window.on_page({
            let (window, continuation, catalog) =
                (window.as_weak(), continuation.clone(), catalog.clone());
            move |direction| {
                callback(&window, &continuation, |window, session| {
                    let mut state = catalog.borrow_mut();
                    state.offset = if direction < 0 {
                        state.offset.saturating_sub(PAGE_SIZE)
                    } else {
                        state.offset.saturating_add(PAGE_SIZE)
                    };
                    refresh_catalog(&window, session, &mut state);
                })
            }
        });
        window.on_preview_model({
            let (window, continuation, token, renderer) = (
                window.as_weak(),
                continuation.clone(),
                token.clone(),
                renderer.clone(),
            );
            move || {
                callback(&window, &continuation, |window, session| {
                    preview(&window, session, token.get(), &mut renderer.borrow_mut())
                })
            }
        });
        window.on_bodygroup_changed({
            let (window, continuation, token, renderer) = (
                window.as_weak(),
                continuation.clone(),
                token.clone(),
                renderer.clone(),
            );
            move |group, variant| {
                callback(&window, &continuation, |window, session| {
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
                        Ok(report) => {
                            present(&window, session, &report, &mut renderer.borrow_mut())
                        }
                        Err(error) => {
                            window.set_can_apply(false);
                            if let Some(token) = token.get()
                                && let Ok(report) = session.model_choice(token)
                            {
                                present(&window, session, &report, &mut renderer.borrow_mut());
                            }
                            window.set_message(error.into());
                        }
                    }
                })
            }
        });
        window.on_choose({
            let (window, continuation, token, renderer) = (
                window.as_weak(),
                continuation.clone(),
                token.clone(),
                renderer.clone(),
            );
            move |path| {
                callback(&window, &continuation, |window, session| {
                    window.set_path(path);
                    window.set_skin(0);
                    window.set_body(0);
                    preview(&window, session, token.get(), &mut renderer.borrow_mut());
                })
            }
        });
        window.on_invalidate({
            let (window, continuation) = (window.as_weak(), continuation.clone());
            move || {
                callback(&window, &continuation, |window, _| {
                    window.set_can_apply(false);
                    window.set_has_preview(false);
                    window.set_provenance("".into());
                    window.set_message("Preview the model path before applying it.".into());
                })
            }
        });
        window.on_cancel({
            let (window, continuation, token) =
                (window.as_weak(), continuation.clone(), token.clone());
            move || {
                let Some(local_window) = window.upgrade() else {
                    return;
                };
                let token = token.take();
                callback(&window, &continuation, |_, session| {
                    if let Some(token) = token {
                        let _ = session.cancel_model_choice(token);
                    }
                });
                let _ = local_window.hide();
            }
        });
        window.on_apply({
            let (window, continuation, token, renderer) = (
                window.as_weak(),
                continuation.clone(),
                token.clone(),
                renderer.clone(),
            );
            move || {
                callback(&window, &continuation, |window, session| {
                    let result = token
                        .get()
                        .ok_or("Reopen the model browser".to_owned())
                        .and_then(|token| {
                            let report = session.model_choice(token)?;
                            if renderer.borrow().seen.as_ref()
                                != choice_key(&report, session.materials().generation()).as_ref()
                            {
                                present(&window, session, &report, &mut renderer.borrow_mut());
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
                })
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_cancel();
                }
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
        self.window.set_source_index(0);
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
        self.window.set_provenance("".into());
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
