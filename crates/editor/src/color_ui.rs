use crate::runtime::{DocumentAccess, DocumentContinuation};
use editor_core::color::{Converted, Input, Space};
use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;

use crate::{ColorWindow, Session};

fn components(window: &ColorWindow) -> [String; 3] {
    [
        window.get_hue(),
        window.get_saturation(),
        window.get_level(),
    ]
    .map(|value| value.to_string())
}

fn component_stamp(window: &ColorWindow) -> String {
    components(window).join("\n")
}

fn canonical(window: &ColorWindow) -> Result<Converted, String> {
    let input = Input::property("color1", &window.get_color_source())?;
    Session::convert_color(input)
}

fn input_space(index: i32) -> Result<Space, String> {
    match index {
        0 => Ok(Space::Rgb255),
        1 => Ok(Space::Hsv),
        2 => Ok(Space::Hsl),
        _ => Err("Choose RGB, HSV or HSL".into()),
    }
}

pub fn read(window: &ColorWindow) -> Result<Converted, String> {
    if window.get_model_index() != window.get_component_model() {
        return Err("Choose RGB, HSV or HSL again".into());
    }
    read_model(window)
}

fn component_values(window: &ColorWindow) -> Result<[f64; 3], String> {
    let channels = components(window).map(|value| {
        value
            .parse::<f64>()
            .map_err(|_| "Enter three valid color numbers".to_owned())
    });
    let [hue, saturation, level] = channels;
    Ok([hue?, saturation?, level?])
}

fn read_model(window: &ColorWindow) -> Result<Converted, String> {
    let color = canonical(window)?;
    let space = input_space(window.get_component_model())?;
    match space {
        Space::Rgb255 => edited_rgb(window, &color),
        Space::Hsv | Space::Hsl => {
            if component_stamp(window) == window.get_component_source().as_str() {
                return Ok(color);
            }
            Session::convert_color(Input {
                space,
                channels: component_values(window)?,
            })
        }
        Space::Rgb1 => unreachable!(),
    }
}

fn edited_rgb(window: &ColorWindow, previous: &Converted) -> Result<Converted, String> {
    let bytes = [window.get_red(), window.get_green(), window.get_blue()];
    let edited = Session::convert_color(Input {
        space: Space::Rgb255,
        channels: bytes.map(f64::from),
    })?;
    let channels = std::array::from_fn(|index| {
        if bytes[index] == i32::from(previous.rgb255[index]) {
            previous.rgb1[index]
        } else {
            edited.rgb1[index]
        }
    });
    Session::convert_color(Input {
        space: Space::Rgb1,
        channels,
    })
}

fn store(window: &ColorWindow, color: &Converted) {
    window.set_color_source(color.rgb1.map(|value| value.to_string()).join(" ").into());
    window.set_red(i32::from(color.rgb255[0]));
    window.set_green(i32::from(color.rgb255[1]));
    window.set_blue(i32::from(color.rgb255[2]));
}

fn display(window: &ColorWindow, color: &Converted) {
    let channels = if window.get_component_model() == 2 {
        color.hsl
    } else {
        color.hsv
    };
    window.set_hue(channels[0].to_string().into());
    window.set_saturation(channels[1].to_string().into());
    window.set_level(channels[2].to_string().into());
    sliders(window, channels);
    window.set_component_source(component_stamp(window).into());
}

fn sliders(window: &ColorWindow, channels: [f64; 3]) {
    window.set_hue_value(channels[0] as f32);
    window.set_saturation_value(channels[1] as f32);
    window.set_level_value(channels[2] as f32);
}

pub fn load(window: &ColorWindow, kind: &str, value: &str) -> Result<(), String> {
    let color = Session::convert_color(Input::property(kind, value)?)?;
    window.set_model_index(0);
    window.set_component_model(0);
    window.set_normalized(kind == "color1");
    store(window, &color);
    display(window, &color);
    Ok(())
}

pub fn bind(window: &ColorWindow) {
    load(window, "color255", "255 255 255").expect("White color is valid");
    window.on_select_model({
        let window = window.as_weak();
        move |index| {
            let Some(window) = window.upgrade() else {
                return;
            };
            let result = input_space(index).and_then(|_| read_model(&window));
            match result {
                Ok(color) => {
                    store(&window, &color);
                    window.set_model_index(index);
                    window.set_component_model(index);
                    display(&window, &color);
                    window.set_message("".into());
                }
                Err(error) => {
                    window.set_model_index(window.get_component_model());
                    window.set_message(error.into());
                }
            }
        }
    });
    window.on_rgb_changed({
        let window = window.as_weak();
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            match canonical(&window).and_then(|previous| edited_rgb(&window, &previous)) {
                Ok(color) => {
                    store(&window, &color);
                    display(&window, &color);
                    window.set_message("".into());
                }
                Err(error) => window.set_message(error.into()),
            }
        }
    });
    window.on_preset({
        let window = window.as_weak();
        move |red, green, blue| {
            let Some(window) = window.upgrade() else {
                return;
            };
            match Session::convert_color(Input {
                space: Space::Rgb255,
                channels: [red, green, blue].map(f64::from),
            }) {
                Ok(color) => {
                    store(&window, &color);
                    display(&window, &color);
                    window.set_message("".into());
                }
                Err(error) => window.set_message(error.into()),
            }
        }
    });
    window.on_components_changed({
        let window = window.as_weak();
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            let result = component_values(&window)
                .and_then(|channels| read(&window).map(|color| (color, channels)));
            match result {
                Ok((color, channels)) => {
                    store(&window, &color);
                    sliders(&window, channels);
                    window.set_component_source(component_stamp(&window).into());
                    window.set_message("".into());
                }
                Err(error) => window.set_message(error.into()),
            }
        }
    });
}

pub fn refresh_palette(window: &ColorWindow, session: &Session) {
    let state = session.color_palette_state();
    let selected = palette_selection(window);
    let entries = state["entries"]
        .as_array()
        .expect("Palette entries are an array");
    let mut names = vec![slint::SharedString::from("Choose a saved color")];
    names.extend(
        entries
            .iter()
            .map(|entry| entry["name"].as_str().unwrap().into()),
    );
    let index = selected
        .as_ref()
        .and_then(|name| real_palette_index(names.iter().cloned(), name))
        .unwrap_or(0);
    window.set_palette_names(Rc::new(VecModel::from(names)).into());
    window.set_palette_index(index as i32);
    window.set_palette_snapshot(state["snapshot"].as_str().unwrap().into());
    window.set_palette_ready(state["ready"] == true);
    window.set_palette_reload_available(state["reload_available"] == true);
    let mut status = format!(
        "{} / 16 saved colors · {}",
        entries.len(),
        if state["ready"] == true {
            "Palette ready"
        } else {
            "Palette unavailable"
        }
    );
    if let Some(path) = state["path"].as_str() {
        status.push_str(&format!("\n{path}"));
    }
    if let Some(error) = state["error"].as_str() {
        status.push_str(&format!("\n{error}"));
    }
    if state["write_committed"] == true && state["ready"] != true {
        status.push_str(
            "\nThe palette was written but could not be verified. Reload palette before using it.",
        );
    }
    window.set_palette_status(status.into());
}

fn palette_selection(window: &ColorWindow) -> Option<slint::SharedString> {
    let index = usize::try_from(window.get_palette_index()).ok()?;
    (index > 0)
        .then(|| window.get_palette_names().row_data(index))
        .flatten()
}

fn real_palette_index(
    names: impl Iterator<Item = slint::SharedString>,
    name: &str,
) -> Option<usize> {
    names
        .enumerate()
        .skip(1)
        .find_map(|(index, entry)| (entry.as_str() == name).then_some(index))
}

fn palette_callback(
    window: &slint::Weak<ColorWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(&ColorWindow, &mut Session) -> Result<(), String>,
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = target
        .upgrade()
        .and_then(|access| access.with_session(|session| body(&window, session)));
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

pub fn bind_palette(window: &ColorWindow, access: &DocumentAccess) {
    window.on_palette_choice({
        let window = window.as_weak();
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            if let Some(name) = palette_selection(&window) {
                window.set_palette_name(name);
            }
        }
    });
    window.on_palette_action({
        let (window, target) = (window.as_weak(), access.downgrade());
        move |action| {
            palette_callback(&window, &target, |window, session| {
                change_palette(window, session, &action)
            })
        }
    });
}

fn change_palette(window: &ColorWindow, session: &mut Session, action: &str) -> Result<(), String> {
    let name = match action {
        "save" => Some(window.get_palette_name()),
        "reload" => None,
        _ => palette_selection(window),
    };
    let color = if action == "save" || action == "replace" {
        Some(Input {
            space: Space::Rgb1,
            channels: read(window)?.rgb1,
        })
    } else {
        None
    };
    let selected_name = if action == "rename" {
        Some(window.get_palette_name())
    } else {
        name.clone()
    };
    let result = if action == "rename" {
        session.rename_color_palette(
            &window.get_palette_snapshot(),
            name.as_deref()
                .ok_or("Choose an existing color to rename")?,
            &window.get_palette_name(),
        )
    } else {
        session.change_color_palette(
            action,
            &window.get_palette_snapshot(),
            name.as_deref(),
            color,
        )
    };
    refresh_palette(window, session);
    let result = result?;
    if action == "use" {
        let rgb1: [f64; 3] = serde_json::from_value(result["color"]["rgb1"].clone())
            .map_err(|error| error.to_string())?;
        let color = Session::convert_color(Input {
            space: Space::Rgb1,
            channels: rgb1,
        })?;
        store(window, &color);
        display(window, &color);
    }
    if action == "save" || action == "rename" {
        let names = window.get_palette_names();
        if let Some(index) = selected_name
            .as_deref()
            .and_then(|name| real_palette_index(names.iter(), name))
        {
            window.set_palette_index(index as i32);
        }
    }
    window.set_message(match action {
        "save" | "replace" => "Saved in your user palette. Palette writes remain saved if you Cancel this color edit.",
        "delete" => "Deleted the saved color. The current color stays unchanged.",
        "rename" => "Renamed the saved color. The current color and brightness stay unchanged.",
        "reload" => "Reloaded the user palette. The current color stays unchanged.",
        _ => "Loaded the saved color locally. Apply color to use it; brightness stays unchanged.",
    }.into());
    Ok(())
}

#[cfg(test)]
mod tests;
