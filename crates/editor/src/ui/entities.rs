use super::*;

pub(super) fn set_inspected_property(
    app: &AppWindow,
    shared: &SharedSession,
    key: &str,
    value: &str,
) {
    let mut session = lock(shared);
    let Some(entity) = single_selected_entity(&session) else {
        return;
    };
    let changes = [(key.to_owned(), Some(value.to_owned()))];
    if let Err(message) = session.set_entity_properties(Actor::User, entity, &changes) {
        refresh_panel(app, &mut session);
        app.set_message(message.into());
    }
}

pub(super) fn selection_summary(session: &Session) -> (String, String, Option<Vec<KeyValue>>) {
    let doc = session.document();
    let ids = session.selected_ids();
    if session.face_mode() {
        let faces = session.selected_faces();
        let names = faces
            .iter()
            .filter_map(|face| map_core::surface::side(doc, *face))
            .map(|side| side.material.to_ascii_lowercase())
            .collect::<std::collections::BTreeSet<_>>();
        return (
            if faces.is_empty() {
                "Select a face in the viewport".into()
            } else {
                format!(
                    "{} selected face{}",
                    faces.len(),
                    if faces.len() == 1 { "" } else { "s" }
                )
            },
            names.into_iter().collect::<Vec<_>>().join(", "),
            None,
        );
    }
    let size_text = |bounds: Option<(DVec3, DVec3)>| {
        bounds.map_or_else(String::new, |(lo, hi)| {
            let (size, center) = (hi - lo, (lo + hi) * 0.5);
            format!(
                "{} × {} × {} at ({}, {}, {})",
                size.x.round(),
                size.y.round(),
                size.z.round(),
                center.x.round(),
                center.y.round(),
                center.z.round()
            )
        })
    };
    match ids[..] {
        [] => ("Nothing selected".into(), String::new(), None),
        [ObjectId::Entity(n)] => {
            let Some(entity) = doc.entity(n) else {
                return ("Nothing selected".into(), String::new(), None);
            };
            let title = inspector::title(entity.classname().unwrap_or("entity"));
            let mut detail = if entity.solids.is_empty() {
                String::new()
            } else {
                size_text(query::entity_bounds(entity))
            };
            if !entity.solids.is_empty() {
                detail += &format!(" · {} brushes", entity.solids.len());
            }
            let rows = entity_property_rows(session, entity);
            (title, detail, Some(rows))
        }
        [ObjectId::Solid(n)] => {
            let materials: Vec<String> = doc
                .solid(n)
                .map(|s| {
                    let mut m: Vec<String> = s
                        .sides
                        .iter()
                        .map(|f| f.material.to_ascii_lowercase())
                        .collect();
                    m.sort_unstable();
                    m.dedup();
                    m
                })
                .unwrap_or_default();
            let detail = format!(
                "{}\n{}",
                size_text(query::bounds(doc, ids[0])),
                materials.join(", ")
            );
            (format!("Brush · {}", id_string(ids[0])), detail, None)
        }
        _ => (
            format!("{} objects", ids.len()),
            size_text(query::bounds_of(doc, &ids)),
            None,
        ),
    }
}

pub(super) fn entity_property_rows(
    session: &Session,
    entity: &source_formats::vmf::Entity,
) -> Vec<KeyValue> {
    let class = entity
        .classname()
        .and_then(|name| session.definitions().class(name));
    inspector::inspect(entity, class)
        .properties
        .into_iter()
        .map(property_row)
        .collect()
}

pub(crate) fn property_row(field: inspector::Field) -> KeyValue {
    use inspector::Control;
    let choice_index = field
        .choices
        .iter()
        .position(|choice| choice.value == field.value)
        .unwrap_or_default() as i32;
    let color = field.color.unwrap_or([0; 3]);
    let range = field.range.unwrap_or([0.0, 100.0]);
    let control = match field.control {
        Control::Text => "text",
        Control::Number => "number",
        Control::Slider => "slider",
        Control::Choices => "choices",
        Control::Flags => "flags",
        Control::Boolean => "boolean",
        Control::Vector => "vector",
        Control::Color => "color",
        Control::Material => "material",
        Control::Model => "model",
        Control::Sound => "sound",
        Control::Particle => "particle",
    };
    KeyValue {
        key: field.key.into(),
        label: field.label.into(),
        description: field.description.into(),
        type_name: field.kind.into(),
        numeric: matches!(field.control, Control::Number | Control::Slider),
        read_only: field.read_only,
        choices: ModelRc::new(VecModel::from(
            field
                .choices
                .iter()
                .map(|choice| choice.label.clone().into())
                .collect::<Vec<_>>(),
        )),
        choice_values: ModelRc::new(VecModel::from(
            field
                .choices
                .into_iter()
                .map(|choice| choice.value.into())
                .collect::<Vec<_>>(),
        )),
        choice_index,
        flags: ModelRc::new(VecModel::from(
            field
                .flags
                .into_iter()
                .map(|flag| EntityFlag {
                    label: flag.label.into(),
                    value: flag.value.into(),
                    checked: flag.checked,
                })
                .collect::<Vec<_>>(),
        )),
        control: control.into(),
        advanced: field.advanced,
        components: ModelRc::new(VecModel::from(
            field
                .components
                .into_iter()
                .map(Into::into)
                .collect::<Vec<_>>(),
        )),
        component_labels: ModelRc::new(VecModel::from(
            field
                .component_labels
                .into_iter()
                .map(Into::into)
                .collect::<Vec<_>>(),
        )),
        swatch: slint::Color::from_rgb_u8(color[0], color[1], color[2]),
        has_intensity: field.intensity.is_some(),
        intensity_value: widget_number(field.intensity.unwrap_or_default()),
        intensity: field
            .intensity
            .map(|value| value.to_string())
            .unwrap_or_default()
            .into(),
        number_value: field.value.parse().map(widget_number).unwrap_or_default(),
        minimum: widget_number(range[0]),
        maximum: widget_number(range[1]),
        value: field.value.into(),
    }
}

pub(super) fn widget_number(value: f64) -> f32 {
    value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
}

pub(super) fn refresh_entity_window(window: &EntityWindow, session: &Session) {
    window.set_catalog_status(session.entity_definitions_report().status().into());
    let kind = if window.get_brush_mode() {
        ClassKind::Solid
    } else {
        ClassKind::Point
    };
    let classes: Vec<EntityClassRow> = session
        .search_entity_classes(&window.get_filter(), Some(kind), 1000)
        .iter()
        .map(|class| EntityClassRow {
            classname: class.name.clone().into(),
            description: class.description.clone().into(),
        })
        .collect();
    window.set_classes(ModelRc::new(VecModel::from(classes)));
    let definition = match session.definitions().class(&window.get_classname()) {
        Some(class) => entity_class_preview(class),
        None => "Custom class. You can place it and edit its raw properties. Load an FGD for property definitions.".into(),
    };
    window.set_definition(definition.into());
}

pub(super) fn entity_class_preview(class: &source_formats::fgd::Class) -> String {
    let mut text = format!(
        "{} ({})\n\n{}\n\nProperties\n",
        class.name,
        class.kind.label(),
        class.description
    );
    for property in &class.properties {
        text += &format!(
            "\n{} — {} ({}){}\n",
            property.label,
            property.name,
            property.kind,
            if property.read_only {
                " [read-only]"
            } else {
                ""
            }
        );
        if let Some(default) = property.default_value() {
            text += &format!("Default: {default}\n");
        }
        if !property.description.is_empty() {
            text += &format!("{}\n", property.description);
        }
        for choice in &property.choices {
            text += &format!(
                "  {}: {}{}\n",
                choice.value,
                choice.label,
                if choice.selected { " [default]" } else { "" }
            );
        }
    }
    for (label, members) in [("Inputs", &class.inputs), ("Outputs", &class.outputs)] {
        text += &format!("\n{label}\n");
        for member in members {
            text += &format!(
                "{} ({}) — {}\n",
                member.name, member.kind, member.description
            );
        }
    }
    text += "\nHelpers\n";
    for helper in &class.helpers {
        text += &format!("{}({})\n", helper.name, helper.arguments.join(", "));
    }
    text
}

pub(super) fn refresh_entity_picker(ui: &SharedUi) {
    let (session, window) = {
        let ui = ui.borrow();
        (
            ui.session.clone(),
            ui.entity_window.as_ref().map(ComponentHandle::clone_strong),
        )
    };
    if let Some(window) = window {
        refresh_entity_window(&window, &lock(&session));
    }
}

fn start_fgd_load(app: &slint::Weak<AppWindow>, ui: &SharedUi, path: &Path) {
    let shared = session_of(ui);
    let window = ui
        .borrow()
        .entity_window
        .as_ref()
        .map(ComponentHandle::as_weak);
    spawn_fgd_load(app, window, &shared, path);
}

fn spawn_fgd_load(
    app: &slint::Weak<AppWindow>,
    window: Option<slint::Weak<EntityWindow>>,
    shared: &SharedSession,
    path: &Path,
) {
    let request = match lock(shared).begin_definitions_load(path) {
        Ok(request) => request,
        Err(error) => {
            if let Some(app) = app.upgrade() {
                app.set_message(error.into());
            }
            return;
        }
    };
    let token = request.token();
    let app_for_worker = app.clone();
    let worker_session = shared.clone();
    let spawned = std::thread::Builder::new()
        .name("fgd-load".into())
        .spawn(move || {
            let completed = request.run();
            let result = lock(&worker_session).finish_definitions_load(Actor::User, completed);
            let message = match result {
                Ok(_) => "Entity definitions loaded".to_owned(),
                Err(error) => error,
            };
            if let Some(window) = window {
                let message = message.clone();
                let _ =
                    window.upgrade_in_event_loop(move |window| window.set_message(message.into()));
            }
            let _ =
                app_for_worker.upgrade_in_event_loop(move |app| app.set_message(message.into()));
        });
    if let Err(error) = spawned {
        lock(shared).fail_definitions_load(token);
        if let Some(app) = app.upgrade() {
            app.set_message(error.to_string().into());
        }
    }
}

pub(super) fn load_fgd(app: &AppWindow, ui: &SharedUi) {
    let (app, ui) = (app.as_weak(), ui.clone());
    slint::spawn_local(async move {
        let Some(file) = rfd::AsyncFileDialog::new()
            .add_filter("Entity definitions", &["fgd"])
            .pick_file()
            .await
        else {
            return;
        };
        start_fgd_load(&app, &ui, file.path());
    })
    .expect("event loop is running");
}

pub(super) fn show_entity_window(app: &AppWindow, ui: &SharedUi, brush_mode: bool) {
    if ui.borrow().entity_window.is_none() {
        let window = match EntityWindow::new() {
            Ok(window) => window,
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        };
        window.on_cancel({
            let window = window.as_weak();
            move || {
                let _ = window.unwrap().hide();
            }
        });
        window.on_filter_changed({
            let ui = Rc::downgrade(ui);
            move || {
                if let Some(ui) = ui.upgrade() {
                    refresh_entity_picker(&ui);
                }
            }
        });
        window.on_class_selected({
            let ui = Rc::downgrade(ui);
            move || {
                if let Some(ui) = ui.upgrade() {
                    refresh_entity_picker(&ui);
                }
            }
        });
        window.on_load_fgd({
            let (app, ui) = (app.as_weak(), Rc::downgrade(ui));
            move || {
                let (Some(app), Some(ui)) = (app.upgrade(), ui.upgrade()) else {
                    return;
                };
                load_fgd(&app, &ui);
            }
        });
        window.on_pick_surface({
            let (app, ui) = (app.as_weak(), Rc::downgrade(ui));
            move || {
                let (Some(app), Some(ui)) = (app.upgrade(), ui.upgrade()) else {
                    return;
                };
                let shared = session_of(&ui);
                let mut state = ui.borrow_mut();
                if state.entity_placement.is_none() {
                    match entity_placement_ui::Controller::new(&shared) {
                        Ok(controller) => state.entity_placement = Some(controller),
                        Err(error) => {
                            app.set_message(error.to_string().into());
                            return;
                        }
                    }
                }
                let window = state.entity_window.as_ref().unwrap();
                let controller = state.entity_placement.as_ref().unwrap();
                controller.initialize(&lock(&shared), &window.get_classname());
                let _ = window.hide();
                if let Err(error) = controller.window.show() {
                    app.set_message(error.to_string().into());
                }
            }
        });
        window.on_create({
            let (app, ui) = (app.as_weak(), Rc::downgrade(ui));
            move || {
                let (Some(app), Some(ui)) = (app.upgrade(), ui.upgrade()) else {
                    return;
                };
                let Some(window) = ui
                    .borrow()
                    .entity_window
                    .as_ref()
                    .map(ComponentHandle::clone_strong)
                else {
                    return;
                };
                let shared = session_of(&ui);
                let mut session = lock(&shared);
                let result = if window.get_brush_mode() {
                    let ids = session.selected_ids();
                    session.tie_to_entity(
                        Actor::User,
                        &ids,
                        &window.get_classname(),
                        BTreeMap::new(),
                    )
                } else {
                    let position = format!(
                        "{} {} {}",
                        window.get_origin_x(),
                        window.get_origin_y(),
                        window.get_origin_z()
                    );
                    source_formats::fgd::vector(&position, 3)
                        .ok_or_else(|| "position expects three finite coordinates".into())
                        .and_then(|values| {
                            session.create_entity(
                                Actor::User,
                                &window.get_classname(),
                                Some(DVec3::new(values[0], values[1], values[2])),
                                BTreeMap::new(),
                            )
                        })
                };
                match result {
                    Ok(_) => {
                        if let Err(error) = window.hide() {
                            app.set_message(error.to_string().into());
                        }
                    }
                    Err(error) => {
                        window.set_message(error.clone().into());
                        app.set_message(error.into());
                    }
                }
            }
        });
        ui.borrow_mut().entity_window = Some(window);
    }
    let shared = session_of(ui);
    let session = lock(&shared);
    let Some(window) = ui
        .borrow()
        .entity_window
        .as_ref()
        .map(ComponentHandle::clone_strong)
    else {
        return;
    };
    window.set_brush_mode(brush_mode);
    window.set_filter("".into());
    let kind = if brush_mode {
        ClassKind::Solid
    } else {
        ClassKind::Point
    };
    let selected = single_selected_entity(&session)
        .and_then(|id| session.document().entity(id))
        .and_then(|entity| entity.classname())
        .filter(|name| {
            session
                .definitions()
                .class(name)
                .is_some_and(|class| class.kind == kind)
        });
    window.set_classname(
        selected
            .unwrap_or(if brush_mode { "func_detail" } else { "light" })
            .into(),
    );
    let origin = session.placement_origin();
    window.set_origin_x(origin.x.to_string().into());
    window.set_origin_y(origin.y.to_string().into());
    window.set_origin_z(origin.z.to_string().into());
    window.set_message("".into());
    refresh_entity_window(&window, &session);
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

pub(super) fn show_connection_window(app: &AppWindow, ui: &SharedUi) {
    let shared = session_of(ui);
    let session = lock(&shared);
    let Some(source) = single_selected_entity(&session) else {
        app.set_message("Select an entity to inspect its connections.".into());
        return;
    };
    if ui.borrow().connection_window.is_none() {
        let window = match ConnectionWindow::new() {
            Ok(window) => window,
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        };
        connection_ui::bind(&window, &shared);
        ui.borrow_mut().connection_window = Some(window);
    }
    let window = ui
        .borrow()
        .connection_window
        .as_ref()
        .unwrap()
        .clone_strong();
    window.set_source_id(id_string(ObjectId::Entity(source)).into());
    connection_ui::refresh(&window, &session);
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

pub(super) fn show_properties_window(app: &AppWindow, ui: &SharedUi, world: bool) {
    let shared = session_of(ui);
    let mut session = lock(&shared);
    let target = if world {
        session::PropertyTarget::World
    } else {
        let ids = session
            .selected_ids()
            .into_iter()
            .map(|id| match id {
                ObjectId::Entity(id) => Ok(id),
                _ => Err("Select entities to edit their common properties."),
            })
            .collect::<Result<Vec<_>, _>>();
        match ids {
            Ok(ids) => session::PropertyTarget::Entities(ids),
            Err(error) => {
                app.set_message(error.into());
                return;
            }
        }
    };
    if ui.borrow().properties_window.is_none() {
        let result = PropertiesWindow::new().and_then(|window| {
            management_ui::bind_properties(&window, &shared)?;
            Ok(window)
        });
        match result {
            Ok(window) => ui.borrow_mut().properties_window = Some(window),
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        }
    }
    if session
        .property_draft()
        .is_none_or(|draft| draft.target != target)
        && let Err(error) = session.begin_property_edit(target)
    {
        app.set_message(error.into());
        return;
    }
    let window = ui
        .borrow()
        .properties_window
        .as_ref()
        .unwrap()
        .clone_strong();
    management_ui::refresh_properties(&window, &session);
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

pub(super) fn show_report_window(app: &AppWindow, ui: &SharedUi) {
    let shared = session_of(ui);
    if ui.borrow().report_window.is_none() {
        match ReportWindow::new() {
            Ok(window) => {
                management_ui::bind_report(&window, &shared);
                ui.borrow_mut().report_window = Some(window);
            }
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        }
    }
    let window = ui.borrow().report_window.as_ref().unwrap().clone_strong();
    window.set_aspect((app.get_viewport_width() / app.get_viewport_height().max(1.0)).max(0.01));
    management_ui::refresh_report(&window, &lock(&shared));
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

pub(super) fn show_rename_window(app: &AppWindow, ui: &SharedUi, name: Option<String>) {
    let shared = session_of(ui);
    let mut session = lock(&shared);
    let Some(entity) = single_selected_entity(&session) else {
        app.set_message("Select an entity to inspect its references.".into());
        return;
    };
    if ui.borrow().rename_window.is_none() {
        let window = match RenameWindow::new() {
            Ok(window) => window,
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        };
        rename_ui::bind(&window, &shared);
        ui.borrow_mut().rename_window = Some(window);
    }
    let begin = if name.is_none()
        && session
            .rename_draft()
            .is_some_and(|draft| draft.entity == entity)
    {
        Ok(())
    } else {
        session.begin_entity_rename(entity)
    };
    let result = begin.and_then(|()| {
        if let Some(name) = name {
            session.update_entity_rename(name, false, false)
        } else {
            Ok(())
        }
    });
    if let Err(error) = result {
        app.set_message(error.into());
        return;
    }
    let window = ui.borrow().rename_window.as_ref().unwrap().clone_strong();
    rename_ui::refresh(&window, &session);
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

pub(super) fn show_color_window(app: &AppWindow, ui: &SharedUi, key: &str) {
    if ui.borrow().color_window.is_none() {
        let window = match ColorWindow::new() {
            Ok(window) => window,
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        };
        window.on_cancel({
            let window = window.as_weak();
            move || {
                let _ = window.unwrap().hide();
            }
        });
        window.on_apply({
            let (window, ui) = (window.as_weak(), Rc::downgrade(ui));
            move || {
                let (Some(window), Some(ui)) = (window.upgrade(), ui.upgrade()) else {
                    return;
                };
                let Some((id, key, generation)) = ui.borrow().color_target.clone() else {
                    return;
                };
                let intensity = if window.get_has_intensity() {
                    match window.get_intensity().parse::<f64>() {
                        Ok(value) => Some(value),
                        Err(_) => {
                            window.set_message("Enter a valid brightness.".into());
                            return;
                        }
                    }
                } else {
                    None
                };
                let color = [
                    window.get_red() as u8,
                    window.get_green() as u8,
                    window.get_blue() as u8,
                ];
                let shared = session_of(&ui);
                let mut session = lock(&shared);
                if session.document_generation() != generation {
                    window.set_message("Map changed. Open this picker again.".into());
                    return;
                }
                let result = session.set_entity_color(Actor::User, id, &key, color, intensity);
                match result {
                    Ok(()) => {
                        let _ = window.hide();
                    }
                    Err(error) => window.set_message(error.into()),
                }
            }
        });
        ui.borrow_mut().color_window = Some(window);
    }
    let shared = session_of(ui);
    let session = lock(&shared);
    let Some(id) = single_selected_entity(&session) else {
        return;
    };
    let Ok(inspector) = session.entity_inspector(id) else {
        return;
    };
    let Some(field) = inspector
        .properties
        .into_iter()
        .find(|field| field.key == key)
    else {
        return;
    };
    let Some(color) = field.color else {
        return;
    };
    let generation = session.document_generation();
    drop(session);
    ui.borrow_mut().color_target = Some((id, key.into(), generation));
    let window = ui.borrow().color_window.as_ref().unwrap().clone_strong();
    window.set_field_label(field.label.into());
    window.set_red(i32::from(color[0]));
    window.set_green(i32::from(color[1]));
    window.set_blue(i32::from(color[2]));
    window.set_has_intensity(field.intensity.is_some());
    window.set_intensity(field.intensity.unwrap_or_default().to_string().into());
    window.set_brightness(widget_number(field.intensity.unwrap_or_default()));
    window.set_message("".into());
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

pub(super) fn single_selected_entity(session: &Session) -> Option<u32> {
    match session.selected_ids()[..] {
        [ObjectId::Entity(n)] => Some(n),
        _ => None,
    }
}

#[cfg(test)]
mod fgd_loading_tests;
