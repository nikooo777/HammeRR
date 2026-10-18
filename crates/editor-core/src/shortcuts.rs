use std::{collections::BTreeSet, path::PathBuf};

use serde::{Deserialize, Serialize};

mod authority;
pub use authority::{Authority, Snapshot};

use crate::preference_file::{CoordinatedFile, ObservedSource, Publication, SaveError, Source};

pub const FILE_NAME: &str = "shortcuts.json";
pub const MAX_FILE_BYTES: u64 = 16 * 1024;
const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    Undo,
    Redo,
    FrameSelection,
    DeleteSelection,
    DuplicateSelection,
    Perspective,
    Top,
    Front,
    Side,
    Objects,
    Faces,
    ShowGrid,
    SnapToGrid,
    ToolTextures,
    SelectionOutlines,
    SelectTool,
    MoveTool,
    RotateTool,
    ScaleTool,
    DrawBrush,
    ClipMode,
}

impl Command {
    pub const ALL: [Self; 21] = [
        Self::Undo,
        Self::Redo,
        Self::FrameSelection,
        Self::DeleteSelection,
        Self::DuplicateSelection,
        Self::Perspective,
        Self::Top,
        Self::Front,
        Self::Side,
        Self::Objects,
        Self::Faces,
        Self::ShowGrid,
        Self::SnapToGrid,
        Self::ToolTextures,
        Self::SelectionOutlines,
        Self::SelectTool,
        Self::MoveTool,
        Self::RotateTool,
        Self::ScaleTool,
        Self::DrawBrush,
        Self::ClipMode,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::FrameSelection => "Frame selection",
            Self::DeleteSelection => "Delete selection",
            Self::DuplicateSelection => "Duplicate selection",
            Self::Perspective => "Perspective",
            Self::Top => "Top",
            Self::Front => "Front",
            Self::Side => "Side",
            Self::Objects => "Objects",
            Self::Faces => "Faces",
            Self::ShowGrid => "Show grid",
            Self::SnapToGrid => "Snap to grid",
            Self::ToolTextures => "Tool textures",
            Self::SelectionOutlines => "Selection outlines",
            Self::SelectTool => "Select tool",
            Self::MoveTool => "Move handles",
            Self::RotateTool => "Rotate handles",
            Self::ScaleTool => "Scale handles",
            Self::DrawBrush => "Draw brush",
            Self::ClipMode => "Clip mode",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Chord {
    pub key: String,
    pub control: bool,
    pub shift: bool,
}

pub fn canonical_key(value: &str) -> Option<String> {
    if value.len() == 1 && value.as_bytes()[0].is_ascii_alphabetic() {
        return Some(value.to_ascii_uppercase());
    }
    if value.eq_ignore_ascii_case("delete") {
        return Some("Delete".into());
    }
    if value.eq_ignore_ascii_case("backspace") {
        return Some("Backspace".into());
    }
    let number = value
        .strip_prefix('F')
        .or_else(|| value.strip_prefix('f'))?
        .parse::<u8>()
        .ok()?;
    (1..=12).contains(&number).then(|| format!("F{number}"))
}

impl Chord {
    pub fn label(&self) -> String {
        format!(
            "{}{}{}",
            if self.control { "Ctrl+" } else { "" },
            if self.shift { "Shift+" } else { "" },
            self.key
        )
    }

    fn validated(mut self) -> Result<Self, String> {
        self.key = canonical_key(&self.key).ok_or("Choose A–Z, F1–F12, Delete or Backspace")?;
        if (!self.control && matches!(self.key.as_str(), "W" | "A" | "S" | "D" | "Q" | "E"))
            || (self.control && matches!(self.key.as_str(), "N" | "O" | "S" | "C" | "X" | "V"))
        {
            return Err(format!(
                "{} is reserved for camera, file or clipboard controls",
                self.label()
            ));
        }
        Ok(self)
    }
}

pub type Slots = [Option<Chord>; 2];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Bindings {
    pub undo: Slots,
    pub redo: Slots,
    pub frame_selection: Slots,
    pub delete_selection: Slots,
    pub duplicate_selection: Slots,
    #[serde(default)]
    pub perspective: Slots,
    #[serde(default)]
    pub top: Slots,
    #[serde(default)]
    pub front: Slots,
    #[serde(default)]
    pub side: Slots,
    #[serde(default)]
    pub objects: Slots,
    #[serde(default)]
    pub faces: Slots,
    #[serde(default)]
    pub show_grid: Slots,
    #[serde(default)]
    pub snap_to_grid: Slots,
    #[serde(default)]
    pub tool_textures: Slots,
    #[serde(default)]
    pub selection_outlines: Slots,
    #[serde(default)]
    pub select_tool: Slots,
    #[serde(default)]
    pub move_tool: Slots,
    #[serde(default)]
    pub rotate_tool: Slots,
    #[serde(default)]
    pub scale_tool: Slots,
    #[serde(default)]
    pub draw_brush: Slots,
    #[serde(default)]
    pub clip_mode: Slots,
}

fn chord(key: &str, control: bool, shift: bool) -> Option<Chord> {
    Some(Chord {
        key: key.into(),
        control,
        shift,
    })
}

impl Default for Bindings {
    fn default() -> Self {
        Self {
            undo: [chord("Z", true, false), None],
            redo: [chord("Z", true, true), chord("Y", true, false)],
            frame_selection: [chord("F", false, false), None],
            delete_selection: [
                chord("Delete", false, false),
                chord("Backspace", false, false),
            ],
            duplicate_selection: [chord("D", true, false), None],
            perspective: [None, None],
            top: [None, None],
            front: [None, None],
            side: [None, None],
            objects: [None, None],
            faces: [None, None],
            show_grid: [None, None],
            snap_to_grid: [None, None],
            tool_textures: [None, None],
            selection_outlines: [None, None],
            select_tool: [None, None],
            move_tool: [None, None],
            rotate_tool: [None, None],
            scale_tool: [None, None],
            draw_brush: [None, None],
            clip_mode: [None, None],
        }
    }
}

impl Bindings {
    pub fn slots(&self, command: Command) -> &Slots {
        match command {
            Command::Undo => &self.undo,
            Command::Redo => &self.redo,
            Command::FrameSelection => &self.frame_selection,
            Command::DeleteSelection => &self.delete_selection,
            Command::DuplicateSelection => &self.duplicate_selection,
            Command::Perspective => &self.perspective,
            Command::Top => &self.top,
            Command::Front => &self.front,
            Command::Side => &self.side,
            Command::Objects => &self.objects,
            Command::Faces => &self.faces,
            Command::ShowGrid => &self.show_grid,
            Command::SnapToGrid => &self.snap_to_grid,
            Command::ToolTextures => &self.tool_textures,
            Command::SelectionOutlines => &self.selection_outlines,
            Command::SelectTool => &self.select_tool,
            Command::MoveTool => &self.move_tool,
            Command::RotateTool => &self.rotate_tool,
            Command::ScaleTool => &self.scale_tool,
            Command::DrawBrush => &self.draw_brush,
            Command::ClipMode => &self.clip_mode,
        }
    }

    pub fn slots_mut(&mut self, command: Command) -> &mut Slots {
        match command {
            Command::Undo => &mut self.undo,
            Command::Redo => &mut self.redo,
            Command::FrameSelection => &mut self.frame_selection,
            Command::DeleteSelection => &mut self.delete_selection,
            Command::DuplicateSelection => &mut self.duplicate_selection,
            Command::Perspective => &mut self.perspective,
            Command::Top => &mut self.top,
            Command::Front => &mut self.front,
            Command::Side => &mut self.side,
            Command::Objects => &mut self.objects,
            Command::Faces => &mut self.faces,
            Command::ShowGrid => &mut self.show_grid,
            Command::SnapToGrid => &mut self.snap_to_grid,
            Command::ToolTextures => &mut self.tool_textures,
            Command::SelectionOutlines => &mut self.selection_outlines,
            Command::SelectTool => &mut self.select_tool,
            Command::MoveTool => &mut self.move_tool,
            Command::RotateTool => &mut self.rotate_tool,
            Command::ScaleTool => &mut self.scale_tool,
            Command::DrawBrush => &mut self.draw_brush,
            Command::ClipMode => &mut self.clip_mode,
        }
    }

    pub fn validated(mut self) -> Result<Self, String> {
        let mut seen = BTreeSet::new();
        for command in Command::ALL {
            for slot in self.slots_mut(command) {
                if let Some(chord) = slot.take() {
                    let chord = chord.validated()?;
                    if !seen.insert((chord.key.clone(), chord.control, chord.shift)) {
                        return Err(format!("{} is assigned more than once", chord.label()));
                    }
                    *slot = Some(chord);
                }
            }
        }
        Ok(self)
    }

    pub fn resolve(&self, chord: &Chord) -> Option<Command> {
        Command::ALL.into_iter().find(|command| {
            self.slots(*command)
                .iter()
                .flatten()
                .any(|binding| binding == chord)
        })
    }

    pub fn label(&self, command: Command) -> String {
        let labels = self
            .slots(command)
            .iter()
            .flatten()
            .map(Chord::label)
            .collect::<Vec<_>>();
        if labels.is_empty() {
            "Unbound".into()
        } else {
            labels.join(" / ")
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    version: u32,
    bindings: Bindings,
}

pub(crate) struct Store {
    pub(crate) directory: PathBuf,
}

impl Store {
    pub(crate) fn path(&self) -> PathBuf {
        self.directory.join(FILE_NAME)
    }

    fn target(&self) -> CoordinatedFile<'_> {
        CoordinatedFile {
            directory: &self.directory,
            file_name: FILE_NAME,
            lock_name: ".shortcuts.lock",
            label: "Keyboard shortcuts",
            reload: "Reload saved",
            saved_description: "saved bindings",
            max_bytes: MAX_FILE_BYTES,
        }
    }

    pub(crate) fn read_source(&self) -> Result<ObservedSource, String> {
        self.target().observe()
    }

    pub(crate) fn read_current(&self, expected: &ObservedSource) -> Result<ObservedSource, String> {
        self.target().observe_from(expected)
    }

    pub(crate) fn parse(&self, source: &ObservedSource) -> Result<Option<Bindings>, String> {
        let Source::File { bytes, length, .. } = &source.source else {
            return Ok(None);
        };
        if *length > MAX_FILE_BYTES {
            return Err(format!(
                "{} exceeds {MAX_FILE_BYTES} bytes; file is preserved",
                self.path().display()
            ));
        }
        let value: Preferences = serde_json::from_slice(bytes)
            .map_err(|error| format!("{}: {error}", self.path().display()))?;
        if value.version != FORMAT_VERSION {
            return Err(format!(
                "Unsupported keyboard shortcuts version {}; file is preserved",
                value.version
            ));
        }
        value.bindings.validated().map(Some)
    }

    #[cfg(test)]
    pub(crate) fn save(&self, bindings: Bindings) -> Result<(), String> {
        let source = self.read_source()?;
        self.save_checked(bindings, &source)
            .map_err(|error| error.message)?
            .source
            .map(|_| ())
    }

    pub(crate) fn save_checked(
        &self,
        bindings: Bindings,
        expected: &ObservedSource,
    ) -> Result<Publication, SaveError> {
        let bytes = serde_json::to_vec_pretty(&Preferences {
            version: FORMAT_VERSION,
            bindings: bindings.validated().map_err(SaveError::failure)?,
        })
        .map_err(|error| SaveError::failure(error.to_string()))?;
        self.target().publish(&bytes, expected)
    }
}

#[cfg(test)]
mod tests;
