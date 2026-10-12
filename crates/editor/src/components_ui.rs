use std::rc::Rc;

use glam::DVec3;
use map_core::subobject::{Component, ComponentTransform, WeldTarget};
use slint::{ComponentHandle, Model, VecModel};

use crate::mcp::{SharedSession, lock};
use crate::session::component_picking::Kind;
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

fn weld_target(window: &ComponentsWindow, session: &Session) -> Result<WeldTarget, String> {
    match window.get_weld_target_mode() {
        0 => Ok(WeldTarget::Mean),
        1 => session
            .component_weld_targets()?
            .get(window.get_weld_corner() as usize)
            .map(|(component, _)| WeldTarget::Corner(component.clone()))
            .ok_or("Choose an existing brush corner".into()),
        2 => {
            let values = [
                window.get_weld_x(),
                window.get_weld_y(),
                window.get_weld_z(),
            ]
            .iter()
            .map(|value| {
                value
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| "Enter numeric weld target coordinates".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
            Ok(WeldTarget::Position(DVec3::new(
                values[0], values[1], values[2],
            )))
        }
        _ => Err("Choose a weld target mode".into()),
    }
}

fn vector(values: [slint::SharedString; 3]) -> Result<DVec3, String> {
    let mut result = DVec3::ZERO;
    for (index, value) in values.iter().enumerate() {
        result[index] = value
            .trim()
            .parse()
            .map_err(|_| "Enter numeric X, Y and Z values".to_string())?;
    }
    Ok(result)
}

fn transform_inputs(
    window: &ComponentsWindow,
) -> Result<(ComponentTransform, Option<DVec3>), String> {
    let spec = if window.get_operation() == 3 {
        ComponentTransform::Scale(vector([
            window.get_scale_x(),
            window.get_scale_y(),
            window.get_scale_z(),
        ])?)
    } else {
        ComponentTransform::Rotate(vector([
            window.get_rotate_x(),
            window.get_rotate_y(),
            window.get_rotate_z(),
        ])?)
    };
    let pivot = if window.get_pivot_mode() == 0 {
        None
    } else {
        Some(vector([
            window.get_pivot_x(),
            window.get_pivot_y(),
            window.get_pivot_z(),
        ])?)
    };
    Ok((spec, pivot))
}

fn refresh_sources(window: &ComponentsWindow, session: &Session) {
    if session.component_picking().is_none() {
        window.set_source_mode(0);
    }
    let mut offset = window.get_source_offset().max(0) as usize;
    let mut report = session
        .component_sources(offset, crate::session::components::SOURCE_PAGE_SIZE)
        .unwrap();
    let total = report["total"].as_u64().unwrap() as usize;
    if offset >= total && offset != 0 {
        offset = 0;
        report = session
            .component_sources(offset, crate::session::components::SOURCE_PAGE_SIZE)
            .unwrap();
    }
    let sources = report["sources"].as_array().unwrap();
    let mut labels = vec![slint::SharedString::from("Choose a brush to inspect…")];
    let mut ids = Vec::new();
    for source in sources {
        let material = source["material"].as_str().unwrap_or("no material");
        let bounds = &source["bounds"];
        let center = if bounds.is_null() {
            String::new()
        } else {
            let point = DVec3::from_array(std::array::from_fn(|index| {
                (bounds["mins"][index].as_f64().unwrap() + bounds["maxs"][index].as_f64().unwrap())
                    * 0.5
            }));
            format!(" · {}", position(point))
        };
        let id = source["solid"].as_u64().unwrap().to_string();
        labels.push(format!("Brush {id} · {material}{center}").into());
        ids.push(slint::SharedString::from(id));
    }
    let active = session.component_solids();
    let index = if active.len() == 1 {
        ids.iter()
            .position(|id| id.as_str() == active[0].to_string())
            .map_or(0, |index| index + 1)
    } else {
        0
    };
    window.set_source_labels(Rc::new(VecModel::from(labels)).into());
    window.set_source_ids(Rc::new(VecModel::from(ids)).into());
    window.set_source_index(index as i32);
    window.set_source_offset(offset as i32);
    window.set_source_total(total.min(i32::MAX as usize) as i32);
    window.set_source_all(total > 0 && total <= map_core::subobject::MAX_SOLIDS);
    window.set_source_next(offset.saturating_add(sources.len()) < total);
    window.set_source_summary(
        format!(
            "{}–{} of {total}",
            offset + usize::from(!sources.is_empty()),
            offset + sources.len()
        )
        .into(),
    );
}

fn capture_sources(window: &ComponentsWindow, shared: &SharedSession, ids: Vec<u32>) {
    let kind = match window.get_kind() {
        1 => Kind::Vertex,
        2 => Kind::Face,
        _ => Kind::Edge,
    };
    let mut session = lock(shared);
    let result = session.set_component_picking(Some(kind), &ids, window.get_stamp().as_str());
    refresh(window, &session);
    window.set_message(result.err().unwrap_or_default().into());
}

pub fn refresh(window: &ComponentsWindow, session: &Session) {
    let was_topology_preview = window.get_topology_preview();
    refresh_sources(window, session);
    let center = session.component_center().ok().flatten();
    window.set_transform_center(
        center
            .map_or("Select components to choose a center".into(), |point| {
                format!("World position: {}", position(point))
            })
            .into(),
    );
    let targets = session.component_weld_targets().unwrap_or_default();
    window.set_weld_corner_labels(
        Rc::new(VecModel::from(
            targets
                .iter()
                .map(|(component, point)| {
                    format!("Brush {} · {}", component.solid, position(*point)).into()
                })
                .collect::<Vec<slint::SharedString>>(),
        ))
        .into(),
    );
    window.set_weld_corner(
        window
            .get_weld_corner()
            .clamp(0, targets.len().saturating_sub(1) as i32),
    );
    let topology_preview = session.component_topology_preview();
    window.set_stamp(session.component_stamp().into());
    window.set_move_handles(session.component_move_options().enabled);
    window.set_snap(session.component_move_options().snap);
    window.set_texture_lock(session.component_move_options().texture_lock);
    window.set_picking(session.component_picking().is_some());
    if topology_preview {
        window.set_kind(1);
        window.set_picking_summary("Previewed shape · Apply or discard before selecting".into());
    } else if let Some(picking) = session.component_picking() {
        window.set_kind(match picking.kind {
            Kind::Edge => 0,
            Kind::Vertex => 1,
            Kind::Face => 2,
        });
        window.set_picking_summary(picking.hint().into());
    } else if was_topology_preview && let Some(component) = session.component_selection().first() {
        window.set_kind(match component.sides.len() {
            1 => 2,
            2 => 0,
            _ => 1,
        });
    }
    window.set_has_selection(!session.component_selection().is_empty());
    window.set_topology_preview(topology_preview);
    let ids = session.component_solids();
    let shapes = match session.component_topology(&ids) {
        Ok(shapes) => shapes,
        Err(error) => {
            window.set_rows(Rc::new(VecModel::default()).into());
            window.set_summary(format!("{error}. Choose a brush in Brush scope above.").into());
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
                    selected: session.displayed_component_selection().contains(&component),
                });
            }
        } else if window.get_kind() == 1 {
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
                    selected: session.displayed_component_selection().contains(&component),
                });
            }
        } else {
            for face in &shape.faces {
                let component = Component {
                    solid: shape.solid,
                    sides: vec![face.side],
                };
                let points = face
                    .vertices
                    .iter()
                    .map(|index| shape.vertices[*index].position)
                    .collect::<Vec<_>>();
                let normal = points[1..]
                    .windows(2)
                    .find_map(|pair| map_core::Plane::from_points([points[0], pair[0], pair[1]]))
                    .map_or(DVec3::ZERO, |plane| plane.normal);
                let direction = if normal.z > 0.999 {
                    "Top (+Z)"
                } else if normal.z < -0.999 {
                    "Bottom (−Z)"
                } else if normal.x > 0.999 {
                    "Right (+X)"
                } else if normal.x < -0.999 {
                    "Left (−X)"
                } else if normal.y > 0.999 {
                    "Back (+Y)"
                } else if normal.y < -0.999 {
                    "Front (−Y)"
                } else {
                    "Sloped face"
                };
                let center = points.iter().sum::<DVec3>() / points.len() as f64;
                rows.push(ComponentRow {
                    key: key(&component).into(),
                    label: format!(
                        "{}{direction} · face {}",
                        if shapes.len() == 1 {
                            String::new()
                        } else {
                            format!("Brush {} · ", shape.solid)
                        },
                        face.side
                    )
                    .into(),
                    detail: format!("{} corners · center {}", points.len(), position(center))
                        .into(),
                    selected: session.displayed_component_selection().contains(&component),
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
            session.displayed_component_selection().len()
        )
        .into()
    });
    window.set_rows(Rc::new(VecModel::from(rows)).into());
    let report = session.component_report(&[]).unwrap();
    let preview = &report["preview"];
    let token = preview["token"].as_str().unwrap_or_default();
    let weld = preview["operation"] == "weld";
    let split = preview["operation"] == "split";
    let transform = preview["operation"] == "scale" || preview["operation"] == "rotate";
    let operation = if transform {
        if preview["operation"] == "scale" {
            3
        } else {
            4
        }
    } else if split {
        2
    } else {
        i32::from(weld)
    };
    if !preview.is_null() && window.get_token() != token {
        window.set_operation(operation);
        if weld {
            let target = &preview["weld_target"];
            window.set_weld_target_mode(match target["mode"].as_str() {
                Some("corner") => 1,
                Some("position") => 2,
                _ => 0,
            });
            if window.get_weld_target_mode() == 1 {
                if let Some(index) = targets.iter().position(|(component, _)| {
                    crate::session::components::component_json(component) == target["component"]
                }) {
                    window.set_weld_corner(index as i32);
                }
            } else if window.get_weld_target_mode() == 2 {
                window.set_weld_x(target["position"][0].as_f64().unwrap().to_string().into());
                window.set_weld_y(target["position"][1].as_f64().unwrap().to_string().into());
                window.set_weld_z(target["position"][2].as_f64().unwrap().to_string().into());
            }
        }
        if transform {
            let values = &preview["transform"]["value"];
            if operation == 3 {
                window.set_scale_x(values[0].as_f64().unwrap().to_string().into());
                window.set_scale_y(values[1].as_f64().unwrap().to_string().into());
                window.set_scale_z(values[2].as_f64().unwrap().to_string().into());
            } else {
                window.set_rotate_x(values[0].as_f64().unwrap().to_string().into());
                window.set_rotate_y(values[1].as_f64().unwrap().to_string().into());
                window.set_rotate_z(values[2].as_f64().unwrap().to_string().into());
            }
            let pivot = &preview["transform"]["pivot"];
            window.set_pivot_mode(i32::from(!pivot.is_null()));
            if !pivot.is_null() {
                window.set_pivot_x(pivot[0].as_f64().unwrap().to_string().into());
                window.set_pivot_y(pivot[1].as_f64().unwrap().to_string().into());
                window.set_pivot_z(pivot[2].as_f64().unwrap().to_string().into());
            }
        }
        if split {
            let rise = preview["split"]["rise"].as_f64().unwrap();
            window.set_split_rise(rise.to_string().into());
            window.set_split_rise_value(rise as f32);
        }
        window.set_texture_lock(preview["texture_lock"].as_bool().unwrap_or(false));
        window.set_move_x(preview["delta"][0].as_f64().unwrap().to_string().into());
        window.set_move_y(preview["delta"][1].as_f64().unwrap().to_string().into());
        window.set_move_z(preview["delta"][2].as_f64().unwrap().to_string().into());
    }
    window.set_token(token.into());
    window.set_ready(
        !preview.is_null()
            && session
                .component_drag()
                .is_none_or(|drag| drag.error.is_none())
            && window.get_operation() == operation
            && (if transform {
                window.get_texture_lock() == preview["texture_lock"].as_bool().unwrap_or(false)
                    && transform_inputs(window).is_ok_and(|(spec, pivot)| {
                        let resolved = pivot.or(center).unwrap();
                        crate::session::components::transform_json(&spec, pivot, resolved)
                            == preview["transform"]
                    })
            } else if split {
                window.get_split_rise().parse::<f64>().ok() == preview["split"]["rise"].as_f64()
            } else if weld {
                weld_target(window, session).is_ok_and(|target| {
                    crate::session::components::weld_target_json(&target) == preview["weld_target"]
                })
            } else {
                window.get_texture_lock() == preview["texture_lock"].as_bool().unwrap_or(false)
                    && [
                        window.get_move_x(),
                        window.get_move_y(),
                        window.get_move_z(),
                    ]
                    .iter()
                    .enumerate()
                    .all(|(index, value)| {
                        value.parse::<f64>().ok() == preview["delta"][index].as_f64()
                    })
            }),
    );
    window.set_preview_summary(if preview.is_null() {
        "Preview the change before applying.".into()
    } else if transform {
        let values = &preview["transform"]["value"];
        let pivot = &preview["transform"]["resolved_pivot"];
        let point = DVec3::new(pivot[0].as_f64().unwrap(),pivot[1].as_f64().unwrap(),pivot[2].as_f64().unwrap());
        format!("{}: {}, {}, {}{}\nPivot: {}\nThe map is unchanged until Apply.",if operation == 3 {"Scale factors"} else {"Angles"},values[0],values[1],values[2],if operation == 3 {""} else {" degrees"},position(point)).into()
    } else if split {
        let changes = preview["split"]["changes"].as_array().unwrap();
        let added: usize = changes.iter().map(|change| change["new_sides"].as_array().unwrap().len()).sum();
        let corners = if changes.len() == 1 { "a corner".into() } else { format!("{} corners", changes.len()) };
        let rise = number(preview["split"]["rise"].as_f64().unwrap());
        format!("Add {corners} and {added} faces. Raise by {rise} units.\nThe map is unchanged until Apply.").into()
    } else if weld {
        let changes = preview["weld"].as_array().unwrap();
        let merged: u64 = changes.iter().map(|change|change["merged_vertices"].as_u64().unwrap()).sum();
        let removed: usize = changes.iter().map(|change|change["removed_sides"].as_array().unwrap().len()).sum();
        let target = if preview["weld_target"]["mode"] == "mean" { "Average selected corners in each brush".into() } else if changes.len() == 1 {
            let values = &changes[0]["position"];
            format!("Target: {}",position(DVec3::new(values[0].as_f64().unwrap(),values[1].as_f64().unwrap(),values[2].as_f64().unwrap())))
        } else { "One shared world-space target".into() };
        let corner_word = if changes.len() == 1 { "corner" } else { "corners" };
        let face_word = if removed == 1 { "face" } else { "faces" };
        format!("Merge {merged} corners into {} {corner_word}. Remove {removed} collapsed {face_word}.\n{target}\nThe map is unchanged until Apply.",changes.len()).into()
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

fn picking(window: &ComponentsWindow, shared: &SharedSession, enabled: bool) {
    let kind = match window.get_kind() {
        1 => Kind::Vertex,
        2 => Kind::Face,
        _ => Kind::Edge,
    };
    let mut session = lock(shared);
    let ids = session.component_solids();
    let result =
        session.set_component_picking(enabled.then_some(kind), &ids, window.get_stamp().as_str());
    refresh(window, &session);
    window.set_message(result.err().unwrap_or_default().into());
}

pub fn bind(window: &ComponentsWindow, shared: &SharedSession) {
    window.on_choose_source({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            if window.get_source_index() <= 0 {
                return;
            }
            if let Some(id) = window
                .get_source_ids()
                .row_data(window.get_source_index() as usize - 1)
                .and_then(|id| id.parse::<u32>().ok())
            {
                let action = match window.get_source_mode() {
                    1 => crate::session::components::ScopeAction::Add,
                    2 => crate::session::components::ScopeAction::Remove,
                    _ => crate::session::components::ScopeAction::Replace,
                };
                let kind = match window.get_kind() {
                    1 => Kind::Vertex,
                    2 => Kind::Face,
                    _ => Kind::Edge,
                };
                let mut session = lock(&shared);
                let result =
                    session.update_component_scope(kind, id, action, window.get_stamp().as_str());
                refresh(&window, &session);
                window.set_message(result.err().unwrap_or_default().into());
            }
        }
    });
    window.on_all_sources({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let session = lock(&shared);
            let report = session
                .component_sources(0, map_core::subobject::MAX_SOLIDS)
                .unwrap();
            if report["total"].as_u64().unwrap() as usize > map_core::subobject::MAX_SOLIDS {
                return;
            }
            let ids = report["sources"]
                .as_array()
                .unwrap()
                .iter()
                .map(|source| source["solid"].as_u64().unwrap() as u32)
                .collect();
            drop(session);
            capture_sources(&window, &shared, ids);
        }
    });
    window.on_page_sources({
        let (window, shared) = (window.as_weak(), shared.clone());
        move |direction| {
            let window = window.unwrap();
            window.set_source_offset(
                window
                    .get_source_offset()
                    .saturating_add(
                        direction
                            .saturating_mul(crate::session::components::SOURCE_PAGE_SIZE as i32),
                    )
                    .max(0),
            );
            refresh(&window, &lock(&shared));
        }
    });
    window.on_toggle_picking({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            picking(&window, &shared, !window.get_picking());
        }
    });
    window.on_change_kind({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            if window.get_picking() {
                picking(&window, &shared, true);
            } else {
                refresh(&window, &lock(&shared));
            }
        }
    });
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
    window.on_choose_pivot({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            if window.get_pivot_mode() == 1
                && let Ok(Some(point)) = lock(&shared).component_center()
            {
                window.set_pivot_x(point.x.to_string().into());
                window.set_pivot_y(point.y.to_string().into());
                window.set_pivot_z(point.z.to_string().into());
            }
            window.invoke_edited();
        }
    });
    window.on_choose_weld_mode({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            if window.get_weld_target_mode() == 2
                && let Ok(Some(center)) = lock(&shared).component_weld_center()
            {
                window.set_weld_x(center.x.to_string().into());
                window.set_weld_y(center.y.to_string().into());
                window.set_weld_z(center.z.to_string().into());
            }
            window.invoke_edited();
        }
    });
    window.on_edited({
        let window = window.as_weak();
        move || {
            let window = window.unwrap();
            if let Ok(rise) = window.get_split_rise().parse::<f64>()
                && (map_core::subobject::MIN_SPLIT_RISE..=map_core::subobject::MAX_SPLIT_RISE)
                    .contains(&rise)
            {
                window.set_split_rise_value(rise as f32);
            }
            window.set_ready(false);
            window.set_message("Settings changed. Preview the change before applying.".into());
        }
    });
    window.on_choose_rise({
        let window = window.as_weak();
        move |value| {
            let window = window.unwrap();
            window.set_split_rise(value.to_string().into());
            window.invoke_edited();
        }
    });
    window.on_move_options({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let result = session.set_component_move_options(
                crate::session::component_drag::Options {
                    enabled: window.get_move_handles(),
                    snap: window.get_snap(),
                    texture_lock: window.get_texture_lock(),
                },
                window.get_stamp().as_str(),
            );
            refresh(&window, &session);
            window.set_message(result.err().unwrap_or_default().into());
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
            let result = if window.get_operation() >= 3 {
                transform_inputs(&window).and_then(|(spec, pivot)| {
                    session.preview_component_transform(
                        spec,
                        pivot,
                        window.get_texture_lock(),
                        window.get_stamp().as_str(),
                    )
                })
            } else if window.get_operation() == 2 {
                window
                    .get_split_rise()
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| "Enter a numeric split rise".to_string())
                    .and_then(|rise| {
                        session.preview_component_split(rise, window.get_stamp().as_str())
                    })
            } else if window.get_operation() == 1 {
                weld_target(&window, &session).and_then(|target| {
                    session.preview_component_weld_target(target, window.get_stamp().as_str())
                })
            } else {
                delta.and_then(|delta| {
                    session.preview_component_move(
                        DVec3::new(delta[0], delta[1], delta[2]),
                        window.get_snap(),
                        window.get_texture_lock(),
                        window.get_stamp().as_str(),
                    )
                })
            };
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
                .map_err(|_| "Preview a change first".to_string())
                .and_then(|token| session.commit_component_edit(Actor::User, token));
            refresh(&window, &session);
            window.set_message(
                result
                    .map(|count| {
                        format!("Changed {count} brushes. Undo restores the previous shape.")
                    })
                    .unwrap_or_else(|error| error)
                    .into(),
            );
        }
    });
    window.on_discard_preview({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let result = window
                .get_token()
                .parse()
                .map_err(|_| "Preview a change first".to_string())
                .and_then(|token| session.cancel_component_move(token));
            refresh(&window, &session);
            window.set_message(result.err().unwrap_or_default().into());
        }
    });
    window.on_cancel({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            if session.component_stamp() == window.get_stamp().as_str() {
                let _ = session.set_component_picking(None, &[], window.get_stamp().as_str());
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
