use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use glam::{DVec2, DVec3};
use map_core::primitive::{Axis, PolygonSpec, Shape};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    PolygonCorner, PolygonWindow,
    runtime::{SharedSession, lock},
    session::{Actor, Session},
    ui::preview_image,
};

pub struct Controller {
    pub window: PolygonWindow,
    points: Rc<RefCell<Vec<DVec2>>>,
    token: Rc<Cell<Option<u64>>>,
    generation: Rc<Cell<u64>>,
    dirty: Rc<Cell<bool>>,
}

fn number(value: &str) -> Result<f64, String> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or("Enter finite coordinates and dimensions".into())
}

fn view(window: &PolygonWindow) -> Result<(DVec2, f64), String> {
    let center = DVec2::new(
        number(&window.get_center_u())?,
        number(&window.get_center_v())?,
    );
    let span = number(&window.get_span())?;
    if !(1.0..=131072.0).contains(&span) {
        return Err("View span must be 1–131072 units".into());
    }
    Ok((center, span))
}

fn request(window: &PolygonWindow, points: &[DVec2]) -> Result<PolygonSpec, String> {
    Ok(PolygonSpec {
        outline: points.to_vec(),
        axis: match window.get_axis() {
            0 => Axis::X,
            1 => Axis::Y,
            _ => Axis::Z,
        },
        base: number(&window.get_base())?,
        depth: number(&window.get_depth())?,
        material: window.get_material().into(),
    })
}

fn display_points(window: &PolygonWindow, points: &[DVec2]) -> Result<(), String> {
    let (center, span) = view(window)?;
    let mut path = String::new();
    let rows = points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let x = (point.x - center.x) / span + 0.5;
            let y = 0.5 - (point.y - center.y) / span;
            path.push_str(&format!(
                "{} {} {} ",
                if index == 0 { "M" } else { "L" },
                x * 512.0,
                y * 512.0
            ));
            PolygonCorner {
                label: (index + 1).to_string().into(),
                u: source_formats::vmf::format_number(point.x).into(),
                v: source_formats::vmf::format_number(point.y).into(),
                x: x as f32,
                y: y as f32,
            }
        })
        .collect::<Vec<_>>();
    if points.len() >= 3 {
        path.push('Z');
    }
    window.set_corners(ModelRc::new(VecModel::from(rows)));
    window.set_outline(path.into());
    Ok(())
}

fn changed(window: &PolygonWindow, dirty: &Cell<bool>, points: &[DVec2]) {
    dirty.set(true);
    window.set_ready(false);
    let result = display_points(window, points);
    window.set_summary(
        format!(
            "{} corners · preview to check the outline and extrusion",
            points.len()
        )
        .into(),
    );
    window.set_message(result.err().unwrap_or_default().into());
}

fn guard(session: &Session, generation: u64, token: Option<u64>) -> Result<(), String> {
    if session.document_generation() != generation
        || session.primitive_preview().map(|preview| preview.token) != token
    {
        return Err("Map or shape preview changed; reopen polygon drawing to review it".into());
    }
    Ok(())
}

fn snap(session: &Session, point: DVec2, enabled: bool) -> DVec2 {
    if enabled {
        crate::spatial::Settings {
            snap: true,
            ..session.spatial()
        }
        .snap_point(DVec3::new(point.x, point.y, 0.0))
        .truncate()
    } else {
        point
    }
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = PolygonWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(900.0, 740.0));
        let points = Rc::new(RefCell::new(Vec::<DVec2>::new()));
        let token = Rc::new(Cell::new(None));
        let generation = Rc::new(Cell::new(0));
        let dirty = Rc::new(Cell::new(false));
        window.on_edited({
            let (window, points, dirty) = (window.as_weak(), points.clone(), dirty.clone());
            move || changed(&window.unwrap(), &dirty, &points.borrow())
        });
        window.on_view_edited({
            let (window, points) = (window.as_weak(), points.clone());
            move || {
                let window = window.unwrap();
                if let Err(error) = display_points(&window, &points.borrow()) {
                    window.set_message(error.into());
                }
            }
        });
        window.on_draw_corner({
            let (window, points, dirty, shared) = (
                window.as_weak(),
                points.clone(),
                dirty.clone(),
                shared.clone(),
            );
            move |x, y| {
                let window = window.unwrap();
                let result = (|| {
                    let (center, span) = view(&window)?;
                    if points.borrow().len() >= map_core::polygon::MAX_CORNERS {
                        return Err("Polygon supports at most 64 corners".into());
                    }
                    let point = center + DVec2::new(f64::from(x) - 0.5, 0.5 - f64::from(y)) * span;
                    let point = snap(&lock(&shared), point, window.get_snap());
                    points.borrow_mut().push(point);
                    changed(&window, &dirty, &points.borrow());
                    Ok::<_, String>(())
                })();
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        window.on_add_corner({
            let (window, points, dirty, shared) = (
                window.as_weak(),
                points.clone(),
                dirty.clone(),
                shared.clone(),
            );
            move || {
                let window = window.unwrap();
                let result = (|| {
                    if points.borrow().len() >= map_core::polygon::MAX_CORNERS {
                        return Err("Polygon supports at most 64 corners".into());
                    }
                    let point = DVec2::new(
                        number(&window.get_corner_u())?,
                        number(&window.get_corner_v())?,
                    );
                    points
                        .borrow_mut()
                        .push(snap(&lock(&shared), point, window.get_snap()));
                    changed(&window, &dirty, &points.borrow());
                    Ok::<_, String>(())
                })();
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        window.on_change_corner({
            let (window, points, dirty, shared) = (
                window.as_weak(),
                points.clone(),
                dirty.clone(),
                shared.clone(),
            );
            move |index, u, v| {
                let window = window.unwrap();
                let result = (|| {
                    let point = DVec2::new(number(&u)?, number(&v)?);
                    let mut points = points.borrow_mut();
                    let selected = points
                        .get_mut(index as usize)
                        .ok_or("Corner no longer exists")?;
                    *selected = snap(&lock(&shared), point, window.get_snap());
                    changed(&window, &dirty, &points);
                    Ok::<_, String>(())
                })();
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        window.on_remove_corner({
            let (window, points, dirty) = (window.as_weak(), points.clone(), dirty.clone());
            move |index| {
                let mut points = points.borrow_mut();
                if index >= 0 && (index as usize) < points.len() {
                    points.remove(index as usize);
                    changed(&window.unwrap(), &dirty, &points);
                }
            }
        });
        window.on_undo_corner({
            let (window, points, dirty) = (window.as_weak(), points.clone(), dirty.clone());
            move || {
                points.borrow_mut().pop();
                changed(&window.unwrap(), &dirty, &points.borrow());
            }
        });
        window.on_clear_corners({
            let (window, points, dirty) = (window.as_weak(), points.clone(), dirty.clone());
            move || {
                points.borrow_mut().clear();
                changed(&window.unwrap(), &dirty, &points.borrow());
            }
        });
        window.on_preview({
            let (window, points, dirty, token, generation, shared) = (
                window.as_weak(),
                points.clone(),
                dirty.clone(),
                token.clone(),
                generation.clone(),
                shared.clone(),
            );
            move || {
                let window = window.unwrap();
                let result = (|| {
                    let mut session = lock(&shared);
                    guard(&session, generation.get(), token.get())?;
                    session.create_polygon(
                        Actor::User,
                        request(&window, &points.borrow())?,
                        window.get_snap(),
                        true,
                    )?;
                    let preview = session.primitive_preview().unwrap();
                    token.set(Some(preview.token));
                    *points.borrow_mut() = preview.spec.outline.clone();
                    dirty.set(false);
                    display_points(&window, &points.borrow())?;
                    window.set_ready(true);
                    window.set_summary(
                        format!(
                            "{} corners · {} editable brushes",
                            points.borrow().len(),
                            preview.ids.len()
                        )
                        .into(),
                    );
                    window.set_message(
                        "Review the map preview, then Create brushes or Cancel.".into(),
                    );
                    Ok::<_, String>(())
                })();
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        window.on_apply({
            let (window, dirty, token, generation, shared) = (
                window.as_weak(),
                dirty.clone(),
                token.clone(),
                generation.clone(),
                shared.clone(),
            );
            move || {
                let window = window.unwrap();
                let result = (|| {
                    let mut session = lock(&shared);
                    guard(&session, generation.get(), token.get())?;
                    if dirty.get() {
                        return Err("Preview changed settings before creating brushes".into());
                    }
                    session.commit_primitive_preview(
                        Actor::User,
                        token.get().ok_or("Preview the polygon first")?,
                    )?;
                    token.set(None);
                    Ok::<_, String>(())
                })();
                match result {
                    Ok(()) => {
                        let _ = window.hide();
                    }
                    Err(error) => window.set_message(error.into()),
                }
            }
        });
        window.on_frame({
            let (window, token, generation, shared) = (
                window.as_weak(),
                token.clone(),
                generation.clone(),
                shared.clone(),
            );
            move || {
                let window = window.unwrap();
                let result = (|| {
                    let mut session = lock(&shared);
                    guard(&session, generation.get(), token.get())?;
                    session.frame_primitive_preview(
                        token.get().ok_or("Preview the polygon first")?,
                        window.get_aspect(),
                    )
                })();
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        window.on_cancel({
            let (window, token, shared) = (window.as_weak(), token.clone(), shared.clone());
            move || {
                if let Some(token) = token.take() {
                    let _ = lock(&shared).cancel_primitive_preview(token);
                }
                let _ = window.unwrap().hide();
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                window.unwrap().invoke_cancel();
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self {
            window,
            points,
            token,
            generation,
            dirty,
        })
    }

    pub fn initialize(&self, session: &mut Session, material: &str) -> Result<(), String> {
        self.generation.set(session.document_generation());
        if let Some(preview) = session.primitive_preview() {
            if preview.spec.shape != Shape::Polygon {
                return Err(
                    "Apply or cancel the other brush shape before drawing a polygon".into(),
                );
            }
            self.token.set(Some(preview.token));
            *self.points.borrow_mut() = preview.spec.outline.clone();
            self.window.set_axis(match preview.spec.axis {
                Axis::X => 0,
                Axis::Y => 1,
                Axis::Z => 2,
            });
            let w = preview.spec.axis.indices()[2];
            self.window
                .set_base(preview.spec.mins[w].to_string().into());
            self.window.set_depth(
                (preview.spec.maxs[w] - preview.spec.mins[w])
                    .to_string()
                    .into(),
            );
            self.window
                .set_material(preview.spec.material.clone().into());
            let low = self
                .points
                .borrow()
                .iter()
                .copied()
                .fold(DVec2::splat(f64::INFINITY), DVec2::min);
            let high = self
                .points
                .borrow()
                .iter()
                .copied()
                .fold(DVec2::splat(f64::NEG_INFINITY), DVec2::max);
            let center = (low + high) * 0.5;
            self.window.set_center_u(center.x.to_string().into());
            self.window.set_center_v(center.y.to_string().into());
            self.window.set_span(
                ((high - low).max_element() * 1.5)
                    .clamp(32.0, 131072.0)
                    .to_string()
                    .into(),
            );
            self.window.set_summary(
                format!(
                    "{} corners · {} editable brushes",
                    self.points.borrow().len(),
                    preview.ids.len()
                )
                .into(),
            );
            self.window.set_ready(true);
        } else {
            self.token.set(None);
            self.points.borrow_mut().clear();
            self.window
                .set_base(session.spatial().elevation.to_string().into());
            self.window
                .set_depth(session.spatial().depth.to_string().into());
            self.window.set_material(material.into());
            self.window.set_ready(false);
            self.window
                .set_summary("Draw at least three corners.".into());
        }
        self.window.set_snap(session.spatial().snap);
        self.dirty.set(false);
        self.window.set_message("".into());
        display_points(&self.window, &self.points.borrow())?;
        self.refresh_material(session);
        Ok(())
    }

    pub fn refresh(&self, session: &Session) {
        if guard(session, self.generation.get(), self.token.get()).is_err() {
            self.window.set_ready(false);
            self.window.set_message(
                "Map or shape preview changed; reopen polygon drawing to review it.".into(),
            );
        }
    }

    pub fn refresh_material(&self, session: &Session) {
        self.window.set_material_image(
            session
                .material_preview(&self.window.get_material())
                .ok()
                .map(|image| preview_image(&image))
                .unwrap_or_default(),
        );
    }

    pub fn choose_material(
        &self,
        session: &mut Session,
        target: (Option<u64>, u64),
        material: &str,
    ) -> Result<(), String> {
        guard(session, self.generation.get(), self.token.get())?;
        if (self.token.get(), self.generation.get()) != target {
            return Err("Polygon preview changed; reopen the material picker".into());
        }
        self.window.set_material(material.into());
        changed(&self.window, &self.dirty, &self.points.borrow());
        self.refresh_material(session);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn polygon_controls_draw_concavity_review_and_preserve_foreign_previews() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let shared = Arc::new(Mutex::new(Session::new(Err("No assets needed".into()))));
        let source = lock(&shared).document().vmf().clone();
        let controller = Controller::new(&shared).unwrap();
        controller
            .initialize(&mut lock(&shared), "dev/dev_measuregeneric01")
            .unwrap();
        controller.window.set_span("512".into());
        controller.window.set_center_u("0".into());
        controller.window.set_center_v("0".into());
        controller.window.set_axis(2);
        controller.window.set_depth("64".into());
        controller.window.show().unwrap();
        for (x, y) in [
            (0.25, 0.75),
            (0.75, 0.75),
            (0.75, 0.5),
            (0.5, 0.5),
            (0.5, 0.25),
            (0.25, 0.25),
        ] {
            controller.window.invoke_draw_corner(x, y);
        }
        assert_eq!(controller.points.borrow().len(), 6);
        controller.window.invoke_preview();
        assert!(
            controller.window.get_ready(),
            "{}",
            controller.window.get_message()
        );
        assert_eq!(lock(&shared).primitive_preview().unwrap().ids.len(), 4);
        assert_eq!(lock(&shared).document().vmf(), &source);
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "polygon-drawing");
        controller.window.set_depth("96".into());
        controller.window.invoke_edited();
        controller.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &source);
        controller.window.invoke_preview();
        controller.window.invoke_apply();
        assert_eq!(lock(&shared).document().undo_labels().count(), 1);
        assert_eq!(lock(&shared).document().vmf().world.solids.len(), 4);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &source);
        controller
            .initialize(&mut lock(&shared), "dev/dev_measuregeneric01")
            .unwrap();
        for (x, y) in [(0.25, 0.75), (0.75, 0.75), (0.5, 0.25)] {
            controller.window.invoke_draw_corner(x, y);
        }
        controller.window.invoke_preview();
        lock(&shared)
            .create_primitive(Actor::Agent, map_core::primitive::Spec::default(), true)
            .unwrap();
        let foreign = lock(&shared).primitive_preview().unwrap().token;
        controller.window.invoke_cancel();
        assert_eq!(lock(&shared).primitive_preview().unwrap().token, foreign);
    }
}
