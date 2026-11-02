use serde::{Deserialize, Serialize};

use super::Command;

pub const MAX_KEY_BYTES: usize = 32;
pub const MAX_SEQUENCE_STROKES: usize = 2;
pub const SEQUENCE_TIMEOUT_MS: u64 = 1000;

pub const LOGICAL_KEYS: &[&str] = &[
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "Delete",
    "Backspace",
    "Space",
    "Enter",
    "Tab",
    "Home",
    "End",
    "Insert",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "PageUp",
    "PageDown",
];

pub const PHYSICAL_KEYS: &[&str] = &[
    "KeyA",
    "KeyB",
    "KeyC",
    "KeyD",
    "KeyE",
    "KeyF",
    "KeyG",
    "KeyH",
    "KeyI",
    "KeyJ",
    "KeyK",
    "KeyL",
    "KeyM",
    "KeyN",
    "KeyO",
    "KeyP",
    "KeyQ",
    "KeyR",
    "KeyS",
    "KeyT",
    "KeyU",
    "KeyV",
    "KeyW",
    "KeyX",
    "KeyY",
    "KeyZ",
    "Digit0",
    "Digit1",
    "Digit2",
    "Digit3",
    "Digit4",
    "Digit5",
    "Digit6",
    "Digit7",
    "Digit8",
    "Digit9",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "Delete",
    "Backspace",
    "Space",
    "Enter",
    "Tab",
    "Home",
    "End",
    "Insert",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "PageUp",
    "PageDown",
];

pub fn canonical_key(value: &str) -> Option<String> {
    if value.len() > MAX_KEY_BYTES {
        return None;
    }
    if value == " " {
        return Some("Space".into());
    }
    if let Some(number) = value.strip_prefix('F').or_else(|| value.strip_prefix('f'))
        && let Ok(number) = number.parse::<u8>()
        && (1..=12).contains(&number)
    {
        return Some(format!("F{number}"));
    }
    LOGICAL_KEYS
        .iter()
        .find(|key| key.eq_ignore_ascii_case(value))
        .map(|key| (*key).into())
}

pub fn canonical_physical_key(value: &str) -> Option<String> {
    if value.len() > MAX_KEY_BYTES {
        return None;
    }
    PHYSICAL_KEYS
        .iter()
        .find(|key| key.eq_ignore_ascii_case(value))
        .map(|key| (*key).into())
}

fn command_search(key: &str, control: bool, shift: bool, alt: bool, meta: bool) -> bool {
    key.eq_ignore_ascii_case("K") && control && !shift && !alt && !meta
}

fn selection_nudge(key: &str, control: bool, alt: bool, meta: bool) -> bool {
    matches!(
        key,
        "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "PageUp" | "PageDown"
    ) && !control
        && !alt
        && !meta
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Chord {
    #[schemars(length(max = 32))]
    pub key: String,
    pub control: bool,
    pub shift: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub meta: bool,
    #[serde(default)]
    pub physical: bool,
}

impl Chord {
    pub fn label(&self) -> String {
        format!(
            "{}{}{}{}{}{}",
            if self.physical { "Physical " } else { "" },
            if self.control { "Ctrl+" } else { "" },
            if self.shift { "Shift+" } else { "" },
            if self.alt { "Alt+" } else { "" },
            if self.meta { "Meta+" } else { "" },
            self.key
        )
    }

    pub(super) fn canonicalized_key(&self) -> Option<String> {
        if self.physical {
            canonical_physical_key(&self.key)
        } else {
            canonical_key(&self.key)
        }
    }

    pub(super) fn validate(&mut self) -> Result<(), String> {
        self.key = self
            .canonicalized_key()
            .ok_or("Choose a supported logical or physical key from the shortcut catalog")?;
        let nudge = selection_nudge(&self.key, self.control, self.alt, self.meta);
        let key = if self.physical && self.key == "KeyK" {
            "K"
        } else {
            &self.key
        };
        let search = command_search(key, self.control, self.shift, self.alt, self.meta);
        if nudge || search {
            return Err(format!(
                "{} is reserved for selection nudging or command search",
                self.label()
            ));
        }
        Ok(())
    }

    pub(super) fn matches(&self, input: &KeyInput, ignore_shift: bool) -> bool {
        let key = if self.physical {
            &input.physical
        } else {
            &input.logical
        };
        key.as_ref() == Some(&self.key)
            && self.control == input.control
            && (ignore_shift || self.shift == input.shift)
            && self.alt == input.alt
            && self.meta == input.meta
    }

    pub(super) fn overlaps(&self, other: &Self, ignore_shift: bool) -> bool {
        self.physical == other.physical
            && self.key == other.key
            && self.control == other.control
            && (ignore_shift || self.shift == other.shift)
            && self.alt == other.alt
            && self.meta == other.meta
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum Binding {
    Single(Chord),
    Sequence {
        sequence: [Chord; MAX_SEQUENCE_STROKES],
    },
}

impl Binding {
    pub fn strokes(&self) -> &[Chord] {
        match self {
            Self::Single(chord) => std::slice::from_ref(chord),
            Self::Sequence { sequence } => sequence,
        }
    }

    fn strokes_mut(&mut self) -> &mut [Chord] {
        match self {
            Self::Single(chord) => std::slice::from_mut(chord),
            Self::Sequence { sequence } => sequence,
        }
    }

    pub fn label(&self) -> String {
        self.strokes()
            .iter()
            .map(Chord::label)
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(super) fn canonicalize_for_search(&mut self) -> Result<(), String> {
        for chord in self.strokes_mut() {
            if chord.key.len() > MAX_KEY_BYTES {
                return Err(format!(
                    "Shortcut keys are limited to {MAX_KEY_BYTES} UTF-8 bytes"
                ));
            }
            if let Some(key) = chord.canonicalized_key() {
                chord.key = key;
            }
        }
        Ok(())
    }

    pub(super) fn validate(&mut self, command: Command) -> Result<(), String> {
        if command.is_camera() && self.strokes().len() != 1 {
            return Err("Camera direction bindings require one stroke".into());
        }
        for chord in self.strokes_mut() {
            chord.validate()?;
            if command.is_camera() && chord.shift {
                return Err("Camera direction bindings cannot include Shift; Shift independently boosts movement".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyInput {
    #[schemars(length(max = 32))]
    pub logical: Option<String>,
    #[schemars(length(max = 32))]
    pub physical: Option<String>,
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl KeyInput {
    pub fn is_command_search(&self) -> bool {
        self.logical
            .as_deref()
            .is_some_and(|key| command_search(key, self.control, self.shift, self.alt, self.meta))
    }

    pub fn is_selection_nudge(&self) -> bool {
        self.logical
            .as_deref()
            .is_some_and(|key| selection_nudge(key, self.control, self.alt, self.meta))
    }

    pub(super) fn reserved_control(&self) -> Option<&'static str> {
        if self.is_command_search() {
            Some("Ctrl+K command search")
        } else if self.is_selection_nudge() {
            Some("selection nudging")
        } else {
            None
        }
    }

    pub(super) fn validated(&self) -> Result<Self, String> {
        let logical = self
            .logical
            .as_deref()
            .map(|key| {
                canonical_key(key)
                    .ok_or_else(|| "Shortcut input has an unsupported logical key".to_owned())
            })
            .transpose()?;
        let physical = self
            .physical
            .as_deref()
            .map(|key| {
                canonical_physical_key(key)
                    .ok_or_else(|| "Shortcut input has an unsupported physical key".to_owned())
            })
            .transpose()?;
        if logical.is_none() && physical.is_none() {
            return Err("Shortcut input requires a logical or physical key".into());
        }
        Ok(Self {
            logical,
            physical,
            control: self.control,
            shift: self.shift,
            alt: self.alt,
            meta: self.meta,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", content = "command", rename_all = "snake_case")]
pub enum Resolution {
    None,
    Prefix,
    Command(Command),
}
