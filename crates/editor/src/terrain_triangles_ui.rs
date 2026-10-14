use std::{cell::RefCell, rc::Rc};

use map_core::{
    displacement::triangles::{Choice, Settings},
    surface::FaceId,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{
    TerrainTriangleRow, TerrainTrianglesWindow,
    mcp::{SharedSession, lock},
    session::{Actor, Session},
};

struct Target {
    faces: Vec<FaceId>,
    generation: u64,
    revision: u64,
    preview: Option<u64>,
}

impl Target {
    fn check(&self, session: &Session) -> Result<(), String> {
        if self.generation != session.document_generation()
            || self.revision != session.document().revision()
            || self.faces != session.selected_faces()
        {
            return Err(
                "Map or face selection changed. Cancel and reopen terrain triangles.".into(),
            );
        }
        Ok(())
    }

    fn cancel(&mut self, session: &mut Session) {
        if self.preview.is_some() && self.preview == session.face_preview_token() {
            session.cancel_face_preview();
        }
        self.preview = None;
    }
}

pub struct Controller {
    pub window: TerrainTrianglesWindow,
    target: Rc<RefCell<Option<Target>>>,
}

fn choice(index: i32) -> Result<Option<Choice>, String> {
    match index {
        0 => Ok(None),
        1 => Ok(Some(Choice::Automatic)),
        2 => Ok(Some(Choice::Enabled)),
        3 => Ok(Some(Choice::Disabled)),
        _ => Err("Choose a valid triangle classification".into()),
    }
}

fn update_selection(window: &TerrainTrianglesWindow) {
    let rows = window.get_triangles();
    let selected = rows.iter().filter(|row| row.selected).count();
    window.set_selection_label(
        format!("{selected} of {} triangles selected", rows.row_count()).into(),
    );
}

fn populate(
    window: &TerrainTrianglesWindow,
    session: &Session,
    target: &Target,
    preview: bool,
    preserve_selection: bool,
) -> Result<(), String> {
    target.check(session)?;
    let face = *target
        .faces
        .get(window.get_face_index() as usize)
        .ok_or("Choose a terrain surface")?;
    let selected = if preserve_selection {
        window
            .get_triangles()
            .iter()
            .filter(|row| row.selected)
            .map(|row| row.id)
            .collect::<std::collections::BTreeSet<_>>()
    } else {
        Default::default()
    };
    let report = session.terrain_triangles(&[face], preview)?;
    let label = |enabled, forced| {
        format!(
            "{} · {}",
            if enabled { "yes" } else { "no" },
            if forced { "forced" } else { "automatic" }
        )
    };
    let rows = report[0]
        .triangles
        .iter()
        .map(|triangle| {
            let p = triangle.centroid;
            TerrainTriangleRow {
                id: triangle.id as i32,
                label: format!(
                    "Triangle {} · center ({:.1}, {:.1}, {:.1})",
                    triangle.id, p.x, p.y, p.z
                )
                .into(),
                detail: format!(
                    "Walkable: {}   Buildable: {}{}{}",
                    label(triangle.walkable, triangle.walkable_override.is_some()),
                    label(triangle.buildable, triangle.buildable_override.is_some()),
                    if triangle.removed {
                        " · removed (build unsupported)"
                    } else {
                        ""
                    },
                    if triangle.unknown_tags != 0 {
                        " · unsupported tags"
                    } else {
                        ""
                    }
                )
                .into(),
                selected: selected.contains(&(triangle.id as i32)),
            }
        })
        .collect::<Vec<_>>();
    window.set_triangles(ModelRc::new(VecModel::from(rows)));
    update_selection(window);
    Ok(())
}

fn edited(window: &TerrainTrianglesWindow, session: &mut Session, target: &mut Option<Target>) {
    if let Some(target) = target {
        target.cancel(session);
        if let Err(error) = populate(window, session, target, false, true) {
            window.set_message(error.into());
            return;
        }
    }
    window.set_ready(false);
    window.set_message("Preview the selected triangle settings before applying.".into());
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = TerrainTrianglesWindow::new()?;
        let target = Rc::new(RefCell::new(None::<Target>));
        window.on_edited({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                edited(
                    &window.unwrap(),
                    &mut lock(&shared),
                    &mut target.borrow_mut(),
                )
            }
        });
        window.on_face_changed({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let mut target = target.borrow_mut();
                edited(&window, &mut session, &mut target);
                if let Some(target) = target.as_ref()
                    && let Err(error) = populate(&window, &session, target, false, false)
                {
                    window.set_message(error.into());
                }
            }
        });
        window.on_toggle_triangle({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move |index, selected| {
                let window = window.unwrap();
                let rows = window.get_triangles();
                if let Some(mut row) = rows.row_data(index as usize) {
                    row.selected = selected;
                    rows.set_row_data(index as usize, row);
                }
                update_selection(&window);
                edited(&window, &mut lock(&shared), &mut target.borrow_mut());
            }
        });
        window.on_select_all({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move |selected| {
                let window = window.unwrap();
                let rows = window.get_triangles();
                for index in 0..rows.row_count() {
                    let mut row = rows.row_data(index).unwrap();
                    row.selected = selected;
                    rows.set_row_data(index, row);
                }
                update_selection(&window);
                edited(&window, &mut lock(&shared), &mut target.borrow_mut());
            }
        });
        window.on_preview_edit({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let mut target = target.borrow_mut();
                let Some(target) = target.as_mut() else { return };
                let result = target.check(&session).and_then(|_| {
                    let face = *target.faces.get(window.get_face_index() as usize).ok_or("Choose a terrain surface")?;
                    let selected = window.get_triangles().iter().filter(|row| row.selected).map(|row| row.id as usize).collect::<Vec<_>>();
                    session.preview_terrain_triangle_tags(Actor::User, &[(face, selected)], Settings {walkable:choice(window.get_walkable_index())?, buildable:choice(window.get_buildable_index())?})
                });
                match result {
                    Ok(count) => { target.preview = session.face_preview_token(); if let Err(error) = populate(&window, &session, target, true, true) { window.set_message(error.into()); return; } window.set_ready(true); window.set_message(format!("Preview changes {count} terrain surfaces. Apply keeps one undo step.").into()); }
                    Err(error) => window.set_message(error.into()),
                }
            }
        });
        window.on_apply({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let mut target = target.borrow_mut();
                let Some(target) = target.as_mut() else {
                    return;
                };
                let result = target.check(&session).and_then(|_| {
                    let token = target
                        .preview
                        .ok_or("Preview selected triangles before applying")?;
                    session.check_face_preview_token(Some(token))?;
                    session.commit_face_preview(Actor::User)
                });
                match result {
                    Ok(_) => {
                        target.preview = None;
                        window.set_ready(false);
                        let _ = window.hide();
                    }
                    Err(error) => window.set_message(error.into()),
                }
            }
        });
        window.on_cancel({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                if let Some(target) = target.borrow_mut().as_mut() {
                    target.cancel(&mut lock(&shared));
                }
                let _ = window.unwrap().hide();
            }
        });
        window.window().on_close_requested({
            let (shared, target) = (shared.clone(), target.clone());
            move || {
                if let Some(target) = target.borrow_mut().as_mut() {
                    target.cancel(&mut lock(&shared));
                }
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self { window, target })
    }

    pub fn initialize(&self, session: &mut Session) -> Result<(), String> {
        let faces = session.selected_faces();
        session.terrain_triangles(&faces, false)?;
        if let Some(target) = self.target.borrow_mut().as_mut() {
            target.cancel(session);
        }
        self.window.set_faces(ModelRc::new(VecModel::from(
            faces
                .iter()
                .map(|face| format!("Brush {} · face {}", face.solid, face.side).into())
                .collect::<Vec<_>>(),
        )));
        self.window.set_face_index(0);
        self.window.set_walkable_index(0);
        self.window.set_buildable_index(0);
        let target = Target {
            faces,
            generation: session.document_generation(),
            revision: session.document().revision(),
            preview: None,
        };
        populate(&self.window, session, &target, false, false)?;
        *self.target.borrow_mut() = Some(target);
        self.window.set_message("Select triangles, choose their settings, then Preview. Cancel leaves the map unchanged.".into());
        self.refresh(session);
        Ok(())
    }

    pub fn refresh(&self, session: &Session) {
        if let Some(target) = self.target.borrow().as_ref() {
            let error = target.check(session).err();
            self.window.set_stale(error.is_some());
            self.window.set_ready(
                error.is_none()
                    && target.preview.is_some()
                    && target.preview == session.face_preview_token(),
            );
            if let Some(error) = error {
                self.window.set_message(error.into());
            }
        }
    }
}
