use super::*;

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

pub(super) fn cancel_surface_pick(app: &AppWindow, ui: &SharedUi) -> bool {
    let shared = session_of(ui);
    let mut session = lock(&shared);
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

/// Keyboard commands; returns whether the key was handled.
pub(super) fn command_key(
    app: &AppWindow,
    ui: &SharedUi,
    text: &str,
    control: bool,
    shift: bool,
) -> bool {
    let key = text.to_lowercase();
    let object_shortcut = if control {
        matches!(key.as_str(), "d" | "c" | "x" | "v")
    } else {
        [
            Key::Delete,
            Key::Backspace,
            Key::UpArrow,
            Key::DownArrow,
            Key::LeftArrow,
            Key::RightArrow,
            Key::PageUp,
            Key::PageDown,
        ]
        .iter()
        .any(|value| key == key_text(*value))
    };
    let components_active = {
        let shared = session_of(ui);
        let session = lock(&shared);
        session.component_picking().is_some() || !session.component_selection().is_empty()
    };
    if object_shortcut && components_active {
        app.set_message("Use Reshape brushes or its move handles. Finish component editing before editing whole objects.".into());
        return true;
    }
    if control {
        match key.as_str() {
            "z" if shift => drop(lock(&session_of(ui)).redo(Actor::User)),
            "z" => drop(lock(&session_of(ui)).undo(Actor::User)),
            "y" => drop(lock(&session_of(ui)).redo(Actor::User)),
            "s" => save(app, ui),
            "c" => app.invoke_copy_objects(),
            "x" => app.invoke_cut_objects(),
            "v" => app.invoke_paste_objects(),
            "n" => {
                let ui2 = ui.clone();
                confirm_discard(ui, move || {
                    drop(lock(&session_of(&ui2)).new_map(Actor::User, true));
                });
            }
            "d" => {
                let shared = session_of(ui);
                let mut session = lock(&shared);
                let ids = session.selected_ids();
                let grid = session.spatial().grid_size;
                let offset = DVec3::new(grid, grid, 0.0);
                let result = session
                    .edit(
                        Actor::User,
                        &format!("duplicate {} objects", ids.len()),
                        |tx| edit::duplicate(tx, &ids, offset),
                    )
                    .and_then(|copies| session.select(&copies, SelectMode::Replace));
                if let Err(message) = result {
                    app.set_message(message.into());
                }
            }
            _ => return false,
        }
        return true;
    }
    if key == key_text(Key::Delete) || key == key_text(Key::Backspace) {
        edit_selection(app, ui, |n| format!("delete {n} objects"), edit::delete);
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
            session.cancel_face_preview();
        } else if session.face_mode() {
            drop(session.select_faces(&[], SelectMode::Replace));
        } else {
            drop(session.select(&[], SelectMode::Replace));
        }
        return true;
    }
    if key == "f" {
        frame_selection(app, ui);
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
