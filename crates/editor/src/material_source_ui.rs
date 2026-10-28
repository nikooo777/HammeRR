use std::cell::RefCell;
use std::rc::Rc;

use compiler::water_dependencies::{
    self, AssetKind, AssetState, DeclarationOrigin, DependencyRole, EdgeState,
    WaterDependencyReport, WaterDependencyState,
};
use slint::ComponentHandle;

use crate::runtime::{DocumentAccess, DocumentContinuation, lock};
use crate::session::{Session, material_source::Report};
use crate::{MaterialSourceWindow, MaterialWindow};

pub struct Controller {
    pub window: MaterialSourceWindow,
    material: Rc<RefCell<String>>,
}

fn water_state(state: WaterDependencyState) -> &'static str {
    match state {
        WaterDependencyState::NotApplicable => "Not applicable",
        WaterDependencyState::SupportedStaticInventory => "Supported static inventory",
        WaterDependencyState::Unsupported => "Unsupported inventory",
        WaterDependencyState::UnavailableSource => "Source unavailable",
        WaterDependencyState::Limited => "Inspection limit reached",
    }
}

fn water_inventory(report: &WaterDependencyReport) -> String {
    let model_basis = match report.model_basis {
        "css_sdk_dx90_static_water" => "CS:S SDK DX90 static Water",
        other => other,
    };
    let mut text = format!(
        "Water dependencies: {} · {} files · {} references · {} issues\nModel basis: {}\nRead: {} materials · {} textures · {:.1} KiB\nLimits: {} materials · {} files · {} references · {} MiB total",
        water_state(report.state),
        report.assets.len(),
        report.edges.len(),
        report.issues.len(),
        model_basis,
        report.counters.material_reads,
        report.counters.texture_reads,
        report.counters.source_bytes as f64 / 1024.0,
        report.limits.material_reads,
        report.limits.assets,
        report.limits.edges,
        report.limits.source_bytes.div_ceil(1024 * 1024),
    );
    for issue in &report.issues {
        text.push_str(&format!(
            "\n\nIssue [{}]: {}{}{}\n{}",
            water_state(issue.state),
            issue.owner,
            issue
                .key
                .as_ref()
                .map_or_else(String::new, |key| format!(" · {key}")),
            issue
                .path
                .as_ref()
                .map_or_else(String::new, |path| format!(" · {path}")),
            issue.message,
        ));
    }
    for edge in &report.edges {
        let role = match edge.role {
            DependencyRole::PatchInclude => "Patch include",
            DependencyRole::NormalTexture => "Normal texture",
            DependencyRole::ColorTexture => "Color texture",
            DependencyRole::CubemapTexture => "Cubemap texture",
            DependencyRole::CompilerMaterial => "Compiler material",
            DependencyRole::RuntimeMaterial => "Runtime material",
            DependencyRole::EditorPreviewTexture => "Editor preview texture",
        };
        let origin = match edge.origin {
            DeclarationOrigin::Authored => "Authored",
            DeclarationOrigin::PatchEffective => "Patch effective",
            DeclarationOrigin::SdkDefault => "SDK default",
        };
        let state = match edge.state {
            EdgeState::Mounted => "Mounted file",
            EdgeState::RuntimeResource => "Runtime resource",
            EdgeState::Disabled => "Disabled",
            EdgeState::Unsupported => "Unsupported",
            EdgeState::Missing => "Missing file",
            EdgeState::Unavailable => "Source unavailable",
        };
        let evidence = match edge.evidence_basis {
            "sdk" => "SDK",
            "installed_client_and_sdk" => "Installed client and SDK",
            other => other,
        };
        text.push_str(&format!(
            "\n\n{}: {} · {} = {}\nTarget: {} · {} · {} · Evidence: {}",
            role,
            edge.owner,
            edge.key,
            edge.reference,
            edge.target.as_deref().unwrap_or("No mounted-file target"),
            origin,
            state,
            evidence,
        ));
    }
    for asset in &report.assets {
        let kind = match asset.kind {
            AssetKind::Material => "VMT",
            AssetKind::Texture => "VTF",
        };
        let state = match asset.state {
            AssetState::Found => "Inspected",
            AssetState::Missing => "Missing",
            AssetState::Unavailable => "Unavailable",
            AssetState::UnsupportedTexture => "Unsupported texture",
        };
        text.push_str(&format!(
            "\n\nFile: {} · {} · {} · {} bytes\nSHA256: {}",
            asset.path,
            kind,
            state,
            asset.bytes,
            asset.sha256.as_deref().unwrap_or("Unavailable"),
        ));
        if let Some(origin) = &asset.origin {
            text.push_str(&format!("\nOrigin: {} · {}", origin.kind, origin.mount));
            if let Some(file) = &origin.file {
                text.push_str(&format!("\nFile: {file}"));
            }
            if !origin.storage_files.is_empty() {
                text.push_str(&format!("\nStorage: {}", origin.storage_files.join(", ")));
            }
            if origin.paths_truncated {
                text.push_str("\nOrigin paths truncated for display");
            }
        }
        if let Some(shader) = &asset.shader {
            text.push_str(&format!("\nShader declaration: {shader}"));
        }
        if let Some(texture) = &asset.texture {
            let depth = texture
                .authored_depth
                .map_or_else(|| "implicit one".into(), |depth| depth.to_string());
            let layout = match texture.layout_scope {
                "existing_vtf_parser" => "existing VTF parser",
                other => other,
            };
            text.push_str(&format!(
                "\nVTF header: {} × {} · {} · version {}.{} · depth {} (authored {}) · authored frames {} · first frame {} · faces {} · mips {} · flags {:#x}\nLayout observation: {}",
                texture.width,
                texture.height,
                texture.format,
                texture.version[0],
                texture.version[1],
                texture.depth,
                depth,
                texture.authored_frames,
                texture.first_frame,
                texture.faces,
                texture.mip_count,
                texture.flags,
                layout,
            ));
        }
    }
    text
}

fn present(window: &MaterialSourceWindow, report: Report) {
    let show_water = report
        .resolved
        .as_ref()
        .is_some_and(|resolved| water_dependencies::is_water_shader(&resolved.shader));
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
    let mut resolved = report.resolved.map_or_else(
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
    );
    if show_water {
        resolved.push_str("\n\n");
        resolved.push_str(&water_inventory(&report.water_dependencies));
    }
    window.set_resolved(resolved.into());
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
    if show_water {
        status.push_str(&format!(
            " Water dependencies: {} ({} issues).",
            water_state(report.water_dependencies.state),
            report.water_dependencies.issues.len(),
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

fn with_source<R>(
    window: &slint::Weak<MaterialSourceWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&MaterialSourceWindow, &Session) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    match access
        .upgrade()
        .and_then(|access| access.with_session(|session| Ok(body(&window, session))))
    {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_status(error.into());
            None
        }
    }
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = MaterialSourceWindow::new()?;
        let material = Rc::new(RefCell::new(String::new()));
        window.on_refresh_source({
            let (window, access, material) =
                (window.as_weak(), access.downgrade(), material.clone());
            move || {
                with_source(&window, &access, |window, session| {
                    refresh(window, session, &material.borrow())
                });
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
        window
            .window()
            .on_close_requested(|| slint::CloseRequestResponse::HideWindow);
        Ok(Self { window, material })
    }

    fn populate(&self, session: &Session, name: &str) {
        *self.material.borrow_mut() = name.to_owned();
        refresh(&self.window, session, name);
    }
}

pub(crate) fn bind_with_access(
    window: &MaterialWindow,
    access: &DocumentAccess,
) -> Rc<RefCell<Option<Controller>>> {
    let controller = Rc::new(RefCell::new(None::<Controller>));
    window.on_inspect_source({
        let (window, controller, access) =
            (window.as_weak(), controller.clone(), access.downgrade());
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            let result = access.upgrade().and_then(|access| {
                access.with_shared(|shared| {
                    let inspected = (|| -> Result<(), String> {
                        if controller.borrow().is_none() {
                            let source = Controller::new_with_access(&access)
                                .map_err(|error| error.to_string())?;
                            *controller.borrow_mut() = Some(source);
                        }
                        let material = if window.get_sprite_resource() {
                            editor_core::sprite_resource::browse_reference(&window.get_material())?
                        } else {
                            window.get_material().to_string()
                        };
                        {
                            let session = lock(shared);
                            controller
                                .borrow()
                                .as_ref()
                                .unwrap()
                                .populate(&session, &material);
                        }
                        controller
                            .borrow()
                            .as_ref()
                            .unwrap()
                            .window
                            .show()
                            .map_err(|error| error.to_string())
                    })();
                    if let Err(error) = inspected {
                        window.set_detail(error.into());
                    }
                    Ok(())
                })
            });
            if let Err(error) = result {
                window.set_detail(error.into());
            }
        }
    });
    controller
}
