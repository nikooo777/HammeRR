use std::{cell::RefCell, rc::Rc};

use glam::DVec3;
use serde_json::Value;
use slint::ComponentHandle;
use source_formats::vmf::format_number;

use crate::{
    AppWindow, VisibilityCategoryRow, VisibilityExclusionRow, VisibilityProfileRow,
    WorkingRegionWindow,
    runtime::{DocumentAccess, DocumentContinuation},
    session::{
        Actor, ObjectFilter, Session,
        visibility_profiles::Action as ProfileAction,
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
const CATEGORY_PAGE_SIZE: usize = 50;

#[derive(Default)]
struct RulesState {
    exclusions: Vec<ObjectFilter>,
    snapshot: String,
    exclusions_snapshot: String,
    profiles_snapshot: String,
    observed_snapshot: String,
    category_offset: usize,
    category_next: Option<usize>,
    profiles: Vec<Profile>,
}

struct Profile {
    name: String,
    filter: Option<ObjectFilter>,
    exclusions: Vec<ObjectFilter>,
}

#[derive(Clone)]
struct RuleBinding {
    window: slint::Weak<WorkingRegionWindow>,
    target: DocumentContinuation,
    snapshot: Rc<RefCell<String>>,
    rules: Rc<RefCell<RulesState>>,
}

enum RuleAction {
    Category(String, bool),
    ExcludeMatches,
    RemoveExclusion(i32),
    SaveProfile(bool),
    RenameProfile,
    DeleteProfile,
    ApplyProfile,
}

pub struct Controller {
    pub window: WorkingRegionWindow,
    snapshot: Rc<RefCell<String>>,
    rules: Rc<RefCell<RulesState>>,
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

fn options(window: &WorkingRegionWindow, rules: &RulesState) -> Result<Options, String> {
    Ok(Options {
        draw_distance: if window.get_draw_distance_enabled() {
            Some(number(window.get_draw_distance(), "Draw distance")?)
        } else {
            None
        },
        follow_camera: window.get_follow_camera(),
        filter: filter(window)?,
        exclusions: rules.exclusions.clone(),
    })
}

fn corner_text(corner: &Value) -> String {
    (0..3)
        .map(|axis| format_number(corner[axis].as_f64().unwrap()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn refresh(
    window: &WorkingRegionWindow,
    session: &mut Session,
    snapshot: &RefCell<String>,
    rules: &RefCell<RulesState>,
    reset_fields: bool,
) -> Result<(), String> {
    let state = session.working_region_summary();
    let options: Options = serde_json::from_value(state["options"].clone())
        .map_err(|error| format!("Could not read visibility options: {error}"))?;
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
    window.set_effective_box(
        if state["enabled"] == true
            && options.follow_camera
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
    if reset_fields {
        window.set_region_enabled(state["enabled"] == true);
        if !state["mins"].is_null() {
            load(window, &serde_json::json!([state["mins"], state["maxs"]]));
        }
        window.set_follow_camera(options.follow_camera);
        window.set_draw_distance_enabled(options.draw_distance.is_some());
        if let Some(distance) = options.draw_distance {
            window.set_draw_distance(format_number(distance).into());
        }
        load_filter(window, &state["options"]["filter"]);
    }
    window.set_exclusion_summary(
        if options.exclusions.is_empty() {
            "No exclusions. Add a category or use Exclude these matches.".to_owned()
        } else {
            format!(
                "{} active exclusions · any match is excluded",
                options.exclusions.len()
            )
        }
        .into(),
    );
    window.set_exclusions(
        Rc::new(slint::VecModel::from(
            options
                .exclusions
                .iter()
                .enumerate()
                .map(|(index, filter)| VisibilityExclusionRow {
                    index: index as i32,
                    summary: filter_summary(filter).into(),
                })
                .collect::<Vec<_>>(),
        ))
        .into(),
    );
    let mut rules = rules.borrow_mut();
    rules.exclusions = options.exclusions;
    rules.exclusions_snapshot = session.visibility_rules_snapshot();
    Ok(())
}

fn filter_summary(filter: &ObjectFilter) -> String {
    let mut parts = Vec::new();
    if let Some(query) = &filter.query {
        parts.push(format!("Search “{query}”"));
    }
    if let Some(kind) = &filter.kind {
        parts.push(match kind.as_str() {
            "solid" => "World brushes".into(),
            "entity" => "All entities".into(),
            "point_entity" => "Point entities".into(),
            "brush_entity" => "Brush entities".into(),
            _ => kind.clone(),
        });
    }
    if let Some(classname) = &filter.classname {
        parts.push(format!(
            "Class {} “{classname}”",
            if filter.exact_class {
                "equals"
            } else {
                "contains"
            }
        ));
    }
    if let Some(name) = &filter.targetname {
        parts.push(format!("Entity name contains “{name}”"));
    }
    if let Some(material) = &filter.material {
        parts.push(format!("Material contains “{material}”"));
    }
    if let Some(hidden) = filter.hidden {
        parts.push(
            if hidden {
                "Saved hidden"
            } else {
                "Saved visible"
            }
            .into(),
        );
    }
    if let Some([mins, maxs]) = filter.within {
        let corner = |point: [f64; 3]| point.map(format_number).join(", ");
        parts.push(format!("Bounds ({}) to ({})", corner(mins), corner(maxs)));
    }
    if parts.is_empty() {
        "All objects".into()
    } else {
        parts.join(" · ")
    }
}

fn profile_detail(profile: &Profile) -> String {
    let mut lines = vec![format!(
        "Include: {}",
        profile
            .filter
            .as_ref()
            .map(filter_summary)
            .unwrap_or_else(|| "All objects".into())
    )];
    if profile.exclusions.is_empty() {
        lines.push("No exclusions".into());
    } else {
        lines.extend(
            profile
                .exclusions
                .iter()
                .enumerate()
                .map(|(index, filter)| {
                    format!("Exclude {}: {}", index + 1, filter_summary(filter))
                }),
        );
    }
    lines.join("\n")
}

fn refresh_categories(
    window: &WorkingRegionWindow,
    session: &Session,
    rules: &RefCell<RulesState>,
) -> Result<(), String> {
    window.set_categories_ready(false);
    window.set_categories(slint::ModelRc::default());
    window.set_category_summary("".into());
    window.set_category_previous(false);
    window.set_category_next(false);
    let offset = rules.borrow().category_offset;
    let report = session.visibility_categories(
        window.get_category_query().as_str(),
        offset,
        CATEGORY_PAGE_SIZE,
    )?;
    let snapshot = report["snapshot"]
        .as_str()
        .ok_or("Missing category snapshot")?;
    let total = report["total"].as_u64().ok_or("Missing category count")? as usize;
    let rows = report["categories"]
        .as_array()
        .ok_or("Missing visibility categories")?
        .iter()
        .map(|category| {
            Ok(VisibilityCategoryRow {
                id: category["id"]
                    .as_str()
                    .ok_or("Missing category identity")?
                    .into(),
                label: category["label"]
                    .as_str()
                    .ok_or("Missing category label")?
                    .into(),
                detail: format!(
                    "{} whole owners",
                    category["total"]
                        .as_u64()
                        .ok_or("Missing category object count")?
                )
                .into(),
                excluded: category["excluded"]
                    .as_bool()
                    .ok_or("Missing category exclusion state")?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let returned = rows.len();
    let next = report["next_offset"].as_u64().map(|value| value as usize);
    window.set_categories(Rc::new(slint::VecModel::from(rows)).into());
    window.set_category_summary(if total == 0 {
        "No matching categories".into()
    } else if returned == 0 {
        format!("No categories on this page · {total} total").into()
    } else {
        format!("Categories {}–{} of {total}", offset + 1, offset + returned).into()
    });
    window.set_category_previous(offset > 0);
    window.set_category_next(next.is_some());
    window.set_categories_ready(true);
    let mut rules = rules.borrow_mut();
    rules.snapshot = snapshot.into();
    rules.category_next = next;
    Ok(())
}

fn refresh_profiles(
    window: &WorkingRegionWindow,
    session: &Session,
    rules: &RefCell<RulesState>,
) -> Result<(), String> {
    window.set_profiles_ready(false);
    window.set_profiles(slint::ModelRc::default());
    window.set_profile_summary("".into());
    window.set_selected_profile_detail("".into());
    let report = session.visibility_profiles()?;
    let snapshot = report["snapshot"]
        .as_str()
        .ok_or("Missing profile snapshot")?;
    let profiles = report["entries"]
        .as_array()
        .ok_or("Missing visibility profiles")?
        .iter()
        .map(|entry| {
            Ok(Profile {
                name: entry["name"].as_str().ok_or("Missing profile name")?.into(),
                filter: serde_json::from_value(entry["rules"]["filter"].clone())
                    .map_err(|error| format!("Could not read profile inclusion filter: {error}"))?,
                exclusions: serde_json::from_value(entry["rules"]["exclusions"].clone())
                    .map_err(|error| format!("Could not read profile exclusions: {error}"))?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    window.set_profiles(
        Rc::new(slint::VecModel::from(
            profiles
                .iter()
                .map(|profile| VisibilityProfileRow {
                    name: profile.name.as_str().into(),
                    detail: format!(
                        "Inclusion: {} · {} exclusions",
                        if profile.filter.is_some() {
                            "filtered"
                        } else {
                            "all objects"
                        },
                        profile.exclusions.len()
                    )
                    .into(),
                })
                .collect::<Vec<_>>(),
        ))
        .into(),
    );
    window.set_profile_summary(
        format!(
            "{} saved profiles · Save the map to keep profile edits on disk",
            profiles.len()
        )
        .into(),
    );
    if let Some(profile) = profiles
        .iter()
        .find(|profile| profile.name == window.get_selected_profile().as_str())
    {
        window.set_selected_profile_detail(profile_detail(profile).into());
    } else {
        window.set_selected_profile("".into());
    }
    window.set_profiles_ready(true);
    let mut rules = rules.borrow_mut();
    rules.profiles_snapshot = snapshot.into();
    rules.profiles = profiles;
    Ok(())
}

fn refresh_reports(
    window: &WorkingRegionWindow,
    session: &Session,
    rules: &RefCell<RulesState>,
) -> Result<(), String> {
    rules.borrow_mut().observed_snapshot = session.visibility_rules_snapshot();
    let mut errors = Vec::new();
    match refresh_categories(window, session, rules) {
        Ok(()) => window.set_category_message("".into()),
        Err(error) => {
            window.set_category_message(error.as_str().into());
            errors.push(error);
        }
    }
    match refresh_profiles(window, session, rules) {
        Ok(()) => window.set_profile_message("".into()),
        Err(error) => {
            window.set_profile_message(error.as_str().into());
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn reload(
    window: &WorkingRegionWindow,
    session: &mut Session,
    snapshot: &RefCell<String>,
    rules: &RefCell<RulesState>,
    reset_fields: bool,
    message: &str,
) {
    let result = refresh(window, session, snapshot, rules, reset_fields)
        .and_then(|()| refresh_reports(window, session, rules));
    window.set_message(match result {
        Ok(()) => message.into(),
        Err(error) => format!("{message}\n{error}").into(),
    });
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

impl RuleBinding {
    fn mutate(
        &self,
        window: &WorkingRegionWindow,
        session: &mut Session,
        action: RuleAction,
    ) -> Result<&'static str, String> {
        match action {
            RuleAction::Category(id, excluded) => {
                let stamp = self.rules.borrow().snapshot.clone();
                session.set_visibility_category(Actor::User, &id, excluded, &stamp)?;
                Ok(if excluded {
                    "Category exclusion added."
                } else {
                    "Category exclusion removed. Other visibility rules still apply."
                })
            }
            RuleAction::ExcludeMatches => {
                let filter =
                    filter(window)?.ok_or("Enable object filter and enter its criteria first")?;
                let stamp = self.rules.borrow().exclusions_snapshot.clone();
                session.exclude_visibility_filter(Actor::User, filter, &stamp)?;
                Ok("Matches excluded. The inclusion filter was cleared.")
            }
            RuleAction::RemoveExclusion(index) => {
                let index = usize::try_from(index).map_err(|_| "Choose an active exclusion")?;
                let stamp = self.rules.borrow().exclusions_snapshot.clone();
                session.remove_visibility_exclusion(Actor::User, index, &stamp)?;
                Ok("Exclusion removed. Other visibility rules still apply.")
            }
            action => {
                let selected = window.get_selected_profile().to_string();
                let name = window.get_profile_name().to_string();
                let (action, message) = match action {
                    RuleAction::SaveProfile(replace) => (
                        ProfileAction::Save {
                            name: if replace { selected } else { name },
                            replace,
                        },
                        if replace {
                            "Profile replaced with the applied rules."
                        } else {
                            "Applied rules saved as a profile."
                        },
                    ),
                    RuleAction::RenameProfile => (
                        ProfileAction::Rename {
                            name: selected,
                            new_name: name,
                        },
                        "Profile renamed.",
                    ),
                    RuleAction::DeleteProfile => (
                        ProfileAction::Delete { name: selected },
                        "Profile deleted. Active rules are unchanged.",
                    ),
                    RuleAction::ApplyProfile => (
                        ProfileAction::Apply { name: selected },
                        "Profile rules applied. Spatial settings are unchanged.",
                    ),
                    _ => unreachable!(),
                };
                let next_selection = match &action {
                    ProfileAction::Save { name, .. } => Some(name.clone()),
                    ProfileAction::Rename { new_name, .. } => Some(new_name.clone()),
                    _ => None,
                };
                let stamp = self.rules.borrow().profiles_snapshot.clone();
                session.change_visibility_profile(Actor::User, action, &stamp)?;
                if let Some(name) = next_selection {
                    window.set_selected_profile(name.into());
                }
                Ok(message)
            }
        }
    }

    fn perform(&self, action: RuleAction) {
        callback(&self.window, &self.target, |window, session| {
            let load_inclusion = matches!(
                action,
                RuleAction::ExcludeMatches | RuleAction::ApplyProfile
            );
            match self.mutate(&window, session, action) {
                Ok(message) => {
                    reload(
                        &window,
                        session,
                        &self.snapshot,
                        &self.rules,
                        false,
                        message,
                    );
                    if load_inclusion {
                        load_filter(
                            &window,
                            &session.working_region_summary()["options"]["filter"],
                        );
                    }
                }
                Err(error) => window.set_message(error.into()),
            }
        });
    }

    fn categories(&self, direction: i32) {
        callback(&self.window, &self.target, |window, session| {
            {
                let mut rules = self.rules.borrow_mut();
                rules.category_offset = if direction < 0 {
                    rules.category_offset.saturating_sub(CATEGORY_PAGE_SIZE)
                } else if direction > 0 {
                    let Some(offset) = rules.category_next else {
                        window.set_message("No next category page.".into());
                        return;
                    };
                    offset
                } else {
                    0
                };
            }
            match refresh_categories(&window, session, &self.rules) {
                Ok(()) => {
                    window.set_category_message("".into());
                    window.set_message("Category list refreshed.".into());
                }
                Err(error) => {
                    window.set_category_message(error.as_str().into());
                    window.set_message(error.into());
                }
            }
        });
    }

    fn choose_profile(&self, name: slint::SharedString) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let rules = self.rules.borrow();
        if let Some(profile) = rules
            .profiles
            .iter()
            .find(|profile| profile.name == name.as_str())
        {
            window.set_selected_profile(name.clone());
            window.set_profile_name(name);
            window.set_selected_profile_detail(profile_detail(profile).into());
        } else {
            window.set_message("Profile is no longer available. Refresh rules.".into());
        }
    }

    fn refresh(&self) {
        callback(&self.window, &self.target, |window, session| {
            reload(
                &window,
                session,
                &self.snapshot,
                &self.rules,
                false,
                "Rules refreshed.",
            );
        });
    }
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = WorkingRegionWindow::new()?;
        let snapshot = Rc::new(RefCell::new(String::new()));
        let rules = Rc::new(RefCell::new(RulesState::default()));
        window.on_apply({
            let (window, target, snapshot, rules) = (
                window.as_weak(),
                access.downgrade(),
                snapshot.clone(),
                rules.clone(),
            );
            move || {
                callback(&window, &target, |window, session| {
                    let result = fields(&window).and_then(|bounds| {
                        session.set_working_region(
                            Actor::User,
                            Action::SetWithOptions {
                                bounds,
                                enabled: window.get_region_enabled(),
                                options: options(&window, &rules.borrow())?,
                            },
                            &snapshot.borrow(),
                        )
                    });
                    match result {
                        Ok(_) => {
                            reload(
                                &window,
                                session,
                                &snapshot,
                                &rules,
                                true,
                                "View updated. Active exclusions were preserved.",
                            );
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
            let (window, target, snapshot, rules) = (
                window.as_weak(),
                access.downgrade(),
                snapshot.clone(),
                rules.clone(),
            );
            move || {
                callback(&window, &target, |window, session| {
                    let stamp = snapshot.borrow().clone();
                    match session.set_working_region(Actor::User, Action::Clear, &stamp) {
                        Ok(_) => {
                            reload(
                                &window,
                                session,
                                &snapshot,
                                &rules,
                                true,
                                "All active filters cleared. Saved profiles were kept.",
                            );
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
        let binding = RuleBinding {
            window: window.as_weak(),
            target: access.downgrade(),
            snapshot: snapshot.clone(),
            rules: rules.clone(),
        };
        window.on_category_query_changed({
            let binding = binding.clone();
            move || binding.categories(0)
        });
        window.on_category_page({
            let binding = binding.clone();
            move |direction| binding.categories(direction)
        });
        window.on_category_exclusion({
            let binding = binding.clone();
            move |id, excluded| binding.perform(RuleAction::Category(id.to_string(), excluded))
        });
        window.on_exclude_matches({
            let binding = binding.clone();
            move || binding.perform(RuleAction::ExcludeMatches)
        });
        window.on_remove_exclusion({
            let binding = binding.clone();
            move |index| binding.perform(RuleAction::RemoveExclusion(index))
        });
        window.on_choose_profile({
            let binding = binding.clone();
            move |name| binding.choose_profile(name)
        });
        window.on_save_profile({
            let binding = binding.clone();
            move |replace| binding.perform(RuleAction::SaveProfile(replace))
        });
        window.on_rename_profile({
            let binding = binding.clone();
            move || binding.perform(RuleAction::RenameProfile)
        });
        window.on_delete_profile({
            let binding = binding.clone();
            move || binding.perform(RuleAction::DeleteProfile)
        });
        window.on_apply_profile({
            let binding = binding.clone();
            move || binding.perform(RuleAction::ApplyProfile)
        });
        window.on_refresh_rules(move || binding.refresh());
        Ok(Self {
            window,
            snapshot,
            rules,
        })
    }

    pub fn show(&self, session: &mut Session) -> Result<(), slint::PlatformError> {
        reload(
            &self.window,
            session,
            &self.snapshot,
            &self.rules,
            true,
            "Apply updates box, distance and inclusion. Clear working region restores otherwise-visible objects.",
        );
        self.window.show()
    }

    pub fn refresh(&self, session: &mut Session) {
        if !self.window.window().is_visible() {
            return;
        }
        let mut messages = Vec::new();
        if *self.snapshot.borrow() != session.working_region_stamp() {
            match refresh(&self.window, session, &self.snapshot, &self.rules, true) {
                Ok(()) => messages
                    .push("Map, selection or view changed. Current filters reloaded.".into()),
                Err(error) => messages.push(error),
            }
        }
        let rules_changed =
            self.rules.borrow().observed_snapshot != session.visibility_rules_snapshot();
        if rules_changed && let Err(error) = refresh_reports(&self.window, session, &self.rules) {
            messages.push(error);
        }
        if !messages.is_empty() {
            self.window.set_message(messages.join("\n").into());
        }
    }
}
