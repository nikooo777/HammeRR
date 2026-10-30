use std::collections::{BTreeMap, BTreeSet};

use slint::{Model, ModelRc, VecModel};

use crate::runtime::DocumentAccess;
use crate::session::{ObjectFilter, ObjectReport, Session};
use crate::{AppWindow, OutlinerRow};

const GROUP_ROWS: usize = 500;
const WORLD: &str = "world";

fn group_row(
    key: &str,
    label: &str,
    members: &[&ObjectReport],
    open: bool,
    selected: &BTreeSet<map_core::ObjectId>,
) -> OutlinerRow {
    OutlinerRow {
        depth: 0,
        key: key.into(),
        label: label.into(),
        detail: members.len().to_string().into(),
        expandable: true,
        expanded: open,
        selected: members
            .iter()
            .any(|member| selected.contains(&member.object)),
        object: false,
    }
}

fn object_row(member: &ObjectReport, selected: &BTreeSet<map_core::ObjectId>) -> OutlinerRow {
    let (label, detail) = if member.targetname.is_empty() {
        (
            member.id.clone(),
            member.materials.first().cloned().unwrap_or_default(),
        )
    } else {
        (member.targetname.clone(), member.id.clone())
    };
    OutlinerRow {
        depth: 1,
        key: member.id.clone().into(),
        label: label.into(),
        detail: detail.into(),
        expandable: false,
        expanded: false,
        selected: selected.contains(&member.object),
        object: true,
    }
}

fn note_row(label: String) -> OutlinerRow {
    OutlinerRow {
        depth: 1,
        key: Default::default(),
        label: label.into(),
        detail: Default::default(),
        expandable: false,
        expanded: false,
        selected: false,
        object: false,
    }
}

fn rebuild(app: &AppWindow, session: &Session, toggle: Option<&str>) {
    let mut expanded: BTreeSet<String> = app
        .get_outliner_rows()
        .iter()
        .filter(|row| row.expandable && row.expanded)
        .map(|row| row.key.to_string())
        .collect();
    if let Some(key) = toggle
        && !expanded.remove(key)
    {
        expanded.insert(key.to_owned());
    }
    let query = app.get_outliner_filter().trim().to_owned();
    let filtering = !query.is_empty();
    let filter = ObjectFilter {
        query: filtering.then_some(query),
        ..ObjectFilter::default()
    };
    let objects = match session.object_report(&filter) {
        Ok(objects) => objects,
        Err(error) => {
            app.set_outliner_rows(ModelRc::new(VecModel::from(vec![note_row(error)])));
            return;
        }
    };
    let selected: BTreeSet<_> = session.selected_ids().into_iter().collect();
    let mut world = Vec::new();
    let mut classes: BTreeMap<&str, Vec<&ObjectReport>> = BTreeMap::new();
    for object in &objects {
        if object.kind == "solid" {
            world.push(object);
        } else {
            classes
                .entry(object.classname.as_str())
                .or_default()
                .push(object);
        }
    }
    let class_keys: Vec<(String, &str)> = classes
        .keys()
        .map(|class| (format!("class:{class}"), *class))
        .collect();
    let groups = std::iter::once((WORLD.to_owned(), "World brushes", world.as_slice()))
        .chain(
            class_keys
                .iter()
                .map(|(key, class)| (key.clone(), *class, classes[class].as_slice())),
        )
        .filter(|(_, _, members)| !members.is_empty());
    let mut rows = Vec::new();
    for (key, label, members) in groups {
        let chosen = expanded.contains(&key);
        rows.push(group_row(&key, label, members, chosen, &selected));
        if filtering || chosen {
            rows.extend(
                members
                    .iter()
                    .take(GROUP_ROWS)
                    .map(|member| object_row(member, &selected)),
            );
            if members.len() > GROUP_ROWS {
                rows.push(note_row(format!(
                    "{} more · refine the filter",
                    members.len() - GROUP_ROWS
                )));
            }
        }
    }
    if rows.is_empty() {
        rows.push(note_row(if filtering {
            "No matching objects".to_owned()
        } else {
            "The map is empty".to_owned()
        }));
    }
    app.set_outliner_rows(ModelRc::new(VecModel::from(rows)));
}

pub(super) fn refresh(app: &AppWindow, session: &Session) {
    if app.get_outliner_visible() {
        rebuild(app, session, None);
    }
}

pub(super) fn bind(app: &AppWindow, access: &DocumentAccess) {
    app.on_outliner_refresh({
        let callback = super::admission::SessionCallback::new(app, access);
        move || callback.input(|app, session| rebuild(&app, session, None))
    });
    app.on_outliner_toggle({
        let callback = super::admission::SessionCallback::new(app, access);
        move |key| callback.input(|app, session| rebuild(&app, session, Some(&key)))
    });
    app.on_outliner_pick({
        let callback = super::admission::SessionCallback::new(app, access);
        move |id| {
            callback.input(|app, session| {
                if let Err(error) = super::palette::pick_object(&app, session, &id) {
                    app.set_message(error.into());
                }
                rebuild(&app, session, None);
            });
        }
    });
}
