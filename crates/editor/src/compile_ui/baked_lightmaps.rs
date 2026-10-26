use std::cell::RefCell;
use std::rc::Rc;

use editor_core::session::baked_lightmaps::{Channel, Inspection, Query, SourceReference};
use serde_json::Value;
use slint::ComponentHandle;

use super::{CompileWindow, DocumentAccess, DocumentContinuation, Session, SharedSession, lock};
use crate::BakedLightmapsWindow;

pub(super) struct Form {
    window: BakedLightmapsWindow,
    baseline: Rc<RefCell<Option<Value>>>,
    lists: Rc<RefCell<Lists>>,
}

#[derive(Default)]
struct Lists {
    origins: Vec<Option<SourceReference>>,
    matches: Vec<usize>,
    source: Option<SourceReference>,
    match_summary: String,
}

fn admitted(
    window: &slint::Weak<BakedLightmapsWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(BakedLightmapsWindow, &SharedSession),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = target.upgrade().and_then(|target| {
        target.with_shared(|shared| {
            body(window.clone_strong(), shared);
            Ok(())
        })
    });
    if let Err(error) = result {
        window.set_diagnostics(error.into());
    }
}

fn clear_lists(window: &BakedLightmapsWindow, lists: &RefCell<Lists>) {
    *lists.borrow_mut() = Lists::default();
    window.set_origin_rows(slint::ModelRc::default());
    window.set_origin_details("".into());
    window.set_origin_selectable(slint::ModelRc::default());
    window.set_match_rows(slint::ModelRc::default());
    window.set_has_source(false);
}

fn source_reference(value: &Value) -> Option<SourceReference> {
    let solid = u32::try_from(value["solid"].as_u64()?).ok()?;
    let side = u32::try_from(value["side"].as_u64()?).ok()?;
    match value["kind"].as_str()? {
        "root" => Some(SourceReference::Root { solid, side }),
        "instance" => Some(SourceReference::Instance {
            solid,
            side,
            instance: u32::try_from(value["instance"].as_u64()?).ok()?,
            file: value["file"].as_str()?.into(),
            occurrence: match value.get("occurrence") {
                Some(route) => Some(serde_json::from_value(route.clone()).ok()?),
                None => None,
            },
        }),
        _ => None,
    }
}

fn present_mapping(window: &BakedLightmapsWindow, mapping: &Value, lists: &RefCell<Lists>) {
    if mapping["status"] != "available" {
        clear_lists(window, lists);
        window.set_mapping_status("Authored mapping unavailable; exact compiled-face inspection remains available when the channel has samples.".into());
        return;
    }
    let mut state = lists.borrow_mut();
    let contributors: Vec<_> = mapping["contributors"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    state.origins = contributors
        .iter()
        .map(|value| source_reference(value))
        .collect();
    let labels: Vec<slint::SharedString> = contributors
        .iter()
        .map(|origin| {
            match origin["kind"].as_str() {
                Some("root") => format!(
                    "Root entity {} · solid {} / side {} — find compiled matches",
                    origin["entity"], origin["solid"], origin["side"]
                ),
                Some("instance") => format!(
                    "External instance {} · {} · solid {} / side {} — find exact occurrence matches",
                    origin["instance"],
                    origin["file"].as_str().unwrap_or_default(),
                    origin["solid"],
                    origin["side"]
                ),
                Some("generated_region") => format!(
                    "Generated compile-region shell · solid {} / side {}",
                    origin["solid"], origin["side"]
                ),
                _ => "Unresolved retained contributor".into(),
            }
            .into()
        })
        .collect();
    window.set_origin_rows(slint::ModelRc::new(slint::VecModel::from(labels)));
    window.set_origin_selectable(slint::ModelRc::new(slint::VecModel::from(
        state
            .origins
            .iter()
            .map(Option::is_some)
            .collect::<Vec<_>>(),
    )));
    if mapping["source"].is_object() {
        state.source = serde_json::from_value(mapping["source"].clone()).ok();
        state.matches = mapping["matches"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row["face"].as_u64().map(|face| face as usize))
            .collect();
        window.set_match_rows(slint::ModelRc::new(slint::VecModel::from(
            state
                .matches
                .iter()
                .map(|face| format!("Compiled face {face} — inspect face").into())
                .collect::<Vec<slint::SharedString>>(),
        )));
        window.set_has_source(state.source.is_some());
        let label = match state.source.as_ref() {
            Some(SourceReference::Root { solid, side }) => format!("root {solid}/{side}"),
            Some(SourceReference::Instance {
                instance,
                file,
                solid,
                side,
                occurrence,
            }) => format!(
                "external {instance} · {file} · {solid}/{side}{}",
                if occurrence.is_some() {
                    " · exact captured occurrence"
                } else {
                    " · all matching occurrences"
                }
            ),
            None => "source".into(),
        };
        state.match_summary = format!(
            "Matches for {label}: {} (offset {})",
            mapping["match_page"]["total"], mapping["match_page"]["offset"]
        );
    }
    let route = mapping["source_occurrence_label"].as_str().or_else(|| {
        let [origin] = contributors.as_slice() else {
            return None;
        };
        origin["occurrence_label"].as_str()
    });
    window.set_origin_details(
        route
            .map(|route| format!("Captured occurrence: {route}"))
            .unwrap_or_default()
            .into(),
    );
    let model = mapping["model"]
        .as_u64()
        .map_or_else(|| "none".into(), |model| model.to_string());
    let displacement = mapping["displacement"].as_i64().map_or_else(
        || "ordinary face".into(),
        |index| format!("displacement {index}"),
    );
    window.set_mapping_status(format!("Authored mapping available · compiled model {model} · {displacement}\nContributors: {} (offset {}) · {}",mapping["contributor_page"]["total"].as_u64().unwrap_or_default(),mapping["contributor_page"]["offset"].as_u64().unwrap_or_default(),state.match_summary).into());
}

fn state_text(state: &Value) -> String {
    format!(
        "Build {} · {} · {}\n{}{}",
        state["token"].as_str().unwrap_or("none"),
        state["status"].as_str().unwrap_or("unavailable"),
        state["lighting"].as_str().unwrap_or("no build"),
        state["bsp"]
            .as_str()
            .unwrap_or("Compile successfully in this session first"),
        if state["render_context"]["pending_preview"] == true {
            "\nDisplayed geometry/face draft was not baked"
        } else {
            ""
        }
    )
}

fn rgb(value: &Value) -> String {
    value
        .as_array()
        .map(|channels| {
            channels
                .iter()
                .map(|channel| format!("{:.3}", channel.as_f64().unwrap_or_default()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn present(
    window: &BakedLightmapsWindow,
    inspection: Inspection,
    baseline: &RefCell<Option<Value>>,
    lists: &RefCell<Lists>,
) {
    let report = inspection.report;
    if baseline.borrow().as_ref() != Some(&report["build"]) {
        clear_lists(window, lists);
    }
    *baseline.borrow_mut() = Some(report["build"].clone());
    present_mapping(window, &report["mapping"], lists);
    window.set_build_state(state_text(&report["build"]).into());
    window.set_channels(
        report["channels"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|channel| {
                format!(
                    "{}: {} · {} faces · {} lit · {} bytes",
                    channel["channel"]
                        .as_str()
                        .unwrap_or_default()
                        .to_uppercase(),
                    if channel["available"] == true {
                        "available"
                    } else {
                        "unavailable"
                    },
                    channel["face_count"],
                    channel["lit_face_count"],
                    channel["lighting_bytes"]
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
            .into(),
    );
    let selected = &report["selected"];
    if let Some(face) = selected["face"].as_u64() {
        window.set_compiled_face(face as i32);
    }
    window.set_selected(if selected["linear_rgb"].is_object() {
        let linear = &selected["linear_rgb"];
        format!("Compiled {} face {}\nStyle index {} → style {}\nMap {} ({})\n{} × {} samples · {} bytes\nAll styles: {} · {} maps\n\nLinear RGB intensity\nMin: {}\nMean: {}\nMax: {}\nStored base average: {}",
            report["channel"].as_str().unwrap_or_default().to_uppercase(),selected["face"], selected["style_index"], selected["style"], selected["map_index"], selected["map_kind"].as_str().unwrap_or_default(),
            selected["dimensions"][0],selected["dimensions"][1],selected["selected_sample_bytes"],selected["styles"], selected["maps"],
            rgb(&linear["minimum"]),rgb(&linear["mean"]),rgb(&linear["maximum"]),rgb(&linear["stored_base_average"]))
    } else { "No valid samples for the selected face/style/map. Review diagnostics and available face indices below.".into() }.into());
    let page = &report["page"];
    let mut faces = format!(
        "Compiled faces: offset {} · total {}\n",
        page["offset"], page["total"]
    );
    for face in report["faces"].as_array().into_iter().flatten() {
        faces.push_str(&format!(
            "Face {} · {} · {} · styles {} · maps {}\n",
            face["face"],
            face["status"].as_str().unwrap_or_default(),
            face["dimensions"],
            face["styles"],
            face["maps"]
        ));
    }
    window.set_faces(faces.into());
    let mut diagnostics = Vec::new();
    for entry in [
        &report["build"],
        &report["build"]["render_context"],
        &report,
        &report["mapping"],
        selected,
    ] {
        diagnostics.extend(
            entry["diagnostics"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    }
    for channel in report["channels"].as_array().into_iter().flatten() {
        diagnostics.extend(
            channel["diagnostics"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|issue| {
                    format!(
                        "{}: {issue}",
                        channel["channel"]
                            .as_str()
                            .unwrap_or_default()
                            .to_uppercase()
                    )
                }),
        );
    }
    window.set_diagnostics(
        if diagnostics.is_empty() {
            "No inspection diagnostics".into()
        } else {
            diagnostics.join("\n")
        }
        .into(),
    );
    window.set_has_preview(inspection.image.is_some());
    window.set_preview(
        inspection
            .image
            .map_or_else(slint::Image::default, |image| {
                slint::Image::from_rgba8(
                    slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                        &image.pixels,
                        image.width,
                        image.height,
                    ),
                )
            }),
    );
}

fn query(window: &BakedLightmapsWindow, automatic_face: bool) -> Query {
    Query {
        channel: if window.get_channel() == "HDR" {
            Channel::Hdr
        } else {
            Channel::Ldr
        },
        face: (!automatic_face).then_some(window.get_compiled_face().max(0) as usize),
        offset: window.get_page_offset().max(0) as usize,
        limit: window.get_page_size() as usize,
        contributor_offset: window.get_contributor_offset().max(0) as usize,
        style_index: window.get_style_index().max(0) as usize,
        map_index: window.get_map_index().max(0) as usize,
        ..Query::default()
    }
}

fn inspect(
    window: &BakedLightmapsWindow,
    session: &Session,
    baseline: &RefCell<Option<Value>>,
    lists: &RefCell<Lists>,
    query: Query,
) {
    match session.inspect_baked_lightmaps(&query) {
        Ok(inspection) => present(window, inspection, baseline, lists),
        Err(error) => {
            *baseline.borrow_mut() = Some(session.baked_lightmap_build_state());
            clear_lists(window, lists);
            window.set_diagnostics(error.into());
            window.set_has_preview(false);
            window.set_preview(slint::Image::default());
        }
    }
}

fn lists_current(
    window: &BakedLightmapsWindow,
    session: &Session,
    baseline: &RefCell<Option<Value>>,
    lists: &RefCell<Lists>,
) -> bool {
    if baseline.borrow().as_ref() == Some(&session.baked_lightmap_build_state()) {
        return true;
    }
    clear_lists(window, lists);
    window.set_has_preview(false);
    window.set_preview(slint::Image::default());
    window.set_diagnostics(
        "Build/source/settings changed. Inspect again before choosing a retained row.".into(),
    );
    false
}

impl Form {
    pub(super) fn hide_window(&self, errors: &mut Vec<String>) {
        crate::ui::hide_window("Baked lightmaps", self.window.window(), errors);
    }

    #[cfg(test)]
    pub(super) fn window(&self) -> &BakedLightmapsWindow {
        &self.window
    }

    pub(super) fn is_visible(&self) -> bool {
        self.window.window().is_visible()
    }

    pub(super) fn refresh(&self, session: &Session) {
        if !self.window.window().is_visible() {
            return;
        }
        let current = session.baked_lightmap_build_state();
        if self.baseline.borrow().as_ref() != Some(&current) {
            clear_lists(&self.window, &self.lists);
            self.window
                .set_mapping_status("Inspect again to refresh retained source/match lists.".into());
            self.window.set_build_state(state_text(&current).into());
            self.window.set_has_preview(false);
            self.window.set_preview(slint::Image::default());
            self.window.set_selected("Build, source or settings changed. Choose Inspect face to refresh this inspection.".into());
            self.window.set_diagnostics(
                current["diagnostics"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
                    .into(),
            );
            *self.baseline.borrow_mut() = Some(current);
        }
    }
}

pub(super) fn bind(
    parent: &CompileWindow,
    access: &DocumentAccess,
) -> Result<Form, slint::PlatformError> {
    let window = BakedLightmapsWindow::new()?;
    let baseline = Rc::new(RefCell::new(None));
    let lists = Rc::new(RefCell::new(Lists::default()));
    parent.on_inspect_baked_lightmaps({
        let (window, target, baseline, lists) = (
            window.as_weak(),
            access.downgrade(),
            baseline.clone(),
            lists.clone(),
        );
        let parent = parent.as_weak();
        move || {
            super::admitted(&parent, &target, |_, shared| {
                let Some(window) = window.upgrade() else {
                    return;
                };
                inspect(
                    &window,
                    &lock(shared),
                    &baseline,
                    &lists,
                    query(&window, true),
                );
                let _ = window.show();
            });
        }
    });
    window.on_inspect_face({
        let (window, target, baseline, lists) = (
            window.as_weak(),
            access.downgrade(),
            baseline.clone(),
            lists.clone(),
        );
        move || {
            admitted(&window, &target, |window, shared| {
                inspect(
                    &window,
                    &lock(shared),
                    &baseline,
                    &lists,
                    query(&window, false),
                );
            });
        }
    });
    window.on_inspect_selected_authored({
        let (window, target, baseline, lists) = (
            window.as_weak(),
            access.downgrade(),
            baseline.clone(),
            lists.clone(),
        );
        move || {
            admitted(&window, &target, |window, shared| {
                let mut query = query(&window, true);
                query.offset = 0;
                query.contributor_offset = 0;
                query.selected_authored_face = true;
                window.set_match_offset(0);
                window.set_contributor_offset(0);
                inspect(&window, &lock(shared), &baseline, &lists, query);
            });
        }
    });
    window.on_inspect_origin({
        let (window, target, baseline, lists) = (
            window.as_weak(),
            access.downgrade(),
            baseline.clone(),
            lists.clone(),
        );
        move |index| {
            admitted(&window, &target, |window, shared| {
                let session = lock(shared);
                if !lists_current(&window, &session, &baseline, &lists) {
                    return;
                }
                let source = lists
                    .borrow()
                    .origins
                    .get(index.max(0) as usize)
                    .cloned()
                    .flatten();
                if let Some(source) = source {
                    let mut query = query(&window, true);
                    query.source = Some(source);
                    query.offset = 0;
                    window.set_match_offset(0);
                    inspect(&window, &session, &baseline, &lists, query);
                }
            });
        }
    });
    window.on_find_matches({
        let (window, target, baseline, lists) = (
            window.as_weak(),
            access.downgrade(),
            baseline.clone(),
            lists.clone(),
        );
        move || {
            admitted(&window, &target, |window, shared| {
                let session = lock(shared);
                if !lists_current(&window, &session, &baseline, &lists) {
                    return;
                }
                let source = lists.borrow().source.clone();
                if let Some(source) = source {
                    let mut query = query(&window, true);
                    query.source = Some(source);
                    query.offset = window.get_match_offset().max(0) as usize;
                    inspect(&window, &session, &baseline, &lists, query);
                }
            });
        }
    });
    window.on_inspect_match({
        let (window, target, baseline, lists) = (
            window.as_weak(),
            access.downgrade(),
            baseline.clone(),
            lists.clone(),
        );
        move |index| {
            admitted(&window, &target, |window, shared| {
                let session = lock(shared);
                if !lists_current(&window, &session, &baseline, &lists) {
                    return;
                }
                let face = lists.borrow().matches.get(index.max(0) as usize).copied();
                if let Some(face) = face {
                    window.set_compiled_face(face as i32);
                    inspect(&window, &session, &baseline, &lists, query(&window, false));
                }
            });
        }
    });
    window.on_query_changed({
        let window = window.as_weak();
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            window.set_has_preview(false);
            window.set_preview(slint::Image::default());
            window.set_selected(
                "Choose Inspect face to inspect the entered channel, face, style and map indices."
                    .into(),
            );
        }
    });
    window.on_close_dialog({
        let window = window.as_weak();
        move || {
            if let Some(window) = window.upgrade() {
                let _ = window.hide();
            }
        }
    });
    Ok(Form {
        window,
        baseline,
        lists,
    })
}

#[cfg(test)]
mod tests;
