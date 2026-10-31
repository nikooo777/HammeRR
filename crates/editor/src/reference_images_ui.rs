use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use editor_core::session::reference_images::Settings;
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::ReferenceImagesWindow;
use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::{Actor, Session};

#[derive(Default)]
struct Data {
    snapshot: String,
    images: Vec<Value>,
}
pub struct Controller {
    pub window: ReferenceImagesWindow,
    data: Rc<RefCell<Data>>,
}

fn selected(window: &ReferenceImagesWindow, data: &Data) -> Result<u32, String> {
    usize::try_from(window.get_image_index())
        .ok()
        .and_then(|index| data.images.get(index))
        .and_then(|image| image["id"].as_u64())
        .map(|id| id as u32)
        .ok_or_else(|| "Choose a reference image".into())
}

fn choose(window: &ReferenceImagesWindow, data: &Data) {
    let index = usize::try_from(window.get_image_index()).ok();
    let image = index.and_then(|index| data.images.get(index));
    window.set_selected(image.is_some());
    let Some(image) = image else {
        window.set_source_status("Enter a PNG path and a new name, then Add image.".into());
        return;
    };
    let settings = &image["settings"];
    let vector = |key: &str| {
        settings[key]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    };
    window.set_image_name(image["name"].as_str().unwrap().into());
    window.set_path(image["path"].as_str().unwrap().into());
    window.set_position(vector("position").into());
    window.set_rotation(vector("rotation").into());
    window.set_size(vector("size").into());
    window.set_image_opacity(settings["opacity"].to_string().into());
    window.set_flip_x(settings["flip"][0] == true);
    window.set_flip_y(settings["flip"][1] == true);
    window.set_top(settings["views"][0] == true);
    window.set_front(settings["views"][1] == true);
    window.set_side(settings["views"][2] == true);
    window.set_perspective(settings["views"][3] == true);
    source_status(window, image);
}

fn source_status(window: &ReferenceImagesWindow, image: &Value) {
    window.set_source_status(
        format!(
            "{} × {} pixels · SHA256 {}\n{}",
            image["dimensions"][0],
            image["dimensions"][1],
            image["sha256"].as_str().unwrap(),
            image["error"].as_str().unwrap_or("Source ready")
        )
        .into(),
    );
}

fn refresh(
    window: &ReferenceImagesWindow,
    data: &mut Data,
    session: &mut Session,
    id: Option<u32>,
) {
    let report = session.reference_images_state();
    data.snapshot = report["snapshot"].as_str().unwrap().into();
    data.images = report["images"].as_array().cloned().unwrap_or_default();
    window.set_images(ModelRc::new(VecModel::from(
        data.images
            .iter()
            .map(|image| {
                format!(
                    "{}{}",
                    image["name"].as_str().unwrap(),
                    if image["ready"] == true {
                        ""
                    } else {
                        " (unavailable)"
                    }
                )
                .into()
            })
            .collect::<Vec<slint::SharedString>>(),
    )));
    let index = id
        .and_then(|id| {
            data.images
                .iter()
                .position(|image| image["id"].as_u64() == Some(u64::from(id)))
        })
        .or_else(|| (!data.images.is_empty()).then_some(0));
    window.set_image_index(index.map_or(-1, |index| index as i32));
    window.set_status(
        report["error"]
            .as_str()
            .unwrap_or_else(|| report["summary"].as_str().unwrap())
            .into(),
    );
    choose(window, data);
}

fn vector<const N: usize>(value: &str) -> Result<[f64; N], String> {
    value
        .split_whitespace()
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| format!("Enter {N} finite numbers separated by spaces"))?
        .try_into()
        .map_err(|_| format!("Enter exactly {N} numbers separated by spaces"))
}
fn settings(window: &ReferenceImagesWindow) -> Result<Settings, String> {
    Ok(Settings {
        name: window.get_image_name().to_string(),
        position: vector(&window.get_position())?,
        rotation: vector(&window.get_rotation())?,
        size: vector(&window.get_size())?,
        opacity: window
            .get_image_opacity()
            .parse()
            .map_err(|_| "Enter an opacity from 0 to 1")?,
        flip: [window.get_flip_x(), window.get_flip_y()],
        views: [
            window.get_top(),
            window.get_front(),
            window.get_side(),
            window.get_perspective(),
        ],
    })
}

fn action(
    window: &ReferenceImagesWindow,
    data: &mut Data,
    session: &mut Session,
    operation: &str,
) -> Result<(), String> {
    let id = selected(window, data).ok();
    match operation {
        "refresh" => refresh(window, data, session, id),
        "add" => {
            let result = session.add_reference_image(
                Actor::User,
                Path::new(window.get_path().as_str()),
                &window.get_image_name(),
                &data.snapshot,
            )?;
            refresh(
                window,
                data,
                session,
                result["created_id"].as_u64().map(|id| id as u32),
            );
        }
        "apply" => {
            let id = id.ok_or("Choose a reference image")?;
            session.update_reference_image(Actor::User, id, settings(window)?, &data.snapshot)?;
            refresh(window, data, session, Some(id));
        }
        "reload" => {
            let id = id.ok_or("Choose a reference image")?;
            session.reload_reference_image(
                Actor::User,
                id,
                Some(Path::new(window.get_path().as_str())),
                &data.snapshot,
            )?;
            refresh(window, data, session, Some(id));
        }
        "delete" => {
            session.delete_reference_image(
                Actor::User,
                id.ok_or("Choose a reference image")?,
                &data.snapshot,
            )?;
            refresh(window, data, session, None);
        }
        "frame" => session.frame_reference_image(
            Actor::User,
            id.ok_or("Choose a reference image")?,
            &data.snapshot,
            window.get_viewport_aspect(),
        )?,
        _ => return Err("Unknown reference image action".into()),
    }
    Ok(())
}

fn callback(
    window: &slint::Weak<ReferenceImagesWindow>,
    target: &DocumentContinuation,
    data: &RefCell<Data>,
    operation: &str,
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = target.upgrade().and_then(|access| {
        access.with_session(|session| action(&window, &mut data.borrow_mut(), session, operation))
    });
    window.set_message(match result { Ok(())=>match operation { "refresh"=>"List refreshed. Unsaved form fields were replaced.","frame"=>"Camera framed around the saved image pose.",_=>"Reference image changed. Save the map to keep it; Undo restores the previous metadata." }.into(),Err(error)=>error.into() });
}

impl Controller {
    pub fn refresh_status(&self, session: &mut Session) {
        let report = session.reference_images_state();
        self.window
            .set_status(report["summary"].as_str().unwrap().into());
        let data = self.data.borrow();
        if report["snapshot"].as_str() != Some(data.snapshot.as_str()) {
            self.window.set_message(
                "Map changed. Refresh list before applying reference image actions.".into(),
            );
            self.window
                .set_source_status("Refresh list to inspect the current source.".into());
            return;
        }
        if let Ok(id) = selected(&self.window, &data)
            && let Some(image) = report["images"].as_array().and_then(|images| {
                images
                    .iter()
                    .find(|image| image["id"].as_u64() == Some(u64::from(id)))
            })
        {
            source_status(&self.window, image);
        }
    }

    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = ReferenceImagesWindow::new()?;
        let data = Rc::new(RefCell::new(Data::default()));
        window.on_choose({
            let (window, data) = (window.as_weak(), data.clone());
            move || {
                if let Some(window) = window.upgrade() {
                    choose(&window, &data.borrow());
                }
            }
        });
        let run = {
            let (window, target, data) = (window.as_weak(), access.downgrade(), data.clone());
            move |operation: &str| callback(&window, &target, &data, operation)
        };
        window.on_refresh({
            let run = run.clone();
            move || run("refresh")
        });
        window.on_add({
            let run = run.clone();
            move || run("add")
        });
        window.on_apply({
            let run = run.clone();
            move || run("apply")
        });
        window.on_reload({
            let run = run.clone();
            move || run("reload")
        });
        window.on_delete({
            let run = run.clone();
            move || run("delete")
        });
        window.on_frame(move || run("frame"));
        window.on_dismiss({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_dismiss();
                }
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self { window, data })
    }
    pub fn show(&self, session: &mut Session, aspect: f32) -> Result<(), slint::PlatformError> {
        self.window.set_viewport_aspect(aspect);
        let mut data = self.data.borrow_mut();
        let id = selected(&self.window, &data).ok();
        refresh(&self.window, &mut data, session, id);
        self.window.set_message("".into());
        self.window.show()
    }
}
