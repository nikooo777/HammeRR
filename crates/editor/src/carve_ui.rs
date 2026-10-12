use std::rc::Rc;

use slint::{ComponentHandle, Model, VecModel};

use crate::mcp::{SharedSession, lock};
use crate::session::{Actor, Session};
use crate::{CarveRow, CarveWindow};

fn parameters(window: &CarveWindow) -> Result<(u32, Vec<u32>), String> {
    let rows = window.get_targets();
    let cutter = rows
        .row_data(window.get_cutter() as usize)
        .ok_or("Select at least two brushes in the map first")?
        .solid
        .parse()
        .map_err(|_| "Invalid cutter brush")?;
    let mut targets = rows
        .iter()
        .filter(|row| row.selected && !row.cutter)
        .map(|row| {
            row.solid
                .parse::<u32>()
                .map_err(|_| "Invalid target brush".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    targets.sort_unstable();
    Ok((cutter, targets))
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
            let chosen = if !token.is_empty() && window.get_token() != token {
                preview["result"]["cutter"].as_u64().map(|id| id as u32)
            } else if unchanged_source {
                previous.as_ref().map(|(cutter, _)| *cutter)
            } else {
                None
            };
            let cutter = chosen
                .and_then(|id| sources.iter().position(|source| source.solid == id))
                .unwrap_or(default);
            let targets = if !token.is_empty() && window.get_token() != token {
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
                .enumerate()
                .map(|(index, source)| {
                    let size = source.maxs - source.mins;
                    CarveRow {
                        solid: source.solid.to_string().into(),
                        label: format!(
                            "Brush {} · {:.1} × {:.1} × {:.1} units · {}",
                            source.solid, size.x, size.y, size.z, source.material
                        )
                        .into(),
                        selected: index != cutter
                            && targets
                                .as_ref()
                                .is_none_or(|targets| targets.contains(&source.solid)),
                        cutter: index == cutter,
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
    window.set_can_preview(
        window.get_targets().row_count() >= 2
            && parameters(window).is_ok_and(|(_, targets)| !targets.is_empty()),
    );
    window.set_ready(
        !preview.is_null()
            && parameters(window).is_ok_and(|(cutter, targets)| {
                preview["result"]["cutter"] == cutter
                    && preview["targets"] == serde_json::json!(targets)
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

pub fn bind(window: &CarveWindow, shared: &SharedSession) {
    window.on_choose_cutter({
        let window = window.as_weak();
        move || {
            let window = window.unwrap();
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
            window
                .set_can_preview(parameters(&window).is_ok_and(|(_, targets)| !targets.is_empty()));
            window.set_message("Cutter changed. Preview the result before applying.".into());
        }
    });
    window.on_choose_target({
        let window = window.as_weak();
        move |id| {
            let window = window.unwrap();
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
            window
                .set_can_preview(parameters(&window).is_ok_and(|(_, targets)| !targets.is_empty()));
            window.set_message("Targets changed. Preview the result before applying.".into());
        }
    });
    window.on_preview_edit({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let result = parameters(&window).and_then(|(cutter, targets)| {
                session.preview_carve(cutter, &targets, window.get_stamp().as_str())
            });
            refresh(&window, &session);
            if let Err(error) = result {
                window.set_ready(false);
                window.set_message(error.into());
            } else {
                window.set_message("".into());
            }
        }
    });
    window.on_discard({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let result = window
                .get_token()
                .parse::<u64>()
                .map_err(|_| "Preview a carve first".to_string())
                .and_then(|token| session.cancel_carve(token));
            refresh(&window, &session);
            window.set_message(result.err().unwrap_or_default().into());
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
                .parse::<u64>()
                .map_err(|_| "Preview a carve first".to_string())
                .and_then(|token| session.commit_carve(Actor::User, token));
            refresh(&window, &session);
            window.set_message(
                result
                    .map(|_| "Carve applied. Undo restores the original brushes.".to_string())
                    .unwrap_or_else(|error| error)
                    .into(),
            );
        }
    });
    window.on_cancel({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            if let Ok(token) = window.get_token().parse::<u64>() {
                let _ = lock(&shared).cancel_carve(token);
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
