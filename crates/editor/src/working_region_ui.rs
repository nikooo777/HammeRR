use std::{cell::RefCell, rc::Rc};

use glam::DVec3;
use serde_json::Value;
use slint::ComponentHandle;
use source_formats::vmf::format_number;

use crate::{
    AppWindow, WorkingRegionWindow,
    runtime::{DocumentAccess, DocumentContinuation},
    session::{
        Actor, ObjectFilter, Session,
        working_region::{Action, Options},
    },
};

const FILTER_KINDS: [Option<&str>; 5] = [
    None,
    Some("solid"),
    Some("entity"),
    Some("point_entity"),
    Some("brush_entity"),
];

pub struct Controller {
    pub window: WorkingRegionWindow,
    snapshot: Rc<RefCell<String>>,
}

fn load(window: &WorkingRegionWindow, bounds: &Value) {
    let text =
        |axis: usize, corner: usize| format_number(bounds[corner][axis].as_f64().unwrap()).into();
    window.set_min_x(text(0, 0));
    window.set_min_y(text(1, 0));
    window.set_min_z(text(2, 0));
    window.set_max_x(text(0, 1));
    window.set_max_y(text(1, 1));
    window.set_max_z(text(2, 1));
}

fn load_filter(window: &WorkingRegionWindow, filter: &Value) {
    let text = |key: &str| filter[key].as_str().unwrap_or_default().into();
    window.set_filter_enabled(filter.is_object());
    window.set_query(text("query"));
    window.set_classname(text("classname"));
    window.set_exact_class(filter["exact_class"] == true);
    window.set_targetname(text("targetname"));
    window.set_material(text("material"));
    window.set_kind(
        FILTER_KINDS
            .iter()
            .position(|kind| *kind == filter["kind"].as_str())
            .unwrap_or_default() as i32,
    );
    window.set_visibility(match filter["hidden"].as_bool() {
        Some(false) => 1,
        Some(true) => 2,
        None => 0,
    });
    let within = &filter["within"];
    window.set_limit_filter_bounds(!within.is_null());
    if !within.is_null() {
        let coordinate = |axis: usize, corner: usize| {
            format_number(within[corner][axis].as_f64().unwrap()).into()
        };
        window.set_filter_min_x(coordinate(0, 0));
        window.set_filter_min_y(coordinate(1, 0));
        window.set_filter_min_z(coordinate(2, 0));
        window.set_filter_max_x(coordinate(0, 1));
        window.set_filter_max_y(coordinate(1, 1));
        window.set_filter_max_z(coordinate(2, 1));
    }
}

fn number(text: slint::SharedString, label: &str) -> Result<f64, String> {
    let value = text
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("{label} must contain finite numbers"))?;
    if !value.is_finite() {
        return Err(format!("{label} must contain finite numbers"));
    }
    Ok(value)
}

fn bounds_from_fields(
    fields: [[slint::SharedString; 3]; 2],
    label: &str,
) -> Result<[DVec3; 2], String> {
    let mut bounds = [DVec3::ZERO; 2];
    for (corner, fields) in fields.into_iter().enumerate() {
        for (axis, text) in fields.into_iter().enumerate() {
            bounds[corner][axis] = number(text, label)?;
        }
    }
    Ok(bounds)
}

fn fields(window: &WorkingRegionWindow) -> Result<[DVec3; 2], String> {
    bounds_from_fields(
        [
            [window.get_min_x(), window.get_min_y(), window.get_min_z()],
            [window.get_max_x(), window.get_max_y(), window.get_max_z()],
        ],
        "Box corners",
    )
}

fn optional_text(value: slint::SharedString) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn filter(window: &WorkingRegionWindow) -> Result<Option<ObjectFilter>, String> {
    if !window.get_filter_enabled() {
        return Ok(None);
    }
    let within = if window.get_limit_filter_bounds() {
        Some(
            bounds_from_fields(
                [
                    [
                        window.get_filter_min_x(),
                        window.get_filter_min_y(),
                        window.get_filter_min_z(),
                    ],
                    [
                        window.get_filter_max_x(),
                        window.get_filter_max_y(),
                        window.get_filter_max_z(),
                    ],
                ],
                "Object filter bounds",
            )?
            .map(|point| point.to_array()),
        )
    } else {
        None
    };
    Ok(Some(ObjectFilter {
        query: optional_text(window.get_query()),
        classname: optional_text(window.get_classname()),
        exact_class: window.get_exact_class(),
        targetname: optional_text(window.get_targetname()),
        material: optional_text(window.get_material()),
        kind: FILTER_KINDS
            .get(window.get_kind() as usize)
            .copied()
            .flatten()
            .map(str::to_owned),
        hidden: match window.get_visibility() {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        },
        within,
    }))
}

fn options(window: &WorkingRegionWindow) -> Result<Options, String> {
    Ok(Options {
        draw_distance: if window.get_draw_distance_enabled() {
            Some(number(window.get_draw_distance(), "Draw distance")?)
        } else {
            None
        },
        follow_camera: window.get_follow_camera(),
        filter: filter(window)?,
    })
}

fn corner_text(corner: &Value) -> String {
    (0..3)
        .map(|axis| format_number(corner[axis].as_f64().unwrap()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn refresh(window: &WorkingRegionWindow, session: &mut Session, snapshot: &RefCell<String>) {
    let state = session.working_region_summary();
    *snapshot.borrow_mut() = state["snapshot"].as_str().unwrap().into();
    window.set_summary(
        if state["active"] == true {
            format!(
                "Working region active · Excluded entries: {} · Excluded selection items: {}",
                state["excluded"], state["selected_excluded"]
            )
        } else {
            "Working region off · Saved visibility and compilation are unchanged".into()
        }
        .into(),
    );
    let options = &state["options"];
    window.set_effective_box(
        if state["enabled"] == true
            && options["follow_camera"] == true
            && !state["effective_mins"].is_null()
        {
            format!(
                "Applied box · Minimum ({}) · Maximum ({})",
                corner_text(&state["effective_mins"]),
                corner_text(&state["effective_maxs"])
            )
        } else {
            String::new()
        }
        .into(),
    );
    window.set_region_enabled(state["enabled"] == true);
    if !state["mins"].is_null() {
        load(window, &serde_json::json!([state["mins"], state["maxs"]]));
    }
    window.set_follow_camera(options["follow_camera"] == true);
    window.set_draw_distance_enabled(!options["draw_distance"].is_null());
    if let Some(distance) = options["draw_distance"].as_f64() {
        window.set_draw_distance(format_number(distance).into());
    }
    load_filter(window, &options["filter"]);
}

pub fn bind_clear_with_access(app: &AppWindow, access: &DocumentAccess) {
    app.on_clear_working_region({
        let (app, target) = (app.as_weak(), access.downgrade());
        move || {
            let Some(app) = app.upgrade() else {
                return;
            };
            let result = target.upgrade().and_then(|access| {
                access.with_session(|session| {
                    let snapshot = session.working_region_stamp();
                    match session.set_working_region(Actor::User, Action::Clear, &snapshot) {
                        Ok(_) => app.set_working_region_active(false),
                        Err(error) => app.set_message(error.into()),
                    }
                    Ok(())
                })
            });
            if let Err(error) = result {
                app.set_message(error.into());
            }
        }
    });
}

fn callback(
    window: &slint::Weak<WorkingRegionWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(WorkingRegionWindow, &mut Session),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = target.upgrade().and_then(|access| {
        access.with_session(|session| {
            body(window.clone_strong(), session);
            Ok(())
        })
    });
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = WorkingRegionWindow::new()?;
        let snapshot = Rc::new(RefCell::new(String::new()));
        window.on_apply({
            let (window, target, snapshot) =
                (window.as_weak(), access.downgrade(), snapshot.clone());
            move || {
                callback(&window, &target, |window, session| {
                    let result = fields(&window).and_then(|bounds| {
                        session.set_working_region(
                            Actor::User,
                            Action::SetWithOptions {
                                bounds,
                                enabled: window.get_region_enabled(),
                                options: options(&window)?,
                            },
                            &snapshot.borrow(),
                        )
                    });
                    match result {
                        Ok(_) => {
                            window.set_message(
                                "View updated. Source and compilation are unchanged.".into(),
                            );
                            refresh(&window, session, &snapshot);
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                });
            }
        });
        window.on_use_bounds({
            let (window, target, snapshot) =
                (window.as_weak(), access.downgrade(), snapshot.clone());
            move || {
                callback(&window, &target, |window, session| {
                    if *snapshot.borrow() != session.working_region_stamp() {
                        window.set_message(
                            "Map, selection or view changed. Reopen Working region to inspect it again."
                                .into(),
                        );
                        return;
                    }
                    match session.working_region_selection_bounds() {
                        Ok(bounds) => {
                            load(
                                &window,
                                &serde_json::json!(bounds.map(|point| point.to_array())),
                            );
                            window.set_region_enabled(true);
                            window.set_message(
                                "Selection bounds loaded. Apply updates the view.".into(),
                            );
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                });
            }
        });
        window.on_clear({
            let (window, target, snapshot) =
                (window.as_weak(), access.downgrade(), snapshot.clone());
            move || {
                callback(&window, &target, |window, session| {
                    let stamp = snapshot.borrow().clone();
                    match session.set_working_region(Actor::User, Action::Clear, &stamp) {
                        Ok(_) => {
                            window.set_message("All working region filters cleared.".into());
                            refresh(&window, session, &snapshot);
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                });
            }
        });
        window.on_cancel({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        Ok(Self { window, snapshot })
    }

    pub fn show(&self, session: &mut Session) -> Result<(), slint::PlatformError> {
        refresh(&self.window, session, &self.snapshot);
        self.window.set_message(
            "Apply updates all filters together. Clear working region restores otherwise-visible objects."
                .into(),
        );
        self.window.show()
    }

    pub fn refresh(&self, session: &mut Session) {
        if *self.snapshot.borrow() != session.working_region_stamp() {
            refresh(&self.window, session, &self.snapshot);
            self.window
                .set_message("Map, selection or view changed. Current filters reloaded.".into());
        }
    }
}
