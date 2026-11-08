use std::rc::Rc;

use map_core::clip::{TerrainBlendMode, TerrainMode};
use slint::{ComponentHandle, Model, VecModel};

use crate::runtime::{DocumentAccess, DocumentContinuation, SharedSession, lock};
use crate::session::{Actor, Session};
use crate::{CarveRow, CarveWindow};

fn terrain_options(window: &CarveWindow) -> Result<(TerrainMode, TerrainBlendMode, bool), String> {
    let terrain_mode = match window.get_terrain_mode() {
        0 => TerrainMode::Grid,
        1 => TerrainMode::Flat,
        2 => TerrainMode::Cells,
        3 => TerrainMode::Fragments,
        _ => return Err("Choose a terrain cut mode".into()),
    };
    let blend_mode = match window.get_blend_mode() {
        0 => TerrainBlendMode::Exact,
        1 => TerrainBlendMode::NativeFloat,
        2 => TerrainBlendMode::NearestByte,
        _ => return Err("Choose a blend sampling policy".into()),
    };
    if blend_mode != TerrainBlendMode::Exact && terrain_mode != TerrainMode::Fragments {
        return Err("Native float and Nearest byte require Retile surface triangles".into());
    }
    let partition_neighbors = window.get_partition_neighbors();
    if partition_neighbors && !terrain_mode.allows_neighbor_partition() {
        return Err("Partition selected neighbors is unavailable for Retile flat terrain".into());
    }
    Ok((terrain_mode, blend_mode, partition_neighbors))
}

fn terrain_options_changed(window: &CarveWindow) {
    window.set_ready(false);
    window.set_can_preview(can_preview(window));
    window.set_message("Terrain settings changed. Preview the result before applying.".into());
}

fn cut_material(window: &CarveWindow) -> Result<map_core::carve::CutMaterialPolicy, String> {
    match window.get_cut_material_policy() {
        0 => Ok(map_core::carve::CutMaterialPolicy::Cutter),
        1 => Ok(map_core::carve::CutMaterialPolicy::Target),
        2 => {
            let material = window.get_cut_material().trim().to_ascii_uppercase();
            if material.is_empty() {
                return Err("Choose a material for new cut faces".into());
            }
            source_formats::keyvalues::validate_string(&material)
                .map_err(|error| error.to_string())?;
            Ok(map_core::carve::CutMaterialPolicy::Material(material))
        }
        _ => Err("Choose the surface settings for new cut faces".into()),
    }
}

fn material_changed(window: &CarveWindow) {
    window.set_ready(false);
    window.set_can_preview(can_preview(window));
    window.set_message("Cut surface settings changed. Preview the result before applying.".into());
}

pub fn material_target(window: &CarveWindow, session: &Session) -> String {
    format!(
        "{}:{:?}:{}:{}:{:?}",
        session.carve_stamp(),
        parameters(window),
        window.get_cut_material_policy(),
        window.get_cut_material(),
        terrain_options(window),
    )
}

pub fn choose_material(
    window: &CarveWindow,
    session: &Session,
    target: &str,
    material: &str,
) -> Result<(), String> {
    if material_target(window, session) != target {
        return Err("Carve fields, map or preview changed. Open the material picker again.".into());
    }
    window.set_cut_material(material.into());
    window.set_cut_material_policy(2);
    material_changed(window);
    Ok(())
}

fn parameters(window: &CarveWindow) -> Result<(Vec<u32>, Vec<u32>), String> {
    let rows = window.get_targets();
    let mut cutters = if window.get_multiple_cutters() {
        rows.iter()
            .filter(|row| row.cutter)
            .map(|row| {
                row.solid
                    .parse::<u32>()
                    .map_err(|_| "Invalid cutter brush".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        vec![
            window
                .get_cutter_ids()
                .row_data(window.get_cutter() as usize)
                .ok_or("Select at least two brushes in the map first")?
                .parse()
                .map_err(|_| "Invalid cutter brush")?,
        ]
    };
    let mut targets = rows
        .iter()
        .filter(|row| row.selected && !row.cutter)
        .map(|row| {
            row.solid
                .parse::<u32>()
                .map_err(|_| "Invalid target brush".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    cutters.sort_unstable();
    targets.sort_unstable();
    if rows.iter().any(|row| {
        row.terrain
            && row
                .solid
                .parse::<u32>()
                .is_ok_and(|id| cutters.contains(&id))
    }) {
        return Err("Terrain brushes can only be targets".into());
    }
    Ok((cutters, targets))
}

fn can_preview(window: &CarveWindow) -> bool {
    cut_material(window).is_ok()
        && terrain_options(window).is_ok()
        && parameters(window).is_ok_and(|(cutters, targets)| {
            !cutters.is_empty()
                && cutters.len() <= map_core::carve::MAX_CUTTERS
                && !targets.is_empty()
                && targets.len() <= map_core::carve::MAX_TARGETS
        })
}

pub fn refresh(window: &CarveWindow, session: &Session) {
    let report = session.carve_state();
    let preview = &report["preview"];
    let token = preview["token"].as_str().unwrap_or_default();
    let source = format!(
        "{}:{}:{:?}",
        session.document_generation(),
        session.document().revision(),
        session.selected_ids()
    );
    let previous = parameters(window).ok();
    let unchanged_source = window.get_source() == source;
    match session.carve_sources() {
        Ok(sources) => {
            let ordinary = sources
                .iter()
                .filter(|source| !source.terrain)
                .collect::<Vec<_>>();
            let default = ordinary.len().saturating_sub(1);
            window.set_has_terrain(sources.iter().any(|source| source.terrain));
            let incoming = !token.is_empty() && window.get_token() != token;
            if !incoming && !unchanged_source {
                window.set_multiple_cutters(false);
                window.set_terrain_mode(0);
                window.set_blend_mode(0);
                window.set_partition_neighbors(false);
            }
            if incoming {
                window.set_multiple_cutters(
                    preview["result"]["cutters"].as_array().unwrap().len() > 1,
                );
                let policy = &preview["result"]["cut_material"];
                window.set_cut_material_policy(match policy["policy"].as_str() {
                    Some("target") => 1,
                    Some("material") => 2,
                    _ => 0,
                });
                if let Some(material) = policy["material"].as_str() {
                    window.set_cut_material(material.into());
                }
                window.set_terrain_mode(match preview["result"]["terrain_mode"].as_str() {
                    Some("flat") => 1,
                    Some("cells") => 2,
                    Some("fragments") => 3,
                    _ => 0,
                });
                window.set_blend_mode(match preview["result"]["blend_mode"].as_str() {
                    Some("native_float") => 1,
                    Some("nearest_byte") => 2,
                    _ => 0,
                });
                window.set_partition_neighbors(
                    preview["result"]["partition_neighbors"]
                        .as_bool()
                        .unwrap_or_default(),
                );
            }
            let chosen = if incoming {
                preview["result"]["cutter"].as_u64().map(|id| id as u32)
            } else if unchanged_source {
                previous
                    .as_ref()
                    .and_then(|(cutters, _)| cutters.first().copied())
            } else {
                None
            };
            let cutter = chosen
                .and_then(|id| ordinary.iter().position(|source| source.solid == id))
                .unwrap_or(default);
            let cutters = if incoming {
                preview["result"]["cutters"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|id| id.as_u64().unwrap() as u32)
                    .collect::<Vec<_>>()
            } else if unchanged_source && window.get_multiple_cutters() {
                previous
                    .as_ref()
                    .map_or_else(Vec::new, |(cutters, _)| cutters.clone())
            } else {
                ordinary
                    .get(cutter)
                    .map(|source| vec![source.solid])
                    .unwrap_or_default()
            };
            let targets = if incoming {
                preview["targets"].as_array().map(|ids| {
                    ids.iter()
                        .map(|id| id.as_u64().unwrap() as u32)
                        .collect::<Vec<_>>()
                })
            } else if unchanged_source {
                previous.map(|(_, targets)| targets)
            } else {
                None
            };
            let rows = sources
                .iter()
                .map(|source| {
                    let size = source.maxs - source.mins;
                    CarveRow {
                        solid: source.solid.to_string().into(),
                        label: format!(
                            "{} {} · {:.1} × {:.1} × {:.1} units · {}{}",
                            if source.terrain {
                                "Terrain brush"
                            } else {
                                "Brush"
                            },
                            source.solid,
                            size.x,
                            size.y,
                            size.z,
                            source.material,
                            if source.terrain_faces > 1 {
                                " · multiple terrain faces unsupported"
                            } else {
                                ""
                            },
                        )
                        .into(),
                        selected: !cutters.contains(&source.solid)
                            && targets
                                .as_ref()
                                .is_none_or(|targets| targets.contains(&source.solid)),
                        cutter: cutters.contains(&source.solid),
                        terrain: source.terrain,
                    }
                })
                .collect::<Vec<_>>();
            window.set_cutters(
                Rc::new(VecModel::from(
                    rows.iter()
                        .filter(|row| !row.terrain)
                        .map(|row| row.label.clone())
                        .collect::<Vec<_>>(),
                ))
                .into(),
            );
            window.set_cutter_ids(
                Rc::new(VecModel::from(
                    rows.iter()
                        .filter(|row| !row.terrain)
                        .map(|row| row.solid.clone())
                        .collect::<Vec<_>>(),
                ))
                .into(),
            );
            window.set_targets(Rc::new(VecModel::from(rows)).into());
            window.set_cutter(cutter as i32);
        }
        Err(error) => {
            window.set_has_terrain(false);
            window.set_targets(Rc::new(VecModel::from(Vec::<CarveRow>::new())).into());
            window.set_cutters(Rc::new(VecModel::from(Vec::<slint::SharedString>::new())).into());
            window
                .set_cutter_ids(Rc::new(VecModel::from(Vec::<slint::SharedString>::new())).into());
            window.set_message(error.into());
        }
    }
    window.set_stamp(session.carve_stamp().into());
    window.set_source(source.into());
    window.set_token(token.into());
    window.set_can_preview(can_preview(window));
    window.set_ready(
        !preview.is_null()
            && !report["handle"].is_null()
            && parameters(window).is_ok_and(|(cutters, targets)| {
                preview["result"]["cutters"] == serde_json::json!(cutters)
                    && preview["targets"] == serde_json::json!(targets)
                    && cut_material(window).is_ok_and(|policy| {
                        preview["result"]["cut_material"]
                            == editor_core::session::carving::cut_material_json(&policy)
                    })
                    && terrain_options(window).is_ok_and(|(terrain, blend, partition)| {
                        let terrain = match terrain {
                            TerrainMode::Grid => "grid",
                            TerrainMode::Flat => "flat",
                            TerrainMode::Cells => "cells",
                            TerrainMode::Fragments => "fragments",
                        };
                        let blend = match blend {
                            TerrainBlendMode::Exact => "exact",
                            TerrainBlendMode::NativeFloat => "native_float",
                            TerrainBlendMode::NearestByte => "nearest_byte",
                        };
                        preview["result"]["terrain_mode"] == terrain
                            && preview["result"]["blend_mode"] == blend
                            && preview["result"]["partition_neighbors"] == partition
                    })
            }),
    );
    window.set_summary(if preview.is_null() {
        "Preview the result before changing the map.".into()
    } else {
        let changes = preview["result"]["changes"].as_array().unwrap();
        let fragments:usize = changes.iter().map(|change| change["fragments"].as_array().unwrap().len()).sum();
        let removed = preview["result"]["removed"].as_array().unwrap().len();
        let untouched = preview["result"]["unchanged"].as_array().unwrap().len();
        let targets = changes.len() + untouched;
        let target_word = if targets == 1 { "brush" } else { "brushes" };
        let fragment_word = if fragments == 1 { "fragment" } else { "fragments" };
        let terrain = &preview["result"]["terrain_work"];
        let patches = terrain["patches"].as_u64().unwrap_or_default();
        let terrain_summary = if terrain["sources"].as_u64().unwrap_or_default() == 0 {String::new()} else {
            format!("\n{patches} terrain patches · {} samples · {} triangles",terrain["vertices"],terrain["triangles"])
        };
        format!("{targets} target {target_word} → {fragments} {fragment_word}\n{removed} fully removed · {untouched} unchanged{terrain_summary}\nThe map is unchanged until Apply.").into()
    });
    if let Some(error) = preview["guard_error"].as_str() {
        window.set_message(error.into());
    }
}

pub fn refresh_changed(window: &CarveWindow, session: &Session) {
    if window.get_stamp() != session.carve_stamp() {
        refresh(window, session);
    }
}

fn choose_cutter(window: &CarveWindow) {
    let chosen = window
        .get_cutter_ids()
        .row_data(window.get_cutter() as usize);
    let rows = window
        .get_targets()
        .iter()
        .map(|mut row| {
            if row.cutter {
                row.selected = true;
            }
            row.cutter = chosen.as_ref() == Some(&row.solid);
            if row.cutter {
                row.selected = false;
            }
            row
        })
        .collect::<Vec<_>>();
    window.set_targets(Rc::new(VecModel::from(rows)).into());
    window.set_ready(false);
    window.set_can_preview(can_preview(window));
    window.set_message("Cutter changed. Preview the result before applying.".into());
}

fn with_window<R>(
    window: &slint::Weak<CarveWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(&CarveWindow, &SharedSession) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    let result = target
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

pub fn bind_with_access(window: &CarveWindow, access: &DocumentAccess) {
    window.on_terrain_mode_changed({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            with_window(&window, &target, |window, _| {
                window.set_blend_mode(0);
                window.set_partition_neighbors(false);
                terrain_options_changed(window);
            });
        }
    });
    window.on_terrain_options_changed({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            with_window(&window, &target, |window, _| {
                terrain_options_changed(window)
            });
        }
    });
    window.on_cut_material_changed({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            with_window(&window, &target, |window, _| material_changed(window));
        }
    });
    window.on_choose_cutter({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            with_window(&window, &target, |window, _| choose_cutter(window));
        }
    });
    window.on_toggle_multiple({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            with_window(&window, &target, |window, _| {
                if !window.get_multiple_cutters() {
                    choose_cutter(window);
                }
                window.set_ready(false);
                window.set_can_preview(can_preview(window));
                window
                    .set_message("Cutter mode changed. Preview the result before applying.".into());
            });
        }
    });
    window.on_choose_cut_role({
        let window = window.as_weak();
        let target = access.downgrade();
        move |id| {
            with_window(&window, &target, |window, _| {
                let rows = window
                    .get_targets()
                    .iter()
                    .map(|mut row| {
                        if row.solid == id && !row.terrain {
                            row.cutter = !row.cutter;
                            row.selected = !row.cutter;
                        }
                        row
                    })
                    .collect::<Vec<_>>();
                window.set_targets(Rc::new(VecModel::from(rows)).into());
                window.set_ready(false);
                window.set_can_preview(can_preview(window));
                window
                    .set_message("Brush roles changed. Preview the result before applying.".into());
            });
        }
    });
    window.on_choose_target({
        let window = window.as_weak();
        let target = access.downgrade();
        move |id| {
            with_window(&window, &target, |window, _| {
                let rows = window
                    .get_targets()
                    .iter()
                    .map(|mut row| {
                        if row.solid == id && !row.cutter {
                            row.selected = !row.selected;
                        }
                        row
                    })
                    .collect::<Vec<_>>();
                window.set_targets(Rc::new(VecModel::from(rows)).into());
                window.set_ready(false);
                window.set_can_preview(can_preview(window));
                window.set_message("Targets changed. Preview the result before applying.".into());
            });
        }
    });
    window.on_preview_edit({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            with_window(&window, &target, |window, shared| {
                let mut session = lock(shared);
                let result = parameters(window).and_then(|(cutters, targets)| {
                    let policy = cut_material(window)?;
                    let (terrain_mode, blend_mode, partition_neighbors) = terrain_options(window)?;
                    session.preview_carve_request(editor_core::session::carving::CarveRequest {
                        cutters: &cutters,
                        targets: &targets,
                        cut_material: &policy,
                        terrain_mode,
                        blend_mode,
                        partition_neighbors,
                        stamp: window.get_stamp().as_str(),
                    })
                });
                refresh(window, &session);
                if let Err(error) = result {
                    window.set_ready(false);
                    window.set_message(error.into());
                } else {
                    window.set_message("".into());
                }
            });
        }
    });
    window.on_discard({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            with_window(&window, &target, |window, shared| {
                let mut session = lock(shared);
                let result = window
                    .get_token()
                    .parse::<u64>()
                    .map_err(|_| "Preview a carve first".to_string())
                    .and_then(|token| session.cancel_carve(token));
                refresh(window, &session);
                window.set_message(result.err().unwrap_or_default().into());
            });
        }
    });
    window.on_apply({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            with_window(&window, &target, |window, shared| {
                if !window.get_ready() {
                    return;
                }
                let mut session = lock(shared);
                let result = window
                    .get_token()
                    .parse::<u64>()
                    .map_err(|_| "Preview a carve first".to_string())
                    .and_then(|token| session.commit_carve(Actor::User, token));
                refresh(window, &session);
                window.set_message(
                    result
                        .map(|_| "Carve applied. Undo restores the original brushes.".to_string())
                        .unwrap_or_else(|error| error)
                        .into(),
                );
            });
        }
    });
    window.on_cancel({
        let window = window.as_weak();
        let target = access.downgrade();
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            with_window(&window.as_weak(), &target, |window, shared| {
                if let Ok(token) = window.get_token().parse::<u64>() {
                    let _ = lock(shared).cancel_carve(token);
                }
            });
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
