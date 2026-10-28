use crate::session::prefabs::{GalleryReview, MAX_GALLERY_ENTRIES, ThumbnailStyle};
use crate::{
    PrefabGalleryRow, PrefabRow, PrefabsWindow, clipboard_ui,
    runtime::{DocumentAccess, DocumentContinuation, SharedSession},
    session::{Actor, Session},
};
use slint::{ComponentHandle, VecModel};
use std::{cell::RefCell, rc::Rc};

const PAGE: usize = MAX_GALLERY_ENTRIES;
type Observation = (u64, u64, u64, u64, u64, u64, u64);

fn observation(session: &Session, registry_revision: u64) -> Observation {
    (
        session.document_generation(),
        session.document().revision(),
        session.view_serial(),
        registry_revision,
        session.build_content_epoch(),
        session.definitions_serial(),
        session.prefab_thumbnail_serial(),
    )
}

fn vector(values: [slint::SharedString; 3], label: &str) -> Result<glam::DVec3, String> {
    let mut result = glam::DVec3::ZERO;
    for (axis, value) in values.into_iter().enumerate() {
        result[axis] = value
            .trim()
            .parse()
            .map_err(|_| format!("{label} must contain finite numbers"))?;
    }
    if !result.is_finite() {
        return Err(format!("{label} must contain finite numbers"));
    }
    Ok(result)
}

fn placement(window: &PrefabsWindow) -> Result<crate::session::prefabs::Placement, String> {
    Ok(crate::session::prefabs::Placement {
        position: vector(
            [
                window.get_center_x(),
                window.get_center_y(),
                window.get_center_z(),
            ],
            "Placement position",
        )?,
        rotation: vector(
            [
                window.get_rotation_x(),
                window.get_rotation_y(),
                window.get_rotation_z(),
            ],
            "Rotation",
        )?,
        scale: vector(
            [
                window.get_scale_x(),
                window.get_scale_y(),
                window.get_scale_z(),
            ],
            "Scale",
        )?,
        pivot: match window.get_pivot_mode() {
            0 => None,
            1 => Some(glam::DVec3::ZERO),
            2 => Some(vector(
                [
                    window.get_pivot_x(),
                    window.get_pivot_y(),
                    window.get_pivot_z(),
                ],
                "Source pivot",
            )?),
            _ => return Err("Choose a source pivot".into()),
        },
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Save,
    Delete,
    Rename,
}

struct Draft {
    action: Action,
    stamp: String,
    source: Option<String>,
}

#[derive(Default)]
struct Form {
    selected: Option<String>,
    offset: usize,
    stamp: String,
    action: Option<Draft>,
    seen: Option<Observation>,
    gallery: Option<GalleryReview>,
    library_message: Option<String>,
}
pub struct Controller {
    pub window: PrefabsWindow,
    form: Rc<RefCell<Form>>,
    paste: Rc<clipboard_ui::Controller>,
}
fn bounds_label(bounds: &serde_json::Value) -> String {
    let low = (0..3)
        .map(|axis| bounds[0][axis].as_f64().unwrap_or_default())
        .collect::<Vec<_>>();
    let high = (0..3)
        .map(|axis| bounds[1][axis].as_f64().unwrap_or_default())
        .collect::<Vec<_>>();
    format!(
        "Size: {} × {} × {} units\nSource center: {}, {}, {}",
        high[0] - low[0],
        high[1] - low[1],
        high[2] - low[2],
        (high[0] + low[0]) * 0.5,
        (high[1] + low[1]) * 0.5,
        (high[2] + low[2]) * 0.5
    )
}
fn thumbnail_pixels(thumbnail: &crate::session::prefabs::Thumbnail) -> slint::Image {
    slint::Image::from_rgba8(
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &thumbnail.image.pixels,
            thumbnail.image.width,
            thumbnail.image.height,
        ),
    )
}

fn thumbnail_image(window: &PrefabsWindow, thumbnail: &crate::session::prefabs::Thumbnail) {
    window.set_thumbnail(thumbnail_pixels(thumbnail));
    window.set_has_thumbnail(true);
}

fn set_rows(window: &PrefabsWindow, rows: Vec<PrefabRow>) {
    let paired = rows
        .chunks(2)
        .map(|pair| PrefabGalleryRow {
            first: pair[0].clone(),
            second: pair.get(1).cloned().unwrap_or_default(),
            has_second: pair.len() == 2,
        })
        .collect::<Vec<_>>();
    window.set_entries(Rc::new(VecModel::from(rows)).into());
    window.set_gallery_rows(Rc::new(VecModel::from(paired)).into());
}

fn clear_gallery_images(window: &PrefabsWindow) {
    use slint::Model;
    let rows = window
        .get_entries()
        .iter()
        .map(|mut row| {
            row.thumbnail = Default::default();
            row.has_image = false;
            row.status = Default::default();
            row
        })
        .collect();
    set_rows(window, rows);
}

fn clear_gallery(window: &PrefabsWindow, form: &RefCell<Form>) {
    form.borrow_mut().gallery = None;
    window.set_gallery_rendered(false);
    window.set_gallery_status("".into());
    clear_gallery_images(window);
}

fn thumbnail_style(window: &PrefabsWindow) -> Result<ThumbnailStyle, String> {
    match window.get_thumbnail_style() {
        0 => Ok(ThumbnailStyle::Flat),
        1 => Ok(ThumbnailStyle::Textured),
        _ => Err("Choose Flat or Textured thumbnails".into()),
    }
}

fn thumbnail_error(style: ThumbnailStyle, error: &str) -> String {
    match style {
        ThumbnailStyle::Flat => error.to_owned(),
        ThumbnailStyle::Textured => format!(
            "Textured unavailable: {error} Repair assets and retry Render thumbnail, or choose Flat explicitly to omit textures."
        ),
    }
}

fn refresh_thumbnail(window: &PrefabsWindow, session: &Session, selected: Option<&str>) {
    window.set_has_thumbnail(false);
    window.set_thumbnail(Default::default());
    window.set_can_render_thumbnail(false);
    let style = match thumbnail_style(window) {
        Ok(style) => style,
        Err(error) => {
            window.set_thumbnail_status(error.into());
            return;
        }
    };
    let Some(name) = selected else {
        window.set_thumbnail_status(
            "Textures stay external. Rendering starts only when requested.".into(),
        );
        return;
    };
    let policy = match style {
        ThumbnailStyle::Flat => {
            "Real assembly geometry and editor point helpers. Textures are omitted; authored hidden objects are included."
        }
        ThumbnailStyle::Textured => {
            "Static opaque base textures with fixed editor shading. No normal maps, authored lighting or sky. Refresh checks external assets. Unsupported materials show an error; Flat is a separate choice."
        }
    };
    match session.cached_prefab_thumbnail_with_style(name, style) {
        Ok(Some(thumbnail)) => {
            thumbnail_image(window, &thumbnail);
            window.set_can_render_thumbnail(true);
            window.set_thumbnail_status(policy.into());
        }
        Ok(None) => {
            let report = session.prefab_thumbnail_report_with_style(name, style);
            if let Some(error) = report["error"].as_str() {
                window.set_thumbnail_status(thumbnail_error(style, error).into());
            } else {
                window.set_can_render_thumbnail(true);
                window.set_thumbnail_status(format!("Ready to render. {policy}").into());
            }
        }
        Err(error) => window.set_thumbnail_status(thumbnail_error(style, &error).into()),
    }
}
fn refresh(window: &PrefabsWindow, session: &Session, state: &RefCell<Form>, force: bool) {
    let mut form = state.borrow_mut();
    let seen = observation(session, session.prefab_registry_serial());
    if !force && form.seen == Some(seen) {
        return;
    }
    let mut report = session.prefab_report(
        &window.get_query(),
        form.offset,
        PAGE,
        form.selected.as_deref(),
    );
    if let Some(total) = report["total"].as_u64()
        && form.offset > 0
        && form.offset >= total as usize
    {
        form.offset = total.saturating_sub(1) as usize / PAGE * PAGE;
        report = session.prefab_report(
            &window.get_query(),
            form.offset,
            PAGE,
            form.selected.as_deref(),
        );
    }
    let report_revision = report["registry_revision"].as_u64().unwrap_or(seen.3);
    let style = thumbnail_style(window);
    if form.gallery.as_ref().is_some_and(|review| match &style {
        Ok(style) => session
            .validate_prefab_gallery(review, &window.get_query(), form.offset, PAGE, *style)
            .is_err(),
        Err(_) => true,
    }) {
        form.gallery = None;
        window.set_gallery_rendered(false);
        clear_gallery_images(window);
        window.set_gallery_status("Images changed. Render page again.".into());
    }
    if let Some(previous) = form.library_message.take()
        && window.get_message().as_str() == previous
    {
        window.set_message("".into());
    }
    window.set_can_save(!session.selected_ids().is_empty());
    window.set_can_preview(!report["selected"].is_null() && form.action.is_none());
    window.set_editing(form.action.is_some());
    window.set_saving(
        form.action
            .as_ref()
            .is_some_and(|draft| draft.action == Action::Save),
    );
    window.set_deleting(
        form.action
            .as_ref()
            .is_some_and(|draft| draft.action == Action::Delete),
    );
    window.set_renaming(
        form.action
            .as_ref()
            .is_some_and(|draft| draft.action == Action::Rename),
    );
    window.set_directory(
        report["directory"]
            .as_str()
            .unwrap_or("Storage unavailable")
            .into(),
    );
    form.stamp = report["snapshot"].as_str().unwrap_or_default().into();
    let selected = if let Some(draft) = &form.action {
        draft.source.as_deref().unwrap_or_default()
    } else if report["selected_exists"] == true {
        form.selected.as_deref().unwrap_or_default()
    } else {
        ""
    };
    window.set_selected(selected.into());
    if form.gallery.is_some() {
        window.set_has_thumbnail(false);
        window.set_thumbnail(Default::default());
        window.set_can_render_thumbnail(form.selected.is_some());
    } else {
        refresh_thumbnail(window, session, form.selected.as_deref());
    }
    window.set_can_render_gallery(
        report["entries"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty())
            && report["error"].is_null(),
    );
    if let Some(error) = report["error"].as_str() {
        window.set_message(error.into());
        form.library_message = Some(error.into());
        set_rows(window, Vec::new());
        window.set_detail(error.into());
        window.set_page_label("Prefabs unavailable".into());
        window.set_previous(false);
        window.set_next(false);
        form.seen = Some(observation(session, report_revision));
        return;
    }
    let rows = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| PrefabRow {
            name: entry["name"].as_str().unwrap().into(),
            detail: format!(
                "{} KiB · VMF assembly",
                entry["bytes"].as_u64().unwrap().div_ceil(1024)
            )
            .into(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    if form.gallery.is_none() {
        set_rows(window, rows);
    }
    if let Some(error) = report["write_error"].as_str() {
        let message = format!("Library write committed: {error}");
        window.set_message(message.clone().into());
        form.library_message = Some(message);
    }
    window.set_page_label(format!("Prefabs: {}", report["total"]).into());
    window.set_previous(form.offset > 0);
    window.set_next(!report["next_offset"].is_null());
    let selected = &report["selected"];
    window.set_detail(if selected.is_null() {
        report["selection_error"]
            .as_str()
            .unwrap_or(
                "Save an assembly from the current selection, or choose one from the library.",
            )
            .into()
    } else {
        format!(
            "Objects: {} · Groups: {} · Layers: {} · Faces: {}\nExternal links: {}\n{}",
            selected["objects"],
            selected["groups"],
            selected["layers"],
            selected["faces"],
            selected["external_links"].as_array().map_or(0, Vec::len),
            bounds_label(&selected["bounds"])
        )
        .into()
    });
    form.seen = Some(observation(session, report_revision));
}
fn finish_thumbnail_render(
    window: &PrefabsWindow,
    session: &Session,
    form: &RefCell<Form>,
    result: Result<crate::session::prefabs::Thumbnail, String>,
) {
    refresh(window, session, form, true);
    if let Err(error) = result {
        window.set_has_thumbnail(false);
        window.set_thumbnail(Default::default());
        window.set_thumbnail_status(
            thumbnail_style(window)
                .map_or(error.clone(), |style| thumbnail_error(style, &error))
                .into(),
        );
    }
}

fn callback(
    window: &slint::Weak<PrefabsWindow>,
    continuation: &DocumentContinuation,
    body: impl FnOnce(PrefabsWindow, &mut Session),
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
    pub(crate) fn hide_windows(&self, errors: &mut Vec<String>) {
        crate::ui::hide_window("Prefab Paste", self.paste.window.window(), errors);
        crate::ui::hide_window("Prefabs", self.window.window(), errors);
    }

    pub fn new_with_access(
        _shared: &SharedSession,
        access: &DocumentAccess,
    ) -> Result<Self, slint::PlatformError> {
        let continuation = access.downgrade();
        let window = PrefabsWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        let paste = Rc::new(clipboard_ui::Controller::new_with_access(access)?);
        window.on_choose({
            let (window, continuation, form) =
                (window.as_weak(), continuation.clone(), form.clone());
            move |name| {
                callback(&window, &continuation, |window, session| {
                    form.borrow_mut().selected = Some(name.to_string());
                    refresh(&window, session, &form, true);
                })
            }
        });
        let refresh_callback = || {
            let (window, continuation, form) =
                (window.as_weak(), continuation.clone(), form.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    refresh(&window, session, &form, true)
                })
            }
        };
        window.on_refresh(refresh_callback());
        window.on_search({
            let (window, continuation, form) =
                (window.as_weak(), continuation.clone(), form.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    form.borrow_mut().offset = 0;
                    refresh(&window, session, &form, true);
                })
            }
        });
        window.on_page({
            let (window, continuation, form) =
                (window.as_weak(), continuation.clone(), form.clone());
            move |direction: i32| {
                callback(&window, &continuation, |window, session| {
                    let mut state = form.borrow_mut();
                    state.offset = if direction < 0 {
                        state.offset.saturating_sub(PAGE)
                    } else {
                        state.offset.saturating_add(PAGE)
                    };
                    drop(state);
                    refresh(&window, session, &form, true);
                })
            }
        });
        let begin_callback = |action| {
            let (window, continuation, form) =
                (window.as_weak(), continuation.clone(), form.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    let mut state = form.borrow_mut();
                    let stamp = state.stamp.clone();
                    let source = state.selected.clone();
                    let initial_name = if action == Action::Rename {
                        source.clone().unwrap_or_default()
                    } else {
                        String::new()
                    };
                    state.action = Some(Draft {
                        action,
                        stamp,
                        source,
                    });
                    drop(state);
                    window.set_prefab_name(initial_name.into());
                    refresh(&window, session, &form, true);
                })
            }
        };
        window.on_begin_save(begin_callback(Action::Save));
        window.on_begin_delete(begin_callback(Action::Delete));
        window.on_begin_rename(begin_callback(Action::Rename));
        let cancel_callback = |close| {
            let (window, continuation, form) =
                (window.as_weak(), continuation.clone(), form.clone());
            move || {
                let Some(local_window) = window.upgrade() else {
                    return;
                };
                form.borrow_mut().action = None;
                local_window.set_editing(false);
                local_window.set_saving(false);
                local_window.set_deleting(false);
                local_window.set_renaming(false);
                callback(&window, &continuation, |window, session| {
                    refresh(&window, session, &form, true)
                });
                if close {
                    let _ = local_window.hide();
                }
            }
        };
        window.on_cancel(cancel_callback(false));
        window.on_close_dialog(cancel_callback(true));
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_close_dialog();
                }
                slint::CloseRequestResponse::HideWindow
            }
        });
        let write_callback = |action| {
            let (window, continuation, form) =
                (window.as_weak(), continuation.clone(), form.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    let result = (|| {
                        let state = form.borrow();
                        let draft = state
                            .action
                            .as_ref()
                            .ok_or("Start a library action first")?;
                        if draft.action != action {
                            return Err("Library action changed".into());
                        }
                        match action {
                            Action::Delete => session.delete_prefab(
                                Actor::User,
                                draft.source.as_deref().ok_or("Choose a prefab first")?,
                                &draft.stamp,
                            ),
                            Action::Rename => session.rename_prefab(
                                Actor::User,
                                draft.source.as_deref().ok_or("Choose a prefab first")?,
                                &window.get_prefab_name(),
                                &draft.stamp,
                            ),
                            Action::Save => {
                                let ids = session.selected_ids();
                                session.save_prefab(
                                    Actor::User,
                                    &window.get_prefab_name(),
                                    &ids,
                                    &draft.stamp,
                                )
                            }
                        }
                    })();
                    match result {
                        Ok(report) => {
                            let mut state = form.borrow_mut();
                            if action == Action::Delete {
                                state.selected = None;
                            } else {
                                state.selected = Some(window.get_prefab_name().to_string());
                            }
                            state.action = None;
                            if action == Action::Rename {
                                state.offset = 0;
                                window.set_query("".into());
                            }
                            drop(state);
                            window.set_message(
                            match action {
                                Action::Delete => "Prefab deleted; placed map copies remain.",
                                Action::Save => "Prefab saved without changing the map.",
                                Action::Rename => {
                                    "Prefab renamed. Choose it and preview again before placing it."
                                }
                            }
                            .into(),
                        );
                            refresh(&window, session, &form, true);
                            if let Some(error) = report["error"].as_str() {
                                let message = format!(
                                    "Library write committed; library unavailable: {error}"
                                );
                                window.set_message(message.clone().into());
                                form.borrow_mut().library_message = Some(message);
                            } else if let Some(error) = report["write_error"].as_str() {
                                let message = format!("Library write committed: {error}");
                                window.set_message(message.clone().into());
                                form.borrow_mut().library_message = Some(message);
                            }
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                })
            }
        };
        window.on_save(write_callback(Action::Save));
        window.on_delete(write_callback(Action::Delete));
        window.on_rename(write_callback(Action::Rename));
        window.on_preview({
            let (window, continuation, form, paste) = (
                window.as_weak(),
                continuation.clone(),
                form.clone(),
                paste.clone(),
            );
            move || {
                callback(&window, &continuation, |window, session| {
                    let result = (|| {
                        let state = form.borrow();
                        session.preview_prefab_placement(
                            Actor::User,
                            state.selected.as_deref().ok_or("Choose a prefab first")?,
                            placement(&window)?,
                            window.get_keep_external(),
                            &state.stamp,
                        )
                    })();
                    match result {
                        Ok(_) => {
                            paste.initialize(session, false);
                            if let Err(error) = paste.window.show() {
                                window.set_message(error.to_string().into());
                            }
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                    refresh(&window, session, &form, true);
                })
            }
        });
        window.on_render_gallery({
            let (window, continuation, form) =
                (window.as_weak(), continuation.clone(), form.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    let (stamp, offset) = {
                        let state = form.borrow();
                        (state.stamp.clone(), state.offset)
                    };
                    clear_gallery(&window, &form);
                    window.set_has_thumbnail(false);
                    window.set_thumbnail(Default::default());
                    let result = thumbnail_style(&window).and_then(|style| {
                        session.render_prefab_gallery(
                            &window.get_query(),
                            offset,
                            PAGE,
                            style,
                            &stamp,
                        )
                    });
                    match result {
                        Ok(gallery) => {
                            let rendered = gallery
                                .entries
                                .iter()
                                .filter(|entry| entry.thumbnail.is_ok())
                                .count();
                            let rows = gallery
                                .entries
                                .iter()
                                .map(|entry| {
                                    let (thumbnail, has_image, status) = match &entry.thumbnail {
                                        Ok(thumbnail) => (
                                            thumbnail_pixels(thumbnail),
                                            true,
                                            slint::SharedString::default(),
                                        ),
                                        Err(error) => {
                                            (slint::Image::default(), false, error.clone().into())
                                        }
                                    };
                                    PrefabRow {
                                        name: entry.name.clone().into(),
                                        detail: format!("{} KiB · VMF", entry.bytes.div_ceil(1024))
                                            .into(),
                                        thumbnail,
                                        has_image,
                                        status,
                                    }
                                })
                                .collect();
                            set_rows(&window, rows);
                            window.set_gallery_status(
                                format!("{rendered} of {} images", gallery.entries.len()).into(),
                            );
                            window.set_gallery_rendered(true);
                            form.borrow_mut().gallery = Some(gallery.review);
                        }
                        Err(error) => window.set_gallery_status(error.into()),
                    }
                    refresh(&window, session, &form, true);
                })
            }
        });
        window.on_render_thumbnail({
            let (window, continuation, form) =
                (window.as_weak(), continuation.clone(), form.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    clear_gallery(&window, &form);
                    let result = form
                        .borrow()
                        .selected
                        .as_deref()
                        .ok_or_else(|| "Choose a prefab first".to_string())
                        .and_then(|name| {
                            session.prefab_thumbnail_with_style(
                                name,
                                true,
                                thumbnail_style(&window)?,
                            )
                        });
                    finish_thumbnail_render(&window, session, &form, result);
                })
            }
        });
        window.on_thumbnail_style_changed({
            let (window, continuation, form) =
                (window.as_weak(), continuation.clone(), form.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    refresh(&window, session, &form, true)
                })
            }
        });
        Ok(Self {
            window,
            form,
            paste,
        })
    }
    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        refresh(&self.window, session, &self.form, true);
        self.window.show()
    }
    pub fn refresh(&self, session: &Session) {
        if self.window.window().is_visible() {
            refresh(&self.window, session, &self.form, false);
        }
        if self.paste.window.window().is_visible() {
            self.paste.refresh(session);
        }
    }
}
