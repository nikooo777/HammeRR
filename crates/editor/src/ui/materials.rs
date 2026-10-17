use super::*;
use editor_core::runtime::{DocumentAccess, DocumentContinuation};

#[cfg(test)]
#[path = "materials_alpha_tests.rs"]
mod alpha_tests;

pub(crate) fn preview_image(preview: &session::MaterialPreview) -> slint::Image {
    preview
        .image
        .as_ref()
        .map_or_else(slint::Image::default, |image| {
            let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &image.pixels,
                image.width,
                image.height,
            );
            slint::Image::from_rgba8(buffer)
        })
}

pub(super) fn refresh_material_preview(app: &AppWindow, session: &mut Session) {
    match session.material_preview(&app.get_material()) {
        Ok(preview) => {
            app.set_has_material_preview(preview.image.is_some());
            app.set_material_preview(preview_image(&preview));
        }
        Err(_) => {
            app.set_has_material_preview(false);
            app.set_material_preview(slint::Image::default());
        }
    }
}

pub(crate) fn refresh_material_window(window: &MaterialWindow, session: &mut Session) {
    let favorites = session.material_favorites_snapshot();
    refresh_material_window_with_favorites(window, session, &favorites);
}

fn refresh_material_window_with_favorites(
    window: &MaterialWindow,
    session: &mut Session,
    favorites: &material_favorites::Snapshot,
) {
    if window.get_favorites_revision() != favorites.serial.to_string() {
        refresh_material_catalog_with_favorites(window, session, favorites);
    } else {
        refresh_favorites_status(window, favorites);
    }
    window.set_asset_generation(session.materials().generation().to_string().into());
    window.set_reload_status(
        session
            .material_reload_report()
            .map_or_else(String::new, |report| {
                std::iter::once(report.summary.clone())
                    .chain(
                        report
                            .failures
                            .iter()
                            .map(|failure| format!("{}: {}", failure.material, failure.error)),
                    )
                    .chain((report.failed > report.failures.len()).then(|| {
                        format!(
                            "Only the first {} unavailable previews are listed.",
                            render::materials::MAX_RELOAD_FAILURES
                        )
                    }))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .into(),
    );
    let channel = if window.get_preview_channel() == 1 {
        session::MaterialPreviewChannel::Alpha
    } else {
        session::MaterialPreviewChannel::Color
    };
    match material_picker_reference(window, &window.get_material())
        .and_then(|material| session.material_preview_channel(&material, channel))
    {
        Ok(preview) => {
            window.set_available(preview.image.is_some());
            window.set_preview(preview_image(&preview));
            let mut detail = format!(
                "{}\n{} × {}{}\n{}",
                preview.name,
                preview.width,
                preview.height,
                if preview.missing {
                    " · Missing texture"
                } else if preview.tool {
                    " · Tool material"
                } else {
                    ""
                },
                preview.channel.label(),
            );
            if let Some([low, high]) = preview.alpha_range {
                detail.push_str(&format!(" · source alpha {low}–{high}"));
            }
            if let Some(source) = &preview.source {
                detail.push_str(&format!(
                    "\n{}\n{} · mip {} · VMT {}",
                    source.texture,
                    if source.cached {
                        "Cached display"
                    } else {
                        "Current load"
                    },
                    source.mip,
                    source.material_identity,
                ));
            }
            if channel == session::MaterialPreviewChannel::Alpha {
                detail.push_str("\nAlpha bytes as grayscale; shader opacity is separate.");
            }
            window.set_detail(detail.into());
        }
        Err(error) => {
            window.set_available(false);
            window.set_preview(slint::Image::default());
            window.set_detail(error.into());
        }
    }
}

fn with_material_window<R>(
    window: &slint::Weak<MaterialWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(&MaterialWindow, &SharedSession) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    let result = target.upgrade().and_then(|access| {
        access.with_shared(|shared| {
            #[cfg(test)]
            {
                let hook = BEFORE_MATERIAL_ACTION.with(|slot| slot.borrow_mut().take());
                if let Some(hook) = hook {
                    hook();
                }
            }
            Ok(body(&window, shared))
        })
    });
    match result {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_detail(error.into());
            None
        }
    }
}

#[cfg(test)]
thread_local! {
    static BEFORE_MATERIAL_ACTION: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn bind_material_preview(window: &MaterialWindow, shared: SharedSession) {
    bind_material_preview_with_access(window, &DocumentAccess::unmanaged(shared));
}

fn bind_material_preview_with_access(window: &MaterialWindow, access: &DocumentAccess) {
    window.on_preview_material({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            with_material_window(&window, &target, |window, shared| {
                refresh_material_window(window, &mut lock(shared));
            });
        }
    });
}

fn bind_material_search_with_access(
    window: &MaterialWindow,
    access: &DocumentAccess,
    alive: impl Fn() -> bool + 'static,
) {
    window.on_search({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            if !alive() {
                return;
            }
            with_material_window(&window.as_weak(), &target, |window, shared| {
                refresh_material_catalog(window, &mut lock(shared));
            });
        }
    });
}

pub(super) fn optional_number(value: SharedString) -> Result<Option<f64>, String> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    let number = value
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("Enter a number: {value}"))?;
    if !number.is_finite() {
        return Err("Texture values must be finite".into());
    }
    Ok(Some(number))
}

pub(super) fn surface_patch(
    window: &SurfaceWindow,
) -> Result<map_core::surface::SurfaceEdit, String> {
    use map_core::surface::{Alignment, Justify};
    let justify = |index| match index {
        1 => Some((Justify::Start, 0)),
        2 => Some((Justify::Center, 0)),
        3 => Some((Justify::End, 0)),
        _ => None,
    };
    Ok(map_core::surface::SurfaceEdit {
        material: (!window.get_material().trim().is_empty())
            .then(|| window.get_material().trim().to_owned()),
        shift_u: optional_number(window.get_shift_u())?,
        shift_v: optional_number(window.get_shift_v())?,
        scale_u: optional_number(window.get_scale_u())?,
        scale_v: optional_number(window.get_scale_v())?,
        u_axis: window
            .get_direct_vectors()
            .then(|| -> Result<_, String> {
                Ok([
                    optional_number(window.get_u_x())?,
                    optional_number(window.get_u_y())?,
                    optional_number(window.get_u_z())?,
                ])
            })
            .transpose()?,
        v_axis: window
            .get_direct_vectors()
            .then(|| -> Result<_, String> {
                Ok([
                    optional_number(window.get_v_x())?,
                    optional_number(window.get_v_y())?,
                    optional_number(window.get_v_z())?,
                ])
            })
            .transpose()?,
        rotation: if window.get_direct_vectors() {
            None
        } else {
            optional_number(window.get_rotation())?
        },
        alignment: match (window.get_direct_vectors(), window.get_alignment_index()) {
            (false, 1) => Some(Alignment::World),
            (false, 2) => Some(Alignment::Face),
            _ => None,
        },
        justify_u: justify(window.get_justify_u_index()),
        justify_v: justify(window.get_justify_v_index()),
        ..Default::default()
    })
}

pub(super) fn apply_surface_dialog(
    window: &SurfaceWindow,
    session: &mut Session,
    target: &(Vec<map_core::surface::FaceId>, u64, u64),
    preview: bool,
) -> Result<usize, String> {
    if session.document_generation() != target.1
        || session.document().revision() != target.2
        || session.selected_faces() != target.0
    {
        return Err("Map or face selection changed. Cancel and reopen texture editing.".into());
    }
    if window.get_operation_index() == 4 {
        let changed = session
            .propagate_face_texture(Actor::User, &target.0, preview)?
            .changed;
        retain_surface_preview(window, session, preview);
        return Ok(changed);
    }
    if window.get_operation_index() != 0 {
        use map_core::surface::PaintMode;
        let changed = session.paint_face_texture(
            Actor::User,
            &target.0,
            match window.get_operation_index() {
                1 => PaintMode::Material,
                2 => PaintMode::Projection,
                _ => PaintMode::Wrap,
            },
            preview,
        )?;
        retain_surface_preview(window, session, preview);
        return Ok(changed);
    }
    let patch = surface_patch(window)?;
    if window.get_direct_vectors() && !preview {
        if window.get_reviewed_fields().as_str() != surface_fields(window)
            || window.get_reviewed_token().is_empty()
        {
            window.set_ready(false);
            return Err("Vector fields changed. Preview textures again before applying.".into());
        }
        let token = window
            .get_reviewed_token()
            .parse::<u64>()
            .map_err(|_| "Preview textures again")?;
        session.check_face_preview_token(Some(token))?;
        return session.commit_face_preview(Actor::User);
    }
    let changed = session.edit_faces(Actor::User, &target.0, patch, window.get_fit(), preview)?;
    retain_surface_preview(window, session, preview);
    if window.get_direct_vectors() && preview {
        window.set_reviewed_fields(surface_fields(window).into());
        window.set_reviewed_token(window.get_owned_token());
        window.set_ready(true);
    }
    Ok(changed)
}

fn retain_surface_preview(window: &SurfaceWindow, session: &Session, preview: bool) {
    if preview {
        window.set_owned_token(session.face_preview_token().unwrap().to_string().into());
    }
}

fn cancel_surface_dialog(window: &SurfaceWindow, session: &mut Session) {
    if window.get_direct_vectors() && window.get_operation_index() == 0 {
        let owned = window.get_owned_token().parse::<u64>().ok();
        if owned.is_some() && owned == session.face_preview_token() {
            session.cancel_face_preview();
        }
    } else {
        session.cancel_face_preview();
    }
}

fn surface_fields(window: &SurfaceWindow) -> String {
    serde_json::json!([
        window.get_material().as_str(),
        window.get_shift_u().as_str(),
        window.get_shift_v().as_str(),
        window.get_scale_u().as_str(),
        window.get_scale_v().as_str(),
        window.get_rotation().as_str(),
        window.get_u_x().as_str(),
        window.get_u_y().as_str(),
        window.get_u_z().as_str(),
        window.get_v_x().as_str(),
        window.get_v_y().as_str(),
        window.get_v_z().as_str(),
        window.get_fit(),
        window.get_alignment_index(),
        window.get_justify_u_index(),
        window.get_justify_v_index(),
        window.get_direct_vectors(),
        window.get_operation_index(),
    ])
    .to_string()
}

fn load_surface_vectors(
    window: &SurfaceWindow,
    document: &map_core::Document,
    faces: &[map_core::surface::FaceId],
) {
    window.set_u_x(common_face_number(document, faces, |side| {
        side.u_axis.axis.x
    }));
    window.set_u_y(common_face_number(document, faces, |side| {
        side.u_axis.axis.y
    }));
    window.set_u_z(common_face_number(document, faces, |side| {
        side.u_axis.axis.z
    }));
    window.set_v_x(common_face_number(document, faces, |side| {
        side.v_axis.axis.x
    }));
    window.set_v_y(common_face_number(document, faces, |side| {
        side.v_axis.axis.y
    }));
    window.set_v_z(common_face_number(document, faces, |side| {
        side.v_axis.axis.z
    }));
}

pub(crate) fn common_face_number(
    document: &map_core::Document,
    faces: &[map_core::surface::FaceId],
    value: fn(&source_formats::vmf::Side) -> f64,
) -> SharedString {
    let values: Vec<_> = faces
        .iter()
        .filter_map(|face| map_core::surface::side(document, *face))
        .map(value)
        .collect();
    values
        .first()
        .filter(|first| values.iter().all(|value| value == *first))
        .map(|value| value.to_string())
        .unwrap_or_default()
        .into()
}

pub(super) fn refresh_surface_dialog(
    window: &SurfaceWindow,
    session: &mut Session,
    target: &(Vec<map_core::surface::FaceId>, u64, u64),
) {
    refresh_surface_sample(window, session);
    let stale = session.document_generation() != target.1
        || session.document().revision() != target.2
        || session.selected_faces() != target.0;
    window.set_stale(stale);
    if stale {
        window.set_ready(false);
        window.set_message(
            "Map or face selection changed. Cancel and reopen texture editing.".into(),
        );
        return;
    }
    if window.get_direct_vectors() && window.get_operation_index() == 0 {
        let reviewed = session.face_preview_token().map(|token| token.to_string());
        let ready = reviewed.as_deref() == Some(window.get_reviewed_token().as_str())
            && window.get_reviewed_fields().as_str() == surface_fields(window);
        window.set_ready(ready);
        window.set_message(if ready {
            "Shared vector preview shown in the viewport. Apply textures keeps one Undo step."
                .into()
        } else {
            "Preview textures to review the current vector fields before applying.".into()
        });
        return;
    }
    let Some(document) = session.face_preview_document() else {
        window.set_message("No active preview. Preview or apply the current fields.".into());
        return;
    };
    if session.preview_faces() != target.0 {
        window.set_message(
            "A preview targets other faces. Preview these fields to replace it.".into(),
        );
        return;
    }
    let names: Vec<_> = target
        .0
        .iter()
        .filter_map(|face| map_core::surface::side(document, *face))
        .map(|side| side.material.to_ascii_lowercase())
        .collect();
    let material = names
        .first()
        .filter(|first| names.iter().all(|name| name == *first))
        .cloned()
        .unwrap_or_default();
    window.set_material(material.clone().into());
    window.set_shift_u(common_face_number(document, &target.0, |side| {
        side.u_axis.offset
    }));
    window.set_shift_v(common_face_number(document, &target.0, |side| {
        side.v_axis.offset
    }));
    window.set_scale_u(common_face_number(document, &target.0, |side| {
        side.u_axis.scale
    }));
    window.set_scale_v(common_face_number(document, &target.0, |side| {
        side.v_axis.scale
    }));
    window.set_rotation(common_face_number(document, &target.0, |side| {
        side.rotation
    }));
    load_surface_vectors(window, document, &target.0);
    window.set_preview(
        session
            .material_preview(&material)
            .ok()
            .map(|preview| preview_image(&preview))
            .unwrap_or_default(),
    );
    window.set_message("Shared preview shown in the viewport. Apply to keep it.".into());
}

pub(super) fn refresh_surface_sample(window: &SurfaceWindow, session: &mut Session) {
    let sample = session.surface_sample().cloned();
    window.set_sample_present(sample.is_some());
    window.set_sample_label(
        sample
            .as_ref()
            .map(|sample| {
                format!(
                    "{} · brush {}, face {}",
                    sample.material.to_ascii_lowercase(),
                    sample.face.solid,
                    sample.face.side
                )
            })
            .unwrap_or_else(|| {
                "Select one source face and choose Sample face texture first.".into()
            })
            .into(),
    );
    window.set_sample_preview(
        sample
            .and_then(|sample| session.material_preview(&sample.material).ok())
            .map(|preview| preview_image(&preview))
            .unwrap_or_default(),
    );
}

pub(super) fn material_browser(app: slint::Weak<AppWindow>, ui: WeakUi) -> impl Fn(&str) {
    let target = ui.upgrade().map(|ui| file_access(&ui).downgrade());
    move |key| {
        let (Some(app), Some(ui), Some(target)) = (app.upgrade(), ui.upgrade(), target.as_ref())
        else {
            return;
        };
        let result = target.upgrade().and_then(|access| {
            access.with_shared(|_| {
                show_material_window(&app, &ui, key);
                Ok(())
            })
        });
        if let Err(error) = result {
            app.set_message(error.into());
        }
    }
}

type SurfaceTarget = (Vec<map_core::surface::FaceId>, u64, u64);

fn with_surface_window<R>(
    window: &slint::Weak<SurfaceWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&SurfaceWindow, &SharedSession) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    let result = access
        .upgrade()
        .and_then(|access| access.with_shared(|shared| Ok(body(&window, shared))));
    match result {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_message(error.into());
            None
        }
    }
}

fn bind_surface_edits_with_access(
    window: &SurfaceWindow,
    access: &DocumentAccess,
    alive: impl Fn() -> bool + 'static,
    target: impl Fn() -> Option<SurfaceTarget> + 'static,
) {
    let alive: Rc<dyn Fn() -> bool> = Rc::new(alive);
    let target: Rc<dyn Fn() -> Option<SurfaceTarget>> = Rc::new(target);
    window.on_preview_edit({
        let (window, access, alive, target) = (
            window.as_weak(),
            access.downgrade(),
            alive.clone(),
            target.clone(),
        );
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            if !alive() {
                return;
            }
            with_surface_window(&window.as_weak(), &access, |window, shared| {
                if let Some(target) = target() {
                    let result = apply_surface_dialog(window, &mut lock(shared), &target, true);
                    window.set_message(match result {
                        Ok(_) => "Preview shown in the viewport. Apply to keep it.".into(),
                        Err(error) => error.into(),
                    });
                }
            });
        }
    });
    window.on_apply({
        let (window, access, alive, target) =
            (window.as_weak(), access.downgrade(), alive.clone(), target);
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            if !alive() {
                return;
            }
            with_surface_window(&window.as_weak(), &access, |window, shared| {
                if let Some(target) = target() {
                    let result = apply_surface_dialog(window, &mut lock(shared), &target, false);
                    match result {
                        Ok(_) => {
                            let _ = window.hide();
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                }
            });
        }
    });
    window.on_cancel({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            if alive() {
                with_surface_window(&window.as_weak(), &access, |window, shared| {
                    cancel_surface_dialog(window, &mut lock(shared));
                });
            }
            let _ = window.hide();
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
}

fn show_atlas_window(app: &AppWindow, ui: &SharedUi, material: &str) {
    if ui.borrow().atlas.is_none() {
        let controller = match atlas_ui::Controller::new_with_access(&file_access(ui)) {
            Ok(controller) => controller,
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        };
        controller.window.on_browse({
            let (window, app, owner, access) = (
                controller.window.as_weak(),
                app.as_weak(),
                Rc::downgrade(ui),
                file_access(ui).downgrade(),
            );
            move || {
                let (Some(_window), Some(app), Some(ui)) =
                    (window.upgrade(), app.upgrade(), owner.upgrade())
                else {
                    return;
                };
                let result = access.upgrade().and_then(|access| {
                    access.with_shared(|_| {
                        show_material_window(&app, &ui, "@atlas");
                        Ok(())
                    })
                });
                if let Err(error) = result {
                    app.set_message(error.into());
                }
            }
        });
        ui.borrow_mut().atlas = Some(controller);
    }
    let shared = session_of(ui);
    let initialized = ui.borrow().atlas.as_ref().map(|controller| {
        (
            controller.initialize(&mut lock(&shared), material),
            controller.window.clone_strong(),
        )
    });
    if let Some((result, window)) = initialized {
        let result = result.and_then(|()| window.show().map_err(|error| error.to_string()));
        if let Err(error) = result {
            app.set_message(error.into());
        }
    }
}

pub(super) fn show_surface_window(app: &AppWindow, ui: &SharedUi) {
    if ui.borrow().surface_window.is_none() {
        let window = match SurfaceWindow::new() {
            Ok(window) => window,
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        };
        let access = file_access(ui);
        let owner = Rc::downgrade(ui);
        bind_surface_edits_with_access(
            &window,
            &access,
            {
                let owner = owner.clone();
                move || owner.upgrade().is_some()
            },
            move || owner.upgrade()?.borrow().surface_target.clone(),
        );
        window.on_browse({
            let (window, app, owner, access) = (
                window.as_weak(),
                app.as_weak(),
                Rc::downgrade(ui),
                access.downgrade(),
            );
            move || {
                let (Some(window), Some(app), Some(ui)) =
                    (window.upgrade(), app.upgrade(), owner.upgrade())
                else {
                    return;
                };
                with_surface_window(&window.as_weak(), &access, |_, _| {
                    show_material_window(&app, &ui, "@faces")
                });
            }
        });
        window.on_fit_atlas({
            let (window, app, owner, access) = (
                window.as_weak(),
                app.as_weak(),
                Rc::downgrade(ui),
                access.downgrade(),
            );
            move || {
                let (Some(window), Some(app), Some(ui)) =
                    (window.upgrade(), app.upgrade(), owner.upgrade())
                else {
                    return;
                };
                with_surface_window(&window.as_weak(), &access, |window, _| {
                    show_atlas_window(&app, &ui, &window.get_material())
                });
            }
        });
        ui.borrow_mut().surface_window = Some(window);
    }
    let shared = session_of(ui);
    let mut session = lock(&shared);
    let faces = session.selected_faces();
    if faces.is_empty() {
        app.set_message("Select faces in the viewport first.".into());
        return;
    }
    session.cancel_face_preview();
    let window = ui.borrow().surface_window.as_ref().unwrap().clone_strong();
    window.set_selection_label(
        format!(
            "Textures on {} selected face{}",
            faces.len(),
            if faces.len() == 1 { "" } else { "s" }
        )
        .into(),
    );
    let names: Vec<_> = faces
        .iter()
        .filter_map(|face| map_core::surface::side(session.document(), *face))
        .map(|side| side.material.to_ascii_lowercase())
        .collect();
    let material = names
        .first()
        .filter(|first| names.iter().all(|name| name == *first))
        .cloned()
        .unwrap_or_default();
    window.set_material(material.clone().into());
    window.set_preview(
        session
            .material_preview(&material)
            .ok()
            .map(|preview| preview_image(&preview))
            .unwrap_or_default(),
    );
    window.set_shift_u(common_face_number(session.document(), &faces, |side| {
        side.u_axis.offset
    }));
    window.set_shift_v(common_face_number(session.document(), &faces, |side| {
        side.v_axis.offset
    }));
    window.set_scale_u(common_face_number(session.document(), &faces, |side| {
        side.u_axis.scale
    }));
    window.set_scale_v(common_face_number(session.document(), &faces, |side| {
        side.v_axis.scale
    }));
    window.set_rotation(common_face_number(session.document(), &faces, |side| {
        side.rotation
    }));
    load_surface_vectors(&window, session.document(), &faces);
    window.set_direct_vectors(false);
    window.set_ready(false);
    window.set_reviewed_fields("".into());
    window.set_reviewed_token("".into());
    window.set_owned_token("".into());
    window.set_fit(false);
    window.set_operation_index(0);
    window.set_alignment_index(0);
    window.set_justify_u_index(0);
    window.set_justify_v_index(0);
    refresh_surface_sample(&window, &mut session);
    window.set_stale(false);
    window.set_message("Preview changes before applying, or Cancel to discard.".into());
    ui.borrow_mut().surface_target = Some((
        faces,
        session.document_generation(),
        session.document().revision(),
    ));
    drop(session);
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

fn refresh_favorites_status(window: &MaterialWindow, favorites: &material_favorites::Snapshot) {
    let status = &favorites.status;
    window.set_favorites_reload_available(status.reload_available);
    window.set_favorite(favorites.contains(&window.get_material()));
    window.set_favorites_status(
        status
            .error
            .as_deref()
            .unwrap_or(match status.persistence {
                material_favorites::Persistence::Session => "Favorites last for this session",
                material_favorites::Persistence::Persistent => "Favorites saved across sessions",
                material_favorites::Persistence::Blocked => "Favorites storage unavailable",
            })
            .into(),
    );
}

pub(super) fn refresh_shared_favorites(
    window: Option<&MaterialWindow>,
    session: &mut Session,
    seen: u64,
) -> u64 {
    let serial = session.material_favorites_serial();
    if serial != seen
        && let Some(window) = window
        && window.window().is_visible()
    {
        refresh_material_catalog(window, session);
    }
    serial
}

#[cfg(test)]
pub(super) fn bind_material_favorites(
    window: &MaterialWindow,
    shared: SharedSession,
    alive: impl Fn() -> bool + 'static,
) {
    bind_material_favorites_with_access(window, &DocumentAccess::unmanaged(shared), alive);
}

fn bind_material_favorites_with_access(
    window: &MaterialWindow,
    access: &DocumentAccess,
    alive: impl Fn() -> bool + 'static,
) {
    let alive: Rc<dyn Fn() -> bool> = Rc::new(alive);
    window.on_toggle_favorite({
        let window = window.as_weak();
        let target = access.downgrade();
        let alive = alive.clone();
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            if !alive() {
                return;
            }
            with_material_window(&window.as_weak(), &target, |window, shared| {
                let mut session = lock(shared);
                let _ = session.favorite_material(&window.get_material(), !window.get_favorite());
                refresh_material_catalog(window, &mut session);
            });
        }
    });
    window.on_reload_favorites({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            if !alive() {
                return;
            }
            with_material_window(&window.as_weak(), &target, |window, shared| {
                let mut session = lock(shared);
                let _ = session.reload_material_favorites();
                refresh_material_catalog(window, &mut session);
            });
        }
    });
}

pub(crate) fn refresh_material_catalog(window: &MaterialWindow, session: &mut Session) {
    let favorites = session.material_favorites_snapshot();
    refresh_material_catalog_with_favorites(window, session, &favorites);
}

fn refresh_material_catalog_with_favorites(
    window: &MaterialWindow,
    session: &mut Session,
    favorites: &material_favorites::Snapshot,
) {
    window.set_favorites_revision(favorites.serial.to_string().into());
    refresh_favorites_status(window, favorites);
    let loaded = session
        .material_catalog_with_favorites(
            &window.get_filter(),
            &window.get_group(),
            window.get_offset().max(0) as usize,
            36,
            favorites,
        )
        .and_then(|catalog| {
            let last_page = catalog.total.saturating_sub(1) / 36 * 36;
            if catalog.offset > last_page {
                window.set_offset(last_page as i32);
                session.material_catalog_with_favorites(
                    &window.get_filter(),
                    &window.get_group(),
                    last_page,
                    36,
                    favorites,
                )
            } else {
                Ok(catalog)
            }
        });
    let catalog = match loaded {
        Ok(catalog) => catalog,
        Err(error) => {
            window.set_rows(ModelRc::new(VecModel::from(Vec::<MaterialTileRow>::new())));
            window.set_groups(ModelRc::new(VecModel::from(Vec::<MaterialGroup>::new())));
            window.set_total(0);
            window.set_catalog_label("Material catalog unavailable".into());
            window.set_available(false);
            window.set_preview(slint::Image::default());
            window.set_detail(error.into());
            return;
        }
    };
    let groups = catalog
        .groups
        .into_iter()
        .map(|group| MaterialGroup {
            id: group.id.into(),
            label: format!("{} ({})", group.label, group.count).into(),
        })
        .collect::<Vec<_>>();
    window.set_groups(ModelRc::new(VecModel::from(groups)));
    window.set_total(catalog.total as i32);
    window.set_catalog_label(if catalog.total == 0 {
        "No matching materials".into()
    } else {
        format!(
            "{}–{} of {} materials",
            catalog.offset + 1,
            catalog.offset + catalog.entries.len(),
            catalog.total
        )
        .into()
    });
    let tiles = catalog
        .entries
        .into_iter()
        .map(|entry| MaterialTile {
            preview: material_picker_reference(window, &entry.name)
                .and_then(|material| session.material_preview(&material))
                .ok()
                .map(|preview| preview_image(&preview))
                .unwrap_or_default(),
            name: entry.name.into(),
            favorite: entry.favorite,
        })
        .collect::<Vec<_>>();
    let rows = tiles
        .chunks(3)
        .map(|tiles| MaterialTileRow {
            tiles: ModelRc::new(VecModel::from(tiles.to_vec())),
        })
        .collect::<Vec<_>>();
    window.set_rows(ModelRc::new(VecModel::from(rows)));
    window.set_favorite(favorites.contains(&window.get_material()));
}

fn bind_material_choice(window: &MaterialWindow, app: &AppWindow, ui: &SharedUi) {
    let target = file_access(ui).downgrade();
    window.on_use_material({
        let (window, app, ui) = (window.as_weak(), app.as_weak(), Rc::downgrade(ui));
        move || {
            let (Some(window), Some(app), Some(ui)) =
                (window.upgrade(), app.upgrade(), ui.upgrade())
            else {
                return;
            };
            with_material_window(&window.as_weak(), &target, |window, shared| {
                let material = window.get_material();
                let mut session = lock(shared);
                if !review_material_choice(window, &mut session) {
                    return;
                }
                if let Some(target) = ui.borrow().overlay_material_target.clone() {
                    if let Some(controller) = &ui.borrow().overlay
                        && let Err(error) =
                            controller.choose_material(&mut session, &target, &material)
                    {
                        window.set_detail(error.into());
                        return;
                    }
                } else if let Some(replacement) = ui.borrow().material_for_replace {
                    if let Some(controller) = &ui.borrow().material_replace {
                        controller.choose_material(&mut session, replacement, &material);
                    }
                } else if ui.borrow().material_for_clip {
                    app.set_clip_material(material);
                    if let Some(points) = session.clip_points().map(|d| d.points.clone()) {
                        if let Err(error) = change_clip_points(&app, &mut session, points) {
                            window.set_detail(error.into());
                            return;
                        }
                    } else {
                        session.cancel_clip_preview();
                    }
                    refresh_clip(&app, &session);
                } else if ui.borrow().material_for_primitive {
                    if let Some(primitive) = &ui.borrow().primitive_window {
                        let Some(target) = ui.borrow().primitive_material_target else {
                            return;
                        };
                        if let Err(error) = primitive_ui::choose_material(
                            primitive,
                            &mut session,
                            target,
                            &material,
                        ) {
                            window.set_detail(error.into());
                            return;
                        }
                    }
                } else if ui.borrow().material_for_polygon {
                    if let Some(polygon) = &ui.borrow().polygon {
                        let Some(target) = ui.borrow().primitive_material_target else {
                            return;
                        };
                        if let Err(error) = polygon.choose_material(&mut session, target, &material)
                        {
                            window.set_detail(error.into());
                            return;
                        }
                    }
                } else if ui.borrow().material_for_partition {
                    if let Some(components) = &ui.borrow().components_window {
                        let Some(target) = ui.borrow().partition_material_target.clone() else {
                            return;
                        };
                        if let Err(error) = components_ui::choose_partition_material(
                            components,
                            &mut session,
                            &target,
                            Some(material.as_str()),
                        ) {
                            window.set_detail(error.into());
                            return;
                        }
                    }
                } else if ui.borrow().material_for_extrusion {
                    if let Some(components) = &ui.borrow().components_window {
                        let Some(target) = ui.borrow().extrusion_material_target else {
                            return;
                        };
                        if let Err(error) = components_ui::choose_extrusion_material(
                            components,
                            &mut session,
                            target,
                            &material,
                        ) {
                            window.set_detail(error.into());
                            return;
                        }
                    }
                } else if ui.borrow().material_for_atlas {
                    if let Some(controller) = &ui.borrow().atlas {
                        controller.choose_material(&mut session, &material);
                    }
                } else if ui.borrow().material_for_group {
                    if let Some(controller) = &ui.borrow().surface_group {
                        controller.choose_material(&mut session, &material);
                    }
                } else if ui.borrow().material_for_faces {
                    if let Some(surface) = &ui.borrow().surface_window {
                        surface.set_material(material.clone());
                        surface.set_preview(
                            session
                                .material_preview(&material)
                                .ok()
                                .map(|preview| preview_image(&preview))
                                .unwrap_or_default(),
                        );
                    }
                } else {
                    app.set_material(material);
                    refresh_material_preview(&app, &mut session);
                }
                drop(session);
                let _ = window.hide();
            });
        }
    });
}

pub(super) fn show_material_window(app: &AppWindow, ui: &SharedUi, key: &str) {
    if ui.borrow().material_window.is_none() {
        let window = match MaterialWindow::new() {
            Ok(window) => window,
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        };
        let access = file_access(ui);
        let owner = Rc::downgrade(ui);
        bind_material_search_with_access(&window, &access, {
            let owner = owner.clone();
            move || owner.upgrade().is_some()
        });
        bind_material_cancel(&window);
        bind_material_favorites_with_access(&window, &access, move || owner.upgrade().is_some());
        bind_material_preview_with_access(&window, &access);
        bind_material_reload_with_access(&window, &access);
        material_source_ui::bind_with_access(&window, &file_access(ui));
        ui.borrow_mut().material_window = Some(window);
    }
    let shared = session_of(ui);
    let mut session = lock(&shared);
    let overlay_target = if key == "@overlay" {
        ui.borrow()
            .overlay
            .as_ref()
            .map(|controller| controller.material_target(&session))
    } else {
        None
    };
    ui.borrow_mut().overlay_material_target = overlay_target;
    ui.borrow_mut().material_for_replace = match key {
        "@replace-source" => Some(false),
        "@replace-target" => Some(true),
        _ => None,
    };
    ui.borrow_mut().material_for_group = key == "@group";
    ui.borrow_mut().material_for_atlas = key == "@atlas";
    ui.borrow_mut().material_for_faces = key == "@faces";
    ui.borrow_mut().material_for_clip = key == "@clip";
    ui.borrow_mut().material_for_primitive = key == "@primitive";
    ui.borrow_mut().material_for_polygon = key == "@polygon";
    ui.borrow_mut().material_for_extrusion = key == "@extrusion";
    ui.borrow_mut().material_for_partition = key == "@partition";
    ui.borrow_mut().partition_material_target =
        (key == "@partition").then(|| components_ui::partition_material_target(&session));
    ui.borrow_mut().extrusion_material_target = (key == "@extrusion").then(|| {
        (
            session.partition_preview_token(),
            session.document_generation(),
        )
    });
    ui.borrow_mut().primitive_material_target =
        (key == "@primitive" || key == "@polygon").then(|| {
            (
                session.primitive_preview().map(|preview| preview.token),
                session.document_generation(),
            )
        });
    let target = if key.is_empty()
        || key == "@faces"
        || key == "@overlay"
        || key == "@clip"
        || key == "@primitive"
        || key == "@polygon"
        || key == "@partition"
        || key == "@extrusion"
        || key.starts_with("@replace-")
        || key == "@group"
        || key == "@atlas"
    {
        None
    } else {
        single_selected_entity(&session)
            .map(|id| (id, key.to_owned(), session.document_generation()))
    };
    let material = if let Some(replacement) = ui.borrow().material_for_replace {
        ui.borrow()
            .material_replace
            .as_ref()
            .map(|controller| {
                if replacement {
                    controller.window.get_replacement().to_string()
                } else {
                    controller.window.get_source().to_string()
                }
            })
            .unwrap_or_default()
    } else if key == "@atlas" {
        ui.borrow()
            .atlas
            .as_ref()
            .map(|controller| controller.window.get_material().to_string())
            .unwrap_or_default()
    } else if key == "@overlay" {
        ui.borrow()
            .overlay
            .as_ref()
            .map(|controller| controller.window.get_material().to_string())
            .unwrap_or_default()
    } else if key == "@partition" {
        ui.borrow()
            .components_window
            .as_ref()
            .map(|window| window.get_top_material().to_string())
            .unwrap_or_default()
    } else if key == "@extrusion" {
        ui.borrow()
            .components_window
            .as_ref()
            .map(|window| window.get_wall_material().to_string())
            .unwrap_or_default()
    } else if key == "@primitive" {
        ui.borrow()
            .primitive_window
            .as_ref()
            .map(|window| window.get_material().to_string())
            .unwrap_or_default()
    } else if key == "@polygon" {
        ui.borrow()
            .polygon
            .as_ref()
            .map(|controller| controller.window.get_material().to_string())
            .unwrap_or_default()
    } else if key == "@group" {
        ui.borrow()
            .surface_group
            .as_ref()
            .map(|controller| controller.window.get_material().to_string())
            .unwrap_or_default()
    } else if key == "@clip" {
        app.get_clip_material().to_string()
    } else if key == "@faces" {
        ui.borrow()
            .surface_window
            .as_ref()
            .map(|window| window.get_material().to_string())
            .unwrap_or_default()
    } else {
        target
            .as_ref()
            .and_then(|(id, key, _)| {
                session
                    .entity_inspector(*id)
                    .ok()?
                    .properties
                    .into_iter()
                    .find(|field| field.key == *key)
                    .map(|field| field.value)
            })
            .unwrap_or_else(|| app.get_material().to_string())
    };
    let sprite_target = target.as_ref().is_some_and(|(id, key, _)| {
        key.eq_ignore_ascii_case("model")
            && session.document().entity(*id).is_some_and(|entity| {
                editor_core::sprite_resource::is_source(
                    entity
                        .classname()
                        .and_then(|name| session.definitions().class(name)),
                )
            })
    });
    let (material, initial_error) = if sprite_target {
        match editor_core::sprite_resource::picker_name(&material) {
            Ok(material) => (material, None),
            Err(error) => (String::new(), Some(error)),
        }
    } else {
        (material, None)
    };
    ui.borrow_mut().material_target = target.clone();
    let window = ui.borrow().material_window.as_ref().unwrap().clone_strong();
    window.set_sprite_resource(sprite_target);
    if let Some(target) = ui.borrow().material_target.clone() {
        bind_entity_material_choice_with_access(&window, &file_access(ui), target);
    } else {
        bind_material_choice(&window, app, ui);
    }
    window.set_filter("".into());
    window.set_material(material.into());
    window.set_group("all".into());
    window.set_offset(0);
    refresh_material_catalog(&window, &mut session);
    refresh_material_window(&window, &mut session);
    if let Some(error) = initial_error {
        window.set_detail(error.into());
    }
    drop(session);
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

fn bind_material_cancel(window: &MaterialWindow) {
    window.on_cancel({
        let window = window.as_weak();
        move || {
            if let Some(window) = window.upgrade() {
                let _ = window.hide();
            }
        }
    });
}

#[cfg(test)]
fn bind_entity_material_choice(
    window: &MaterialWindow,
    shared: SharedSession,
    target: (u32, String, u64),
) {
    bind_entity_material_choice_with_access(window, &DocumentAccess::unmanaged(shared), target);
}

fn bind_entity_material_choice_with_access(
    window: &MaterialWindow,
    access: &DocumentAccess,
    target: (u32, String, u64),
) {
    window.on_use_material({
        let window = window.as_weak();
        let access = access.downgrade();
        move || {
            with_material_window(&window, &access, |window, shared| {
                let mut session = lock(shared);
                if session.document_generation() != target.2 {
                    window.set_detail("Map changed. Open this picker again.".into());
                    return;
                }
                let material = match material_picker_reference(window, &window.get_material()) {
                    Ok(material) => material,
                    Err(error) => {
                        window.set_detail(error.into());
                        return;
                    }
                };
                if !review_material_choice(window, &mut session) {
                    return;
                }
                let result = session.set_entity_properties(
                    Actor::User,
                    target.0,
                    &[(target.1.clone(), Some(material))],
                );
                if let Err(error) = result {
                    window.set_detail(error.into());
                    return;
                }
                drop(session);
                let _ = window.hide();
            });
        }
    });
}

pub(crate) fn material_picker_reference(
    window: &MaterialWindow,
    material: &str,
) -> Result<String, String> {
    if window.get_sprite_resource() {
        editor_core::sprite_resource::browse_reference(material)
    } else {
        Ok(material.into())
    }
}

fn reload_material_window(window: &MaterialWindow, session: &mut Session) {
    session.reload_material_assets(Actor::User);
    refresh_material_catalog(window, session);
    refresh_material_window(window, session);
}

fn bind_material_reload_with_access(window: &MaterialWindow, access: &DocumentAccess) {
    window.on_reload_material_assets({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            with_material_window(&window, &target, |window, shared| {
                reload_material_window(window, &mut lock(shared));
            });
        }
    });
}

pub(crate) fn bind_material_reload(window: &MaterialWindow, shared: SharedSession) {
    window.on_reload_material_assets({
        let window = window.as_weak();
        move || {
            let window = window.unwrap();
            reload_material_window(&window, &mut lock(&shared));
        }
    });
}

pub(super) fn refresh_surface_material(window: &SurfaceWindow, session: &Session) {
    window.set_preview(
        session
            .material_preview(&window.get_material())
            .ok()
            .map(|preview| preview_image(&preview))
            .unwrap_or_default(),
    );
}

pub(crate) fn review_material_choice(window: &MaterialWindow, session: &mut Session) -> bool {
    if window.get_asset_generation().as_str() == session.materials().generation().to_string() {
        return true;
    }
    refresh_material_catalog(window, session);
    refresh_material_window(window, session);
    window.set_detail(
        "Material assets changed. Review the refreshed preview and choose again.".into(),
    );
    false
}

#[cfg(test)]
#[path = "materials_vectors_tests.rs"]
mod vectors_tests;

#[cfg(test)]
#[path = "materials_sprite_tests.rs"]
mod sprite_tests;

#[cfg(test)]
#[path = "materials_favorites_tests.rs"]
mod favorites_tests;

#[cfg(test)]
#[path = "materials_admission_tests.rs"]
mod admission_tests;

#[cfg(test)]
#[path = "surface_admission_tests.rs"]
mod surface_admission_tests;
