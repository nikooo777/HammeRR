use std::cell::RefCell;
use std::rc::Rc;

use editor_core::sky_assets::{SkyAssetSource, SkyAssetsReport, SkyChannelReport};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::runtime::{SharedSession, lock};
use crate::session::Session;
use crate::{PropertiesWindow, SkyAssetFaceRow, SkyAssetsWindow};

pub struct Controller {
    pub window: SkyAssetsWindow,
}

fn source_text(source: Option<&SkyAssetSource>) -> String {
    match source {
        Some(SkyAssetSource::Directory { mount, file }) => {
            format!(
                "Directory mount: {}\nFile: {}",
                mount.display(),
                file.display()
            )
        }
        Some(SkyAssetSource::Vpk { index, files }) => format!(
            "VPK index: {}\nStorage: {}",
            index.display(),
            files
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        None => "No mounted source resolved".into(),
    }
}

fn ready(value: bool) -> &'static str {
    if value { "ready" } else { "unavailable" }
}

fn dependency_source_text(
    source: Option<&SkyAssetSource>,
    root: Option<&SkyAssetSource>,
) -> String {
    match (source, root) {
        (
            Some(SkyAssetSource::Directory { mount, file }),
            Some(SkyAssetSource::Directory { mount: root, .. }),
        ) if mount == root => format!("File: {}", file.display()),
        (
            Some(SkyAssetSource::Vpk { index, files }),
            Some(SkyAssetSource::Vpk { index: root, .. }),
        ) if index == root => format!(
            "Storage: {}",
            files
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => source_text(source),
    }
}

fn channel_text(label: &str, channel: &SkyChannelReport, root: Option<&SkyAssetSource>) -> String {
    let mut text = format!("{label} assets: {} (structure)", ready(channel.ready));
    for issue in &channel.issues {
        text.push_str(&format!("\n  Issue: {issue}"));
    }
    for texture in &channel.textures {
        text.push_str(&format!(
            "\n  {} = {:?}{}\n  {} · {} · {} bytes",
            texture.parameter,
            texture.reference,
            if texture.reference_truncated {
                " [truncated]"
            } else {
                ""
            },
            texture.path,
            ready(texture.ready),
            texture.bytes
        ));
        if let Some([width, height]) = texture.size {
            text.push_str(&format!(" · {width} × {height}"));
        }
        if let Some(format) = &texture.format {
            text.push_str(&format!(" · {format}"));
        }
        text.push_str(&format!(
            "\n  {}",
            dependency_source_text(texture.source.as_ref(), root)
        ));
        for issue in &texture.issues {
            text.push_str(&format!("\n  Issue: {issue}"));
        }
    }
    text
}

fn present(window: &SkyAssetsWindow, report: SkyAssetsReport) {
    window.set_family(
        report
            .authored_name
            .as_ref()
            .map_or_else(
                || "Applied skyname: absent".into(),
                |name| {
                    format!(
                        "Applied skyname: {name:?}{}",
                        if report.authored_name_truncated {
                            " [truncated]"
                        } else {
                            ""
                        }
                    )
                },
            )
            .into(),
    );
    window.set_summary(
        format!(
            "LDR assets: {} · HDR assets: {} (structure only)",
            ready(report.ldr_ready),
            ready(report.hdr_ready)
        )
        .into(),
    );
    let mut issues = report.issues.join("\n");
    if report.report_truncated {
        issues.push_str("\nReport text or diagnostics truncated by inspection limits.");
    }
    window.set_issues(issues.into());
    window.set_scope(report.readiness_scope.into());
    let faces = report
        .faces
        .iter()
        .map(|face| {
            let label = format!(
                "{} ({})",
                match face.suffix.as_str() {
                    "rt" => "Right",
                    "bk" => "Back",
                    "lf" => "Left",
                    "ft" => "Front",
                    "up" => "Up",
                    "dn" => "Down",
                    _ => "Face",
                },
                face.suffix
            );
            let mut text = format!(
                "Material: {}\nShader: {}{}\n{}",
                face.material,
                face.shader.as_deref().unwrap_or("unresolved"),
                if face.shader_truncated {
                    " [truncated]"
                } else {
                    ""
                },
                source_text(face.source.as_ref())
            );
            for material in face.material_sources.iter().skip(1) {
                text.push_str(&format!(
                    "\nPatch source: {} · {} bytes\n{}",
                    material.path,
                    material.bytes,
                    dependency_source_text(Some(&material.source), face.source.as_ref())
                ));
            }
            for issue in &face.issues {
                text.push_str(&format!("\nIssue: {issue}"));
            }
            text.push_str(&format!(
                "\n\n{}\n\n{}",
                channel_text("LDR", &face.ldr, face.source.as_ref()),
                channel_text("HDR", &face.hdr, face.source.as_ref())
            ));
            SkyAssetFaceRow {
                label: label.into(),
                ldr_status: ready(face.ldr.ready).into(),
                hdr_status: ready(face.hdr.ready).into(),
                detail: text.into(),
            }
        })
        .collect::<Vec<_>>();
    if window.get_selected_face() < 0 || window.get_selected_face() as usize >= faces.len() {
        window.set_selected_face(0);
    }
    window.set_faces(ModelRc::new(VecModel::from(faces)));
    window.set_read_summary(format!(
        "{} assets read · {} bytes · Limits: {} asset lookups, {} total bytes, {} bytes/material, {} pixels/axis",
        report.assets_read,
        report.bytes_read,
        report.limits.asset_lookups,
        report.limits.total_bytes,
        report.limits.material_bytes,
        report.limits.texture_axis
    ).into());
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = SkyAssetsWindow::new()?;
        window.on_refresh_assets({
            let (window, shared) = (window.as_weak(), shared.clone());
            move || present(&window.unwrap(), lock(&shared).inspect_sky_assets())
        });
        window.on_close_dialog({
            let window = window.as_weak();
            move || {
                let _ = window.unwrap().hide();
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                window.unwrap().invoke_close_dialog();
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self { window })
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        present(&self.window, session.inspect_sky_assets());
        self.window.show()
    }
}

pub(crate) fn bind(
    parent: &PropertiesWindow,
    shared: SharedSession,
) -> Rc<RefCell<Option<Controller>>> {
    let controller = Rc::new(RefCell::new(None::<Controller>));
    parent.on_inspect_sky_assets({
        let (parent, controller) = (parent.as_weak(), controller.clone());
        move || {
            if !parent.unwrap().get_world_settings() {
                return;
            }
            if controller.borrow().is_none() {
                match Controller::new(&shared) {
                    Ok(window) => *controller.borrow_mut() = Some(window),
                    Err(error) => {
                        parent.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            if let Err(error) = controller.borrow().as_ref().unwrap().show(&lock(&shared)) {
                parent.unwrap().set_message(error.to_string().into());
            }
        }
    });
    controller
}

#[cfg(test)]
mod tests;
