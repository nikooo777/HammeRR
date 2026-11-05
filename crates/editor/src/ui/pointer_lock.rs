use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
};

use editor_core::shortcuts::{canonical_key, canonical_physical_key};
use slint::ComponentHandle;
use slint::winit_030::winit::event::{
    DeviceEvent, DeviceId, ElementState, Ime, MouseButton, TouchPhase, WindowEvent,
};
use slint::winit_030::winit::event_loop::ActiveEventLoop;
use slint::winit_030::winit::keyboard::{Key, PhysicalKey};
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use slint::winit_030::winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use slint::winit_030::winit::window::{CursorGrabMode, WindowId};
use slint::winit_030::{CustomApplicationHandler, EventResult, WinitWindowAccessor};

use crate::{AppWindow, shortcut_input::NativeKey};

#[derive(Default)]
struct State {
    owner: Option<WindowId>,
    relative: bool,
    delta: (f64, f64),
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
    static KEYBOARDS: RefCell<HashMap<WindowId, Keyboard>> = RefCell::default();
}

#[derive(Default)]
struct Keyboard {
    pending: Option<NativeKey>,
    pressed: HashSet<String>,
    released: HashSet<String>,
    focused: bool,
    shift: bool,
    composing: bool,
    reset: bool,
    cancel_prefix: bool,
    mouse_buttons: HashSet<MouseButton>,
    touches: HashSet<u64>,
}

impl Keyboard {
    fn retire_pending(&mut self) {
        if self
            .pending
            .take()
            .is_some_and(|event| event.down && !event.synthetic && !event.is_modifier())
        {
            self.cancel_prefix = true;
        }
    }
}

#[derive(Default)]
pub(super) struct KeyboardFrame {
    pub released: HashSet<String>,
    pub focused: bool,
    pub shift: bool,
    pub composing: bool,
    pub reset: bool,
    pub cancel_prefix: bool,
}

pub(super) struct Handler;

impl CustomApplicationHandler for Handler {
    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        window_id: WindowId,
        window: Option<&slint::winit_030::winit::window::Window>,
        _slint_window: Option<&slint::Window>,
        event: &WindowEvent,
    ) -> EventResult {
        KEYBOARDS.with_borrow_mut(|keyboards| {
            if matches!(event, WindowEvent::Destroyed) {
                keyboards.remove(&window_id);
                return;
            }
            let keyboard = keyboards.entry(window_id).or_insert_with(|| Keyboard {
                focused: window.is_some_and(slint::winit_030::winit::window::Window::has_focus),
                ..Keyboard::default()
            });
            keyboard.retire_pending();
            match event {
                WindowEvent::KeyboardInput {
                    event,
                    is_synthetic,
                    ..
                } => {
                    let identity = match event.physical_key {
                        PhysicalKey::Code(code) => Some(format!("{code:?}")),
                        PhysicalKey::Unidentified(_) => None,
                    };
                    let down = event.state == ElementState::Pressed;
                    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
                    let logical_key = event.key_without_modifiers();
                    #[cfg(not(any(
                        target_os = "linux",
                        target_os = "windows",
                        target_os = "macos"
                    )))]
                    let logical_key = event.logical_key.clone();
                    let logical = match logical_key {
                        Key::Character(text) => canonical_key(text.as_str()),
                        Key::Named(key) => canonical_key(&format!("{key:?}")),
                        _ => None,
                    };
                    let mut repeat = event.repeat;
                    let dead = matches!(event.logical_key, Key::Dead(_));
                    if dead {
                        keyboard.reset = true;
                    }
                    if !is_synthetic && let Some(identity) = &identity {
                        if down {
                            repeat |= !keyboard.pressed.insert(identity.clone());
                        } else {
                            keyboard.pressed.remove(identity);
                            keyboard.released.insert(identity.clone());
                        }
                    }
                    keyboard.pending = Some(NativeKey {
                        logical,
                        physical: identity.as_deref().and_then(canonical_physical_key),
                        identity,
                        down,
                        repeat,
                        synthetic: *is_synthetic,
                        dead,
                    });
                }
                WindowEvent::ModifiersChanged(modifiers) => {
                    keyboard.shift = modifiers.state().shift_key();
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    if *state == ElementState::Pressed {
                        keyboard.mouse_buttons.insert(*button);
                        keyboard.cancel_prefix = true;
                    } else {
                        keyboard.mouse_buttons.remove(button);
                    }
                }
                WindowEvent::Touch(touch) => {
                    match touch.phase {
                        TouchPhase::Started => {
                            keyboard.touches.insert(touch.id);
                        }
                        TouchPhase::Ended | TouchPhase::Cancelled => {
                            keyboard.touches.remove(&touch.id);
                        }
                        TouchPhase::Moved => {}
                    }
                    keyboard.cancel_prefix = true;
                }
                WindowEvent::MouseWheel { .. } => keyboard.cancel_prefix = true,
                WindowEvent::Focused(focused) => {
                    keyboard.focused = *focused;
                    if !*focused {
                        keyboard.pressed.clear();
                        keyboard.released.clear();
                        keyboard.shift = false;
                        keyboard.composing = false;
                        keyboard.reset = true;
                        keyboard.mouse_buttons.clear();
                        keyboard.touches.clear();
                    }
                }
                WindowEvent::CloseRequested => {
                    keyboard.pressed.clear();
                    keyboard.reset = true;
                    keyboard.mouse_buttons.clear();
                    keyboard.touches.clear();
                }
                WindowEvent::Ime(event) => {
                    keyboard.composing = matches!(event, Ime::Preedit(text, _) if !text.is_empty());
                    keyboard.reset = true;
                }
                _ => {}
            }
        });
        EventResult::Propagate
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device: DeviceId,
        event: DeviceEvent,
    ) -> EventResult {
        if let DeviceEvent::MouseMotion { delta } = event {
            STATE.with_borrow_mut(|state| {
                if state.relative {
                    state.delta.0 += delta.0;
                    state.delta.1 += delta.1;
                }
            });
        }
        EventResult::Propagate
    }
}

pub(super) fn keyboard_event(app: &AppWindow, down: bool) -> Option<NativeKey> {
    app.window()
        .with_winit_window(|window| {
            KEYBOARDS.with_borrow_mut(|keyboards| {
                keyboards
                    .get_mut(&window.id())?
                    .pending
                    .take()
                    .filter(|event| event.down == down)
            })
        })
        .flatten()
}

pub(super) fn keyboard_frame(app: &AppWindow) -> KeyboardFrame {
    app.window()
        .with_winit_window(|window| {
            KEYBOARDS.with_borrow_mut(|keyboards| {
                let keyboard = keyboards.entry(window.id()).or_insert_with(|| Keyboard {
                    focused: window.has_focus(),
                    ..Keyboard::default()
                });
                keyboard.retire_pending();
                KeyboardFrame {
                    released: std::mem::take(&mut keyboard.released),
                    focused: keyboard.focused,
                    shift: keyboard.shift,
                    composing: keyboard.composing,
                    reset: std::mem::take(&mut keyboard.reset),
                    cancel_prefix: std::mem::take(&mut keyboard.cancel_prefix),
                }
            })
        })
        .unwrap_or_default()
}

pub(super) fn reset_keyboard(app: &AppWindow) {
    app.window().with_winit_window(|window| {
        KEYBOARDS.with_borrow_mut(|keyboards| {
            if let Some(keyboard) = keyboards.get_mut(&window.id()) {
                keyboard.pending = None;
                keyboard.pressed.clear();
                keyboard.released.clear();
                keyboard.reset = true;
            }
        });
    });
}

pub(super) fn admit_component_change(window: &slint::Window) -> Result<(), String> {
    let busy = window
        .with_winit_window(|window| {
            let keyboard_busy = KEYBOARDS.with_borrow(|keyboards| {
                keyboards.get(&window.id()).is_some_and(|keyboard| {
                    keyboard.composing
                        || !keyboard.mouse_buttons.is_empty()
                        || !keyboard.touches.is_empty()
                })
            });
            keyboard_busy || STATE.with_borrow(|state| state.owner == Some(window.id()))
        })
        .ok_or("The workspace native input state is unavailable")?;
    if busy {
        return Err(
            "Finish pointer input, camera capture or text composition before changing documents"
                .into(),
        );
    }
    Ok(())
}

fn grab(window: &slint::winit_030::winit::window::Window) -> bool {
    window.set_cursor_visible(false);
    if window.set_cursor_grab(CursorGrabMode::Locked).is_ok() {
        return true;
    }
    if window.set_cursor_grab(CursorGrabMode::Confined).is_err() {
        window.set_cursor_visible(true);
    }
    false
}

fn release(window: &slint::winit_030::winit::window::Window) {
    if window.set_cursor_grab(CursorGrabMode::None).is_ok() {
        window.set_cursor_visible(true);
    }
}

/// Keeps the cursor locked and hidden while this window's viewport looks or pans.
pub(super) fn sync(app: &AppWindow, looking: bool) {
    app.window().with_winit_window(|window| {
        let id = window.id();
        STATE.with_borrow_mut(|state| {
            let owned = state.owner == Some(id);
            if looking && !owned {
                state.relative = grab(window);
                state.owner = Some(id);
                state.delta = (0.0, 0.0);
            } else if !looking && owned {
                release(window);
                *state = State::default();
            }
        });
    });
}

/// Whether this window's viewport receives relative motion instead of cursor positions.
pub(super) fn relative(app: &AppWindow) -> bool {
    app.window()
        .with_winit_window(|window| {
            STATE.with_borrow(|state| state.relative && state.owner == Some(window.id()))
        })
        .unwrap_or(false)
}

/// Returns and clears the motion collected for this window since the last call.
pub(super) fn take_delta(app: &AppWindow) -> Option<(f32, f32)> {
    if !relative(app) {
        return None;
    }
    STATE.with_borrow_mut(|state| {
        let delta = std::mem::take(&mut state.delta);
        (delta != (0.0, 0.0)).then_some((delta.0 as f32, delta.1 as f32))
    })
}
