use super::super::*;
use crate::shortcuts::{Bindings, Command, KeyInput, Resolution};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum ShortcutsArgs {
    Save {
        snapshot: String,
        bindings: Box<Bindings>,
    },
    Reload {
        snapshot: String,
    },
    Reset {
        snapshot: String,
    },
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditorCommandArgs {
    command: Command,
    aspect: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindEditorCommandsArgs {
    #[serde(default)]
    query: String,
    bindings: Option<Box<Bindings>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolveKeyboardShortcutArgs {
    #[schemars(length(min = 1, max = 2))]
    inputs: Vec<KeyInput>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CameraNavigationArgs {
    #[schemars(length(min = 1, max = 6))]
    commands: Vec<Command>,
    #[schemars(range(min = 0.0, max = 1.0))]
    seconds: f32,
    #[serde(default)]
    fast: bool,
}

#[tool_router(router = shortcut_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Search the same 36-command catalog as Keyboard shortcuts… → Search commands. query is at most 128 UTF-8 bytes; case-insensitive whitespace-separated literal terms must all match a command label, typed ID, current/form binding display or factory binding display. Empty query returns every command in catalog order. Optional typed bindings searches an unsaved form without activating it; incomplete or invalid draft bindings remain searchable and Save performs validation. Unknown fields reject. Returns query, shared preference snapshot, total, matched and command rows. Reads current in-memory settings only; no execution, storage access, map/history/camera or preference changes.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn find_editor_commands(
        &self,
        Parameters(args): Parameters<FindEditorCommandsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .find_editor_commands(&args.query, args.bindings.as_deref())
                .map(|report| json!(report))
        })
    }

    #[tool(
        description = "Inspect all 36 shared keyboard commands with two nullable primary/alternate bindings, factory values, labels, logical/physical key catalogs, reserved controls, sequence timeout, preference snapshot, storage path and load errors. Includes New/Open/Save/Save as, Copy/Cut/Paste, six held camera directions and the existing viewport commands. Bindings accept one chord or a two-stroke sequence, logical or physical keys and exact Ctrl/Shift/Alt/Meta modifiers. Held camera bindings are single-stroke and ignore Shift for the independent speed boost. Text fields retain native editing. Saved shortcuts load at startup; malformed storage stays untouched and current valid bindings remain usable. No map/history/camera mutation.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_keyboard_shortcuts(&self) -> ToolResult {
        ok(self.session().keyboard_shortcuts_state())
    }

    #[tool(
        description = "Configure shortcuts through the same Session operation as Keyboard shortcuts…. Requires get_keyboard_shortcuts.snapshot. action=save takes bindings keyed by command, each exactly two nullable slots. A slot is a legacy single chord {key,control,shift,alt,meta,physical} or {sequence:[chord,chord]}; alt/meta/physical default false. physical=false names logical keys; physical=true uses the reported physical-key catalog. Same-family duplicates and single/sequence prefix conflicts reject; mixed logical/physical overlaps are checked for ambiguity during resolution. Camera directions require a single chord with shift=false; event Shift remains the speed boost. Escape, unmodified or Shift-only arrows/PageUp/PageDown, and Ctrl+K without extra modifiers remain reserved. Legacy version1 files missing new file/clipboard/camera command fields receive their former fixed defaults; older optional viewport commands remain unbound. Save persists atomically and activates; reload activates fully valid storage and preserves current bindings on failure; reset explicitly writes/activates factory defaults. Save is blocked after load/source errors until Reload succeeds or explicit Reset. Writes compare inspected file identity/bytes and stale snapshots reject. write_committed reports completed writes even if later verification fails. Participating writers use the existing coordinated publication and original directory binding. No map/history/source/camera changes."
    )]
    pub(in crate::mcp) fn set_keyboard_shortcuts(
        &self,
        Parameters(args): Parameters<ShortcutsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            match args {
                ShortcutsArgs::Save { snapshot, bindings } => session.change_keyboard_shortcuts(
                    Actor::Agent,
                    &snapshot,
                    "save",
                    Some(*bindings),
                ),
                ShortcutsArgs::Reload { snapshot } => {
                    session.change_keyboard_shortcuts(Actor::Agent, &snapshot, "reload", None)
                }
                ShortcutsArgs::Reset { snapshot } => {
                    session.change_keyboard_shortcuts(Actor::Agent, &snapshot, "reset", None)
                }
            }
        })
    }

    #[tool(
        description = "Resolve one or two observed key inputs against the current shared keyboard bindings, using the same resolver as native input. Each input requires at least one supported logical/physical key name and all control/shift/alt/meta booleans; omit an unavailable key family or use null, using names from get_keyboard_shortcuts.logical_keys/physical_keys. Returns status none, prefix or command with the matched typed command. Overlapping logical/physical matches produce an explicit ambiguity error instead of choosing a command. Held camera matches ignore Shift; camera bindings cannot be sequences. This stateless inspection does not hold keys, advance a native sequence timeout, execute commands or change preferences/map/history/camera.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn resolve_keyboard_shortcut(
        &self,
        Parameters(args): Parameters<ResolveKeyboardShortcutArgs>,
    ) -> ToolResult {
        run(|| {
            let resolution = self
                .session()
                .resolve_editor_shortcut_sequence(&args.inputs)?;
            let (status, command) = match resolution {
                Resolution::None => ("none", None),
                Resolution::Prefix => ("prefix", None),
                Resolution::Command(command) => ("command", Some(command)),
            };
            Ok(json!({"status":status,"command":command}))
        })
    }

    #[tool(
        description = "Move the shared camera with the same normalized movement calculation as held native camera shortcuts. commands contains 1–6 distinct camera_forward/camera_backward/camera_left/camera_right/camera_up/camera_down values; opposing directions cancel. seconds is finite from 0 to 1. fast defaults false and applies the independent Shift speed multiplier to the Session's current fly_speed. Perspective moves forward/right/world-up; planar views move screen-up/right and ignore up/down before normalization. Returns actual position/projection and camera_changed. No map/history/preferences writes. Changed cameras use the normal Session camera operation, including captured-gesture and camera-following visibility invalidation; this does not synthesize keyboard input."
    )]
    pub(in crate::mcp) fn step_camera_navigation(
        &self,
        Parameters(args): Parameters<CameraNavigationArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .navigate_camera(Actor::Agent, &args.commands, args.seconds, args.fast)
        })
    }

    #[tool(
        description = "Execute a shared Session viewport command through the same operations used by native shortcuts. New/Open/Save/Save as and Copy/Cut/Paste return explicit guidance to new_map/open_map/save_map and the guarded clipboard tools, which accept their required inputs; this generic call never opens a dialog or fabricates a file/clipboard action. Camera direction commands require step_camera_navigation with an explicit duration. undo/redo performs one history step, with label=null when unavailable. frame_selection uses positive finite aspect (default1). delete_selection and duplicate_selection require whole-object scope and a nonempty selection; duplication uses [grid,grid,0] and selects copies. perspective/top/front/side preserve remembered projection cameras. objects/faces use shared selection-mode cleanup. show_grid/snap_to_grid/tool_textures/selection_outlines toggle the current Session setting without saving defaults. select_tool/move_tool/rotate_tool/scale_tool use shared transform and paint-disable operations; unchanged tools preserve pending work. draw_brush and clip_mode activate their existing authoring workflows, selecting Top when required; clip requires selected brushes. hide_selection_in_editor/show_all_editor_hidden use shared temporary visibility and return its report; saved visibility, selection, source, history and builds stay unchanged. Existing gesture retirement and ownership guards apply. This does not synthesize input or change shortcut preferences."
    )]
    pub(in crate::mcp) fn execute_editor_command(
        &self,
        Parameters(args): Parameters<EditorCommandArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().execute_editor_command(
                Actor::Agent,
                args.command,
                args.aspect.unwrap_or(1.0),
            )
        })
    }
}
