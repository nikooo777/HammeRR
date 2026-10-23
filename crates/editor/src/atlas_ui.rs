use std::cell::RefCell;
use std::rc::Rc;

use editor_core::session::atlas_texturing::{FitMode, FitOptions, FitRegion, Rectangle};
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::runtime::{DocumentAccess, DocumentContinuation, SharedSession, lock};
use crate::session::{Actor, Session};
use crate::{AtlasWindow, face_edit_ui};

pub struct Controller {
    pub window: AtlasWindow,
    target: Rc<RefCell<Option<Target>>>,
}

struct Target {
    context: face_edit_ui::Context,
    atlas: Option<Value>,
    token: Option<u64>,
    reviewed: Option<String>,
}

impl Target {
    fn check(&self, session: &Session) -> Result<(), String> {
        self.context.check(
            session,
            "Map or face selection changed. Cancel and reopen atlas fitting.",
        )
    }

    fn cancel(&mut self, session: &mut Session) {
        if self.token.is_some() && self.token == session.face_preview_token() {
            let _ = session.cancel_owned_face_preview(Actor::User, self.token);
        }
        self.token = None;
        self.reviewed = None;
    }
}

fn fields(window: &AtlasWindow) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}",
        window.get_material(),
        window.get_region_x(),
        window.get_region_y(),
        window.get_region_pixels_wide(),
        window.get_region_pixels_high(),
        window.get_region_padding(),
        window.get_fit_mode(),
        window.get_reference_index()
    )
}

fn rectangle(window: &AtlasWindow) -> Result<Rectangle, String> {
    let values = [
        window.get_region_x(),
        window.get_region_y(),
        window.get_region_pixels_wide(),
        window.get_region_pixels_high(),
    ];
    let values: Result<Vec<u32>, _> = values
        .iter()
        .map(|value| value.trim().parse::<u32>())
        .collect();
    let values = values.map_err(|_| "Enter nonnegative whole-pixel rectangle values")?;
    Ok(Rectangle {
        x: values[0],
        y: values[1],
        width: values[2],
        height: values[3],
    })
}

fn region(window: &AtlasWindow) -> Result<FitRegion, String> {
    Ok(FitRegion {
        rectangle: rectangle(window)?,
        padding: window
            .get_region_padding()
            .trim()
            .parse()
            .map_err(|_| "Enter nonnegative whole-pixel padding")?,
    })
}

fn options(window: &AtlasWindow, target: &Target) -> Result<FitOptions, String> {
    let mode = match window.get_fit_mode() {
        0 => FitMode::PerFace,
        1 => FitMode::Together {
            reference: usize::try_from(window.get_reference_index())
                .ok()
                .and_then(|index| target.context.faces.get(index))
                .copied()
                .ok_or("Choose a reference face from the target selection")?,
        },
        _ => return Err("Choose an atlas fit mode".into()),
    };
    Ok(FitOptions {
        region: region(window)?,
        mode,
    })
}

fn show_rectangle(window: &AtlasWindow) {
    let size = [
        window.get_texture_width() as u32,
        window.get_texture_height() as u32,
    ];
    if let Ok(bounds) = rectangle(window)
        .and_then(|rectangle| rectangle.uv_bounds(size).map_err(|error| error.to_string()))
    {
        window.set_region_u(bounds[0][0] as f32);
        window.set_region_v(bounds[0][1] as f32);
        window.set_region_width((bounds[1][0] - bounds[0][0]) as f32);
        window.set_region_height((bounds[1][1] - bounds[0][1]) as f32);
        window.set_valid_region(true);
    } else {
        window.set_valid_region(false);
    }
    match region(window).and_then(|region| {
        region
            .usable_rectangle(size)
            .map(|usable| (region.padding, usable))
            .map_err(|error| error.to_string())
    }) {
        Ok((padding, usable)) => {
            let bounds = usable.uv_bounds(size).expect("validated usable rectangle");
            window.set_usable_u(bounds[0][0] as f32);
            window.set_usable_v(bounds[0][1] as f32);
            window.set_usable_width((bounds[1][0] - bounds[0][0]) as f32);
            window.set_usable_height((bounds[1][1] - bounds[0][1]) as f32);
            window.set_usable_details(
                format!(
                    "Usable area · source image pixels\nX {} · Y {} · {} × {}",
                    usable.x, usable.y, usable.width, usable.height
                )
                .into(),
            );
            window.set_has_padding(padding != 0);
            window.set_valid_usable_region(true);
        }
        Err(error) => {
            window.set_valid_usable_region(false);
            window.set_usable_details(error.into());
        }
    }
}

fn inspect(window: &AtlasWindow, target: &mut Target, session: &mut Session) -> Result<(), String> {
    target.check(session)?;
    let atlas = session.inspect_material_atlas(&window.get_material())?;
    let width = atlas.report["width"].as_u64().unwrap() as u32;
    let height = atlas.report["height"].as_u64().unwrap() as u32;
    window.set_material(atlas.report["material"].as_str().unwrap().into());
    window.set_texture_width(width as f32);
    window.set_texture_height(height as f32);
    window.set_preview(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
        slint::Rgba8Pixel,
    >::clone_from_slice(
        &atlas.image.pixels,
        atlas.image.width,
        atlas.image.height,
    )));
    window.set_loaded(true);
    window.set_ready(false);
    if !rectangle(window).is_ok_and(|rectangle| rectangle.validate([width, height]).is_ok())
        || target.atlas.is_none()
    {
        window.set_region_x("0".into());
        window.set_region_y("0".into());
        window.set_region_pixels_wide(width.to_string().into());
        window.set_region_pixels_high(height.to_string().into());
        window.set_region_padding("0".into());
    }
    window.set_details(
        format!(
            "{} · {} × {} pixels\n{}",
            atlas.report["material"].as_str().unwrap(),
            width,
            height,
            atlas.report["texture"].as_str().unwrap()
        )
        .into(),
    );
    target.atlas = Some(atlas.report);
    target.reviewed = None;
    show_rectangle(window);
    Ok(())
}

fn preview(window: &AtlasWindow, target: &mut Target, session: &mut Session) -> Result<(), String> {
    target.check(session)?;
    let atlas = target.atlas.as_ref().ok_or("Inspect the material first")?;
    let state = session.preview_atlas_fit_options(
        Actor::User,
        &target.context.faces,
        &window.get_material(),
        options(window, target)?,
        atlas["snapshot"].as_str().unwrap(),
        target.token,
    )?;
    target.token = session.face_preview_token();
    target.reviewed = Some(fields(window));
    window.set_ready(true);
    window.set_message(
        format!(
            "Preview fits usable UV {}. Apply to keep exactly this fit in one Undo step.",
            state["preview"]["uv_bounds"]
        )
        .into(),
    );
    Ok(())
}

fn apply(window: &AtlasWindow, target: &mut Target, session: &mut Session) -> Result<(), String> {
    target.check(session)?;
    if target.reviewed.as_deref() != Some(fields(window).as_str()) || !window.get_ready() {
        return Err("Fields changed. Preview atlas fit before applying.".into());
    }
    session.check_face_preview_token(target.token)?;
    session.commit_face_preview(Actor::User)?;
    target.token = None;
    target.reviewed = None;
    Ok(())
}

fn with_window<R>(
    window: &slint::Weak<AtlasWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&AtlasWindow, &SharedSession) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    let result = access
        .upgrade()
        .and_then(|access| access.with_shared(|shared| Ok(body(&window, shared))));
    match result {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_message(error.into());
            None
        }
    }
}

impl Controller {
    #[cfg(test)]
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        Self::new_with_access(&DocumentAccess::unmanaged(shared.clone()))
    }

    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = AtlasWindow::new()?;
        let target = Rc::new(RefCell::new(None::<Target>));
        window.on_inspect_material({
            let (window, target, access) = (window.as_weak(), target.clone(), access.downgrade());
            move || {
                with_window(&window, &access, |window, shared| {
                    if let Some(target) = target.borrow_mut().as_mut() {
                        let result = inspect(window, target, &mut lock(shared));
                        window.set_message(match result {
                            Ok(()) => "Enter a rectangle, then Preview atlas fit.".into(),
                            Err(error) => {
                                window.set_loaded(false);
                                window.set_ready(false);
                                window.set_preview(slint::Image::default());
                                error.into()
                            }
                        });
                    }
                });
            }
        });
        window.on_preview_fit({
            let (window, target, access) = (window.as_weak(), target.clone(), access.downgrade());
            move || {
                with_window(&window, &access, |window, shared| {
                    if let Some(target) = target.borrow_mut().as_mut() {
                        let result = preview(window, target, &mut lock(shared));
                        if let Err(error) = result {
                            window.set_ready(false);
                            window.set_message(error.into());
                        }
                    }
                });
            }
        });
        window.on_apply_fit({
            let (window, target, access) = (window.as_weak(), target.clone(), access.downgrade());
            move || {
                with_window(&window, &access, |window, shared| {
                    let result = target
                        .borrow_mut()
                        .as_mut()
                        .map(|target| apply(window, target, &mut lock(shared)));
                    match result {
                        Some(Ok(())) => {
                            let _ = window.hide();
                        }
                        Some(Err(error)) => {
                            window.set_ready(false);
                            window.set_message(error.into());
                        }
                        None => {}
                    }
                });
            }
        });
        window.on_edited({
            let (window, target, access) = (window.as_weak(), target.clone(), access.downgrade());
            move || {
                with_window(&window, &access, |window, _| {
                    window.set_ready(false);
                    let same_material = target
                        .borrow()
                        .as_ref()
                        .and_then(|target| target.atlas.as_ref())
                        .is_some_and(|atlas| {
                            atlas["material"].as_str() == Some(window.get_material().as_str())
                        });
                    if !same_material {
                        window.set_loaded(false);
                        window.set_preview(slint::Image::default());
                    }
                    show_rectangle(window);
                    window.set_message(
                        "Fields changed. Preview atlas fit again before applying.".into(),
                    );
                });
            }
        });
        window.on_cancel({
            let (window, target, access) = (window.as_weak(), target.clone(), access.downgrade());
            move || {
                let Some(window) = window.upgrade() else {
                    return;
                };
                with_window(&window.as_weak(), &access, |_, shared| {
                    if let Some(target) = target.borrow_mut().as_mut() {
                        target.cancel(&mut lock(shared));
                    }
                });
                let _ = window.hide();
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_cancel();
                }
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self { window, target })
    }

    #[cfg(test)]
    pub fn show(&self, session: &mut Session, material: &str) -> Result<(), String> {
        self.initialize(session, material)?;
        self.window.show().map_err(|error| error.to_string())
    }

    pub(crate) fn initialize(&self, session: &mut Session, material: &str) -> Result<(), String> {
        let faces = session.selected_faces();
        if faces.is_empty() {
            return Err("Select flat brush faces before fitting an atlas region".into());
        }
        if session.has_face_preview() {
            return Err("Finish the active face preview before fitting an atlas region".into());
        }
        let mut target = Target {
            context: face_edit_ui::Context::capture(session, faces),
            atlas: None,
            token: None,
            reviewed: None,
        };
        self.window.set_selection_label(
            format!("Fit {} selected flat faces", target.context.faces.len()).into(),
        );
        self.window.set_fit_mode(0);
        self.window.set_reference_index(0);
        self.window.set_references(ModelRc::new(VecModel::from(
            target
                .context
                .faces
                .iter()
                .map(|face| format!("Brush {} · face {}", face.solid, face.side).into())
                .collect::<Vec<_>>(),
        )));
        self.window.set_material(material.into());
        self.window.set_loaded(false);
        self.window.set_ready(false);
        self.window.set_stale(false);
        self.window.set_preview(slint::Image::default());
        self.window.set_details(
            "Choose an existing material atlas and inspect its actual base image.".into(),
        );
        self.window
            .set_message(match inspect(&self.window, &mut target, session) {
                Ok(()) => "Enter a rectangle, then Preview atlas fit.".into(),
                Err(error) => error.into(),
            });
        *self.target.borrow_mut() = Some(target);
        Ok(())
    }

    pub fn choose_material(&self, session: &mut Session, material: &str) {
        self.window.set_material(material.into());
        if let Some(target) = self.target.borrow_mut().as_mut() {
            let result = inspect(&self.window, target, session);
            self.window.set_ready(false);
            self.window.set_message(match result {
                Ok(()) => "Enter a rectangle, then Preview atlas fit.".into(),
                Err(error) => {
                    self.window.set_loaded(false);
                    self.window.set_preview(slint::Image::default());
                    error.into()
                }
            });
        }
    }

    pub fn refresh(&self, session: &Session) {
        if let Some(target) = self.target.borrow().as_ref() {
            let epoch = session.build_content_epoch().to_string();
            let assets_changed = target
                .atlas
                .as_ref()
                .is_some_and(|atlas| atlas["content_epoch"].as_str() != Some(epoch.as_str()));
            let invalid = target.check(session).err().or_else(|| {
                if target.token.is_some() && target.token != session.face_preview_token() {
                    Some(
                        "Atlas preview was closed or replaced. Cancel and reopen atlas fitting."
                            .into(),
                    )
                } else if assets_changed {
                    Some(
                        "Material assets changed. Inspect the material again before previewing."
                            .into(),
                    )
                } else {
                    None
                }
            });
            self.window.set_stale(target.check(session).is_err());
            if let Some(error) = invalid {
                self.window.set_ready(false);
                if assets_changed {
                    self.window.set_loaded(false);
                    self.window.set_preview(slint::Image::default());
                }
                self.window.set_message(error.into());
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod admission_tests;
