use std::path::PathBuf;

use serde::{Deserialize, Serialize};

mod authority;
mod binding;
mod matcher;
pub use authority::{Authority, Snapshot};
pub use binding::{
    Binding, Chord, KeyInput, LOGICAL_KEYS, MAX_KEY_BYTES, MAX_SEQUENCE_STROKES, PHYSICAL_KEYS,
    Resolution, SEQUENCE_TIMEOUT_MS, canonical_key, canonical_physical_key,
};

use crate::preference_file::{CoordinatedFile, ObservedSource, Publication, SaveError, Source};

pub const FILE_NAME: &str = "shortcuts.json";
pub const MAX_FILE_BYTES: u64 = 64 * 1024;
pub const MAX_QUERY_BYTES: usize = 128;
const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    NewMap,
    OpenMap,
    SaveMap,
    SaveMapAs,
    Copy,
    Cut,
    Paste,
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
    HideSelectionInEditor,
    ShowAllEditorHidden,
    CameraForward,
    CameraBackward,
    CameraLeft,
    CameraRight,
    CameraUp,
    CameraDown,
}

impl Command {
    pub const ALL: [Self; 36] = [
        Self::NewMap,
        Self::OpenMap,
        Self::SaveMap,
        Self::SaveMapAs,
        Self::Copy,
        Self::Cut,
        Self::Paste,
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
        Self::HideSelectionInEditor,
        Self::ShowAllEditorHidden,
        Self::CameraForward,
        Self::CameraBackward,
        Self::CameraLeft,
        Self::CameraRight,
        Self::CameraUp,
        Self::CameraDown,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::NewMap => "New map",
            Self::OpenMap => "Open map",
            Self::SaveMap => "Save map",
            Self::SaveMapAs => "Save map as",
            Self::Copy => "Copy objects",
            Self::Cut => "Cut objects",
            Self::Paste => "Paste objects",
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
            Self::HideSelectionInEditor => "Hide selected in editor",
            Self::ShowAllEditorHidden => "Show all editor-hidden",
            Self::CameraForward => "Camera forward",
            Self::CameraBackward => "Camera backward",
            Self::CameraLeft => "Camera left",
            Self::CameraRight => "Camera right",
            Self::CameraUp => "Camera up",
            Self::CameraDown => "Camera down",
        }
    }

    pub fn is_camera(self) -> bool {
        matches!(
            self,
            Self::CameraForward
                | Self::CameraBackward
                | Self::CameraLeft
                | Self::CameraRight
                | Self::CameraUp
                | Self::CameraDown
        )
    }

    pub fn is_frontend(self) -> bool {
        matches!(
            self,
            Self::NewMap
                | Self::OpenMap
                | Self::SaveMap
                | Self::SaveMapAs
                | Self::Copy
                | Self::Cut
                | Self::Paste
        )
    }
}

pub type Slots = [Option<Binding>; 2];

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CommandRow {
    pub command: Command,
    pub label: &'static str,
    pub bindings: Slots,
    pub display: String,
    pub factory_display: String,
}

#[derive(Debug, Serialize)]
pub struct CommandSearch {
    pub query: String,
    pub snapshot: String,
    pub total: usize,
    pub matched: usize,
    pub commands: Vec<CommandRow>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Bindings {
    #[serde(default = "new_map_default")]
    pub new_map: Slots,
    #[serde(default = "open_map_default")]
    pub open_map: Slots,
    #[serde(default = "save_map_default")]
    pub save_map: Slots,
    #[serde(default = "save_map_as_default")]
    pub save_map_as: Slots,
    #[serde(default = "copy_default")]
    pub copy: Slots,
    #[serde(default = "cut_default")]
    pub cut: Slots,
    #[serde(default = "paste_default")]
    pub paste: Slots,
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
    #[serde(default)]
    pub hide_selection_in_editor: Slots,
    #[serde(default)]
    pub show_all_editor_hidden: Slots,
    #[serde(default = "camera_forward_default")]
    pub camera_forward: Slots,
    #[serde(default = "camera_backward_default")]
    pub camera_backward: Slots,
    #[serde(default = "camera_left_default")]
    pub camera_left: Slots,
    #[serde(default = "camera_right_default")]
    pub camera_right: Slots,
    #[serde(default = "camera_up_default")]
    pub camera_up: Slots,
    #[serde(default = "camera_down_default")]
    pub camera_down: Slots,
}

fn chord(key: &str, control: bool, shift: bool) -> Option<Binding> {
    Some(Binding::Single(Chord {
        key: key.into(),
        control,
        shift,
        alt: false,
        meta: false,
        physical: false,
    }))
}

fn new_map_default() -> Slots {
    [chord("N", true, false), None]
}

fn open_map_default() -> Slots {
    [chord("O", true, false), None]
}

fn save_map_default() -> Slots {
    [chord("S", true, false), None]
}

fn save_map_as_default() -> Slots {
    [chord("S", true, true), None]
}

fn copy_default() -> Slots {
    [chord("C", true, false), None]
}

fn cut_default() -> Slots {
    [chord("X", true, false), None]
}

fn paste_default() -> Slots {
    [chord("V", true, false), None]
}

fn camera_forward_default() -> Slots {
    [chord("W", false, false), None]
}

fn camera_backward_default() -> Slots {
    [chord("S", false, false), None]
}

fn camera_left_default() -> Slots {
    [chord("A", false, false), None]
}

fn camera_right_default() -> Slots {
    [chord("D", false, false), None]
}

fn camera_up_default() -> Slots {
    [chord("E", false, false), None]
}

fn camera_down_default() -> Slots {
    [chord("Q", false, false), None]
}

impl Default for Bindings {
    fn default() -> Self {
        Self {
            new_map: new_map_default(),
            open_map: open_map_default(),
            save_map: save_map_default(),
            save_map_as: save_map_as_default(),
            copy: copy_default(),
            cut: cut_default(),
            paste: paste_default(),
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
            hide_selection_in_editor: [None, None],
            show_all_editor_hidden: [None, None],
            camera_forward: camera_forward_default(),
            camera_backward: camera_backward_default(),
            camera_left: camera_left_default(),
            camera_right: camera_right_default(),
            camera_up: camera_up_default(),
            camera_down: camera_down_default(),
        }
    }
}

impl Bindings {
    pub fn command_rows(&self) -> Vec<CommandRow> {
        let factory = Self::default();
        Command::ALL
            .into_iter()
            .map(|command| CommandRow {
                command,
                label: command.label(),
                bindings: self.slots(command).clone(),
                display: self.label(command),
                factory_display: factory.label(command),
            })
            .collect()
    }

    pub fn find_commands(&self, query: &str) -> Result<Vec<CommandRow>, String> {
        if query.len() > MAX_QUERY_BYTES {
            return Err(format!(
                "Command search is limited to {MAX_QUERY_BYTES} UTF-8 bytes"
            ));
        }
        let mut bindings = self.clone();
        for command in Command::ALL {
            for binding in bindings.slots_mut(command).iter_mut().flatten() {
                binding.canonicalize_for_search()?;
            }
        }
        let query = query.to_lowercase();
        let terms = query.split_whitespace().collect::<Vec<_>>();
        Ok(bindings
            .command_rows()
            .into_iter()
            .filter(|row| {
                let id = serde_json::to_value(row.command).unwrap();
                let fields = [
                    row.label,
                    id.as_str().unwrap(),
                    &row.display,
                    &row.factory_display,
                ]
                .map(str::to_lowercase);
                terms
                    .iter()
                    .all(|term| fields.iter().any(|field| field.contains(*term)))
            })
            .collect())
    }

    pub fn slots(&self, command: Command) -> &Slots {
        match command {
            Command::NewMap => &self.new_map,
            Command::OpenMap => &self.open_map,
            Command::SaveMap => &self.save_map,
            Command::SaveMapAs => &self.save_map_as,
            Command::Copy => &self.copy,
            Command::Cut => &self.cut,
            Command::Paste => &self.paste,
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
            Command::HideSelectionInEditor => &self.hide_selection_in_editor,
            Command::ShowAllEditorHidden => &self.show_all_editor_hidden,
            Command::CameraForward => &self.camera_forward,
            Command::CameraBackward => &self.camera_backward,
            Command::CameraLeft => &self.camera_left,
            Command::CameraRight => &self.camera_right,
            Command::CameraUp => &self.camera_up,
            Command::CameraDown => &self.camera_down,
        }
    }

    pub fn slots_mut(&mut self, command: Command) -> &mut Slots {
        match command {
            Command::NewMap => &mut self.new_map,
            Command::OpenMap => &mut self.open_map,
            Command::SaveMap => &mut self.save_map,
            Command::SaveMapAs => &mut self.save_map_as,
            Command::Copy => &mut self.copy,
            Command::Cut => &mut self.cut,
            Command::Paste => &mut self.paste,
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
            Command::HideSelectionInEditor => &mut self.hide_selection_in_editor,
            Command::ShowAllEditorHidden => &mut self.show_all_editor_hidden,
            Command::CameraForward => &mut self.camera_forward,
            Command::CameraBackward => &mut self.camera_backward,
            Command::CameraLeft => &mut self.camera_left,
            Command::CameraRight => &mut self.camera_right,
            Command::CameraUp => &mut self.camera_up,
            Command::CameraDown => &mut self.camera_down,
        }
    }

    pub fn label(&self, command: Command) -> String {
        let labels = self
            .slots(command)
            .iter()
            .flatten()
            .map(Binding::label)
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
