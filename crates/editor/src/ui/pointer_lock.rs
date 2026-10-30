use std::cell::RefCell;

use slint::ComponentHandle;
use slint::winit_030::winit::event::{DeviceEvent, DeviceId};
use slint::winit_030::winit::event_loop::ActiveEventLoop;
use slint::winit_030::winit::window::{CursorGrabMode, WindowId};
use slint::winit_030::{CustomApplicationHandler, EventResult, WinitWindowAccessor};

use crate::AppWindow;

#[derive(Default)]
struct State {
    owner: Option<WindowId>,
    relative: bool,
    delta: (f64, f64),
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

/// Collects relative mouse motion, which is the only motion a locked pointer reports.
pub(super) struct Handler;

impl CustomApplicationHandler for Handler {
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
