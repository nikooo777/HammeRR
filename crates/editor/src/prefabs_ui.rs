use crate::session::prefabs::{GalleryReview, MAX_GALLERY_ENTRIES, ThumbnailStyle};
use crate::{
    PrefabGalleryRow, PrefabRow, PrefabsWindow, clipboard_ui,
    runtime::{SharedSession, lock},
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

#[cfg(test)]
mod gallery_tests;
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

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = PrefabsWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        let paste = Rc::new(clipboard_ui::Controller::new(shared)?);
        window.on_choose({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |name| {
                form.borrow_mut().selected = Some(name.to_string());
                refresh(&window.unwrap(), &lock(&shared), &form, true);
            }
        });
        let refresh_callback = || {
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || refresh(&window.unwrap(), &lock(&shared), &form, true)
        };
        window.on_refresh(refresh_callback());
        window.on_search({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                form.borrow_mut().offset = 0;
                refresh(&window.unwrap(), &lock(&shared), &form, true);
            }
        });
        window.on_page({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |direction: i32| {
                let mut state = form.borrow_mut();
                state.offset = if direction < 0 {
                    state.offset.saturating_sub(PAGE)
                } else {
                    state.offset.saturating_add(PAGE)
                };
                drop(state);
                refresh(&window.unwrap(), &lock(&shared), &form, true);
            }
        });
        let begin_callback = |action| {
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
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
                window.unwrap().set_prefab_name(initial_name.into());
                refresh(&window.unwrap(), &lock(&shared), &form, true);
            }
        };
        window.on_begin_save(begin_callback(Action::Save));
        window.on_begin_delete(begin_callback(Action::Delete));
        window.on_begin_rename(begin_callback(Action::Rename));
        let cancel_callback = |close| {
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                form.borrow_mut().action = None;
                let window = window.unwrap();
                refresh(&window, &lock(&shared), &form, true);
                if close {
                    let _ = window.hide();
                }
            }
        };
        window.on_cancel(cancel_callback(false));
        window.on_close_dialog(cancel_callback(true));
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                window.unwrap().invoke_close_dialog();
                slint::CloseRequestResponse::HideWindow
            }
        });
        let write_callback = |action| {
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
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
                        refresh(&window, &session, &form, true);
                        if let Some(error) = report["error"].as_str() {
                            let message =
                                format!("Library write committed; library unavailable: {error}");
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
            }
        };
        window.on_save(write_callback(Action::Save));
        window.on_delete(write_callback(Action::Delete));
        window.on_rename(write_callback(Action::Rename));
        window.on_preview({
            let (window, shared, form, paste) = (
                window.as_weak(),
                shared.clone(),
                form.clone(),
                paste.clone(),
            );
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
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
                        paste.initialize(&mut session, false);
                        if let Err(error) = paste.window.show() {
                            window.set_message(error.to_string().into());
                        }
                    }
                    Err(error) => window.set_message(error.into()),
                }
                refresh(&window, &session, &form, true);
            }
        });
        window.on_render_gallery({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let (stamp, offset) = {
                    let state = form.borrow();
                    (state.stamp.clone(), state.offset)
                };
                clear_gallery(&window, &form);
                window.set_has_thumbnail(false);
                window.set_thumbnail(Default::default());
                let result = thumbnail_style(&window).and_then(|style| {
                    session.render_prefab_gallery(&window.get_query(), offset, PAGE, style, &stamp)
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
                refresh(&window, &session, &form, true);
            }
        });
        window.on_render_thumbnail({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                clear_gallery(&window, &form);
                let result = form
                    .borrow()
                    .selected
                    .as_deref()
                    .ok_or_else(|| "Choose a prefab first".to_string())
                    .and_then(|name| {
                        session.prefab_thumbnail_with_style(name, true, thumbnail_style(&window)?)
                    });
                finish_thumbnail_render(&window, &session, &form, result);
            }
        });
        window.on_thumbnail_style_changed({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || refresh(&window.unwrap(), &lock(&shared), &form, true)
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

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;
    use std::sync::{Arc, Mutex};

    #[test]
    fn peer_catalog_refresh_preserves_frozen_actions_clamps_pages_and_clears_repaired_errors() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let authority =
            editor_core::prefabs::Authority::from_directory(Ok(directory.path().into()));
        let mut origin = Session::with_prefabs(Err("No assets required".into()), authority.clone());
        let mut peer = Session::with_prefabs(Err("No assets required".into()), authority);
        for session in [&mut origin, &mut peer] {
            session
                .create_block(
                    Actor::User,
                    glam::DVec3::ZERO,
                    glam::DVec3::splat(32.0),
                    "brick/test",
                    false,
                )
                .unwrap();
        }
        let peer_ids = peer.selected_ids();
        for index in 0..9 {
            let stamp = peer.prefab_report("", 0, PAGE, None)["snapshot"]
                .as_str()
                .unwrap()
                .to_string();
            peer.save_prefab(Actor::Agent, &format!("Item{index:03}"), &peer_ids, &stamp)
                .unwrap();
        }
        let shared = Arc::new(Mutex::new(origin));
        let controller = Controller::new(&shared).unwrap();
        controller.show(&lock(&shared)).unwrap();
        controller.window.invoke_begin_save();
        controller.window.set_prefab_name("Typed name".into());
        controller.window.set_center_x("123".into());
        let frozen = controller
            .form
            .borrow()
            .action
            .as_ref()
            .unwrap()
            .stamp
            .clone();
        let stamp = peer.prefab_report("", 0, PAGE, None)["snapshot"]
            .as_str()
            .unwrap()
            .to_string();
        peer.save_prefab(Actor::Agent, "Peer", &peer_ids, &stamp)
            .unwrap();
        controller.refresh(&lock(&shared));
        assert_eq!(controller.window.get_prefab_name(), "Typed name");
        assert_eq!(controller.window.get_center_x(), "123");
        assert_eq!(
            controller.form.borrow().action.as_ref().unwrap().stamp,
            frozen
        );
        controller.window.invoke_save();
        assert!(controller.window.get_message().contains("changed"));
        assert!(controller.window.get_editing());
        assert!(!directory.path().join("Typed name.vmf").exists());
        controller.window.invoke_cancel();
        controller.window.invoke_choose("Item008".into());
        controller.window.invoke_begin_rename();
        controller.window.set_prefab_name("Typed rename".into());
        let stamp = peer.prefab_report("", 0, PAGE, None)["snapshot"]
            .as_str()
            .unwrap()
            .to_string();
        peer.rename_prefab(Actor::Agent, "Item008", "Renamed", &stamp)
            .unwrap();
        controller.refresh(&lock(&shared));
        assert_eq!(controller.window.get_selected(), "Item008");
        assert_eq!(controller.window.get_prefab_name(), "Typed rename");
        controller.window.invoke_rename();
        assert!(controller.window.get_message().contains("changed"));
        controller.window.invoke_cancel();
        assert_eq!(controller.window.get_selected(), "");
        assert!(!controller.window.get_can_preview());
        controller.window.invoke_choose("Item007".into());
        controller.window.invoke_begin_delete();
        let stamp = peer.prefab_report("", 0, PAGE, None)["snapshot"]
            .as_str()
            .unwrap()
            .to_string();
        peer.delete_prefab(Actor::Agent, "Item007", &stamp).unwrap();
        controller.refresh(&lock(&shared));
        assert_eq!(controller.window.get_selected(), "Item007");
        controller.window.invoke_delete();
        assert!(controller.window.get_message().contains("changed"));
        controller.window.invoke_cancel();
        controller.window.invoke_page(1);
        assert_eq!(controller.form.borrow().offset, PAGE);
        let stamp = peer.prefab_report("", 0, PAGE, None)["snapshot"]
            .as_str()
            .unwrap()
            .to_string();
        peer.delete_prefab(Actor::Agent, "Renamed", &stamp).unwrap();
        controller.refresh(&lock(&shared));
        assert_eq!(controller.form.borrow().offset, 0);
        assert_eq!(controller.window.get_entries().row_count(), PAGE);
        std::fs::create_dir(directory.path().join("Blocked.vmf")).unwrap();
        peer.prefab_report("", 0, PAGE, None);
        controller.refresh(&lock(&shared));
        assert!(controller.window.get_message().contains("regular VMF"));
        assert_eq!(controller.window.get_entries().row_count(), 0);
        assert!(controller.window.get_detail().contains("regular VMF"));
        std::fs::remove_dir(directory.path().join("Blocked.vmf")).unwrap();
        peer.prefab_report("", 0, PAGE, None);
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_message().contains("regular VMF"));
        assert_eq!(controller.window.get_entries().row_count(), PAGE);
        std::fs::write(directory.path().join("Malformed.vmf"), "not VMF").unwrap();
        controller.window.invoke_refresh();
        controller.window.invoke_choose("Malformed".into());
        assert_eq!(controller.window.get_selected(), "Malformed");
        assert!(!controller.window.get_can_preview());
        controller.window.invoke_begin_delete();
        controller.window.invoke_delete();
        assert!(!directory.path().join("Malformed.vmf").exists());
        assert!(!controller.window.get_editing());
    }

    #[test]
    fn prefab_thumbnail_controls_show_cpu_diagnostics_and_preserve_owned_placement() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (session, directory, _) = crate::test_assets::prefabs();
        let source = session.document().vmf().clone();
        let history = session.history_snapshot();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.show(&lock(&shared)).unwrap();
        assert!(!controller.window.get_has_thumbnail());
        assert!(!controller.window.get_can_render_thumbnail());
        controller.window.invoke_begin_save();
        controller.window.set_prefab_name("Visual fixture".into());
        controller.window.invoke_save();
        controller.window.invoke_choose("Visual fixture".into());
        assert!(controller.window.get_can_render_thumbnail());
        assert!(!controller.window.get_has_thumbnail());
        assert!(
            controller
                .window
                .get_thumbnail_status()
                .contains("Ready to render")
        );
        controller.window.invoke_preview();
        let rendered = lock(&shared).render_document().vmf().clone();
        let clipboard = lock(&shared).clipboard_report();
        assert_eq!(controller.window.get_thumbnail_style(), 0);
        controller.window.set_thumbnail_style(1);
        controller.window.invoke_thumbnail_style_changed();
        assert!(!controller.window.get_has_thumbnail());
        assert!(!controller.window.get_can_render_thumbnail());
        assert!(controller.window.get_thumbnail_status().contains("missing"));
        controller.window.invoke_render_thumbnail();
        assert!(!controller.window.get_has_thumbnail());
        assert_eq!(lock(&shared).render_document().vmf(), &rendered);
        assert_eq!(lock(&shared).clipboard_report(), clipboard);
        crate::ui::tests::save_ui_snapshot(
            controller.window.window(),
            "prefab-textured-thumbnail-error",
        );
        controller.window.set_thumbnail_style(0);
        controller.window.invoke_thumbnail_style_changed();
        assert!(controller.window.get_can_render_thumbnail());
        std::fs::write(directory.path().join("Visual fixture.vmf"), "invalid VMF").unwrap();
        let clipboard = lock(&shared).clipboard_report();
        controller.window.invoke_refresh();
        assert!(!controller.window.get_can_render_thumbnail());
        assert!(!controller.window.get_has_thumbnail());
        assert!(controller.window.get_thumbnail_status().contains("source"));
        controller.window.invoke_render_thumbnail();
        assert!(!controller.window.get_has_thumbnail());
        assert!(controller.window.get_thumbnail_status().contains("source"));
        assert_eq!(
            lock(&shared).prefab_thumbnail_report("Visual fixture")["graphics"]["initialized"],
            false
        );
        assert_eq!(lock(&shared).document().vmf(), &source);
        assert_eq!(lock(&shared).render_document().vmf(), &rendered);
        assert_eq!(lock(&shared).clipboard_report(), clipboard);
        assert_eq!(lock(&shared).history_snapshot(), history);
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "prefab-thumbnail-failure");
    }

    #[test]
    #[ignore = "requires a graphics adapter for actual prefab thumbnail UI validation"]
    fn prefab_thumbnail_controls_render_refresh_and_clear_actual_pixels_on_source_failure() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (session, directory, assets, _) = crate::test_assets::textured_prefabs();
        let source = session.document().vmf().clone();
        let history = session.history_snapshot();
        let clipboard = session.clipboard_report();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.show(&lock(&shared)).unwrap();
        controller.window.invoke_begin_save();
        controller
            .window
            .set_prefab_name("Visual brush assembly".into());
        controller.window.invoke_save();
        controller
            .window
            .invoke_choose("Visual brush assembly".into());
        controller.window.invoke_render_thumbnail();
        assert!(
            controller.window.get_has_thumbnail(),
            "{}",
            controller.window.get_thumbnail_status()
        );
        let first = lock(&shared)
            .cached_prefab_thumbnail("Visual brush assembly")
            .unwrap()
            .unwrap();
        assert!(
            first
                .image
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                > 3
        );
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "prefab-thumbnail");
        controller.window.invoke_render_thumbnail();
        assert!(controller.window.get_has_thumbnail());
        let refreshed = lock(&shared)
            .cached_prefab_thumbnail("Visual brush assembly")
            .unwrap()
            .unwrap();
        assert!(!Arc::ptr_eq(&first.image, &refreshed.image));
        controller.window.set_thumbnail_style(1);
        controller.window.invoke_thumbnail_style_changed();
        assert!(!controller.window.get_has_thumbnail());
        assert!(controller.window.get_can_render_thumbnail());
        controller.window.invoke_render_thumbnail();
        assert!(
            controller.window.get_has_thumbnail(),
            "{}",
            controller.window.get_thumbnail_status()
        );
        let textured = lock(&shared)
            .cached_prefab_thumbnail_with_style("Visual brush assembly", ThumbnailStyle::Textured)
            .unwrap()
            .unwrap();
        assert_ne!(textured.image.pixels, first.image.pixels);
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "prefab-textured-thumbnail");
        let texture = assets.path().join("materials/test/checker.vtf");
        let original_texture = std::fs::read(&texture).unwrap();
        let mut changed = original_texture.clone();
        changed[80] ^= 0xff;
        std::fs::write(&texture, changed).unwrap();
        finish_thumbnail_render(
            &controller.window,
            &lock(&shared),
            &controller.form,
            Ok(textured),
        );
        assert!(!controller.window.get_has_thumbnail());
        assert!(
            controller
                .window
                .get_thumbnail_status()
                .contains("source changed")
        );
        std::fs::write(texture, original_texture).unwrap();
        controller.window.invoke_render_thumbnail();
        assert!(controller.window.get_has_thumbnail());
        controller.window.set_thumbnail_style(0);
        controller.window.invoke_thumbnail_style_changed();
        assert!(controller.window.get_has_thumbnail());
        assert!(Arc::ptr_eq(
            &refreshed.image,
            &lock(&shared)
                .cached_prefab_thumbnail("Visual brush assembly")
                .unwrap()
                .unwrap()
                .image
        ));
        std::fs::remove_file(directory.path().join("Visual brush assembly.vmf")).unwrap();
        controller.window.invoke_refresh();
        assert!(!controller.window.get_has_thumbnail());
        assert!(!controller.window.get_can_render_thumbnail());
        assert!(
            controller
                .window
                .get_thumbnail_status()
                .contains("no longer exists")
        );
        assert_eq!(lock(&shared).document().vmf(), &source);
        assert_eq!(lock(&shared).clipboard_report(), clipboard);
        assert_eq!(lock(&shared).history_snapshot(), history);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn prefab_rename_controls_cancel_guard_collisions_and_stale_owned_placement() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (session, directory, brush) = crate::test_assets::prefabs();
        let original = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.show(&lock(&shared)).unwrap();
        for name in ["Alpha assembly", "Beta assembly"] {
            controller.window.invoke_begin_save();
            controller.window.set_prefab_name(name.into());
            controller.window.invoke_save();
        }
        let bytes = std::fs::read(directory.path().join("Alpha assembly.vmf")).unwrap();
        controller.window.invoke_choose("Alpha assembly".into());
        controller.window.invoke_preview();
        let clipboard = lock(&shared).clipboard_report();
        controller.window.invoke_begin_rename();
        assert_eq!(controller.window.get_prefab_name(), "Alpha assembly");
        controller.window.set_prefab_name("Renamed assembly".into());
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "prefab-rename");
        controller.window.invoke_cancel();
        assert!(directory.path().join("Alpha assembly.vmf").exists());
        assert!(!directory.path().join("Renamed assembly.vmf").exists());
        assert_eq!(lock(&shared).clipboard_report(), clipboard);
        controller.window.invoke_begin_rename();
        controller.window.set_prefab_name("beta assembly".into());
        controller.window.invoke_rename();
        assert!(controller.window.get_renaming());
        assert!(controller.window.get_message().contains("already exists"));
        assert_eq!(lock(&shared).clipboard_report(), clipboard);
        controller.window.set_prefab_name("Renamed assembly".into());
        controller.window.invoke_rename();
        assert!(!controller.window.get_editing());
        assert_eq!(controller.window.get_selected(), "Renamed assembly");
        assert_eq!(
            std::fs::read(directory.path().join("Renamed assembly.vmf")).unwrap(),
            bytes
        );
        controller.paste.refresh(&lock(&shared));
        assert!(!controller.paste.window.get_ready());
        assert!(controller.paste.window.get_message().contains("renamed"));
        controller.paste.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        controller.window.invoke_preview();
        assert!(controller.paste.window.get_ready());
        controller.paste.window.invoke_cancel();
        controller.refresh(&lock(&shared));
        controller.window.invoke_begin_rename();
        controller.window.set_prefab_name("Outdated action".into());
        let mut session = lock(&shared);
        let stamp = session.prefab_report("", 0, 64, None)["snapshot"]
            .as_str()
            .unwrap()
            .to_string();
        session
            .save_prefab(Actor::Agent, "Other entry", &[brush], &stamp)
            .unwrap();
        drop(session);
        controller.refresh(&lock(&shared));
        controller.window.invoke_rename();
        assert!(controller.window.get_message().contains("changed"));
        assert!(!directory.path().join("Outdated action.vmf").exists());
        controller
            .window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Escape.into(),
            });
        assert!(!controller.window.get_editing());
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
    }
    #[test]
    fn prefab_transform_controls_review_exact_geometry_and_retain_foreign_preview() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (session, directory, _) = crate::test_assets::prefabs();
        let original = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.show(&lock(&shared)).unwrap();
        controller.window.invoke_begin_save();
        controller
            .window
            .set_prefab_name("Scaled brush assembly".into());
        controller.window.invoke_save();
        let file = std::fs::read(directory.path().join("Scaled brush assembly.vmf")).unwrap();
        controller
            .window
            .invoke_choose("Scaled brush assembly".into());
        controller.window.set_center_x("128".into());
        controller.window.set_center_y("64".into());
        controller.window.set_center_z("16".into());
        controller.window.set_rotation_z("90".into());
        controller.window.set_scale_x("2".into());
        controller.window.set_scale_y("0.5".into());
        controller.window.set_scale_z("1.25".into());
        controller.window.set_pivot_mode(2);
        controller.window.set_pivot_x("8".into());
        controller.window.set_pivot_y("4".into());
        controller.window.invoke_preview();
        let report = lock(&shared).clipboard_report();
        assert_eq!(report["preview"]["valid"], true);
        assert!(controller.paste.window.get_ready());
        assert_eq!(lock(&shared).document().vmf(), &original);
        let expected = lock(&shared).render_document().vmf().clone();
        let bounds = &report["preview"]["bounds"];
        for (actual, expected) in bounds
            .as_array()
            .unwrap()
            .iter()
            .zip([[114.0, 48.0, 16.0], [130.0, 112.0, 56.0]])
        {
            let actual: [f64; 3] = serde_json::from_value(actual.clone()).unwrap();
            assert!(
                glam::DVec3::from_array(actual)
                    .abs_diff_eq(glam::DVec3::from_array(expected), 1e-8)
            );
        }
        crate::ui::tests::save_ui_snapshot(
            controller.window.window(),
            "prefab-transformed-placement",
        );
        crate::ui::tests::save_ui_snapshot(
            controller.paste.window.window(),
            "prefab-transformed-paste",
        );
        controller.window.set_rotation_x("NaN".into());
        controller.window.invoke_preview();
        assert!(controller.window.get_message().contains("finite"));
        assert_eq!(lock(&shared).clipboard_report(), report);
        controller.window.set_rotation_x("0".into());
        controller.paste.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &expected);
        assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
        assert_eq!(
            std::fs::read(directory.path().join("Scaled brush assembly.vmf")).unwrap(),
            file
        );
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        controller.refresh(&lock(&shared));
        controller.window.invoke_preview();
        let mut session = lock(&shared);
        let stamp = session.prefab_report("", 0, 64, None)["snapshot"]
            .as_str()
            .unwrap()
            .to_string();
        session
            .preview_prefab(
                Actor::Agent,
                "Scaled brush assembly",
                glam::DVec3::splat(256.0),
                false,
                &stamp,
            )
            .unwrap();
        let replacement = session.clipboard_report();
        drop(session);
        controller.paste.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        controller.paste.window.invoke_cancel();
        assert_eq!(lock(&shared).clipboard_report(), replacement);
    }

    #[test]
    fn prefab_controls_share_capture_review_apply_and_cancel_delete() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (mut session, directory, brush) = crate::test_assets::prefabs();
        let mut ids = vec![brush];
        for x in [0.0, 32.0] {
            ids.push(
                session
                    .create_entity(
                        Actor::User,
                        "light",
                        Some(glam::DVec3::new(x, 0.0, 48.0)),
                        std::collections::BTreeMap::new(),
                    )
                    .unwrap(),
            );
        }
        session
            .select(&ids, crate::session::SelectMode::Replace)
            .unwrap();
        let original = session.document().vmf().clone();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.show(&lock(&shared)).unwrap();
        controller.window.invoke_begin_save();
        controller
            .window
            .set_prefab_name("Corridor light cluster".into());
        controller.window.invoke_cancel();
        assert!(!directory.path().join("Corridor light cluster.vmf").exists());
        controller.window.invoke_begin_save();
        controller
            .window
            .set_prefab_name("Corridor light cluster".into());
        controller.window.invoke_save();
        assert!(directory.path().join("Corridor light cluster.vmf").exists());
        assert_eq!(lock(&shared).document().vmf(), &original);
        controller
            .window
            .invoke_choose("Corridor light cluster".into());
        controller.window.set_center_x("80".into());
        controller.window.set_center_z("48".into());
        controller.window.invoke_preview();
        assert!(lock(&shared).clipboard_report()["preview"]["valid"] == true);
        controller.paste.window.invoke_cancel();
        assert_eq!(lock(&shared).document().vmf(), &original);
        controller.refresh(&lock(&shared));
        controller.window.invoke_preview();
        controller.paste.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf().world.solids.len(), 2);
        assert_eq!(lock(&shared).document().vmf().entities.len(), 4);
        controller.refresh(&lock(&shared));
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "prefab-library");
        controller.window.invoke_begin_delete();
        controller.window.invoke_cancel();
        assert!(directory.path().join("Corridor light cluster.vmf").exists());
        controller.window.invoke_begin_delete();
        controller.window.invoke_delete();
        assert!(!directory.path().join("Corridor light cluster.vmf").exists());
        assert_eq!(lock(&shared).document().vmf().world.solids.len(), 2);
    }
}
