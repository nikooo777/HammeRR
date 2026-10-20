use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::identity::parse_id;
use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::{Actor, ObjectFilter, PropertyChange, PropertyTarget, SelectMode, Session};
use crate::{
    ColorWindow, ManagedFieldRow, MaterialWindow, ObjectReportRow, PropertiesWindow, ReportWindow,
};

#[cfg(test)]
use crate::runtime::{SharedSession, lock};

fn token(window: &PropertiesWindow) -> Result<u64, String> {
    window
        .get_draft_token()
        .parse()
        .map_err(|_| "Reopen properties".into())
}

pub fn refresh_properties(window: &PropertiesWindow, session: &Session) {
    window.set_can_apply(false);
    window.set_world_settings(false);
    let Some(draft) = session.property_draft() else {
        window.set_fields(ModelRc::default());
        window.set_message("Property edit closed. Reopen properties to start again.".into());
        return;
    };
    window.set_draft_token(draft.token.to_string().into());
    window.set_world_settings(draft.target == PropertyTarget::World);
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
                field: crate::ui::property_row(property.field),
                status: status.into(),
                can_reset: property.can_reset,
            }
        })
        .collect::<Vec<_>>();
    window.set_fields(ModelRc::new(VecModel::from(rows)));
}

fn stage(window: &PropertiesWindow, session: &mut Session, change: PropertyChange) {
    let result = token(window).and_then(|token| session.append_property_change(token, change));
    refresh_properties(window, session);
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

fn property_component(world: bool, key: &str, index: i32, value: &str) -> Result<f64, String> {
    let index = usize::try_from(index).map_err(|_| "Enter a finite number".to_owned())?;
    if world && key.eq_ignore_ascii_case(compiler::WATER_CURRENT_VELOCITY_KEY) {
        let mut parts = ["0"; compiler::WATER_CURRENT_COMPONENTS];
        let component = parts
            .get_mut(index)
            .ok_or_else(|| format!("{key} has no component {index}"))?;
        *component = value;
        compiler::validate_water_current_velocity(Some(&parts.join(" ")))?;
    }
    value.parse().map_err(|_| "Enter a finite number".into())
}

fn with_properties<R>(
    window: &slint::Weak<PropertiesWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&PropertiesWindow, &mut Session) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    match access
        .upgrade()
        .and_then(|access| access.with_session(|session| Ok(body(&window, session))))
    {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_message(error.into());
            None
        }
    }
}

fn with_color<R>(
    window: &slint::Weak<ColorWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&ColorWindow, &mut Session) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    match access
        .upgrade()
        .and_then(|access| access.with_session(|session| Ok(body(&window, session))))
    {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_message(error.into());
            None
        }
    }
}

fn with_material<R>(
    window: &slint::Weak<MaterialWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&MaterialWindow, &mut Session) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    match access
        .upgrade()
        .and_then(|access| access.with_session(|session| Ok(body(&window, session))))
    {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_detail(error.into());
            None
        }
    }
}

#[cfg(test)]
pub fn bind_properties(
    window: &PropertiesWindow,
    shared: &SharedSession,
) -> Result<(), slint::PlatformError> {
    bind_properties_with_access(window, &DocumentAccess::unmanaged(shared.clone())).map(|_| ())
}

pub(crate) struct PropertyPickers {
    pub(crate) color: ColorWindow,
    pub(crate) material: MaterialWindow,
    pub(crate) sky: Rc<RefCell<Option<crate::sky_assets_ui::Controller>>>,
}

impl PropertyPickers {
    pub(crate) fn hide_windows(&self, errors: &mut Vec<String>) {
        crate::ui::hide_window("Properties Color", self.color.window(), errors);
        crate::ui::hide_window("Properties Material", self.material.window(), errors);
        match self.sky.try_borrow() {
            Ok(sky) => {
                if let Some(sky) = sky.as_ref() {
                    crate::ui::hide_window("Properties Sky", sky.window.window(), errors);
                }
            }
            Err(error) => errors.push(format!("Properties Sky is still borrowed: {error}")),
        }
    }
}

pub(crate) fn bind_properties_with_access(
    window: &PropertiesWindow,
    access: &DocumentAccess,
) -> Result<PropertyPickers, slint::PlatformError> {
    window.on_search({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_properties(&window, &access, |window, session| {
                refresh_properties(window, session)
            });
        }
    });
    window.on_edit({
        let (window, access) = (window.as_weak(), access.downgrade());
        move |key, value| {
            with_properties(&window, &access, |window, session| {
                stage(
                    window,
                    session,
                    PropertyChange::Set {
                        key: key.into(),
                        value: value.into(),
                    },
                )
            });
        }
    });
    window.on_reset({
        let (window, access) = (window.as_weak(), access.downgrade());
        move |key| {
            with_properties(&window, &access, |window, session| {
                stage(window, session, PropertyChange::Default { key: key.into() })
            });
        }
    });
    window.on_remove({
        let (window, access) = (window.as_weak(), access.downgrade());
        move |key| {
            with_properties(&window, &access, |window, session| {
                stage(window, session, PropertyChange::Remove { key: key.into() })
            });
        }
    });
    window.on_flag_edit({
        let (window, access) = (window.as_weak(), access.downgrade());
        move |key, flag, enabled| {
            with_properties(&window, &access, |window, session| match flag.parse() {
                Ok(flag) => stage(
                    window,
                    session,
                    PropertyChange::Flag {
                        key: key.into(),
                        flag,
                        enabled,
                    },
                ),
                Err(_) => window.set_message("Choose a valid flag".into()),
            });
        }
    });
    window.on_component_edit({
        let (window, access) = (window.as_weak(), access.downgrade());
        move |key, index, value| {
            with_properties(
                &window,
                &access,
                |window, session| match property_component(
                    window.get_world_settings(),
                    &key,
                    index,
                    &value,
                ) {
                    Ok(value) => stage(
                        window,
                        session,
                        PropertyChange::Component {
                            key: key.into(),
                            index: index as usize,
                            value,
                        },
                    ),
                    Err(error) => window.set_message(error.into()),
                },
            );
        }
    });
    window.on_apply({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_properties(&window, &access, |window, session| {
                match token(window)
                    .and_then(|token| session.commit_property_edit(Actor::User, token))
                {
                    Ok(()) => {
                        let _ = window.hide();
                    }
                    Err(error) => window.set_message(error.into()),
                }
            });
        }
    });
    window.on_cancel({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            let Some(owner) = window.upgrade() else {
                return;
            };
            with_properties(&window, &access, |window, session| {
                if let Ok(token) = token(window) {
                    let _ = session.cancel_property_edit(token);
                }
            });
            let _ = owner.hide();
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
    let color = bind_color(window, access)?;
    let sky = crate::sky_assets_ui::bind_with_access(window, access);
    let material = bind_material(window, access)?;
    Ok(PropertyPickers {
        color,
        material,
        sky,
    })
}

fn bind_color(
    parent: &PropertiesWindow,
    access: &DocumentAccess,
) -> Result<ColorWindow, slint::PlatformError> {
    let window = ColorWindow::new()?;
    crate::color_ui::bind(&window);
    crate::color_ui::bind_palette(&window, access);
    let target = Rc::new(RefCell::new(None::<(u64, String, bool)>));
    parent.on_pick_color({
        let (parent, window, access, target) = (
            parent.as_weak(),
            window.clone_strong(),
            access.downgrade(),
            target.clone(),
        );
        move |key| {
            with_properties(&parent, &access, |parent, session| {
                let result = token(parent).and_then(|token| {
                    session.check_property_draft(token)?;
                    let property = session
                        .preview_property_edit()?
                        .properties
                        .into_iter()
                        .find(|property| property.field.key == key.as_str())
                        .ok_or("Property is no longer available")?;
                    let field = property.field;
                    field.color.ok_or("Property is not a color")?;
                    crate::color_ui::load(&window, &field.kind, &field.value)?;
                    crate::color_ui::refresh_palette(&window, session);
                    *target.borrow_mut() = Some((token, key.into(), field.kind == "color1"));
                    window.set_field_label(field.label.into());
                    window.set_has_intensity(false);
                    window.set_message(
                        "Color applies to every target. Brightness stays unchanged.".into(),
                    );
                    window.show().map_err(|error| error.to_string())
                });
                if let Err(error) = result {
                    parent.set_message(error.into());
                }
            });
        }
    });
    window.on_apply({
        let (window, access, target, parent) = (
            window.as_weak(),
            access.downgrade(),
            target.clone(),
            parent.as_weak(),
        );
        move || {
            with_color(&window, &access, |window, session| {
                let Some(parent) = parent.upgrade() else {
                    return;
                };
                let Some((token, key, normalized)) = target.borrow().clone() else {
                    return;
                };
                let result = session.check_property_draft(token).and_then(|()| {
                    let color = crate::color_ui::read(window)?;
                    let channels = if normalized {
                        color.rgb1
                    } else {
                        color.rgb255.map(f64::from)
                    };
                    let mut changes = session.property_draft().unwrap().changes.clone();
                    for (index, value) in channels.into_iter().enumerate() {
                        changes.push(PropertyChange::Component {
                            key: key.clone(),
                            index,
                            value,
                        });
                    }
                    session.update_property_edit(token, changes)
                });
                match result {
                    Ok(()) => {
                        refresh_properties(&parent, session);
                        let _ = window.hide();
                    }
                    Err(error) => window.set_message(error.into()),
                }
            });
        }
    });
    window.on_cancel({
        let window = window.as_weak();
        move || {
            if let Some(window) = window.upgrade() {
                let _ = window.hide();
            }
        }
    });
    window
        .window()
        .on_close_requested(|| slint::CloseRequestResponse::HideWindow);
    Ok(window)
}

fn bind_material(
    parent: &PropertiesWindow,
    access: &DocumentAccess,
) -> Result<MaterialWindow, slint::PlatformError> {
    let window = MaterialWindow::new()?;
    crate::ui::bind_material_reload_with_access(&window, access);
    let target = Rc::new(RefCell::new(None::<(u64, String)>));
    parent.on_pick_material({
        let (parent, window, access, target) = (
            parent.as_weak(),
            window.clone_strong(),
            access.downgrade(),
            target.clone(),
        );
        move |key| {
            with_properties(&parent, &access, |parent, session| {
                let result = token(parent).and_then(|token| {
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
                    crate::ui::refresh_material_catalog(&window, session);
                    crate::ui::refresh_material_window(&window, session);
                    window.show().map_err(|error| error.to_string())
                });
                if let Err(error) = result {
                    parent.set_message(error.into());
                }
            });
        }
    });
    window.on_search({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_material(&window, &access, |window, session| {
                crate::ui::refresh_material_catalog(window, session)
            });
        }
    });
    window.on_preview_material({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_material(&window, &access, |window, session| {
                crate::ui::refresh_material_window(window, session)
            });
        }
    });
    window.on_toggle_favorite({
        let (window, access) = (window.as_weak(), access.downgrade());
        move || {
            with_material(&window, &access, |window, session| {
                if let Err(error) =
                    session.favorite_material(&window.get_material(), !window.get_favorite())
                {
                    window.set_detail(error.into());
                }
                crate::ui::refresh_material_catalog(window, session);
            });
        }
    });
    window.on_use_material({
        let (window, access, target, parent) = (
            window.as_weak(),
            access.downgrade(),
            target.clone(),
            parent.as_weak(),
        );
        move || {
            with_material(&window, &access, |window, session| {
                let Some(parent) = parent.upgrade() else {
                    return;
                };
                let Some((token, key)) = target.borrow().clone() else {
                    return;
                };
                if !crate::ui::review_material_choice(window, session) {
                    return;
                }
                match session.append_property_change(
                    token,
                    PropertyChange::Set {
                        key,
                        value: window.get_material().into(),
                    },
                ) {
                    Ok(()) => {
                        refresh_properties(&parent, session);
                        let _ = window.hide();
                    }
                    Err(error) => window.set_detail(error.into()),
                }
            });
        }
    });
    window.on_cancel({
        let window = window.as_weak();
        move || {
            if let Some(window) = window.upgrade() {
                let _ = window.hide();
            }
        }
    });
    window
        .window()
        .on_close_requested(|| slint::CloseRequestResponse::HideWindow);
    Ok(window)
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

fn with_report<R>(
    window: &slint::Weak<ReportWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&ReportWindow, &mut Session) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    match access
        .upgrade()
        .and_then(|access| access.with_session(|session| Ok(body(&window, session))))
    {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_message(error.into());
            None
        }
    }
}

#[cfg(test)]
pub fn bind_report(window: &ReportWindow, shared: &SharedSession) {
    bind_report_with_access(window, &DocumentAccess::unmanaged(shared.clone()));
}

pub fn bind_report_with_access(window: &ReportWindow, access: &DocumentAccess) {
    let access = access.downgrade();
    window.on_search({
        let (window, access) = (window.as_weak(), access.clone());
        move || {
            with_report(&window, &access, |window, session| {
                refresh_report(window, session);
            });
        }
    });
    window.on_select_result({
        let (window, access) = (window.as_weak(), access.clone());
        move |id, frame| {
            with_report(&window, &access, |window, session| {
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
            });
        }
    });
    window.on_select_all({
        let (window, access) = (window.as_weak(), access.clone());
        move |frame| {
            with_report(&window, &access, |window, session| {
                let mode = match window.get_selection_mode() {
                    1 => SelectMode::Add,
                    2 => SelectMode::Remove,
                    3 => SelectMode::Toggle,
                    _ => SelectMode::Replace,
                };
                let result = filter(window).and_then(|filter| {
                    session.select_report(Actor::User, &filter, mode, frame, window.get_aspect())
                });
                window.set_message(
                    result
                        .map(|count| format!("Selected {count} matching objects"))
                        .unwrap_or_else(|error| error)
                        .into(),
                );
            });
        }
    });
    window.on_visibility_action({
        let (window, access) = (window.as_weak(), access.clone());
        move |action| {
            with_report(&window, &access, |window, session| {
                let ids = session.selected_ids();
                let result = session.set_visibility(Actor::User, &action, &ids);
                refresh_report(window, session);
                window.set_message(result.err().unwrap_or_default().into());
            });
        }
    });
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
mod tests;

#[cfg(test)]
mod entity_report_admission_tests;

#[cfg(test)]
mod water_current_tests;

#[cfg(test)]
mod water_damping_tests;
