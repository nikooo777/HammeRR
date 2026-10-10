use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::mcp::{SharedSession, lock, parse_id};
use crate::session::{Actor, ObjectFilter, PropertyChange, PropertyTarget, SelectMode, Session};
use crate::{
    ColorWindow, ManagedFieldRow, MaterialWindow, ObjectReportRow, PropertiesWindow, ReportWindow,
};

fn token(window: &PropertiesWindow) -> Result<u64, String> {
    window
        .get_draft_token()
        .parse()
        .map_err(|_| "Reopen properties".into())
}

pub fn refresh_properties(window: &PropertiesWindow, session: &Session) {
    window.set_can_apply(false);
    let Some(draft) = session.property_draft() else {
        window.set_fields(ModelRc::default());
        window.set_message("Property edit closed. Reopen properties to start again.".into());
        return;
    };
    window.set_draft_token(draft.token.to_string().into());
    let preview = match session.preview_property_edit() {
        Ok(preview) => preview,
        Err(error) => {
            window.set_message(error.into());
            return;
        }
    };
    window.set_heading(if preview.target == PropertyTarget::World {
        "World settings".into()
    } else {
        format!("Properties · {} entities", preview.count).into()
    });
    window.set_summary(
        format!(
            "{} staged {} · Apply saves them together; Cancel discards them.",
            preview.pending,
            if preview.pending == 1 {
                "change"
            } else {
                "changes"
            }
        )
        .into(),
    );
    window.set_can_apply(preview.pending > 0);
    window.set_message(
        "Use default writes the FGD value. Remove override deletes the stored key.".into(),
    );
    let filter = window.get_filter().to_lowercase();
    let rows = preview
        .properties
        .into_iter()
        .filter(|property| {
            format!("{} {}", property.field.label, property.field.key)
                .to_lowercase()
                .contains(&filter)
        })
        .map(|mut property| {
            let defaults = property
                .values
                .iter()
                .filter_map(|value| value.default.as_deref())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(" / ");
            let values = property
                .values
                .iter()
                .map(|value| value.effective.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(" / ");
            let status = format!(
                "{} · {} of {} stored{} · {}",
                property.field.key,
                property.stored_count,
                preview.count,
                if defaults.is_empty() {
                    String::new()
                } else {
                    format!(" · Default: {defaults}")
                },
                if property.mixed {
                    format!("Mixed: {values}")
                } else if property.stored_count == 0 {
                    "No stored override".into()
                } else {
                    "Same value".into()
                }
            );
            for flag in &mut property.field.flags {
                if property.mixed_flags.contains(&flag.value) {
                    flag.label.push_str(" (mixed)");
                }
            }
            ManagedFieldRow {
                field: crate::property_row(property.field),
                status: status.into(),
                can_reset: property.can_reset,
            }
        })
        .collect::<Vec<_>>();
    window.set_fields(ModelRc::new(VecModel::from(rows)));
}

fn stage(window: &PropertiesWindow, shared: &SharedSession, change: PropertyChange) {
    let mut session = lock(shared);
    let result = token(window).and_then(|token| session.append_property_change(token, change));
    refresh_properties(window, &session);
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

pub fn bind_properties(
    window: &PropertiesWindow,
    shared: &SharedSession,
) -> Result<(), slint::PlatformError> {
    window.on_search({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || refresh_properties(&window.unwrap(), &lock(&shared))
    });
    window.on_edit({
        let (window, shared) = (window.as_weak(), shared.clone());
        move |key, value| {
            stage(
                &window.unwrap(),
                &shared,
                PropertyChange::Set {
                    key: key.into(),
                    value: value.into(),
                },
            )
        }
    });
    window.on_reset({
        let (window, shared) = (window.as_weak(), shared.clone());
        move |key| {
            stage(
                &window.unwrap(),
                &shared,
                PropertyChange::Default { key: key.into() },
            )
        }
    });
    window.on_remove({
        let (window, shared) = (window.as_weak(), shared.clone());
        move |key| {
            stage(
                &window.unwrap(),
                &shared,
                PropertyChange::Remove { key: key.into() },
            )
        }
    });
    window.on_flag_edit({
        let (window, shared) = (window.as_weak(), shared.clone());
        move |key, flag, enabled| {
            let window = window.unwrap();
            match flag.parse() {
                Ok(flag) => stage(
                    &window,
                    &shared,
                    PropertyChange::Flag {
                        key: key.into(),
                        flag,
                        enabled,
                    },
                ),
                Err(_) => window.set_message("Choose a valid flag".into()),
            }
        }
    });
    window.on_component_edit({
        let (window, shared) = (window.as_weak(), shared.clone());
        move |key, index, value| {
            let window = window.unwrap();
            match value.parse() {
                Ok(value) if index >= 0 => stage(
                    &window,
                    &shared,
                    PropertyChange::Component {
                        key: key.into(),
                        index: index as usize,
                        value,
                    },
                ),
                _ => window.set_message("Enter a finite number".into()),
            }
        }
    });
    window.on_apply({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let result = token(&window)
                .and_then(|token| lock(&shared).commit_property_edit(Actor::User, token));
            match result {
                Ok(()) => {
                    let _ = window.hide();
                }
                Err(error) => window.set_message(error.into()),
            }
        }
    });
    window.on_cancel({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            if let Ok(token) = token(&window) {
                let _ = lock(&shared).cancel_property_edit(token);
            }
            let _ = window.hide();
        }
    });
    window.window().on_close_requested({
        let window = window.as_weak();
        move || {
            window.unwrap().invoke_cancel();
            slint::CloseRequestResponse::HideWindow
        }
    });
    bind_color(window, shared)?;
    bind_material(window, shared)
}

fn bind_color(
    parent: &PropertiesWindow,
    shared: &SharedSession,
) -> Result<(), slint::PlatformError> {
    let window = ColorWindow::new()?;
    let target = Rc::new(RefCell::new(None::<(u64, String, bool)>));
    parent.on_pick_color({
        let (parent, window, shared, target) = (
            parent.as_weak(),
            window.clone_strong(),
            shared.clone(),
            target.clone(),
        );
        move |key| {
            let session = lock(&shared);
            let result = token(&parent.unwrap()).and_then(|token| {
                session.check_property_draft(token)?;
                let property = session
                    .preview_property_edit()?
                    .properties
                    .into_iter()
                    .find(|property| property.field.key == key.as_str())
                    .ok_or("Property is no longer available")?;
                let field = property.field;
                let color = field.color.ok_or("Property is not a color")?;
                *target.borrow_mut() = Some((token, key.into(), field.kind == "color1"));
                window.set_field_label(field.label.into());
                window.set_red(color[0].into());
                window.set_green(color[1].into());
                window.set_blue(color[2].into());
                window.set_has_intensity(false);
                window.set_message(
                    "Color applies to every target. Brightness stays unchanged.".into(),
                );
                window.show().map_err(|error| error.to_string())
            });
            if let Err(error) = result {
                parent.unwrap().set_message(error.into());
            }
        }
    });
    window.on_apply({
        let (window, shared, target, parent) = (
            window.as_weak(),
            shared.clone(),
            target.clone(),
            parent.as_weak(),
        );
        move || {
            let window = window.unwrap();
            let Some((token, key, normalized)) = target.borrow().clone() else {
                return;
            };
            let mut session = lock(&shared);
            let result = session.check_property_draft(token).and_then(|()| {
                let mut changes = session.property_draft().unwrap().changes.clone();
                for (index, value) in [window.get_red(), window.get_green(), window.get_blue()]
                    .into_iter()
                    .enumerate()
                {
                    changes.push(PropertyChange::Component {
                        key: key.clone(),
                        index,
                        value: f64::from(value) / if normalized { 255.0 } else { 1.0 },
                    });
                }
                session.update_property_edit(token, changes)
            });
            match result {
                Ok(()) => {
                    refresh_properties(&parent.unwrap(), &session);
                    let _ = window.hide();
                }
                Err(error) => window.set_message(error.into()),
            }
        }
    });
    window.on_cancel({
        let window = window.as_weak();
        move || {
            let _ = window.unwrap().hide();
        }
    });
    Ok(())
}

fn bind_material(
    parent: &PropertiesWindow,
    shared: &SharedSession,
) -> Result<(), slint::PlatformError> {
    let window = MaterialWindow::new()?;
    let target = Rc::new(RefCell::new(None::<(u64, String)>));
    parent.on_pick_material({
        let (parent, window, shared, target) = (
            parent.as_weak(),
            window.clone_strong(),
            shared.clone(),
            target.clone(),
        );
        move |key| {
            let mut session = lock(&shared);
            let result = token(&parent.unwrap()).and_then(|token| {
                session.check_property_draft(token)?;
                let property = session
                    .preview_property_edit()?
                    .properties
                    .into_iter()
                    .find(|property| property.field.key == key.as_str())
                    .ok_or("Property is no longer available")?;
                *target.borrow_mut() = Some((token, key.into()));
                window.set_material(property.field.value.into());
                window.set_filter("".into());
                window.set_group("all".into());
                window.set_offset(0);
                crate::refresh_material_catalog(&window, &mut session);
                crate::refresh_material_window(&window, &mut session);
                window.show().map_err(|error| error.to_string())
            });
            if let Err(error) = result {
                parent.unwrap().set_message(error.into());
            }
        }
    });
    window.on_search({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || crate::refresh_material_catalog(&window.unwrap(), &mut lock(&shared))
    });
    window.on_preview_material({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || crate::refresh_material_window(&window.unwrap(), &mut lock(&shared))
    });
    window.on_toggle_favorite({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            if let Err(error) =
                session.favorite_material(&window.get_material(), !window.get_favorite())
            {
                window.set_detail(error.into());
            }
            crate::refresh_material_catalog(&window, &mut session);
        }
    });
    window.on_use_material({
        let (window, shared, target, parent) = (
            window.as_weak(),
            shared.clone(),
            target.clone(),
            parent.as_weak(),
        );
        move || {
            let window = window.unwrap();
            let Some((token, key)) = target.borrow().clone() else {
                return;
            };
            let mut session = lock(&shared);
            match session.append_property_change(
                token,
                PropertyChange::Set {
                    key,
                    value: window.get_material().into(),
                },
            ) {
                Ok(()) => {
                    refresh_properties(&parent.unwrap(), &session);
                    let _ = window.hide();
                }
                Err(error) => window.set_detail(error.into()),
            }
        }
    });
    window.on_cancel({
        let window = window.as_weak();
        move || {
            let _ = window.unwrap().hide();
        }
    });
    Ok(())
}

fn filter(window: &ReportWindow) -> Result<ObjectFilter, String> {
    let within = if window.get_limit_bounds() {
        let fields = [
            window.get_min_x(),
            window.get_min_y(),
            window.get_min_z(),
            window.get_max_x(),
            window.get_max_y(),
            window.get_max_z(),
        ];
        let values = fields
            .iter()
            .map(|value| {
                value
                    .parse::<f64>()
                    .map_err(|_| "Region coordinates must be numbers".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        Some([
            [values[0], values[1], values[2]],
            [values[3], values[4], values[5]],
        ])
    } else {
        None
    };
    fn text(value: slint::SharedString) -> Option<String> {
        (!value.is_empty()).then(|| value.to_string())
    }
    Ok(ObjectFilter {
        query: text(window.get_query()),
        classname: text(window.get_classname()),
        targetname: text(window.get_targetname()),
        material: text(window.get_material()),
        kind: [
            None,
            Some("solid"),
            Some("entity"),
            Some("point_entity"),
            Some("brush_entity"),
        ]
        .get(window.get_kind() as usize)
        .copied()
        .flatten()
        .map(str::to_owned),
        hidden: match window.get_visibility() {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        },
        within,
    })
}

pub fn refresh_report(window: &ReportWindow, session: &Session) {
    match filter(window).and_then(|filter| session.object_report(&filter)) {
        Ok(rows) => {
            window.set_message("".into());
            let offset = window.get_page().max(0) as usize * 100;
            window.set_generation(session.document_generation().to_string().into());
            window.set_revision(session.document().revision().to_string().into());
            window.set_summary(
                format!("{} matches · Page {}", rows.len(), window.get_page() + 1).into(),
            );
            window.set_has_next(offset + 100 < rows.len());
            window.set_rows(ModelRc::new(VecModel::from(
                rows.into_iter()
                    .skip(offset)
                    .take(100)
                    .map(|row| ObjectReportRow {
                        id: row.id.clone().into(),
                        label: format!(
                            "{}{}",
                            if row.targetname.is_empty() {
                                if row.classname.is_empty() {
                                    "World brush".into()
                                } else {
                                    crate::inspector::title(&row.classname)
                                }
                            } else {
                                row.targetname.clone()
                            },
                            if row.hidden { " · Hidden" } else { "" }
                        )
                        .into(),
                        detail: format!(
                            "{} · {} · {}{}",
                            row.id,
                            row.classname,
                            row.materials.join(", "),
                            if row.hidden_brushes > 0 {
                                format!(" · {} hidden brushes", row.hidden_brushes)
                            } else {
                                String::new()
                            }
                        )
                        .into(),
                    })
                    .collect::<Vec<_>>(),
            )));
        }
        Err(error) => {
            window.set_rows(ModelRc::default());
            window.set_summary("Invalid region or filter".into());
            window.set_has_next(false);
            window.set_message(error.into());
        }
    }
}

pub fn bind_report(window: &ReportWindow, shared: &SharedSession) {
    window.on_search({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || refresh_report(&window.unwrap(), &lock(&shared))
    });
    window.on_select_result({
        let (window, shared) = (window.as_weak(), shared.clone());
        move |id, frame| {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let result = if window.get_generation() != session.document_generation().to_string()
                || window.get_revision() != session.document().revision().to_string()
            {
                Err("Map changed; refresh results before selecting".into())
            } else {
                parse_id(&id).and_then(|id| {
                    session.select(&[id], SelectMode::Replace)?;
                    if frame {
                        session.frame_selection(Actor::User, window.get_aspect())?;
                    }
                    Ok(())
                })
            };
            window.set_message(result.err().unwrap_or_default().into());
        }
    });
    window.on_select_all({
        let (window, shared) = (window.as_weak(), shared.clone());
        move |frame| {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let mode = match window.get_selection_mode() {
                1 => SelectMode::Add,
                2 => SelectMode::Remove,
                3 => SelectMode::Toggle,
                _ => SelectMode::Replace,
            };
            let result = filter(&window).and_then(|filter| {
                session.select_report(Actor::User, &filter, mode, frame, window.get_aspect())
            });
            window.set_message(
                result
                    .map(|count| format!("Selected {count} matching objects"))
                    .unwrap_or_else(|error| error)
                    .into(),
            );
        }
    });
    window.on_visibility_action({
        let (window, shared) = (window.as_weak(), shared.clone());
        move |action| {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let ids = session.selected_ids();
            let result = session.set_visibility(Actor::User, &action, &ids);
            refresh_report(&window, &session);
            window.set_message(result.err().unwrap_or_default().into());
        }
    });
    window.on_cancel({
        let window = window.as_weak();
        move || {
            let _ = window.unwrap().hide();
        }
    });
}
