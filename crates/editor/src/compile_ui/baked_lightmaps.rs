use std::cell::RefCell;
use std::rc::Rc;

use editor_core::session::baked_lightmaps::{Channel, Inspection, Query};
use serde_json::Value;
use slint::ComponentHandle;

use super::{CompileWindow, Session, SharedSession, lock};
use crate::BakedLightmapsWindow;

pub(super) struct Form {
    window: BakedLightmapsWindow,
    baseline: Rc<RefCell<Option<Value>>>,
}

fn state_text(state: &Value) -> String {
    format!(
        "Build {} · {} · {}\n{}",
        state["token"].as_str().unwrap_or("none"),
        state["status"].as_str().unwrap_or("unavailable"),
        state["lighting"].as_str().unwrap_or("no build"),
        state["bsp"]
            .as_str()
            .unwrap_or("Compile successfully in this session first")
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

fn present(window: &BakedLightmapsWindow, inspection: Inspection) {
    let report = inspection.report;
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
    for entry in [&report["build"], &report, selected] {
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

fn inspect(
    window: &BakedLightmapsWindow,
    session: &Session,
    baseline: &RefCell<Option<Value>>,
    automatic_face: bool,
) {
    let query = Query {
        channel: if window.get_channel() == "HDR" {
            Channel::Hdr
        } else {
            Channel::Ldr
        },
        face: (!automatic_face).then_some(window.get_compiled_face().max(0) as usize),
        offset: window.get_page_offset().max(0) as usize,
        style_index: window.get_style_index().max(0) as usize,
        map_index: window.get_map_index().max(0) as usize,
        ..Query::default()
    };
    match session.inspect_baked_lightmaps(&query) {
        Ok(inspection) => present(window, inspection),
        Err(error) => {
            window.set_diagnostics(error.into());
            window.set_has_preview(false);
            window.set_preview(slint::Image::default());
        }
    }
    *baseline.borrow_mut() = Some(session.baked_lightmap_build_state());
}

impl Form {
    pub(super) fn is_visible(&self) -> bool {
        self.window.window().is_visible()
    }

    pub(super) fn refresh(&self, session: &Session) {
        if !self.window.window().is_visible() {
            return;
        }
        let current = session.baked_lightmap_build_state();
        if self.baseline.borrow().as_ref() != Some(&current) {
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
    shared: &SharedSession,
) -> Result<Form, slint::PlatformError> {
    let window = BakedLightmapsWindow::new()?;
    let baseline = Rc::new(RefCell::new(None));
    parent.on_inspect_baked_lightmaps({
        let (window, shared, baseline) = (window.as_weak(), shared.clone(), baseline.clone());
        move || {
            let window = window.unwrap();
            inspect(&window, &lock(&shared), &baseline, true);
            let _ = window.show();
        }
    });
    window.on_inspect_face({
        let (window, shared, baseline) = (window.as_weak(), shared.clone(), baseline.clone());
        move || inspect(&window.unwrap(), &lock(&shared), &baseline, false)
    });
    window.on_query_changed({
        let window = window.as_weak();
        move || {
            let window = window.unwrap();
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
            let _ = window.unwrap().hide();
        }
    });
    Ok(Form { window, baseline })
}

#[cfg(test)]
mod tests;
