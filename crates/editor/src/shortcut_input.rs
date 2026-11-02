use std::time::{Duration, Instant};

use editor_core::shortcuts::{Command, KeyInput, Resolution, SEQUENCE_TIMEOUT_MS, canonical_key};
use slint::platform::Key;

use crate::{session::Session, viewport::key_text};

#[derive(Default)]
pub struct State {
    authority: Option<String>,
    prefix: Option<(KeyInput, Instant)>,
}

impl State {
    pub fn reset(&mut self) {
        self.prefix = None;
    }

    pub fn sync(&mut self, authority: String) -> bool {
        if self.authority.as_ref() == Some(&authority) {
            return false;
        }
        self.authority = Some(authority);
        self.reset();
        true
    }

    pub fn expire(&mut self) {
        if self.prefix.as_ref().is_some_and(|(_, started)| {
            started.elapsed() >= Duration::from_millis(SEQUENCE_TIMEOUT_MS)
        }) {
            self.reset();
        }
    }

    fn resolve(
        &mut self,
        session: &Session,
        input: &KeyInput,
    ) -> Result<(bool, Resolution), String> {
        if let Some((prefix, _)) = self.prefix.take() {
            session
                .resolve_editor_shortcut_sequence(&[prefix, input.clone()])
                .map(|resolution| (true, resolution))
        } else {
            session
                .resolve_editor_shortcut_sequence(std::slice::from_ref(input))
                .map(|resolution| (false, resolution))
        }
    }
}

pub struct NativeKey {
    pub identity: Option<String>,
    pub logical: Option<String>,
    pub physical: Option<String>,
    pub down: bool,
    pub repeat: bool,
    pub synthetic: bool,
    pub dead: bool,
}

impl NativeKey {
    pub fn is_modifier(&self) -> bool {
        self.identity.as_deref().is_some_and(|identity| {
            ["Shift", "Control", "Alt", "Super"]
                .iter()
                .any(|prefix| identity.starts_with(prefix))
        })
    }
}

pub struct ViewportKey<'a> {
    pub text: &'a str,
    pub down: bool,
    pub repeat: bool,
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

pub enum Routed {
    Unhandled,
    Consumed,
    Command(Command),
    Camera { command: Command, identity: String },
}

fn event_key(text: &str) -> Option<String> {
    for (key, name) in [
        (Key::Delete, "Delete"),
        (Key::Backspace, "Backspace"),
        (Key::Return, "Enter"),
        (Key::Tab, "Tab"),
        (Key::Home, "Home"),
        (Key::End, "End"),
        (Key::Insert, "Insert"),
        (Key::UpArrow, "ArrowUp"),
        (Key::DownArrow, "ArrowDown"),
        (Key::LeftArrow, "ArrowLeft"),
        (Key::RightArrow, "ArrowRight"),
        (Key::PageUp, "PageUp"),
        (Key::PageDown, "PageDown"),
    ] {
        if text == key_text(key) {
            return Some(name.into());
        }
    }
    let functions = [
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
    ];
    functions
        .into_iter()
        .position(|key| text == key_text(key))
        .map(|index| format!("F{}", index + 1))
        .or_else(|| canonical_key(text))
}

pub fn route_viewport_key(
    state: &mut State,
    session: &Session,
    key: &ViewportKey<'_>,
    native: Option<&NativeKey>,
) -> Result<Routed, String> {
    state.expire();
    if !key.down {
        return Ok(Routed::Unhandled);
    }
    if native.is_some_and(|native| native.synthetic || native.dead) {
        state.reset();
        return Ok(Routed::Consumed);
    }
    if key.text == key_text(Key::Escape) {
        if key.repeat || native.is_some_and(|native| native.repeat) {
            return Ok(Routed::Consumed);
        }
        return Ok(if state.prefix.take().is_some() {
            Routed::Consumed
        } else {
            Routed::Unhandled
        });
    }
    let input = KeyInput {
        logical: native
            .and_then(|native| native.logical.clone())
            .or_else(|| event_key(key.text)),
        physical: native.and_then(|native| native.physical.clone()),
        control: key.control,
        shift: key.shift,
        alt: key.alt,
        meta: key.meta,
    };
    if input.is_command_search() {
        if key.repeat || native.is_some_and(|native| native.repeat) {
            state.reset();
            return Ok(Routed::Consumed);
        }
        state.resolve(session, &input)?;
        return Ok(Routed::Unhandled);
    }
    if input.logical.is_none() && input.physical.is_none() {
        let modifier = native.is_some_and(NativeKey::is_modifier);
        return Ok(if !modifier && state.prefix.take().is_some() {
            Routed::Consumed
        } else {
            Routed::Unhandled
        });
    }
    if key.repeat || native.is_some_and(|native| native.repeat) {
        return Ok(
            if state.prefix.is_some()
                || !matches!(
                    session.resolve_editor_shortcut_sequence(std::slice::from_ref(&input))?,
                    Resolution::None
                )
            {
                Routed::Consumed
            } else {
                Routed::Unhandled
            },
        );
    }
    let (pending, resolution) = state.resolve(session, &input)?;
    match resolution {
        Resolution::None if pending => Ok(Routed::Consumed),
        Resolution::None => Ok(Routed::Unhandled),
        Resolution::Prefix => {
            state.prefix = Some((input, Instant::now()));
            Ok(Routed::Consumed)
        }
        Resolution::Command(command) if command.is_camera() => {
            let identity = native
                .and_then(|native| native.identity.clone())
                .ok_or("Held camera shortcuts require a native physical key event")?;
            Ok(Routed::Camera { command, identity })
        }
        Resolution::Command(command) => Ok(Routed::Command(command)),
    }
}
