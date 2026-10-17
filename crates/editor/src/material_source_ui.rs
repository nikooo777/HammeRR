use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::runtime::{SharedSession, lock};
use crate::session::{Session, material_source::Report};
use crate::{MaterialSourceWindow, MaterialWindow};

pub struct Controller {
    pub window: MaterialSourceWindow,
    material: Rc<RefCell<String>>,
}

fn present(window: &MaterialSourceWindow, report: Report) {
    window.set_material(format!("{} · {}", report.material, report.vmt_path).into());
    let provenance = report
        .files
        .iter()
        .map(|file| {
            let origin = file.origin.as_ref().map_or_else(
                || "Not found in mounted game content".into(),
                |origin| {
                    let mut text = format!(
                        "{} mount: {}",
                        if origin.kind == "vpk" {
                            "VPK"
                        } else {
                            "Directory"
                        },
                        origin.mount
                    );
                    if let Some(file) = &origin.file {
                        text.push_str(&format!("\nFile: {file}"));
                    }
                    if !origin.storage_files.is_empty() {
                        text.push_str(&format!("\nStorage: {}", origin.storage_files.join(", ")));
                    }
                    if origin.paths_truncated {
                        text.push_str("\nOrigin paths truncated for display");
                    }
                    text
                },
            );
            format!("{} ({} bytes)\n{}", file.path, file.bytes, origin)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    window.set_provenance(
        if provenance.is_empty() {
            "No source file could be read.".into()
        } else {
            provenance
        }
        .into(),
    );
    window.set_resolved(
        report
            .resolved
            .map_or_else(
                || "Material declarations could not be resolved.".into(),
                |resolved| {
                    let textures = resolved
                        .texture_parameters
                        .iter()
                        .map(|texture| {
                            format!(
                                "{} = {}{}",
                                texture.parameter,
                                texture.value,
                                if texture.truncated {
                                    "… [truncated]"
                                } else {
                                    ""
                                }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!(
                        "Resolved shader declaration: {}{}\n{}",
                        resolved.shader,
                        if resolved.shader_truncated {
                            "… [truncated]"
                        } else {
                            ""
                        },
                        if textures.is_empty() {
                            "No recognized texture parameters declared.".into()
                        } else {
                            textures
                        }
                    )
                },
            )
            .into(),
    );
    let mut status = report.error.map_or_else(
        || "Source and Patch declarations resolved.".into(),
        |error| {
            format!(
                "Resolution error: {error}{}",
                if report.error_truncated {
                    "… [truncated]"
                } else {
                    ""
                }
            )
        },
    );
    if report.raw_source_truncated {
        status.push_str(&format!(
            " Showing the first {} source bytes of {}.",
            report.raw_source.as_ref().map_or(0, String::len),
            report.raw_source_bytes
        ));
    }
    window.set_status(status.into());
    window.set_source(
        report
            .raw_source
            .unwrap_or_else(|| "Source text unavailable; see the resolution error.".into())
            .into(),
    );
}

fn refresh(window: &MaterialSourceWindow, session: &Session, name: &str) {
    match session.inspect_material_source(name) {
        Ok(report) => present(window, report),
        Err(error) => {
            window.set_material(name.into());
            window.set_provenance("No source file was read.".into());
            window.set_resolved("Material declarations unavailable.".into());
            window.set_source("".into());
            window.set_status(error.into());
        }
    }
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = MaterialSourceWindow::new()?;
        let material = Rc::new(RefCell::new(String::new()));
        window.on_refresh_source({
            let (window, shared, material) = (window.as_weak(), shared.clone(), material.clone());
            move || refresh(&window.unwrap(), &lock(&shared), &material.borrow())
        });
        window.on_close_dialog({
            let window = window.as_weak();
            move || {
                let _ = window.unwrap().hide();
            }
        });
        Ok(Self { window, material })
    }

    pub fn show(&self, session: &Session, name: &str) -> Result<(), slint::PlatformError> {
        *self.material.borrow_mut() = name.to_owned();
        refresh(&self.window, session, name);
        self.window.show()
    }
}

pub(crate) fn bind(
    window: &MaterialWindow,
    shared: SharedSession,
) -> Rc<RefCell<Option<Controller>>> {
    let controller = Rc::new(RefCell::new(None::<Controller>));
    window.on_inspect_source({
        let (window, controller) = (window.as_weak(), controller.clone());
        move || {
            let window = window.unwrap();
            if controller.borrow().is_none() {
                match Controller::new(&shared) {
                    Ok(source) => *controller.borrow_mut() = Some(source),
                    Err(error) => {
                        window.set_detail(error.to_string().into());
                        return;
                    }
                }
            }
            let material = if window.get_sprite_resource() {
                match editor_core::sprite_resource::browse_reference(&window.get_material()) {
                    Ok(material) => material,
                    Err(error) => {
                        window.set_detail(error.into());
                        return;
                    }
                }
            } else {
                window.get_material().to_string()
            };
            if let Err(error) = controller
                .borrow()
                .as_ref()
                .unwrap()
                .show(&lock(&shared), &material)
            {
                window.set_detail(error.to_string().into());
            }
        }
    });
    controller
}

#[cfg(test)]
mod tests;
