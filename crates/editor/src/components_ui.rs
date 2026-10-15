use std::rc::Rc;

use glam::DVec3;
use map_core::subobject::{Component, ComponentTransform, WeldTarget};
use slint::{ComponentHandle, Model, VecModel};

use crate::runtime::{SharedSession, lock};
use crate::session::component_picking::Kind;
use crate::session::{Actor, SelectMode, Session};
use crate::{ComponentCreaseRow, ComponentRow, ComponentsWindow};

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

fn percentage(value: f64) -> String {
    format!("{:.12}", value * 100.0)
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

fn crease_inputs(
    window: &ComponentsWindow,
    session: &Session,
) -> Result<Vec<crate::session::components::CreaseEdges>, String> {
    let choices = if window.get_operation() == 6 {
        session.component_partition_choices()?
    } else {
        session.component_crease_choices()?
    };
    if choices.len() != session.component_selection().len() {
        return Err(if window.get_operation() == 6 {
            "Select one face with 3–8 corners per brush"
        } else {
            "Select one face with four corners per brush"
        }
        .into());
    }
    choices
        .iter()
        .map(|(component, pairs)| {
            let row = window
                .get_crease_rows()
                .iter()
                .find(|row| row.key == key(component))
                .ok_or("Choose a cut direction for each face")?;
            let pair = pairs
                .get(row.choice as usize)
                .ok_or("Choose an available direction")?;
            Ok(crate::session::components::CreaseEdges {
                solid: component.solid,
                edges: pair.edges,
            })
        })
        .collect()
}

fn ridge_direction(pair: &map_core::subobject::CreasePair) -> String {
    let delta = pair.midpoints[1] - pair.midpoints[0];
    let unit = delta.normalize().abs();
    let axis = if unit.x > 0.999 {
        "X direction"
    } else if unit.y > 0.999 {
        "Y direction"
    } else if unit.z > 0.999 {
        "Z direction"
    } else {
        "Sloped direction"
    };
    if axis == "Sloped direction" {
        format!("{axis} · {}", position(delta))
    } else {
        format!("{axis} · {} units", number(delta.length()))
    }
}

fn refresh_crease_choices(
    window: &ComponentsWindow,
    report: &serde_json::Value,
    session: &Session,
) {
    let previous = window.get_crease_rows().iter().collect::<Vec<_>>();
    let preview = &report["preview"];
    let changed = !preview.is_null() && window.get_token() != preview["token"].as_str().unwrap();
    let partition = if changed {
        preview["operation"] == "partition" || preview["operation"] == "extrusion"
    } else {
        window.get_operation() == 6
    };
    let choices = if partition {
        session.component_partition_choices()
    } else {
        session.component_crease_choices()
    }
    .unwrap_or_default();
    let rows = choices
        .iter()
        .map(|(component, pairs)| {
            let shape = map_core::subobject::solid(session.document(), component.solid)
                .and_then(|(_, source)| map_core::subobject::topology(source))
                .ok();
            let quad = shape.as_ref().is_some_and(|shape| {
                shape
                    .faces
                    .iter()
                    .any(|face| face.side == component.sides[0] && face.vertices.len() == 4)
            });
            let key = key(component);
            let mut choice = previous
                .iter()
                .find(|row| row.key == key)
                .map_or(0, |row| row.choice)
                .clamp(0, pairs.len() as i32 - 1);
            let requests =
                if preview["operation"] == "partition" || preview["operation"] == "extrusion" {
                    &preview["partition"]["partitions"]
                } else {
                    &preview["crease"]["pairs"]
                };
            if changed
                && let Some(requests) = requests.as_array()
                && let Some(request) = requests
                    .iter()
                    .find(|request| request["solid"] == component.solid)
            {
                choice = pairs
                    .iter()
                    .position(|pair| serde_json::json!(pair.edges) == request["edges"])
                    .unwrap_or(0) as i32;
            }
            ComponentCreaseRow {
                key: key.into(),
                label: format!(
                    "Brush {} · {} direction",
                    component.solid,
                    if preview["operation"] == "partition"
                        || preview["operation"] == "extrusion"
                        || window.get_operation() == 6
                    {
                        "cut"
                    } else {
                        "ridge"
                    }
                )
                .into(),
                choices: Rc::new(VecModel::from(
                    pairs
                        .iter()
                        .enumerate()
                        .map(|(index, pair)| {
                            if quad && index < 2 {
                                ridge_direction(pair).into()
                            } else if let Some(corner) = shape.as_ref().and_then(|shape| {
                                shape.vertices.iter().find(|vertex| {
                                    vertex.sides.contains(&component.sides[0])
                                        && pair.edges.iter().all(|edge| vertex.sides.contains(edge))
                                })
                            }) {
                                format!("Trim corner at {}", position(corner.position)).into()
                            } else {
                                format!(
                                    "Cut from {} to {}",
                                    position(pair.midpoints[0]),
                                    position(pair.midpoints[1])
                                )
                                .into()
                            }
                        })
                        .collect::<Vec<slint::SharedString>>(),
                ))
                .into(),
                choice,
            }
        })
        .collect::<Vec<_>>();
    window.set_crease_rows(Rc::new(VecModel::from(rows)).into());
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
    let report = session.component_report(&[]).unwrap();
    refresh_crease_choices(window, &report, session);
    let partition = &report["preview"]["partition"];
    window.set_has_partition_preview(!partition.is_null());
    window.set_can_raise_partition(partition["ridge_supported"].as_bool().unwrap_or(true));
    window.set_has_extrusion_preview(!report["preview"]["extrusion"].is_null());
    window.set_partition_flat(partition["rise"].as_f64() == Some(0.0));
    window.set_cut_is_midpoint(
        partition["edge_parameters"][0].as_f64() == Some(0.5)
            && partition["edge_parameters"][1].as_f64() == Some(0.5),
    );
    let saved = session.saved_partition_report();
    let saved_rows = saved["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            let solid = entry["solid"].as_u64().unwrap();
            let side = entry["side"].as_u64().unwrap();
            ComponentRow {
                key: format!("{solid}:{side}").into(),
                label: format!("Brush {solid} · face {side}").into(),
                detail: entry["error"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        let parameters = &entry["edge_parameters"];
                        if parameters == &serde_json::json!([0.5, 0.5]) {
                            "Two flat pieces · resume to inspect, raise or extrude".into()
                        } else {
                            format!(
                                "Two flat pieces · cut {}% / {}% · resume to inspect or extrude",
                                number(parameters[0].as_f64().unwrap() * 100.0),
                                number(parameters[1].as_f64().unwrap() * 100.0)
                            )
                        }
                    })
                    .into(),
                selected: entry["valid"].as_bool().unwrap(),
            }
        })
        .collect::<Vec<_>>();
    window.set_saved_partition_index(
        window
            .get_saved_partition_index()
            .max(0)
            .min(saved_rows.len().saturating_sub(1) as i32),
    );
    window.set_saved_partition_labels(
        Rc::new(VecModel::from(
            saved_rows
                .iter()
                .map(|row| row.label.clone())
                .collect::<Vec<_>>(),
        ))
        .into(),
    );
    window.set_saved_partitions(Rc::new(VecModel::from(saved_rows)).into());
    window.set_saved_partition_error(saved["error"].as_str().unwrap_or("").into());
    let mut parts = Vec::new();
    if let Some(partitions) = partition["partitions"].as_array() {
        for partition in partitions {
            let solid = partition["solid"].as_u64().unwrap();
            for piece in partition["loops"].as_array().unwrap() {
                let id = piece["id"].as_u64().unwrap();
                parts.push(ComponentRow {
                    key: format!("{solid}:{id}").into(),
                    label: format!(
                        "Brush {solid} · {} {}",
                        piece["shape"].as_str().unwrap(),
                        id + 1
                    )
                    .into(),
                    detail: format!(
                        "{}{}",
                        piece["effective_material"]
                            .as_str()
                            .unwrap_or("Unavailable"),
                        if piece["inherited"] == true {
                            " (inherited)"
                        } else {
                            " (planned)"
                        }
                    )
                    .into(),
                    selected: piece["selected"].as_bool().unwrap(),
                });
            }
        }
    }
    let highlighted = partition["partitions"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|part| {
            part["loops"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|piece| piece["selected"] == true)
        })
        .collect::<Vec<_>>();
    let scope = format!(
        "{}:{}",
        session.component_stamp(),
        serde_json::json!(partition["selection"])
    );
    window.set_has_material_targets(!highlighted.is_empty());
    if window.get_top_material_scope() != scope {
        let names = highlighted
            .iter()
            .filter_map(|piece| piece["effective_material"].as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let material = if names.len() == 1 {
            names.first().copied().unwrap_or("")
        } else {
            ""
        };
        window.set_top_material(material.into());
        window.set_top_material_baseline(material.into());
        let values: [String; 5] = std::array::from_fn(|index| {
            let read = |piece: &&serde_json::Value| {
                if index < 2 {
                    piece["effective_texture"]["shift"][index].as_f64()
                } else if index < 4 {
                    piece["effective_texture"]["scale"][index - 2].as_f64()
                } else {
                    piece["effective_texture"]["rotation"].as_f64()
                }
            };
            highlighted
                .first()
                .and_then(read)
                .filter(|first| highlighted.iter().all(|piece| read(piece) == Some(*first)))
                .map(|value| value.to_string())
                .unwrap_or_default()
        });
        window.set_texture_shift_u(values[0].as_str().into());
        window.set_texture_shift_v(values[1].as_str().into());
        window.set_texture_scale_u(values[2].as_str().into());
        window.set_texture_scale_v(values[3].as_str().into());
        window.set_texture_rotation(values[4].as_str().into());
        window.set_texture_rotation_value(
            values[4].parse::<f32>().unwrap_or(0.0).clamp(-180.0, 180.0),
        );
        window.set_top_texture_baseline(texture_input_key(window).into());
        window.set_top_texture_summary(if highlighted.is_empty() {
            "Highlight pieces to change their texture placement".into()
        } else {
            format!(
                "{} highlighted · {}",
                highlighted.len(),
                if values.iter().any(String::is_empty) {
                    "mixed placement"
                } else if highlighted
                    .iter()
                    .all(|piece| piece["texture_inherited"] == true)
                {
                    "inherited from source"
                } else {
                    "planned placement"
                }
            )
            .into()
        });
        window.set_top_material_scope(scope.into());
        window.set_top_material_preview(
            session
                .material_preview(material)
                .ok()
                .map(|preview| crate::ui::preview_image(&preview))
                .unwrap_or_default(),
        );
        window.set_top_material_summary(if highlighted.is_empty() {
            "Highlight pieces to choose their top material".into()
        } else {
            format!(
                "{} highlighted · {}",
                highlighted.len(),
                if names.len() > 1 {
                    "mixed materials"
                } else if highlighted.iter().all(|piece| piece["inherited"] == true) {
                    "inherited from source"
                } else {
                    "planned material"
                }
            )
            .into()
        });
    }
    window.set_partition_rows(Rc::new(VecModel::from(parts)).into());
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
    if let Some(partitions) = report["preview"]["partition"]["partitions"].as_array() {
        rows.clear();
        for partition in partitions {
            let solid = partition["solid"].as_u64().unwrap();
            let source_corners = partition["source_corner_count"].as_u64().unwrap();
            for vertex in partition["vertices"].as_array().unwrap() {
                let id = vertex["id"].as_u64().unwrap();
                let values = &vertex["preview_position"];
                let point = DVec3::new(
                    values[0].as_f64().unwrap(),
                    values[1].as_f64().unwrap(),
                    values[2].as_f64().unwrap(),
                );
                let label = if vertex["source"]["kind"] == "corner" {
                    format!("Source corner {}", id + 1)
                } else {
                    format!(
                        "Edge {} {}",
                        if vertex["source"]["parameter"].as_f64() == Some(0.5) {
                            "midpoint"
                        } else {
                            "point"
                        },
                        id - source_corners + 1
                    )
                };
                rows.push(ComponentRow {
                    key: format!("{solid}:{id}").into(),
                    label: format!("Brush {solid} · {label}").into(),
                    detail: format!("X, Y, Z: {}", position(point)).into(),
                    selected: false,
                });
            }
        }
        window.set_summary(
            format!(
                "{} staged pieces · {} source corners and edge points · coordinates in X, Y, Z",
                partitions.len() * 2,
                rows.len()
            )
            .into(),
        );
        window.set_picking_summary(
            "Click staged pieces · Ctrl toggles · highlight to inspect and frame".into(),
        );
    }
    window.set_rows(Rc::new(VecModel::from(rows)).into());
    let preview = &report["preview"];
    let token = preview["token"].as_str().unwrap_or_default();
    let weld = preview["operation"] == "weld";
    let split = preview["operation"] == "split";
    let crease = preview["operation"] == "crease";
    let extrusion = preview["operation"] == "extrusion";
    let partition = preview["operation"] == "partition" || extrusion;
    let transform = preview["operation"] == "scale" || preview["operation"] == "rotate";
    let operation = if partition {
        6
    } else if crease {
        5
    } else if transform {
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
        if partition {
            window.set_partition_mode(i32::from(extrusion));
            let parameters = &preview["partition"]["edge_parameters"];
            window.set_cut_first(
                parameters[0]
                    .as_f64()
                    .map(percentage)
                    .unwrap_or_default()
                    .into(),
            );
            window.set_cut_second(
                parameters[1]
                    .as_f64()
                    .map(percentage)
                    .unwrap_or_default()
                    .into(),
            );
            window.set_cut_first_value((parameters[0].as_f64().unwrap_or(0.5) * 100.0) as f32);
            window.set_cut_second_value((parameters[1].as_f64().unwrap_or(0.5) * 100.0) as f32);
            if parameters[0].as_f64() != Some(0.5) || parameters[1].as_f64() != Some(0.5) {
                window.set_show_cut_settings(true);
            }
        }
        if extrusion {
            let spec = &preview["extrusion"];
            window.set_extrusion_height(spec["height"].as_f64().unwrap().to_string().into());
            window.set_extrusion_height_value(spec["height"].as_f64().unwrap() as f32);
            window.set_extrusion_texture_lock(spec["texture_lock"].as_bool().unwrap());
            window.set_wall_override(!spec["wall_material"].is_null());
            if let Some(material) = spec["wall_material"].as_str() {
                if window.get_wall_material() != material {
                    window.set_wall_preview(slint::Image::default());
                }
                window.set_wall_material(material.into());
            }
        }
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
        if split || crease || partition {
            let rise = preview[if partition {
                "partition"
            } else if crease {
                "crease"
            } else {
                "split"
            }]["rise"]
                .as_f64()
                .unwrap();
            window.set_split_rise(rise.to_string().into());
            window.set_split_rise_value(rise as f32);
            if partition || crease {
                window.set_roof_texture_lock(
                    preview[if partition { "partition" } else { "crease" }]["roof_texture_lock"]
                        .as_bool()
                        .unwrap_or(false),
                );
            }
        }
        window.set_texture_lock(preview["texture_lock"].as_bool().unwrap_or(false));
        window.set_move_x(preview["delta"][0].as_f64().unwrap().to_string().into());
        window.set_move_y(preview["delta"][1].as_f64().unwrap().to_string().into());
        window.set_move_z(preview["delta"][2].as_f64().unwrap().to_string().into());
    }
    window.set_token(token.into());
    window.set_ready(
        !preview.is_null()
            && (preview["can_apply"].as_bool().unwrap_or(true) || (partition && !extrusion && window.get_partition_flat()))
            && session
                .component_drag()
                .is_none_or(|drag| drag.error.is_none())
            && window.get_operation() == operation
            && (!partition || window.get_top_material()==window.get_top_material_baseline())
            && (!partition || texture_inputs_match(window))
            && (!partition || cut_inputs_match(window,&preview["partition"]["edge_parameters"]))
            && (if transform {
                window.get_texture_lock() == preview["texture_lock"].as_bool().unwrap_or(false)
                    && transform_inputs(window).is_ok_and(|(spec, pivot)| {
                        let resolved = pivot.or(center).unwrap();
                        crate::session::components::transform_json(&spec, pivot, resolved)
                            == preview["transform"]
                    })
            } else if extrusion {
                window.get_partition_mode() == 1
                    && extrusion_inputs(window).is_ok_and(|spec|spec.height == preview["extrusion"]["height"].as_f64().unwrap()
                        && spec.wall_material.as_ref().map(|value|serde_json::json!(value)).unwrap_or(serde_json::Value::Null)==preview["extrusion"]["wall_material"]
                        && spec.texture_lock==preview["extrusion"]["texture_lock"].as_bool().unwrap())
            } else if partition {
                window.get_partition_mode() == 0 && window.get_split_rise().parse::<f64>().ok() == preview["partition"]["rise"].as_f64()
                    && window.get_roof_texture_lock() == preview["partition"]["roof_texture_lock"].as_bool().unwrap_or(false)
            } else if crease {
                window.get_split_rise().parse::<f64>().ok() == preview["crease"]["rise"].as_f64()
                    && window.get_roof_texture_lock() == preview["crease"]["roof_texture_lock"].as_bool().unwrap_or(false)
                    && crease_inputs(window, session).is_ok_and(|inputs| serde_json::json!(inputs.iter().map(|pair|serde_json::json!({"solid":pair.solid,"edges":pair.edges})).collect::<Vec<_>>()) == preview["crease"]["pairs"])
            } else if split {
                window.get_split_rise().parse::<f64>().ok() == preview["split"]["rise"].as_f64()
                    && window.get_texture_lock() == preview["texture_lock"].as_bool().unwrap_or(false)
            } else if weld {
                window.get_texture_lock() == preview["texture_lock"].as_bool().unwrap_or(false)
                    && weld_target(window, session).is_ok_and(|target| {
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
    } else if extrusion {
        let spec = &preview["extrusion"];
        format!("Adds {} volumes, {} units high. Original brushes stay unchanged.\nApply selects the new faces. Undo removes the volumes.",spec["pieces"].as_array().unwrap().len(),number(spec["height"].as_f64().unwrap())).into()
    } else if partition {
        let count = preview["partition"]["partitions"].as_array().unwrap().len();
        let rise = preview["partition"]["rise"].as_f64().unwrap();
        if rise==0.0 && !window.get_cut_is_midpoint() {
            format!("{} adjusted flat pieces staged. The source face is unchanged.\nKeep, extrude highlighted pieces, or Cancel.",count*2).into()
        } else if rise == 0.0 && preview["partition"]["ridge_supported"] != true {
            format!("{} flat pieces staged. The source face is unchanged.\nKeep, extrude highlighted pieces, or Cancel.",count*2).into()
        } else if rise == 0.0 {
            format!("{} flat pieces staged. The source face is unchanged.\nKeep the pieces for later, raise their shared ridge, or Cancel.",count*2).into()
        } else {
            format!("{} pieces · shared ridge raised {} units.\nThe map is unchanged until Apply.",count*2,number(rise)).into()
        }
    } else if crease {
        let count = preview["crease"]["changes"].as_array().unwrap().len();
        let rise = number(preview["crease"]["rise"].as_f64().unwrap());
        format!("Raise {count} {} by {rise} units. Add {} corners and {count} {}.\nThe map is unchanged until Apply.", if count == 1 {"ridge"} else {"ridges"}, count * 2, if count == 1 {"face"} else {"faces"}).into()
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

fn cut_inputs(window: &ComponentsWindow) -> Result<[f64; 2], String> {
    let values = [window.get_cut_first(), window.get_cut_second()];
    let mut parameters = [0.0; 2];
    for (index, value) in values.iter().enumerate() {
        parameters[index] = value
            .trim()
            .parse::<f64>()
            .map_err(|_| "Enter both cut positions as percentages".to_string())?
            / 100.0;
    }
    Ok(parameters)
}

fn cut_inputs_match(window: &ComponentsWindow, parameters: &serde_json::Value) -> bool {
    [window.get_cut_first(), window.get_cut_second()]
        .iter()
        .enumerate()
        .all(|(index, input)| {
            parameters[index].as_f64().map_or_else(
                || input.is_empty(),
                |value| {
                    input
                        .parse::<f64>()
                        .is_ok_and(|input| (input / 100.0 - value).abs() <= 1e-12)
                },
            )
        })
}

fn extrusion_inputs(
    window: &ComponentsWindow,
) -> Result<map_core::subobject::ExtrusionSpec, String> {
    Ok(map_core::subobject::ExtrusionSpec {
        height: window
            .get_extrusion_height()
            .trim()
            .parse()
            .map_err(|_| "Enter a numeric extrusion height".to_string())?,
        wall_material: window
            .get_wall_override()
            .then(|| window.get_wall_material().to_string()),
        texture_lock: window.get_extrusion_texture_lock(),
    })
}

fn texture_input_key(window: &ComponentsWindow) -> String {
    serde_json::json!([
        window.get_texture_shift_u().as_str(),
        window.get_texture_shift_v().as_str(),
        window.get_texture_scale_u().as_str(),
        window.get_texture_scale_v().as_str(),
        window.get_texture_rotation().as_str()
    ])
    .to_string()
}

fn texture_inputs_match(window: &ComponentsWindow) -> bool {
    window.get_top_texture_baseline() == texture_input_key(window)
}

fn texture_inputs(
    window: &ComponentsWindow,
) -> Result<map_core::subobject::PartitionTexture, String> {
    let values = [
        window.get_texture_shift_u(),
        window.get_texture_shift_v(),
        window.get_texture_scale_u(),
        window.get_texture_scale_v(),
        window.get_texture_rotation(),
    ];
    let values = values
        .iter()
        .map(|value| {
            value
                .trim()
                .parse::<f64>()
                .map_err(|_| "Enter both shifts, both scales and a rotation".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(map_core::subobject::PartitionTexture {
        shift: [values[0], values[1]],
        scale: [values[2], values[3]],
        rotation: values[4],
    })
}

pub fn choose_partition_texture(
    window: &ComponentsWindow,
    session: &mut Session,
    target: &PartitionMaterialTarget,
    texture: Option<map_core::subobject::PartitionTexture>,
) -> Result<(), String> {
    if target != &partition_material_target(session)
        || window.get_token().parse::<u64>().ok() != target.0
    {
        return Err(
            "Map, preview or highlighted pieces changed; choose texture placement again".into(),
        );
    }
    if window.get_top_material() != window.get_top_material_baseline() {
        return Err("Preview the planned top material before changing placement".into());
    }
    session.update_component_partition_texture(
        target.0.ok_or("Stage a partition first")?,
        &target.2,
        texture,
    )?;
    refresh(window, session);
    window
        .set_message("Top texture placement updated. Keep the pieces or preview geometry.".into());
    Ok(())
}

pub type PartitionMaterialTarget = (
    Option<u64>,
    u64,
    Vec<crate::session::components::PartitionRef>,
);

pub fn partition_material_target(session: &Session) -> PartitionMaterialTarget {
    let report = session.component_report(&[]).unwrap();
    let pieces = report["preview"]["partition"]["selection"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|piece| crate::session::components::PartitionRef {
            solid: piece["solid"].as_u64().unwrap() as u32,
            partition: piece["partition"].as_u64().unwrap() as u32,
        })
        .collect();
    (
        session.partition_preview_token(),
        session.document_generation(),
        pieces,
    )
}

pub fn choose_partition_material(
    window: &ComponentsWindow,
    session: &mut Session,
    target: &PartitionMaterialTarget,
    material: Option<&str>,
) -> Result<(), String> {
    if target != &partition_material_target(session)
        || window.get_token().parse::<u64>().ok() != target.0
    {
        return Err(
            "Map, preview or highlighted pieces changed; choose the top material again".into(),
        );
    }
    if !texture_inputs_match(window) {
        return Err("Preview the texture placement before changing material".into());
    }
    session.update_component_partition_material(
        target.0.ok_or("Stage a partition first")?,
        &target.2,
        material,
    )?;
    refresh(window, session);
    window.set_message("Top material plan updated. Keep the pieces or preview geometry.".into());
    Ok(())
}

pub fn choose_extrusion_material(
    window: &ComponentsWindow,
    session: &mut Session,
    target: (Option<u64>, u64),
    material: &str,
) -> Result<(), String> {
    if target
        != (
            session.partition_preview_token(),
            session.document_generation(),
        )
    {
        return Err("Map or partition preview changed; choose the wall material again".into());
    }
    window.set_wall_material(material.into());
    window.set_wall_override(true);
    window.set_wall_preview(
        session
            .material_preview(material)
            .ok()
            .map(|preview| crate::ui::preview_image(&preview))
            .unwrap_or_default(),
    );
    window.set_ready(false);
    window.set_message("Wall material changed. Preview the extrusion before Apply.".into());
    Ok(())
}

fn saved_partition_action(window: &ComponentsWindow, shared: &SharedSession, remove: bool) {
    let result = window
        .get_saved_partitions()
        .row_data(window.get_saved_partition_index() as usize)
        .ok_or("Choose saved pieces first".to_string())
        .and_then(|row| {
            let component = component(row.key.as_str())?;
            let sources = [(component.solid, component.sides[0])];
            let mut session = lock(shared);
            if remove {
                session
                    .remove_saved_component_partitions(
                        Actor::User,
                        &sources,
                        window.get_stamp().as_str(),
                    )
                    .map(|_| ())
            } else {
                session
                    .resume_component_partitions(&sources, window.get_stamp().as_str())
                    .map(|_| ())
            }
        });
    refresh(window, &lock(shared));
    window.set_message(result.err().unwrap_or_default().into());
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
    window.on_choose_texture_rotation({
        let (window, shared) = (window.as_weak(), shared.clone());
        move |value| {
            let window = window.unwrap();
            window.set_texture_rotation(number(f64::from(value)).into());
            window.set_texture_rotation_value(value);
            refresh(&window, &lock(&shared));
        }
    });
    window.on_preview_partition_texture({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let target = partition_material_target(&session);
            let result = texture_inputs(&window).and_then(|texture| {
                choose_partition_texture(&window, &mut session, &target, Some(texture))
            });
            if let Err(error) = result {
                window.set_message(error.into());
            }
        }
    });
    window.on_inherit_partition_texture({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let target = partition_material_target(&session);
            if let Err(error) = choose_partition_texture(&window, &mut session, &target, None) {
                window.set_message(error.into());
            }
        }
    });
    window.on_preview_partition_material({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let target = partition_material_target(&session);
            let material = window.get_top_material();
            let result =
                choose_partition_material(&window, &mut session, &target, Some(material.as_str()));
            if let Err(error) = result {
                window.set_message(error.into());
            }
        }
    });
    window.on_inherit_partition_material({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let target = partition_material_target(&session);
            let result = choose_partition_material(&window, &mut session, &target, None);
            if let Err(error) = result {
                window.set_message(error.into());
            }
        }
    });
    window.on_resume_saved_partition({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || saved_partition_action(&window.unwrap(), &shared, false)
    });
    window.on_remove_saved_partition({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || saved_partition_action(&window.unwrap(), &shared, true)
    });
    window.on_choose_partition({
        let (window, shared) = (window.as_weak(), shared.clone());
        move |key| {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let result = component(key.as_str()).and_then(|component| {
                session.select_component_partitions(
                    window
                        .get_token()
                        .parse()
                        .map_err(|_| "Stage a partition first".to_string())?,
                    &[crate::session::components::PartitionRef {
                        solid: component.solid,
                        partition: component.sides[0],
                    }],
                    SelectMode::Toggle,
                )
            });
            refresh(&window, &session);
            window.set_message(result.err().unwrap_or_default().into());
        }
    });
    window.on_frame_partitions({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let result = window
                .get_token()
                .parse()
                .map_err(|_| "Stage a partition first".to_string())
                .and_then(|token| {
                    session.frame_component_partitions(Actor::User, token, window.get_aspect())
                });
            refresh(&window, &session);
            window.set_message(result.err().unwrap_or_default().into());
        }
    });
    window.on_flatten_partitions({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let result = window
                .get_token()
                .parse()
                .map_err(|_| "Stage a partition first".to_string())
                .and_then(|token| session.raise_component_partition(token, 0.0));
            refresh(&window, &session);
            window.set_message(result.err().unwrap_or_default().into());
        }
    });
    window.on_choose_crease_pair({
        let window = window.as_weak();
        move |key, choice| {
            let window = window.unwrap();
            let rows = window
                .get_crease_rows()
                .iter()
                .map(|mut row| {
                    if row.key == key {
                        row.choice = choice;
                    }
                    row
                })
                .collect::<Vec<_>>();
            window.set_crease_rows(Rc::new(VecModel::from(rows)).into());
            window.invoke_edited();
        }
    });
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
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            if let Ok(rise) = window.get_split_rise().parse::<f64>()
                && ((window.get_operation() == 6 && rise == 0.0)
                    || (map_core::subobject::MIN_SPLIT_RISE..=map_core::subobject::MAX_SPLIT_RISE)
                        .contains(&rise))
            {
                window.set_split_rise_value(rise as f32);
            }
            if let Ok(height) = window.get_extrusion_height().parse::<f64>()
                && (map_core::subobject::MIN_SPLIT_RISE..=map_core::subobject::MAX_SPLIT_RISE)
                    .contains(&height)
            {
                window.set_extrusion_height_value(height as f32);
            }
            if window.get_operation() == 6
                && window.get_partition_mode() == 1
                && window.get_wall_override()
            {
                window.set_wall_preview(
                    lock(&shared)
                        .material_preview(window.get_wall_material().as_str())
                        .ok()
                        .map(|preview| crate::ui::preview_image(&preview))
                        .unwrap_or_default(),
                );
            }
            for (index, value) in [window.get_cut_first(), window.get_cut_second()]
                .iter()
                .enumerate()
            {
                if let Ok(value) = value.parse::<f32>()
                    && value > 0.0
                    && value < 100.0
                {
                    if index == 0 {
                        window.set_cut_first_value(value);
                    } else {
                        window.set_cut_second_value(value);
                    }
                }
            }
            window.set_ready(false);
            window.set_message("Settings changed. Preview the change before applying.".into());
        }
    });
    window.on_choose_cut_position({
        let window = window.as_weak();
        move |index, value| {
            let window = window.unwrap();
            if index == 0 {
                window.set_cut_first(value.to_string().into());
                window.set_cut_first_value(value);
            } else {
                window.set_cut_second(value.to_string().into());
                window.set_cut_second_value(value);
            }
            window.invoke_edited();
        }
    });
    window.on_preview_cut({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let result = cut_inputs(&window).and_then(|parameters| {
                session.update_component_partition_cut(
                    window
                        .get_token()
                        .parse()
                        .map_err(|_| "Stage flat pieces first".to_string())?,
                    parameters,
                )
            });
            refresh(&window, &session);
            if result.is_err() {
                window.set_ready(false);
            }
            window.set_message(result.err().unwrap_or_default().into());
        }
    });
    window.on_choose_extrusion_height({
        let window = window.as_weak();
        move |value| {
            let window = window.unwrap();
            window.set_extrusion_height(value.to_string().into());
            window.invoke_edited();
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
            let result = if window.get_operation() == 6 {
                if window.get_has_partition_preview() && window.get_top_material()!=window.get_top_material_baseline() {
                    Err("Preview the planned top material before raising or extruding".into())
                } else if window.get_has_partition_preview() && !texture_inputs_match(&window) {
                    Err("Preview the texture placement before raising or extruding".into())
                } else if window.get_has_partition_preview() && !cut_inputs_match(&window,&session.component_report(&[]).unwrap()["preview"]["partition"]["edge_parameters"]) {
                    Err("Preview the cut positions before raising or extruding".into())
                } else if window.get_has_partition_preview() && window.get_partition_mode() == 1 {
                    extrusion_inputs(&window).and_then(|spec| {
                        session.preview_partition_extrusion(
                            window
                                .get_token()
                                .parse()
                                .map_err(|_| "Stage flat pieces first".to_string())?,
                            spec,
                        )
                    })
                } else if window.get_has_partition_preview() {
                    (if window.get_can_raise_partition() {
                        window.get_split_rise().trim().parse::<f64>().map_err(|_| "Enter a numeric ridge height".to_string())
                    } else { Ok(0.0) })
                        .and_then(|rise| {
                            session.raise_component_partition_with_texture(
                                window
                                    .get_token()
                                    .parse()
                                    .map_err(|_| "Stage a partition first".to_string())?,
                                rise,
                                Some(window.get_roof_texture_lock()),
                            )
                        })
                } else {
                    crease_inputs(&window, &session).and_then(|pairs| {
                        session.preview_component_partition(&pairs, window.get_stamp().as_str())
                    })
                }
            } else if window.get_operation() == 5 {
                window
                    .get_split_rise()
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| "Enter a numeric ridge height".to_string())
                    .and_then(|rise| {
                        let inputs = crease_inputs(&window, &session)?;
                        session.preview_component_crease_with_texture(
                            &inputs,
                            rise,
                            window.get_roof_texture_lock(),
                            window.get_stamp().as_str(),
                        )
                    })
            } else if window.get_operation() == 3 || window.get_operation() == 4 {
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
                        session.preview_component_split_with_texture(
                            rise,
                            window.get_texture_lock(),
                            window.get_stamp().as_str(),
                        )
                    })
            } else if window.get_operation() == 1 {
                weld_target(&window, &session).and_then(|target| {
                    session.preview_component_weld_with_texture(
                        target,
                        window.get_texture_lock(),
                        window.get_stamp().as_str(),
                    )
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
            let extrusion = window.get_has_extrusion_preview();
            let flat = window.get_has_partition_preview() && window.get_partition_flat() && !extrusion;
            let result = window
                .get_token()
                .parse()
                .map_err(|_| "Preview a change first".to_string())
                .and_then(|token| if flat { session.store_component_partition(Actor::User, token) } else { session.commit_component_edit(Actor::User, token) });
            refresh(&window, &session);
            window.set_message(
                result
                    .map(|count| {
                        if extrusion { format!("Added {count} volumes. Their faces are selected; Undo removes the volumes.") } else if flat { format!("Kept {count} flat partitions. Save the map to retain them after reopening.") } else { format!("Changed {count} brushes. Undo restores the previous shape.") }
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

pub fn refresh_material_previews(window: &ComponentsWindow, session: &Session) {
    window.set_top_material_preview(
        session
            .material_preview(&window.get_top_material())
            .ok()
            .map(|preview| crate::ui::preview_image(&preview))
            .unwrap_or_default(),
    );
    window.set_wall_preview(
        session
            .material_preview(&window.get_wall_material())
            .ok()
            .map(|preview| crate::ui::preview_image(&preview))
            .unwrap_or_default(),
    );
}
