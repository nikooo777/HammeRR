use std::rc::Rc;

use glam::DVec3;
use map_core::subobject::Component;
use slint::{ComponentHandle, Model, VecModel};

use crate::mcp::{SharedSession, lock};
use crate::session::{Actor, SelectMode, Session};
use crate::{ComponentRow, ComponentsWindow};

fn key(component: &Component) -> String {
    format!(
        "{}:{}",
        component.solid,
        component
            .sides
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn component(key: &str) -> Result<Component, String> {
    let (solid, sides) = key.split_once(':').ok_or("Invalid component")?;
    Ok(Component {
        solid: solid.parse().map_err(|_| "Invalid brush ID")?,
        sides: sides
            .split(',')
            .map(|side| side.parse().map_err(|_| "Invalid face ID".to_string()))
            .collect::<Result<_, _>>()?,
    })
}

fn number(value: f64) -> String {
    if value.abs() < 0.0005 {
        return "0".into();
    }
    format!("{value:.3}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .into()
}

fn position(point: DVec3) -> String {
    format!(
        "({}, {}, {})",
        number(point.x),
        number(point.y),
        number(point.z)
    )
}

pub fn refresh(window: &ComponentsWindow, session: &Session) {
    window.set_stamp(session.component_stamp().into());
    window.set_has_selection(!session.component_selection().is_empty());
    let ids = session.component_solids();
    let shapes = match session.component_topology(&ids) {
        Ok(shapes) => shapes,
        Err(error) => {
            window.set_rows(Rc::new(VecModel::default()).into());
            window.set_summary(error.into());
            window.set_ready(false);
            return;
        }
    };
    let mut rows = Vec::new();
    for shape in &shapes {
        if window.get_kind() == 0 {
            for (index, edge) in shape.edges.iter().enumerate() {
                let component = Component {
                    solid: shape.solid,
                    sides: edge.sides.clone(),
                };
                let [a, b] = edge.vertices.map(|index| shape.vertices[index].position);
                rows.push(ComponentRow {
                    key: key(&component).into(),
                    label: format!(
                        "{}Edge {} · {} units",
                        if shapes.len() == 1 {
                            String::new()
                        } else {
                            format!("Brush {} · ", shape.solid)
                        },
                        index + 1,
                        number(a.distance(b))
                    )
                    .into(),
                    detail: format!("{} → {}", position(a), position(b)).into(),
                    selected: session.component_selection().contains(&component),
                });
            }
        } else {
            for (index, vertex) in shape.vertices.iter().enumerate() {
                let component = Component {
                    solid: shape.solid,
                    sides: vertex.sides.clone(),
                };
                rows.push(ComponentRow {
                    key: key(&component).into(),
                    label: format!(
                        "{}Vertex {}",
                        if shapes.len() == 1 {
                            String::new()
                        } else {
                            format!("Brush {} · ", shape.solid)
                        },
                        index + 1
                    )
                    .into(),
                    detail: format!("X, Y, Z: {}", position(vertex.position)).into(),
                    selected: session.component_selection().contains(&component),
                });
            }
        }
    }
    window.set_summary(if shapes.is_empty() {
        "Select brushes in the map to inspect their shape.".into()
    } else {
        format!(
            "{} · {} selected · coordinates in X, Y, Z",
            if shapes.len() == 1 {
                format!("Brush {}", shapes[0].solid)
            } else {
                format!("{} brushes", shapes.len())
            },
            session.component_selection().len()
        )
        .into()
    });
    window.set_rows(Rc::new(VecModel::from(rows)).into());
    let report = session.component_report(&[]).unwrap();
    let preview = &report["preview"];
    let token = preview["token"].as_str().unwrap_or_default();
    if !preview.is_null() && window.get_token() != token {
        window.set_move_x(preview["delta"][0].as_f64().unwrap().to_string().into());
        window.set_move_y(preview["delta"][1].as_f64().unwrap().to_string().into());
        window.set_move_z(preview["delta"][2].as_f64().unwrap().to_string().into());
    }
    window.set_token(token.into());
    window.set_ready(
        !preview.is_null()
            && [
                window.get_move_x(),
                window.get_move_y(),
                window.get_move_z(),
            ]
            .iter()
            .enumerate()
            .all(|(index, value)| value.parse::<f64>().ok() == preview["delta"][index].as_f64()),
    );
    window.set_preview_summary(if preview.is_null() {
        "Preview a move before applying.".into()
    } else {
        let delta = preview["delta"].as_array().unwrap();
        format!(
            "Showing offset: {}, {}, {}\nThe map is unchanged until Apply.",
            delta[0], delta[1], delta[2]
        )
        .into()
    });
}

pub fn refresh_changed(window: &ComponentsWindow, session: &Session) {
    if window.get_stamp() != session.component_stamp() {
        refresh(window, session);
    }
}

fn selection(
    window: &ComponentsWindow,
    shared: &SharedSession,
    components: Result<Vec<Component>, String>,
    mode: SelectMode,
) {
    let mut session = lock(shared);
    let result = components.and_then(|components| {
        session.select_components(&components, mode, window.get_stamp().as_str())
    });
    refresh(window, &session);
    window.set_message(result.err().unwrap_or_default().into());
}

pub fn bind(window: &ComponentsWindow, shared: &SharedSession) {
    window.on_refresh({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || refresh(&window.unwrap(), &lock(&shared))
    });
    window.on_choose({
        let (window, shared) = (window.as_weak(), shared.clone());
        move |key| {
            selection(
                &window.unwrap(),
                &shared,
                component(&key).map(|component| vec![component]),
                SelectMode::Toggle,
            )
        }
    });
    window.on_select_all({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let components = window
                .get_rows()
                .iter()
                .map(|row| component(row.key.as_str()))
                .collect();
            selection(&window, &shared, components, SelectMode::Replace);
        }
    });
    window.on_clear({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            selection(
                &window.unwrap(),
                &shared,
                Ok(Vec::new()),
                SelectMode::Replace,
            )
        }
    });
    window.on_edited({
        let window = window.as_weak();
        move || {
            let window = window.unwrap();
            window.set_ready(false);
            window.set_message("Offsets changed. Preview the new move before applying.".into());
        }
    });
    window.on_preview_edit({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let delta = [
                window.get_move_x(),
                window.get_move_y(),
                window.get_move_z(),
            ]
            .iter()
            .map(|value| {
                value
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| "Enter numeric X, Y and Z offsets".to_string())
            })
            .collect::<Result<Vec<_>, _>>();
            let mut session = lock(&shared);
            let result = delta.and_then(|delta| {
                session.preview_component_move(
                    DVec3::new(delta[0], delta[1], delta[2]),
                    window.get_snap(),
                    window.get_stamp().as_str(),
                )
            });
            refresh(&window, &session);
            match result {
                Ok(_) => window.set_message("".into()),
                Err(error) => {
                    window.set_ready(false);
                    window.set_message(error.into());
                }
            }
        }
    });
    window.on_apply({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            if !window.get_ready() {
                return;
            }
            let mut session = lock(&shared);
            let result = window
                .get_token()
                .parse()
                .map_err(|_| "Preview a move first".to_string())
                .and_then(|token| session.commit_component_move(Actor::User, token));
            refresh(&window, &session);
            window.set_message(
                result
                    .map(|count| {
                        format!(
                            "Moved components in {count} brushes. Undo restores the previous shape."
                        )
                    })
                    .unwrap_or_else(|error| error)
                    .into(),
            );
        }
    });
    window.on_cancel({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            if session.component_stamp() == window.get_stamp().as_str() {
                let _ = session.select_components(
                    &[],
                    SelectMode::Replace,
                    window.get_stamp().as_str(),
                );
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
}
