use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

use map_core::{
    displacement::triangles::{Choice, Settings},
    query::Ray,
    surface::FaceId,
};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    TerrainTriangleRow, TerrainTrianglesWindow,
    runtime::{SharedSession, lock},
    session::{Actor, Session},
};

struct Target {
    faces: Vec<FaceId>,
    generation: u64,
    revision: u64,
    preview: Option<u64>,
    highlight: Option<u64>,
    selected: BTreeMap<FaceId, BTreeSet<usize>>,
    vertices: bool,
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

    fn cancel_preview(&mut self, session: &mut Session) {
        if self.preview.is_some() && self.preview == session.face_preview_token() {
            session.cancel_face_preview();
        }
        self.preview = None;
    }

    fn clear_highlight(&mut self, session: &mut Session) {
        if let Some(token) = self.highlight.take() {
            let _ = session.clear_terrain_triangle_highlight(Some(token));
        }
    }

    fn cancel(&mut self, session: &mut Session) {
        self.cancel_preview(session);
        self.clear_highlight(session);
    }

    fn targets(&self) -> Vec<(FaceId, Vec<usize>)> {
        self.selected
            .iter()
            .filter(|(_, ids)| !ids.is_empty())
            .map(|(face, ids)| (*face, ids.iter().copied().collect()))
            .collect()
    }

    fn highlight(&mut self, session: &mut Session) -> Result<(), String> {
        self.highlight = Some(if self.vertices {
            session.highlight_terrain_vertices(Actor::User, &self.targets())?
        } else {
            session.highlight_terrain_triangles(Actor::User, &self.targets())?
        });
        Ok(())
    }
}

pub struct Controller {
    pub window: TerrainTrianglesWindow,
    target: Rc<RefCell<Option<Target>>>,
    picking: Rc<Cell<bool>>,
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

fn face(window: &TerrainTrianglesWindow, target: &Target) -> Result<FaceId, String> {
    target
        .faces
        .get(window.get_face_index() as usize)
        .copied()
        .ok_or("Choose a terrain surface".into())
}

fn populate(
    window: &TerrainTrianglesWindow,
    session: &Session,
    target: &Target,
) -> Result<(), String> {
    target.check(session)?;
    let face = face(window, target)?;
    let preview = target.preview.is_some() && target.preview == session.face_preview_token();
    let label = |enabled, forced| {
        format!(
            "{} · {}",
            if enabled { "yes" } else { "no" },
            if forced { "forced" } else { "automatic" }
        )
    };
    let rows = if target.vertices {
        let grids = session.terrain_grids(&[face], preview)?;
        let grid = &grids[0];
        grid.mesh
            .positions
            .iter()
            .enumerate()
            .map(|(id, p)| TerrainTriangleRow {
                id: id as i32,
                label: format!(
                    "Vertex {id} · row {}, column {}",
                    id / grid.mesh.size,
                    id % grid.mesh.size
                )
                .into(),
                detail: format!(
                    "World ({:.1}, {:.1}, {:.1})   Blend {:.0} / 255",
                    p.x, p.y, p.z, grid.mesh.alphas[id]
                )
                .into(),
                selected: target
                    .selected
                    .get(&face)
                    .is_some_and(|ids| ids.contains(&id)),
            })
            .collect::<Vec<_>>()
    } else {
        let report = session.terrain_triangles(&[face], preview)?;
        report[0]
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
                    selected: target
                        .selected
                        .get(&face)
                        .is_some_and(|ids| ids.contains(&triangle.id)),
                }
            })
            .collect::<Vec<_>>()
    };
    window.set_triangles(ModelRc::new(VecModel::from(rows)));
    let count = target.selected.values().map(BTreeSet::len).sum::<usize>();
    let surfaces = target
        .selected
        .values()
        .filter(|ids| !ids.is_empty())
        .count();
    window.set_selection_label(
        format!(
            "{count} {} across {surfaces} surface{}",
            match (target.vertices, count == 1) {
                (true, true) => "vertex",
                (true, false) => "vertices",
                (false, true) => "triangle",
                (false, false) => "triangles",
            },
            if surfaces == 1 { "" } else { "s" }
        )
        .into(),
    );

    Ok(())
}

fn edited(
    window: &TerrainTrianglesWindow,
    session: &mut Session,
    target: &mut Target,
) -> Result<(), String> {
    target.check(session)?;
    target.cancel_preview(session);
    populate(window, session, target)?;
    window.set_ready(false);
    window.set_message(
        if target.vertices {
            "Use selection to keep these exact vertices as a brush mask."
        } else {
            "Preview the selected triangle settings before applying."
        }
        .into(),
    );
    Ok(())
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = TerrainTrianglesWindow::new()?;
        let target = Rc::new(RefCell::new(None::<Target>));
        let picking = Rc::new(Cell::new(false));
        window.on_kind_changed({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                if let Some(target) = target.borrow_mut().as_mut() {
                    let result = target.check(&session).and_then(|_| {
                        target.cancel(&mut session);
                        target.selected.clear();
                        target.vertices = window.get_kind_index() == 1;
                        edited(&window, &mut session, target)
                    });
                    if let Err(error) = result {
                        window.set_message(error.into());
                    }
                }
            }
        });
        window.on_edited({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let window = window.unwrap();
                if let Some(target) = target.borrow_mut().as_mut()
                    && let Err(error) = edited(&window, &mut lock(&shared), target)
                {
                    window.set_message(error.into());
                }
            }
        });
        window.on_face_changed({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let window = window.unwrap();
                if let Some(target) = target.borrow().as_ref()
                    && let Err(error) = populate(&window, &lock(&shared), target)
                {
                    window.set_message(error.into());
                }
            }
        });
        window.on_toggle_triangle({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move |id, selected| {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let mut target = target.borrow_mut();
                let Some(target) = target.as_mut() else {
                    return;
                };
                let result = target.check(&session).and_then(|_| {
                    let face = face(&window, target)?;
                    let valid = if target.vertices {
                        session.terrain_grids(&[face], false)?[0]
                            .mesh
                            .positions
                            .len()
                    } else {
                        session.terrain_triangles(&[face], false)?[0]
                            .triangles
                            .len()
                    };
                    let id = usize::try_from(id).map_err(|_| "Choose a valid terrain component")?;
                    if id >= valid {
                        return Err("Choose a valid terrain component".into());
                    }
                    let ids = target.selected.entry(face).or_default();
                    if selected {
                        ids.insert(id);
                    } else {
                        ids.remove(&id);
                    }
                    target.highlight(&mut session)?;
                    edited(&window, &mut session, target)
                });
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        window.on_select_all({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move |selected| {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let mut target = target.borrow_mut();
                let Some(target) = target.as_mut() else {
                    return;
                };
                let result = target.check(&session).and_then(|_| {
                    if selected {
                        let face = face(&window, target)?;
                        let count = if target.vertices {
                            session.terrain_grids(&[face], false)?[0]
                                .mesh
                                .positions
                                .len()
                        } else {
                            session.terrain_triangles(&[face], false)?[0]
                                .triangles
                                .len()
                        };
                        target.selected.insert(face, (0..count).collect());
                    } else {
                        target.selected.clear();
                    }
                    target.highlight(&mut session)?;
                    edited(&window, &mut session, target)
                });
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        window.on_pick_triangle({
            let (window, shared, target, picking) = (
                window.as_weak(),
                shared.clone(),
                target.clone(),
                picking.clone(),
            );
            move || {
                let window = window.unwrap();
                let result = target
                    .borrow()
                    .as_ref()
                    .ok_or("Reopen terrain triangles".into())
                    .and_then(|target| target.check(&lock(&shared)));
                match result {
                    Ok(()) => {
                        picking.set(true);
                        let _ = window.hide();
                    }
                    Err(error) => window.set_message(error.into()),
                }
            }
        });
        window.on_frame_selected({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                if let Some(target) = target.borrow().as_ref() {
                    let result = target.check(&session).and_then(|_| {
                        let frame = if target.vertices {
                            Session::frame_terrain_vertices
                        } else {
                            Session::frame_terrain_triangles
                        };
                        frame(
                            &mut session,
                            Actor::User,
                            &target.targets(),
                            window.get_aspect(),
                        )
                    });
                    if let Err(error) = result {
                        window.set_message(error.into());
                    }
                }
            }
        });
        window.on_preview_edit({
            let (window,shared,target)=(window.as_weak(),shared.clone(),target.clone());
            move || {
                let window=window.unwrap();let mut session=lock(&shared);let mut target=target.borrow_mut();let Some(target)=target.as_mut() else {return};
                let result=target.check(&session).and_then(|_| {
                    if target.vertices { return Err("Use selection to retain vertices; classification edits require Triangles mode".into()); }
                    session.preview_terrain_triangle_tags(Actor::User,&target.targets(),Settings {walkable:choice(window.get_walkable_index())?,buildable:choice(window.get_buildable_index())?})
                });
                match result {Ok(count)=>{target.preview=session.face_preview_token();if let Err(error)=populate(&window,&session,target) {window.set_message(error.into());return;}window.set_ready(true);window.set_message(format!("Preview changes {count} terrain surfaces. Apply keeps one undo step.").into());},Err(error)=>window.set_message(error.into())}
            }
        });
        window.on_select_volume({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let mut target = target.borrow_mut();
                let Some(target) = target.as_mut() else {
                    return;
                };
                let result = target.check(&session).and_then(|_| {
                    let token = session
                        .volume_preview()
                        .ok_or("Preview a box with Select a volume… first")?
                        .token;
                    let select = if target.vertices {
                        Session::select_terrain_vertices_in_volume
                    } else {
                        Session::select_terrain_triangles_in_volume
                    };
                    let selected = select(&mut session, Actor::User, &target.faces, token)?;
                    target.highlight = Some(selected.token);
                    target.selected = selected
                        .targets
                        .into_iter()
                        .map(|(face, ids)| (face, ids.into_iter().collect()))
                        .collect();
                    edited(&window, &mut session, target)
                });
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        window.on_use_selection({
            let (window, shared, target, picking) = (
                window.as_weak(),
                shared.clone(),
                target.clone(),
                picking.clone(),
            );
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let mut target = target.borrow_mut();
                let Some(target) = target.as_mut() else {
                    return;
                };
                let result = target.check(&session).and_then(|_| {
                    if target.targets().is_empty() {
                        return Err(
                            "Select terrain triangles or vertices before using a brush mask".into(),
                        );
                    }
                    target.cancel_preview(&mut session);
                    target.highlight(&mut session)?;
                    target.highlight = None;
                    Ok(())
                });
                match result {
                    Ok(()) => {
                        picking.set(false);
                        window.set_ready(false);
                        let _ = window.hide();
                    }
                    Err(error) => window.set_message(error.into()),
                }
            }
        });
        window.on_apply({
            let (window, shared, target, picking) = (
                window.as_weak(),
                shared.clone(),
                target.clone(),
                picking.clone(),
            );
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
                        target.clear_highlight(&mut session);
                        picking.set(false);
                        window.set_ready(false);
                        let _ = window.hide();
                    }
                    Err(error) => window.set_message(error.into()),
                }
            }
        });
        window.on_cancel({
            let (window, shared, target, picking) = (
                window.as_weak(),
                shared.clone(),
                target.clone(),
                picking.clone(),
            );
            move || {
                if let Some(target) = target.borrow_mut().as_mut() {
                    target.cancel(&mut lock(&shared));
                }
                picking.set(false);
                let _ = window.unwrap().hide();
            }
        });
        window.window().on_close_requested({
            let (shared, target, picking) = (shared.clone(), target.clone(), picking.clone());
            move || {
                if let Some(target) = target.borrow_mut().as_mut() {
                    target.cancel(&mut lock(&shared));
                }
                picking.set(false);
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self {
            window,
            target,
            picking,
        })
    }

    pub fn initialize(&self, session: &mut Session) -> Result<(), String> {
        let faces = session.selected_faces();
        session.terrain_triangles(&faces, false)?;
        if let Some(target) = self.target.borrow_mut().as_mut() {
            target.cancel(session);
        }
        self.picking.set(false);
        self.window.set_faces(ModelRc::new(VecModel::from(
            faces
                .iter()
                .map(|face| format!("Brush {} · face {}", face.solid, face.side).into())
                .collect::<Vec<_>>(),
        )));
        self.window.set_face_index(0);
        self.window.set_walkable_index(0);
        self.window.set_buildable_index(0);
        self.window.set_kind_index(0);
        let target = Target {
            faces,
            generation: session.document_generation(),
            revision: session.document().revision(),
            preview: None,
            highlight: None,
            selected: BTreeMap::new(),
            vertices: false,
        };
        populate(&self.window, session, &target)?;
        *self.target.borrow_mut() = Some(target);
        self.window.set_message(
            "Select triangles in the list or viewport, choose settings, then Preview.".into(),
        );
        self.refresh(session);
        Ok(())
    }

    pub fn is_picking(&self) -> bool {
        self.picking.get()
    }

    pub fn cancel_pick(&self) {
        self.picking.set(false);
        self.window
            .set_message("Terrain picking cancelled. Selection and fields are unchanged.".into());
        let _ = self.window.show();
    }

    pub fn select_at_ray(&self, session: &mut Session, ray: &Ray) -> Result<usize, String> {
        if !self.picking.get() {
            return Err("Triangle picking is not active".into());
        }
        let mut target = self.target.borrow_mut();
        let target = target.as_mut().ok_or("Reopen terrain triangles")?;
        target.check(session)?;
        let (face, id, power) = if target.vertices {
            let hit = session
                .pick_terrain_vertex(ray, &target.faces)?
                .ok_or("Click visible selected terrain, or cancel picking")?;
            (hit.vertex.face, hit.vertex.vertex, hit.power)
        } else {
            let hit = session
                .pick_terrain_triangle(ray, &target.faces)?
                .ok_or("Click visible selected terrain, or cancel picking")?;
            (hit.triangle.face, hit.triangle.triangle, hit.power)
        };
        if session.terrain_grids(&[face], false)?[0].power != power {
            return Err(
                "Finish the grid-resolution preview before selecting terrain components".into(),
            );
        }
        self.window.set_face_index(
            target
                .faces
                .iter()
                .position(|candidate| *candidate == face)
                .unwrap() as i32,
        );
        target.selected.entry(face).or_default().insert(id);
        target.highlight(session)?;
        edited(&self.window, session, target)?;
        self.picking.set(false);
        self.window.set_message(
            format!(
                "Selected {} {id}. Its {} appears in the viewport.",
                if target.vertices {
                    "vertex"
                } else {
                    "triangle"
                },
                if target.vertices {
                    "cyan marker"
                } else {
                    "yellow outline"
                },
            )
            .into(),
        );
        Ok(id)
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
                if self.picking.replace(false) {
                    let _ = self.window.show();
                }
                self.window.set_message(error.into());
            }
        }
    }
}
