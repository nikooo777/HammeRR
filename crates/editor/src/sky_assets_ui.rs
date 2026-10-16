use std::cell::RefCell;
use std::rc::Rc;

use editor_core::sky_assets::{SkyAssetSource, SkyAssetsReport, SkyChannelReport};
use slint::ComponentHandle;

use crate::runtime::{SharedSession, lock};
use crate::session::Session;
use crate::{PropertiesWindow, SkyAssetsWindow};

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

fn channel_text(label: &str, channel: &SkyChannelReport) -> String {
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
        text.push_str(&format!("\n  {}", source_text(texture.source.as_ref())));
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
    let faces = report
        .faces
        .iter()
        .map(|face| {
            let mut text = format!(
                "{} ({}) · {}\nShader: {}{}\n{}",
                match face.suffix.as_str() {
                    "rt" => "Right",
                    "bk" => "Back",
                    "lf" => "Left",
                    "ft" => "Front",
                    "up" => "Up",
                    "dn" => "Down",
                    _ => "Face",
                },
                face.suffix,
                face.material,
                face.shader.as_deref().unwrap_or("unresolved"),
                if face.shader_truncated {
                    " [truncated]"
                } else {
                    ""
                },
                source_text(face.source.as_ref())
            );
            for material in &face.material_sources {
                text.push_str(&format!(
                    "\nMaterial source: {} · {} bytes\n{}",
                    material.path,
                    material.bytes,
                    source_text(Some(&material.source))
                ));
            }
            for issue in &face.issues {
                text.push_str(&format!("\nIssue: {issue}"));
            }
            text.push_str(&format!(
                "\n{}\n{}",
                channel_text("LDR", &face.ldr),
                channel_text("HDR", &face.hdr)
            ));
            text
        })
        .collect::<Vec<_>>()
        .join("\n\n────────────────────────────────────────\n\n");
    window.set_report(format!(
        "{}\n\n{}\n{} assets read · {} bytes read\nLimits: {} asset lookups · {} total bytes · {} bytes per material · {} pixels per texture axis",
        if faces.is_empty() {
            "No sky faces inspected. Set a valid mounted family in World settings and apply it, then refresh."
        } else {
            &faces
        },
        report.readiness_scope,
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
