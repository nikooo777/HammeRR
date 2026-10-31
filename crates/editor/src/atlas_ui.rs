use std::cell::RefCell;
use std::rc::Rc;

use editor_core::session::atlas_texturing::{
    FitMode, FitOptions, FitRegion, PackOptions, Rectangle,
};
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::runtime::{DocumentAccess, DocumentContinuation, SharedSession, lock};
use crate::session::{Actor, Session};
use crate::{AtlasIslandOverlay, AtlasWindow, face_edit_ui};

pub struct Controller {
    pub window: AtlasWindow,
    target: Rc<RefCell<Option<Target>>>,
}

struct Target {
    context: face_edit_ui::Context,
    atlas: Option<Value>,
    token: Option<u64>,
    reviewed: Option<String>,
    drag: Option<RectangleDrag>,
    catalog: Option<Value>,
    saved_regions: Vec<Value>,
}

struct RectangleDrag {
    previous: [slint::SharedString; 4],
    expected_rectangle: [slint::SharedString; 4],
    expected: String,
    snapshot: String,
    size: [u32; 2],
    start: [u32; 2],
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
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        window.get_material(),
        window.get_region_x(),
        window.get_region_y(),
        window.get_region_pixels_wide(),
        window.get_region_pixels_high(),
        window.get_region_padding(),
        window.get_fit_mode(),
        window.get_reference_index(),
        window.get_island_scale(),
        window.get_island_padding()
    )
}

fn rectangle_fields(window: &AtlasWindow) -> [slint::SharedString; 4] {
    [
        window.get_region_x(),
        window.get_region_y(),
        window.get_region_pixels_wide(),
        window.get_region_pixels_high(),
    ]
}

fn set_rectangle_fields(window: &AtlasWindow, values: [slint::SharedString; 4]) {
    let [x, y, width, height] = values;
    window.set_region_x(x);
    window.set_region_y(y);
    window.set_region_pixels_wide(width);
    window.set_region_pixels_high(height);
}

fn image_pixel(position: [f32; 2], size: [u32; 2], clip: bool) -> Result<[u32; 2], String> {
    if size.contains(&0) || position.iter().any(|value| !value.is_finite()) {
        return Err("Rectangle selection needs finite coordinates on an inspected image".into());
    }
    if !clip && position.iter().any(|value| !(0.0..=1.0).contains(value)) {
        return Err("Start rectangle selection inside the inspected image".into());
    }
    Ok(std::array::from_fn(|axis| {
        (f64::from(position[axis].clamp(0.0, 1.0)) * f64::from(size[axis])).round() as u32
    }))
}

fn cancel_rectangle(window: &AtlasWindow, target: &mut Target) {
    if let Some(drag) = target.drag.take() {
        set_rectangle_fields(window, drag.previous);
        window.set_ready(false);
        window.set_rectangle_dragging(false);
        show_rectangle(window);
    }
}

fn rectangle_pointer(
    window: &AtlasWindow,
    target: &mut Target,
    session: &mut Session,
    phase: i32,
    position: [f32; 2],
) -> Result<(), String> {
    target.check(session)?;
    let atlas = target.atlas.as_ref().ok_or("Inspect the material first")?;
    if !window.get_loaded()
        || atlas["material"].as_str() != Some(window.get_material().as_str())
        || atlas["content_epoch"].as_str()
            != Some(session.build_content_epoch().to_string().as_str())
    {
        return Err(
            "Material inspection changed. Inspect the material before selecting a rectangle".into(),
        );
    }
    if phase == 0 {
        let size = [
            atlas["width"].as_u64().unwrap() as u32,
            atlas["height"].as_u64().unwrap() as u32,
        ];
        let start = image_pixel(position, size, false)?;
        let snapshot = atlas["snapshot"].as_str().unwrap().to_owned();
        cancel_rectangle(window, target);
        target.drag = Some(RectangleDrag {
            previous: rectangle_fields(window),
            expected_rectangle: rectangle_fields(window),
            expected: fields(window),
            snapshot,
            size,
            start,
        });
        window.set_ready(false);
        clear_packing_layout(window);
        window.set_rectangle_dragging(true);
        window.set_message(
            "Drag to choose pixel edges. Escape restores the previous rectangle.".into(),
        );
        return Ok(());
    }
    let Some(drag) = target.drag.as_ref() else {
        return Ok(());
    };
    if fields(window) != drag.expected || atlas["snapshot"].as_str() != Some(drag.snapshot.as_str())
    {
        return Err("Atlas fields or inspection changed during rectangle selection".into());
    }
    if phase == 2 {
        let current = session.inspect_material_atlas(&window.get_material())?;
        if current.report["snapshot"].as_str() != Some(drag.snapshot.as_str()) {
            return Err(
                "Atlas source changed during rectangle selection. Inspect the material again."
                    .into(),
            );
        }
    }
    let end = image_pixel(position, drag.size, true)?;
    let rectangle = Rectangle {
        x: drag.start[0].min(end[0]),
        y: drag.start[1].min(end[1]),
        width: drag.start[0].abs_diff(end[0]),
        height: drag.start[1].abs_diff(end[1]),
    };
    if rectangle.width == 0 || rectangle.height == 0 {
        if phase == 2 {
            cancel_rectangle(window, target);
        }
        window.set_message("Choose a rectangle with positive width and height; the previous rectangle is retained.".into());
        return Ok(());
    }
    set_rectangle_fields(
        window,
        [rectangle.x, rectangle.y, rectangle.width, rectangle.height]
            .map(|value| value.to_string().into()),
    );
    show_rectangle(window);
    if phase == 2 {
        target.drag = None;
        window.set_rectangle_dragging(false);
        window.set_message("Rectangle selected. Preview atlas fit before applying.".into());
    } else if let Some(drag) = &mut target.drag {
        drag.expected = fields(window);
        drag.expected_rectangle = rectangle_fields(window);
    }
    Ok(())
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
        2 => FitMode::Pack(PackOptions {
            scale: window
                .get_island_scale()
                .trim()
                .parse()
                .map_err(|_| "Enter a positive Island UV size multiplier")?,
            padding: window
                .get_island_padding()
                .trim()
                .parse()
                .map_err(|_| "Enter nonnegative whole-pixel island padding")?,
        }),
        _ => return Err("Choose an atlas fit mode".into()),
    };
    Ok(FitOptions {
        region: region(window)?,
        mode,
    })
}

fn show_rectangle(window: &AtlasWindow) {
    clear_packing_layout(window);
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

fn clear_packing_layout(window: &AtlasWindow) {
    window.set_packed_islands(ModelRc::default());
    window.set_packing_summary("".into());
}

fn show_packing_layout(window: &AtlasWindow, state: &Value) {
    clear_packing_layout(window);
    let Some(islands) = state["preview"]["islands"].as_array() else {
        return;
    };
    let rows = islands
        .iter()
        .map(|island| {
            let rectangle = &island["rectangle"];
            let usable = &island["usable_rectangle"];
            let [x, y, width, height] =
                ["x", "y", "width", "height"].map(|field| rectangle[field].as_f64().unwrap());
            AtlasIslandOverlay {
                u: x as f32 / window.get_texture_width(),
                v: y as f32 / window.get_texture_height(),
                width: width as f32 / window.get_texture_width(),
                height: height as f32 / window.get_texture_height(),
                content_u: ((usable["x"].as_f64().unwrap() - x) / width) as f32,
                content_v: ((usable["y"].as_f64().unwrap() - y) / height) as f32,
                content_width: (island["texel_size"][0].as_f64().unwrap() / width) as f32,
                content_height: (island["texel_size"][1].as_f64().unwrap() / height) as f32,
                label: format!("{}:{}", island["face"]["solid"], island["face"]["side"]).into(),
                detail: format!(
                    "Brush {} face {} · allocation X {x}, Y {y}, {width} × {height} source pixels",
                    island["face"]["solid"], island["face"]["side"]
                )
                .into(),
            }
        })
        .collect::<Vec<_>>();
    window.set_packed_islands(ModelRc::new(VecModel::from(rows)));
    window.set_packing_summary(format!("{} face islands · UV size {}× · {} px padding on each island edge. Review the green layout and face preview before Apply.", islands.len(), state["preview"]["packing"]["scale"], state["preview"]["packing"]["padding"]).into());
}

fn inspect(window: &AtlasWindow, target: &mut Target, session: &mut Session) -> Result<(), String> {
    clear_packing_layout(window);
    cancel_rectangle(window, target);
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
        || target
            .atlas
            .as_ref()
            .is_none_or(|previous| previous["image_identity"] != atlas.report["image_identity"])
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
    refresh_regions(window, target, session, None)?;
    show_rectangle(window);
    Ok(())
}

fn refresh_regions(
    window: &AtlasWindow,
    target: &mut Target,
    session: &Session,
    selected: Option<u32>,
) -> Result<(), String> {
    let catalog = session.atlas_regions_state()?;
    let material = window.get_material();
    target.saved_regions = catalog["regions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|region| region["material"].as_str() == Some(material.as_str()))
        .cloned()
        .collect();
    let identity = target.atlas.as_ref().map(|atlas| &atlas["image_identity"]);
    window.set_saved_regions(ModelRc::new(VecModel::from(
        target
            .saved_regions
            .iter()
            .map(|region| {
                let suffix = if identity == Some(&region["image_identity"]) {
                    ""
                } else {
                    " (source changed)"
                };
                format!("{}{suffix}", region["name"].as_str().unwrap()).into()
            })
            .collect::<Vec<_>>(),
    )));
    let index = selected.and_then(|id| {
        target
            .saved_regions
            .iter()
            .position(|region| region["id"].as_u64() == Some(u64::from(id)))
    });
    window.set_saved_region_index(index.map_or(-1, |index| index as i32));
    target.catalog = Some(catalog);
    window.set_catalog_editable(!session.has_face_preview());
    Ok(())
}

fn selected_region(window: &AtlasWindow, target: &Target) -> Result<Value, String> {
    usize::try_from(window.get_saved_region_index())
        .ok()
        .and_then(|index| target.saved_regions.get(index))
        .cloned()
        .ok_or_else(|| "Choose a saved atlas region first".into())
}

fn choose_saved_region(
    window: &AtlasWindow,
    target: &mut Target,
    session: &mut Session,
) -> Result<(), String> {
    target.check(session)?;
    if target.drag.is_some() {
        return Err("Finish or cancel rectangle selection before choosing a saved region".into());
    }
    let saved = selected_region(window, target)?;
    let atlas = target.atlas.as_ref().ok_or("Inspect the material first")?;
    let catalog = target
        .catalog
        .as_ref()
        .ok_or("Inspect the saved regions first")?;
    let chosen = session.choose_atlas_region(
        saved["id"].as_u64().unwrap() as u32,
        &window.get_material(),
        atlas["snapshot"].as_str().unwrap(),
        catalog["snapshot"].as_str().unwrap(),
    )?;
    set_rectangle_fields(
        window,
        ["x", "y", "width", "height"].map(|field| chosen["rectangle"][field].to_string().into()),
    );
    window.set_region_padding(chosen["padding"].to_string().into());
    window.set_saved_region_name(chosen["name"].as_str().unwrap().into());
    window.set_ready(false);
    target.reviewed = None;
    show_rectangle(window);
    window.set_message("Saved region chosen. Preview atlas fit before applying.".into());
    Ok(())
}

fn edit_saved_region(
    window: &AtlasWindow,
    target: &mut Target,
    session: &mut Session,
    operation: i32,
) -> Result<(), String> {
    target.check(session)?;
    if target.drag.is_some() {
        return Err("Finish or cancel rectangle selection before changing saved regions".into());
    }
    let snapshot = target
        .catalog
        .as_ref()
        .ok_or("Inspect the saved regions first")?["snapshot"]
        .as_str()
        .unwrap();
    let state = match operation {
        0 => {
            let atlas = target.atlas.as_ref().ok_or("Inspect the material first")?;
            session.save_atlas_region(
                Actor::User,
                &window.get_saved_region_name(),
                &window.get_material(),
                region(window)?,
                atlas["snapshot"].as_str().unwrap(),
                snapshot,
            )?
        }
        1 => session.rename_atlas_region(
            Actor::User,
            selected_region(window, target)?["id"].as_u64().unwrap() as u32,
            &window.get_saved_region_name(),
            snapshot,
        )?,
        2 => session.delete_atlas_region(
            Actor::User,
            selected_region(window, target)?["id"].as_u64().unwrap() as u32,
            snapshot,
        )?,
        _ => return Err("Unknown saved atlas region action".into()),
    };
    let selected = if operation == 0 {
        state["saved_id"].as_u64().map(|id| id as u32)
    } else if operation == 1 {
        Some(selected_region(window, target)?["id"].as_u64().unwrap() as u32)
    } else {
        None
    };
    target.context = face_edit_ui::Context::capture(session, target.context.faces.clone());
    target.reviewed = None;
    window.set_ready(false);
    refresh_regions(window, target, session, selected)?;
    window.set_message(match operation {
        0 => "Region saved in the map in one Undo step. Preview atlas fit to use the current rectangle.",
        1 => "Region renamed in one Undo step.",
        _ => "Region deleted in one Undo step.",
    }.into());
    Ok(())
}

fn preview(window: &AtlasWindow, target: &mut Target, session: &mut Session) -> Result<(), String> {
    if target.drag.is_some() {
        return Err("Finish or cancel rectangle selection before previewing".into());
    }
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
    show_packing_layout(window, &state);
    window.set_ready(true);
    window.set_catalog_editable(false);
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
    if target.drag.is_some() {
        return Err("Finish or cancel rectangle selection before applying".into());
    }
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
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = AtlasWindow::new()?;
        let target = Rc::new(RefCell::new(None::<Target>));
        window.on_select_saved_region({
            let (window, target, access) = (window.as_weak(), target.clone(), access.downgrade());
            move || {
                with_window(&window, &access, |window, _| {
                    if let Some(target) = target.borrow().as_ref()
                        && let Ok(region) = selected_region(window, target)
                    {
                        window.set_saved_region_name(region["name"].as_str().unwrap().into());
                    }
                });
            }
        });
        window.on_choose_saved_region({
            let (window, target, access) = (window.as_weak(), target.clone(), access.downgrade());
            move || {
                with_window(&window, &access, |window, shared| {
                    if let Some(target) = target.borrow_mut().as_mut()
                        && let Err(error) = choose_saved_region(window, target, &mut lock(shared))
                    {
                        window.set_message(error.into());
                    }
                });
            }
        });
        let edit_region = {
            let (window, target, access) = (window.as_weak(), target.clone(), access.downgrade());
            move |operation| {
                with_window(&window, &access, |window, shared| {
                    if let Some(target) = target.borrow_mut().as_mut()
                        && let Err(error) =
                            edit_saved_region(window, target, &mut lock(shared), operation)
                    {
                        window.set_message(error.into());
                    }
                });
            }
        };
        window.on_save_region({
            let edit_region = edit_region.clone();
            move || edit_region(0)
        });
        window.on_rename_region({
            let edit_region = edit_region.clone();
            move || edit_region(1)
        });
        window.on_delete_region(move || edit_region(2));
        window.on_rectangle_pointer({
            let (window, target, access) = (window.as_weak(), target.clone(), access.downgrade());
            move |phase, x, y| {
                with_window(&window, &access, |window, shared| {
                    if let Some(target) = target.borrow_mut().as_mut() {
                        let result =
                            rectangle_pointer(window, target, &mut lock(shared), phase, [x, y]);
                        if let Err(error) = result {
                            clear_packing_layout(window);
                            cancel_rectangle(window, target);
                            window.set_message(error.into());
                        }
                    }
                });
            }
        });
        window.on_cancel_rectangle({
            let (window, target) = (window.as_weak(), target.clone());
            move || {
                if let Some(window) = window.upgrade()
                    && let Some(target) = target.borrow_mut().as_mut()
                    && target.drag.is_some()
                {
                    cancel_rectangle(&window, target);
                    window.set_message(
                        "Rectangle selection cancelled. Preview atlas fit before applying.".into(),
                    );
                }
            }
        });
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
                            clear_packing_layout(window);
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
                    if let Some(target) = target.borrow_mut().as_mut()
                        && let Some(drag) = &target.drag
                    {
                        let current = rectangle_fields(window);
                        let edited = std::array::from_fn(|axis| {
                            if current[axis] != drag.expected_rectangle[axis] {
                                current[axis].clone()
                            } else {
                                drag.previous[axis].clone()
                            }
                        });
                        cancel_rectangle(window, target);
                        set_rectangle_fields(window, edited);
                    }
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
                if window.get_rectangle_dragging() {
                    window.invoke_cancel_rectangle();
                }
                with_window(&window.as_weak(), &access, |window, shared| {
                    if let Some(target) = target.borrow_mut().as_mut() {
                        cancel_rectangle(window, target);
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

    pub(crate) fn initialize(&self, session: &mut Session, material: &str) -> Result<(), String> {
        if let Some(previous) = self.target.borrow_mut().as_mut() {
            cancel_rectangle(&self.window, previous);
        }
        self.window.set_rectangle_dragging(false);
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
            drag: None,
            catalog: None,
            saved_regions: Vec::new(),
        };
        self.window.set_selection_label(
            format!("Fit {} selected flat faces", target.context.faces.len()).into(),
        );
        self.window.set_fit_mode(0);
        self.window.set_island_scale("1".into());
        self.window.set_island_padding("0".into());
        clear_packing_layout(&self.window);
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
        self.window.set_saved_region_name("".into());
        self.window.set_saved_regions(ModelRc::new(VecModel::from(
            Vec::<slint::SharedString>::new(),
        )));
        self.window.set_saved_region_index(-1);
        self.window.set_catalog_editable(false);
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
        let mut target = self.target.borrow_mut();
        if let Some(target) = target.as_mut() {
            cancel_rectangle(&self.window, target);
        }
        self.window.set_material(material.into());
        if let Some(target) = target.as_mut() {
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
        if let Some(target) = self.target.borrow_mut().as_mut() {
            self.window
                .set_catalog_editable(!session.has_face_preview());
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
                clear_packing_layout(&self.window);
                cancel_rectangle(&self.window, target);
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
