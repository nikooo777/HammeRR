use super::*;

#[derive(Clone, Copy)]
pub(super) struct PaneCapture {
    pane: usize,
    buttons: u8,
}

#[derive(Clone, Copy)]
pub(super) struct PaneImage {
    pub camera: render::Camera,
    pub scene: render::SceneKey,
    pub size: (u32, u32),
}

pub(super) fn release_pane_input(app: &AppWindow, ui: &mut Ui) {
    ui.viewport.release_all();
    ui.pane_capture = None;
    ui.brush_drag = None;
    ui.clip_drag = None;
    ui.object_cycle.cancel();
    ui.interaction = interaction::Interaction::default();
    ui.pick_release = false;
    app.set_viewport_capture_pane(-1);
    app.set_flying(false);
    pointer_lock::sync(app, false);
    pointer_lock::reset_keyboard(app);
}

pub(super) fn sync_view_panes(
    app: &AppWindow,
    ui: &mut Ui,
    session: &Session,
) -> Result<(), String> {
    let layout = session.view_layout();
    let active = session.active_view_pane();
    let resized = layout != ui.view_layout;
    let changed = layout != ui.view_layout || active != ui.active_view_pane;
    let camera_changed = changed || session.camera_serial() != ui.seen_camera;
    if camera_changed {
        release_pane_input(app, ui);
        ui.viewport.set_active_pane(active)?;
        ui.viewport.camera = session.camera();
        ui.seen_camera = session.camera_serial();
    }
    if changed {
        ui.viewport.redraw();
    }
    if resized {
        clear_view_frames(app, ui, "");
    }
    ui.view_layout = layout;
    ui.active_view_pane = active;
    ui.seen_view_layout = session.view_layout_serial();
    app.set_view_layout_index(match layout {
        session::view_layout::Layout::Single => 0,
        session::view_layout::Layout::TwoColumns => 1,
        session::view_layout::Layout::TwoRows => 2,
        session::view_layout::Layout::Quad => 3,
    });
    app.set_active_pane_index(active as i32);
    app.set_linked_orthographic(session.linked_orthographic_views());
    for index in 0..session::view_layout::MAX_PANES {
        let mut row = ui.view_panes.row_data(index).unwrap_or_default();
        let projection = session.view_pane_camera(index)?.projection.index() as i32;
        if changed && index >= layout.pane_count() {
            row.frame = slint::Image::default();
            row.error = SharedString::default();
            row.projection = projection;
            ui.view_panes.set_row_data(index, row);
        } else if row.projection != projection {
            row.projection = projection;
            ui.view_panes.set_row_data(index, row);
        }
    }
    if changed {
        let frame = ui.view_panes.row_data(active).unwrap_or_default().frame;
        app.set_frame(frame);
    }
    if camera_changed {
        refresh_spatial(app, session);
    }
    Ok(())
}

fn activate_view_pane(app: &AppWindow, ui: &SharedUi, pane: i32) -> Result<usize, String> {
    let pane = usize::try_from(pane).map_err(|_| "Unknown viewport pane")?;
    let mut ui = ui.borrow_mut();
    let shared = ui.session.clone();
    let mut session = lock(&shared);
    sync_view_panes(app, &mut ui, &session)?;
    if pane != session.active_view_pane() {
        if ui.pane_capture.is_some() {
            return Err("Finish the captured viewport gesture before changing panes".into());
        }
        session.publish_camera(ui.viewport.camera);
        session.activate_view_pane(Actor::User, pane, None)?;
        sync_view_panes(app, &mut ui, &session)?;
    }
    session.publish_camera(ui.viewport.camera);
    if session.camera_serial() != ui.seen_camera {
        sync_view_panes(app, &mut ui, &session)?;
    }
    Ok(pane)
}

fn pane_button_down(app: &AppWindow, ui: &SharedUi, pane: i32, button: i32) -> bool {
    let result = activate_view_pane(app, ui, pane);
    let pane = match result {
        Ok(pane) => pane,
        Err(error) => {
            app.set_message(error.into());
            return false;
        }
    };
    if button == 0 && !pane_image_current(app, ui, pane) {
        app.set_message("Wait for this view to refresh before picking".into());
        return false;
    }
    let Some(bit) = u32::try_from(button)
        .ok()
        .filter(|button| *button < u8::BITS)
        .and_then(|button| 1_u8.checked_shl(button))
    else {
        return false;
    };
    let mut ui = ui.borrow_mut();
    let capture = ui
        .pane_capture
        .get_or_insert(PaneCapture { pane, buttons: 0 });
    if capture.pane != pane {
        return false;
    }
    capture.buttons |= bit;
    app.set_viewport_capture_pane(pane as i32);
    true
}

fn pane_image_current(app: &AppWindow, ui: &SharedUi, pane: usize) -> bool {
    let ui = ui.borrow();
    let Some(image) = ui.pane_images[pane] else {
        return false;
    };
    let session = lock(&ui.session);
    let scene = session.render_scene_key(session.view());
    app.get_viewport_pane_sizes().iter().any(|size| {
        usize::try_from(size.id).ok() == Some(pane)
            && image.camera == ui.viewport.camera
            && image.scene == scene
            && image.size
                == (
                    size.pixel_width.max(0) as u32,
                    size.pixel_height.max(0) as u32,
                )
    })
}

fn pane_pointer_current(app: &AppWindow, ui: &SharedUi, pane: i32) -> bool {
    let Ok(pane) = usize::try_from(pane) else {
        return false;
    };
    let mut ui = ui.borrow_mut();
    let shared = ui.session.clone();
    let session = lock(&shared);
    if let Err(error) = sync_view_panes(app, &mut ui, &session) {
        app.set_message(error.into());
        return false;
    }
    pane == ui.active_view_pane && ui.pane_capture.is_none_or(|capture| capture.pane == pane)
}

fn pane_button_up(app: &AppWindow, ui: &SharedUi, pane: i32, button: i32) -> bool {
    if !pane_pointer_current(app, ui, pane) {
        return false;
    }
    let Some(bit) = u32::try_from(button)
        .ok()
        .filter(|button| *button < u8::BITS)
        .and_then(|button| 1_u8.checked_shl(button))
    else {
        return false;
    };
    let mut ui = ui.borrow_mut();
    let Some(capture) = &mut ui.pane_capture else {
        return false;
    };
    if capture.buttons & bit == 0 {
        return false;
    }
    capture.buttons &= !bit;
    if capture.buttons == 0 {
        ui.pane_capture = None;
        app.set_viewport_capture_pane(-1);
    }
    true
}

fn pane_wheel(app: &AppWindow, ui: &SharedUi, pane: i32) -> bool {
    match activate_view_pane(app, ui, pane) {
        Ok(_) => true,
        Err(error) => {
            app.set_message(error.into());
            false
        }
    }
}

pub(super) fn clear_view_frames(app: &AppWindow, ui: &mut Ui, error: &str) {
    ui.pane_images.fill(None);
    for index in 0..session::view_layout::MAX_PANES {
        let mut row = ui.view_panes.row_data(index).unwrap_or_default();
        row.frame = slint::Image::default();
        row.error = error.into();
        ui.view_panes.set_row_data(index, row);
    }
    app.set_frame(slint::Image::default());
}

fn pane_aspects(app: &AppWindow, visible: usize) -> Result<[f32; 4], String> {
    let mut aspects = [1.0; session::view_layout::MAX_PANES];
    for size in app.get_viewport_pane_sizes().iter() {
        let index = usize::try_from(size.id).map_err(|_| "Unknown viewport pane")?;
        if index >= aspects.len() {
            return Err("Unknown viewport pane".into());
        }
        if index < visible {
            if size.pixel_width <= 0 || size.pixel_height <= 0 {
                return Err("Visible viewport has no size".into());
            }
            aspects[index] = size.pixel_width as f32 / size.pixel_height as f32;
        }
    }
    Ok(aspects)
}

pub(super) fn pane_projection(index: i32) -> Result<Projection, String> {
    match index {
        0 => Ok(Projection::Perspective),
        1 => Ok(Projection::Top),
        2 => Ok(Projection::Front),
        3 => Ok(Projection::Side),
        _ => Err("Unknown viewport projection".into()),
    }
}

pub(super) fn change_view_panes(
    app: &AppWindow,
    ui: &SharedUi,
    change: impl FnOnce(&mut Session) -> Result<serde_json::Value, String>,
) {
    let result = (|| {
        let mut ui = ui.borrow_mut();
        let shared = ui.session.clone();
        let mut session = lock(&shared);
        sync_view_panes(app, &mut ui, &session)?;
        if ui.pane_capture.is_some() {
            return Err("Finish the captured viewport gesture before changing views".into());
        }
        session.publish_camera(ui.viewport.camera);
        change(&mut session)?;
        sync_view_panes(app, &mut ui, &session)?;
        Ok::<(), String>(())
    })();
    if let Err(error) = result {
        app.set_message(error.into());
    }
}

pub(super) fn bind_view_panes(app: &AppWindow, ui: &SharedUi) {
    app.set_viewport_panes(ui.borrow().view_panes.clone().into());
    app.set_viewport_scale_factor(app.window().scale_factor());
    {
        let mut ui = ui.borrow_mut();
        let shared = ui.session.clone();
        if let Err(error) = sync_view_panes(app, &mut ui, &lock(&shared)) {
            app.set_message(error.into());
        }
    }
    app.on_view_layout_changed({
        let scope = admission::Callback::new(app, ui, &file_access(ui));
        move |index| {
            scope.input((), |app, ui| {
                change_view_panes(&app, &ui, |session| {
                    let layout = match index {
                        0 => session::view_layout::Layout::Single,
                        1 => session::view_layout::Layout::TwoColumns,
                        2 => session::view_layout::Layout::TwoRows,
                        3 => session::view_layout::Layout::Quad,
                        _ => return Err("Unknown viewport layout".into()),
                    };
                    session.update_view_layout(Actor::User, Some(layout), None, None)
                });
            });
        }
    });
    app.on_view_link_changed({
        let scope = admission::Callback::new(app, ui, &file_access(ui));
        move |linked| {
            scope.input((), |app, ui| {
                change_view_panes(&app, &ui, |session| {
                    session.update_view_layout(Actor::User, None, Some(linked), None)
                });
            });
        }
    });
    app.on_frame_all_views({
        let scope = admission::Callback::new(app, ui, &file_access(ui));
        move || {
            scope.input((), |app, ui| {
                change_view_panes(&app, &ui, |session| {
                    session.frame_all_views(
                        Actor::User,
                        pane_aspects(&app, session.view_layout().pane_count())?,
                        None,
                    )
                });
            });
        }
    });
    app.on_pane_projection_changed({
        let scope = admission::Callback::new(app, ui, &file_access(ui));
        move |pane, projection| {
            scope.input((), |app, ui| {
                if !pane_wheel(&app, &ui, pane) {
                    return;
                }
                change_view_panes(&app, &ui, |session| {
                    let pane = usize::try_from(pane).map_err(|_| "Unknown viewport pane")?;
                    session.set_view_pane_projection(
                        Actor::User,
                        pane,
                        pane_projection(projection)?,
                        None,
                    )
                });
            });
        }
    });
    app.on_pane_mouse_button({
        let scope = admission::Callback::new(app, ui, &file_access(ui));
        move |pane, button, down, x, y, control, alt| {
            scope.input((), |app, ui| {
                let accepted = if down {
                    pane_button_down(&app, &ui, pane, button)
                } else {
                    pane_button_up(&app, &ui, pane, button)
                };
                if accepted {
                    app.invoke_mouse_button(button, down, x, y, control, alt);
                } else if !down {
                    let misplaced = ui
                        .borrow()
                        .pane_capture
                        .is_some_and(|capture| usize::try_from(pane).ok() != Some(capture.pane));
                    if misplaced {
                        app.invoke_viewport_focus_lost();
                    }
                }
            });
        }
    });
    app.on_pane_mouse_moved({
        let scope = admission::Callback::new(app, ui, &file_access(ui));
        move |pane, x, y| {
            scope.input((), |app, ui| {
                if pane_pointer_current(&app, &ui, pane) {
                    app.invoke_mouse_moved(x, y);
                }
            });
        }
    });
    app.on_pane_wheel({
        let scope = admission::Callback::new(app, ui, &file_access(ui));
        move |pane, delta, x, y| {
            scope.input((), |app, ui| {
                if pane_wheel(&app, &ui, pane) {
                    app.invoke_wheel(delta, x, y);
                }
            });
        }
    });
    app.on_pane_focus_lost({
        let weak_app = app.as_weak();
        let weak_ui = Rc::downgrade(ui);
        move |pane| {
            let (Some(app), Some(ui)) = (weak_app.upgrade(), weak_ui.upgrade()) else {
                return;
            };
            if !pane_pointer_current(&app, &ui, pane) {
                return;
            }
            let captured = ui.borrow().pane_capture.is_some();
            if captured {
                app.invoke_viewport_focus_lost();
            } else {
                release_pane_input(&app, &mut ui.borrow_mut());
            }
        }
    });
}

/// Edits the selection as the user and shows any error in the status bar.
pub(super) fn edit_selection(
    app: &AppWindow,
    ui: &SharedUi,
    label: impl Fn(usize) -> String,
    f: impl FnOnce(&mut map_core::Transaction, &[ObjectId]) -> Result<(), map_core::EditError>,
) {
    let shared = session_of(ui);
    let mut session = lock(&shared);
    let ids = session.selected_ids();
    if ids.is_empty() {
        app.set_message("Nothing selected".into());
        return;
    }
    if let Err(message) = session.edit(Actor::User, &label(ids.len()), |tx| f(tx, &ids)) {
        app.set_message(message.into());
    }
}

pub(super) fn overlay_handles_interactive(
    app: &AppWindow,
    session: &Session,
    camera: render::Camera,
) -> bool {
    session.overlay_corner_handles_interactive(
        Actor::User,
        camera,
        (
            app.get_viewport_width().round().max(1.0) as u32,
            app.get_viewport_height().round().max(1.0) as u32,
        ),
    )
}

pub(super) fn refresh_overlay_handle_controls(
    app: &AppWindow,
    session: &Session,
    camera: render::Camera,
) {
    let size = (
        app.get_viewport_width().round().max(1.0) as u32,
        app.get_viewport_height().round().max(1.0) as u32,
    );
    app.set_overlay_handling(session.overlay_corner_handles_interactive(Actor::User, camera, size));
    app.set_can_cancel_overlay_handles(session.overlay_corner_handles_owned(Actor::User));
    app.set_overlay_handle_notice(
        session
            .overlay_corner_handle_notice(Actor::User, camera, size)
            .unwrap_or_default()
            .into(),
    );
}

pub(super) fn cancel_overlay_handles(app: &AppWindow, ui: &SharedUi) -> bool {
    let shared = session_of(ui);
    let camera = ui.borrow().viewport.camera;
    let mut session = lock(&shared);
    if session.overlay_corner_handles_owned(Actor::User) {
        let result = if let Some(controller) = &ui.borrow().overlay {
            controller.cancel_handles(&mut session)
        } else {
            let state = session.get_overlay_state();
            session
                .cancel_overlay_corner_handles(
                    Actor::User,
                    state["draft"]["token"].as_u64().unwrap(),
                )
                .map(|_| ())
        };
        if let Err(error) = result {
            app.set_message(error.into());
        }
        refresh_overlay_handle_controls(app, &session, camera);
        return true;
    }
    false
}

pub(super) fn cancel_surface_pick(app: &AppWindow, ui: &SharedUi) -> bool {
    let shared = session_of(ui);
    let camera = ui.borrow().viewport.camera;
    if lock(&shared).transform_pivot_pick_interactive(
        Actor::User,
        camera,
        (
            app.get_viewport_width().round().max(1.0) as u32,
            app.get_viewport_height().round().max(1.0) as u32,
        ),
    ) {
        return cancel_pivot_pick(app, &mut lock(&shared));
    }
    if overlay_handles_interactive(app, &lock(&shared), camera) {
        return cancel_overlay_handles(app, ui);
    }
    let mut session = lock(&shared);
    let path = session.linked_path();
    if !path["preview"]["handles"].is_null() {
        let result = if let Some(controller) = &ui.borrow().linked_path {
            controller.cancel_handles(&mut session)
        } else {
            Err("Open Linked paths… and Review preview before cancelling node handles".into())
        };
        match result {
            Ok(()) => app.set_linked_path_handling(false),
            Err(error) => app.set_message(error.into()),
        }
        return true;
    }
    if !path["preview"]["picking"].is_null() {
        if path["preview"]["owner"] != Actor::User.label() {
            app.set_message(
                "Review the shared linked path before cancelling position picking.".into(),
            );
            return true;
        }
        let result = if let Some(controller) = &ui.borrow().linked_path {
            controller.cancel_pick(&mut session)
        } else {
            session
                .cancel_linked_path_pick(Actor::User, path["preview"]["token"].as_u64().unwrap())
                .map(|_| ())
        };
        match result {
            Ok(()) => app.set_surface_picking(false),
            Err(error) => app.set_message(error.into()),
        }
        return true;
    }
    let measurement_report = session.measurement_state();
    if !measurement_report["draft"]["picking"].is_null() {
        if measurement_report["draft"]["owner"] != Actor::User.label() {
            app.set_message("Review the shared measurement in Measurements… before cancelling endpoint picking.".into());
            return true;
        }
        let result = if let Some(controller) = &ui.borrow().measurements {
            controller.cancel_pick(&mut session)
        } else {
            session
                .cancel_measurement_pick(
                    Actor::User,
                    measurement_report["draft"]["token"].as_u64().unwrap(),
                )
                .map(|_| ())
        };
        match result {
            Ok(()) => app.set_surface_picking(false),
            Err(error) => app.set_message(error.into()),
        }
        return true;
    }
    let entity_report = session.get_entity_placement();
    if entity_report["draft"]["picking"] == true {
        if entity_report["draft"]["owner"] != Actor::User.label() {
            app.set_message(
                "Review shared entity placement in Entity… → Pick surface… before cancelling it."
                    .into(),
            );
            return true;
        }
        let token = entity_report["draft"]["token"].as_u64().unwrap();
        let _ = session.cancel_entity_placement(Actor::User, token);
        app.set_surface_picking(false);
        return true;
    }
    let report = session.surface_placement();
    if report["picking"] != true {
        return false;
    }
    if let Some(controller) = &ui.borrow().placement {
        controller.cancel_pick(&mut session);
    } else {
        let _ = session.cancel_surface_placement(report["token"].as_u64().unwrap());
    }
    if let Some(token) = session.surface_placement()["token"].as_u64() {
        let _ = session.cancel_surface_placement(token);
    }
    app.set_surface_picking(false);
    true
}

pub(super) fn copy_clipboard_selection(app: &AppWindow, ui: &SharedUi, cut: bool) {
    let shared = session_of(ui);
    let mut session = lock(&shared);
    if session.face_mode()
        || session.component_picking().is_some()
        || !session.component_selection().is_empty()
    {
        app.set_message(
            "Switch to Objects and finish component editing before copying whole objects.".into(),
        );
        return;
    }
    let ids = session.selected_ids();
    let snapshot = session.clipboard_stamp();
    match session.capture_clipboard(Actor::User, &ids, &snapshot, cut, true) {
        Ok(_) => app.set_message(
            format!(
                "{} {} objects. Paste… previews their placement.",
                if cut { "Cut" } else { "Copied" },
                ids.len()
            )
            .into(),
        ),
        Err(error) => app.set_message(error.into()),
    }
}

pub(super) fn cancel_viewport_face_preview(app: &AppWindow, session: &mut Session) {
    let token = session.face_preview_token();
    if let Err(error) = session.cancel_owned_face_preview(Actor::User, token) {
        app.set_message(error.into());
    }
}

fn frontend_shortcut(
    app: &AppWindow,
    command: editor_core::shortcuts::Command,
) -> Result<(), String> {
    use editor_core::shortcuts::Command;
    match command {
        Command::NewMap => app.invoke_new_map(),
        Command::OpenMap => app.invoke_open_map(),
        Command::SaveMap => app.invoke_save_map(),
        Command::SaveMapAs => app.invoke_save_map_as(),
        Command::Copy => app.invoke_copy_objects(),
        Command::Cut => app.invoke_cut_objects(),
        Command::Paste => app.invoke_paste_objects(),
        _ => return Err("This shortcut has no native command adapter".into()),
    }
    Ok(())
}

pub(super) fn synchronize_shortcut_input(app: &AppWindow, viewport: &mut Viewport) -> Option<bool> {
    let keyboard = pointer_lock::keyboard_frame(app);
    for identity in keyboard.released {
        viewport.release_camera(&identity);
    }
    viewport.shortcut_input.expire();
    if keyboard.cancel_prefix {
        viewport.shortcut_input.reset();
    }
    if keyboard.reset {
        viewport.release_all();
    }
    if !keyboard.focused || keyboard.composing || app.get_shortcut_input_blocked() {
        viewport.release_all();
        return None;
    }
    Some(keyboard.shift)
}

pub(super) fn route_shortcut_key(
    app: &AppWindow,
    ui: &SharedUi,
    key: shortcut_input::ViewportKey<'_>,
) -> bool {
    let native = pointer_lock::keyboard_event(app, key.down);
    let shared = session_of(ui);
    {
        let session = lock(&shared);
        if let Err(error) = sync_view_panes(app, &mut ui.borrow_mut(), &session) {
            app.set_message(error.into());
            return true;
        }
    }
    if synchronize_shortcut_input(app, &mut ui.borrow_mut().viewport).is_none() {
        return false;
    }
    if !key.down {
        return false;
    }
    let mut session = lock(&shared);
    let routed = {
        let mut ui = ui.borrow_mut();
        ui.viewport.sync_shortcuts(session.shortcuts_stamp());
        shortcut_input::route_viewport_key(
            &mut ui.viewport.shortcut_input,
            &session,
            &key,
            native.as_ref(),
        )
    };
    match routed {
        Err(error) => {
            ui.borrow_mut().viewport.clear_keyboard();
            app.set_message(error.into());
            true
        }
        Ok(shortcut_input::Routed::Consumed) => true,
        Ok(shortcut_input::Routed::Camera { command, identity }) => {
            ui.borrow_mut().viewport.hold_camera(identity, command);
            true
        }
        Ok(shortcut_input::Routed::Command(command)) => {
            let result = if command.is_frontend() {
                drop(session);
                frontend_shortcut(app, command)
            } else {
                let aspect = app.get_viewport_width() / app.get_viewport_height().max(1.0);
                session
                    .execute_editor_command(Actor::User, command, aspect)
                    .map(|_| ())
            };
            if let Err(error) = result {
                app.set_message(error.into());
            }
            true
        }
        Ok(shortcut_input::Routed::Unhandled) => {
            drop(session);
            !key.control && !key.alt && !key.meta && command_key(app, ui, key.text, key.shift)
        }
    }
}

/// Keyboard commands; returns whether the key was handled.
pub(super) fn command_key(app: &AppWindow, ui: &SharedUi, text: &str, shift: bool) -> bool {
    let key = text.to_lowercase();
    let object_shortcut = [
        Key::UpArrow,
        Key::DownArrow,
        Key::LeftArrow,
        Key::RightArrow,
        Key::PageUp,
        Key::PageDown,
    ]
    .iter()
    .any(|value| key == key_text(*value));
    let components_active = {
        let shared = session_of(ui);
        let session = lock(&shared);
        session.component_picking().is_some() || !session.component_selection().is_empty()
    };
    if object_shortcut && components_active {
        app.set_message("Use Reshape brushes or its move handles. Finish component editing before editing whole objects.".into());
        return true;
    }
    if key == key_text(Key::Escape) {
        if ui.borrow().object_cycle.active() {
            ui.borrow_mut().object_cycle.cancel();
            return true;
        }
        if cancel_surface_pick(app, ui) {
            return true;
        }
        if let Some(controller) = &ui.borrow().terrain
            && controller.is_handling()
        {
            controller.cancel_handles(&mut lock(&session_of(ui)), true);
            app.set_terrain_handling(false);
            return true;
        }
        if let Some(controller) = &ui.borrow().terrain
            && controller.is_stroking()
        {
            controller.cancel_stroke(&mut lock(&session_of(ui)));
            app.set_terrain_stroking(false);
            return true;
        }
        if let Some(controller) = &ui.borrow().terrain_triangles
            && controller.is_picking()
        {
            controller.cancel_pick();
            app.set_terrain_triangle_picking(false);
            return true;
        }
        if let Some(controller) = &ui.borrow().terrain
            && controller.is_picking()
        {
            controller.cancel_center_pick();
            app.set_terrain_center_picking(false);
            return true;
        }
        {
            let mut ui = ui.borrow_mut();
            let shared = ui.session.clone();
            let mut session = lock(&shared);
            if let Some(token) = session.clip_drag().map(|d| d.token) {
                let _ = session.cancel_clip_drag(token);
                ui.interaction.cancel(&mut session);
                refresh_clip(app, &session);
                return true;
            }
            if session.component_picking().is_some() || !session.component_selection().is_empty() {
                ui.interaction.cancel(&mut session);
                session.clear_component_edit();
                refresh_interaction(app, &session);
                return true;
            }
            if session.volume_preview().is_some() {
                session.clear_volume_selection();
                return true;
            }
            if ui.interaction.active()
                || session.transform_preview().is_some()
                || session.transform_drag().is_some()
                || session.area_preview().is_some()
                || session.paint_stroke().is_some()
            {
                ui.interaction.cancel(&mut session);
                refresh_interaction(app, &session);
                return true;
            }
        }
        ui.borrow_mut().clip_drag = None;
        let cancelled = ui.borrow_mut().brush_drag.take().is_some();
        let shared = session_of(ui);
        let mut session = lock(&shared);
        if session.clip_tool() || session.clip_preview().is_some() {
            cancel_clip(app, &mut session);
            return true;
        }
        let gallery = session.get_entity_gallery();
        if let Some(token) = gallery["draft"]["token"].as_u64() {
            if gallery["draft"]["owner"] == Actor::User.label() {
                if let Err(error) = session.cancel_entity_gallery(Actor::User, token) {
                    app.set_message(error.into());
                }
            } else {
                app.set_message(
                    "Review shared gallery in Entity → Point-entity gallery… before cancelling it."
                        .into(),
                );
            }
            return true;
        }
        if session.primitive_preview().is_some() {
            session.clear_primitive_preview();
            return true;
        }
        if cancelled || session.block_preview().is_some() {
            session.cancel_block_preview();
            return true;
        }
        if session.spatial().draw_block {
            drop(session.set_spatial(
                Actor::User,
                spatial::Update {
                    draw_block: Some(false),
                    ..spatial::Update::default()
                },
            ));
            return true;
        }
        if session.has_face_preview() {
            cancel_viewport_face_preview(app, &mut session);
        } else if session.face_mode() {
            drop(session.select_faces(&[], SelectMode::Replace));
        } else {
            drop(session.select(&[], SelectMode::Replace));
        }
        return true;
    }
    let camera = ui.borrow().viewport.camera;
    let (forward, right, normal) = if let Some((_, _, normal)) = camera.projection.axes() {
        let mut axis = DVec3::ZERO;
        axis[normal] = 1.0;
        (camera.up().as_dvec3(), camera.right().as_dvec3(), axis)
    } else {
        let (forward, right) = nudge_axes(camera.forward());
        (forward, right, DVec3::Z)
    };
    let direction = [
        (Key::UpArrow, forward),
        (Key::DownArrow, -forward),
        (Key::RightArrow, right),
        (Key::LeftArrow, -right),
        (Key::PageUp, normal),
        (Key::PageDown, -normal),
    ]
    .into_iter()
    .find(|(k, _)| key == key_text(*k))
    .map(|(_, d)| d);
    if let Some(direction) = direction {
        let grid = lock(&session_of(ui)).spatial().grid_size;
        let step = if shift { grid.min(1.0) } else { grid };
        edit_selection(
            app,
            ui,
            |n| format!("nudge {n} objects"),
            |tx, ids| edit::translate(tx, ids, direction * step, true),
        );
        return true;
    }
    false
}
