use std::collections::BTreeSet;
use std::rc::Rc;

use map_core::surface::FaceId;
use slint::{ComponentHandle, Model, VecModel};

use crate::mcp::{SharedSession, lock};
use crate::session::{Actor, Session};
use crate::{HollowWindow, OpeningRow};

fn key(face: FaceId) -> String {
    format!("{}:{}", face.solid, face.side)
}

fn openings(window: &HollowWindow) -> Result<BTreeSet<FaceId>, String> {
    window
        .get_faces()
        .iter()
        .filter(|face| face.open)
        .map(|face| {
            let (solid, side) = face.key.split_once(':').ok_or("Invalid opening face")?;
            Ok(FaceId {
                solid: solid.parse().map_err(|_| "Invalid opening brush")?,
                side: side.parse().map_err(|_| "Invalid opening side")?,
            })
        })
        .collect()
}

fn preview_openings(preview: &serde_json::Value) -> BTreeSet<FaceId> {
    preview["open_faces"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|face| FaceId {
            solid: face["solid"].as_u64().unwrap() as u32,
            side: face["side"].as_u64().unwrap() as u32,
        })
        .collect()
}

fn direction(normal: glam::DVec3) -> &'static str {
    if normal.z > 0.999 {
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
    }
}

pub fn refresh(window: &HollowWindow, session: &Session) {
    let report = session.hollow_state();
    window.set_stamp(session.hollow_stamp().into());
    window.set_can_preview(session.can_clip_selection());
    window.set_selection_label(if session.can_clip_selection() {
        format!(
            "Selection: {} {} · preview before replacing solid geometry",
            session.selected_ids().len(),
            if session.selected_ids().len() == 1 {
                "object"
            } else {
                "objects"
            }
        )
        .into()
    } else {
        "Select brushes or brush entities in the map first.".into()
    });
    let preview = &report["preview"];
    let token = preview["token"].as_str().unwrap_or_default();
    let source = format!(
        "{}:{}:{:?}",
        session.document_generation(),
        session.document().revision(),
        session.selected_ids()
    );
    let mut chosen = if window.get_source() == source {
        openings(window).unwrap_or_default()
    } else {
        BTreeSet::new()
    };
    if !token.is_empty() && window.get_token() != token {
        window.set_thickness(preview["thickness"].as_f64().unwrap().to_string().into());
        window.set_direction(i32::from(preview["outward"].as_bool().unwrap()));
        chosen = preview_openings(preview);
    }
    let mut rows = Vec::new();
    match session.hollow_faces() {
        Ok(faces) => {
            let multiple = faces
                .iter()
                .map(|face| face.id.solid)
                .collect::<BTreeSet<_>>()
                .len()
                > 1;
            for face in faces {
                rows.push(OpeningRow {
                    key: key(face.id).into(),
                    label: format!(
                        "{}{} · face {}",
                        if multiple {
                            format!("Brush {} · ", face.id.solid)
                        } else {
                            String::new()
                        },
                        direction(face.normal),
                        face.id.side
                    )
                    .into(),
                    open: chosen.contains(&face.id),
                });
            }
        }
        Err(error) => {
            window.set_can_preview(false);
            window.set_selection_label(error.into());
        }
    }
    window.set_source(source.into());
    window.set_faces(Rc::new(VecModel::from(rows)).into());
    window.set_token(token.into());
    window.set_ready(
        !preview.is_null()
            && window.get_thickness().parse::<f64>().ok() == preview["thickness"].as_f64()
            && (window.get_direction() == 1) == preview["outward"].as_bool().unwrap_or(false)
            && openings(window).is_ok_and(|faces| faces == preview_openings(preview)),
    );
    window.set_cavities(
        preview["result"]["cavities"]
            .as_array()
            .map_or(0, |cavities| cavities.len()) as i32,
    );
    window.set_cavity(window.get_cavity().clamp(1, window.get_cavities().max(1)));
    window.set_summary(if preview.is_null() {
        "Preview leaves the map unchanged.".into()
    } else {
        format!(
            "{} {} → {} walls",
            preview["result"]["sources"],
            if preview["result"]["sources"] == 1 {
                "solid"
            } else {
                "solids"
            },
            preview["result"]["brushes"].as_array().unwrap().len()
        )
        .into()
    });
}

pub fn refresh_changed(window: &HollowWindow, session: &Session) {
    if window.get_stamp() != session.hollow_stamp() {
        refresh(window, session);
    }
}

pub fn bind(window: &HollowWindow, shared: &SharedSession) {
    window.on_close_openings({
        let window = window.as_weak();
        move || {
            let window = window.unwrap();
            let rows = window
                .get_faces()
                .iter()
                .map(|mut row| {
                    row.open = false;
                    row
                })
                .collect::<Vec<_>>();
            window.set_faces(Rc::new(VecModel::from(rows)).into());
            window.invoke_edited();
        }
    });
    window.on_choose_opening({
        let window = window.as_weak();
        move |key| {
            let window = window.unwrap();
            let rows = window
                .get_faces()
                .iter()
                .map(|mut row| {
                    if row.key == key {
                        row.open = !row.open;
                    }
                    row
                })
                .collect::<Vec<_>>();
            window.set_faces(Rc::new(VecModel::from(rows)).into());
            window.invoke_edited();
        }
    });
    window.on_edited({
        let window = window.as_weak();
        move || {
            let window = window.unwrap();
            window.set_ready(false);
            window.set_message("Settings changed. Preview the new walls before applying.".into());
        }
    });
    window.on_preview_edit({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let ids = session.selected_ids();
            let result = window
                .get_thickness()
                .trim()
                .parse::<f64>()
                .map_err(|_| "Enter a numeric wall thickness".to_string())
                .and_then(|thickness| {
                    let open_faces = openings(&window)?;
                    session.preview_hollow(
                        &ids,
                        map_core::hollow::Spec {
                            thickness,
                            outward: window.get_direction() == 1,
                            open_faces,
                        },
                        window.get_stamp().as_str(),
                    )
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
    window.on_look_inside({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let result = window
                .get_token()
                .parse::<u64>()
                .map_err(|_| "Preview walls first".to_string())
                .and_then(|token| {
                    lock(&shared)
                        .look_inside_hollow(token, window.get_cavity().saturating_sub(1) as usize)
                });
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
                .map_err(|_| "Preview walls first".to_string())
                .and_then(|token| session.commit_hollow(Actor::User, token));
            refresh(&window, &session);
            window.set_message(
                result
                    .map(|result| {
                        format!(
                            "Created {} wall brushes. Undo restores the original solids.",
                            result.brushes.len()
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
            if let Ok(token) = window.get_token().parse::<u64>() {
                let _ = lock(&shared).cancel_hollow(token);
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
