use editor_core::color::{Converted, Input, Space};
use slint::ComponentHandle;

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

#[cfg(test)]
mod tests;
