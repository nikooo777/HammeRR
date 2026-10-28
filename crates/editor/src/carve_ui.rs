use std::rc::Rc;

use slint::{ComponentHandle, Model, VecModel};

use crate::runtime::{DocumentAccess, DocumentContinuation, SharedSession, lock};
use crate::session::{Actor, Session};
use crate::{CarveRow, CarveWindow};

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
        "{}:{:?}:{}:{}",
        session.carve_stamp(),
        parameters(window),
        window.get_cut_material_policy(),
        window.get_cut_material(),
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
            rows.row_data(window.get_cutter() as usize)
                .ok_or("Select at least two brushes in the map first")?
                .solid
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
    Ok((cutters, targets))
}

fn can_preview(window: &CarveWindow) -> bool {
    cut_material(window).is_ok()
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
            let default = sources.len().saturating_sub(1);
            let incoming = !token.is_empty() && window.get_token() != token;
            if !incoming && !unchanged_source {
                window.set_multiple_cutters(false);
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
                .and_then(|id| sources.iter().position(|source| source.solid == id))
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
                sources
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
                            "Brush {} · {:.1} × {:.1} × {:.1} units · {}",
                            source.solid, size.x, size.y, size.z, source.material
                        )
                        .into(),
                        selected: !cutters.contains(&source.solid)
                            && targets
                                .as_ref()
                                .is_none_or(|targets| targets.contains(&source.solid)),
                        cutter: cutters.contains(&source.solid),
                    }
                })
                .collect::<Vec<_>>();
            window.set_cutters(
                Rc::new(VecModel::from(
                    rows.iter().map(|row| row.label.clone()).collect::<Vec<_>>(),
                ))
                .into(),
            );
            window.set_targets(Rc::new(VecModel::from(rows)).into());
            window.set_cutter(cutter as i32);
        }
        Err(error) => {
            window.set_targets(Rc::new(VecModel::from(Vec::<CarveRow>::new())).into());
            window.set_cutters(Rc::new(VecModel::from(Vec::<slint::SharedString>::new())).into());
            window.set_message(error.into());
        }
    }
    window.set_stamp(session.carve_stamp().into());
    window.set_source(source.into());
    window.set_token(token.into());
    window.set_can_preview(can_preview(window));
    window.set_ready(
        !preview.is_null()
            && parameters(window).is_ok_and(|(cutters, targets)| {
                preview["result"]["cutters"] == serde_json::json!(cutters)
                    && preview["targets"] == serde_json::json!(targets)
                    && cut_material(window).is_ok_and(|policy| {
                        preview["result"]["cut_material"]
                            == editor_core::session::carving::cut_material_json(&policy)
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
        format!("{targets} target {target_word} → {fragments} {fragment_word}\n{removed} fully removed · {untouched} unchanged\nThe map is unchanged until Apply.").into()
    });
}

pub fn refresh_changed(window: &CarveWindow, session: &Session) {
    if window.get_stamp() != session.carve_stamp() {
        refresh(window, session);
    }
}

fn choose_cutter(window: &CarveWindow) {
    let rows = window
        .get_targets()
        .iter()
        .enumerate()
        .map(|(index, mut row)| {
            if row.cutter {
                row.selected = true;
            }
            row.cutter = index == window.get_cutter() as usize;
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
    let result = target.upgrade().and_then(|access| {
        access.with_shared(|shared| {
            #[cfg(test)]
            {
                let hook = BEFORE_ACTION.with(|slot| slot.borrow_mut().take());
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
            window.set_message(error.into());
            None
        }
    }
}

#[cfg(test)]
pub fn bind(window: &CarveWindow, shared: &SharedSession) {
    bind_with_access(window, &DocumentAccess::unmanaged(shared.clone()));
}

pub fn bind_with_access(window: &CarveWindow, access: &DocumentAccess) {
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
                        if row.solid == id {
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
                    session.preview_carve_many_with_material(
                        &cutters,
                        &targets,
                        &policy,
                        window.get_stamp().as_str(),
                    )
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

#[cfg(test)]
std::thread_local! {
    static BEFORE_ACTION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod admission_tests;

#[cfg(test)]
mod material_tests {
    use super::*;
    use crate::session::SelectMode;
    use glam::DVec3;
    use std::sync::{Arc, Mutex};

    #[test]
    fn cut_material_controls_share_preview_commit_and_preserve_draft_on_invalid_fields() {
        i_slint_backend_testing::init_no_event_loop();
        let mut session = Session::new(Err("no assets needed".into()));
        let mut ids = Vec::new();
        for (mins, maxs, material) in [
            (DVec3::ZERO, DVec3::splat(64.0), "brick/wall"),
            (DVec3::splat(16.0), DVec3::splat(48.0), "tools/toolsnodraw"),
        ] {
            ids.push(
                session
                    .create_block(Actor::Agent, mins, maxs, material, false)
                    .unwrap(),
            );
        }
        session.select(&ids, SelectMode::Replace).unwrap();
        let original = session.document().vmf().clone();
        let shared = Arc::new(Mutex::new(session));
        let window = CarveWindow::new().unwrap();
        bind(&window, &shared);
        refresh(&window, &lock(&shared));
        window.set_cut_material_policy(2);
        window.set_cut_material("brick/jamb".into());
        window.invoke_cut_material_changed();
        window.invoke_preview_edit();
        assert!(window.get_ready(), "{}", window.get_message());
        assert_eq!(
            lock(&shared).carve_state()["preview"]["result"]["cut_material"],
            serde_json::json!({"policy":"material","material":"BRICK/JAMB"})
        );
        let draft = lock(&shared).render_document().vmf().clone();
        let retained = lock(&shared).carve_state();
        window.set_cut_material(" ".into());
        window.invoke_cut_material_changed();
        assert!(!window.get_ready());
        assert!(!window.get_can_preview());
        window.invoke_preview_edit();
        assert_eq!(lock(&shared).carve_state(), retained);
        assert_eq!(lock(&shared).render_document().vmf(), &draft);
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        let picker = material_target(&window, &lock(&shared));
        window.set_cut_material("brick/changed".into());
        assert!(choose_material(&window, &lock(&shared), &picker, "brick/new").is_err());
        let picker = material_target(&window, &lock(&shared));
        choose_material(&window, &lock(&shared), &picker, "brick/jamb").unwrap();
        assert!(!window.get_ready());
        window.invoke_preview_edit();
        assert!(window.get_ready(), "{}", window.get_message());
        window.set_cut_material_policy(1);
        window.invoke_cut_material_changed();
        assert!(!window.get_ready());
        window.invoke_preview_edit();
        assert!(window.get_ready(), "{}", window.get_message());
        assert_eq!(
            lock(&shared).carve_state()["preview"]["result"]["cut_material"],
            serde_json::json!({"policy":"target"})
        );
        let draft = lock(&shared).render_document().vmf().clone();
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &draft);
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
    }
}
