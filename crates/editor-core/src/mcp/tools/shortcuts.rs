use super::super::*;
use crate::shortcuts::{Bindings, Command};

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

#[tool_router(router = shortcut_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Search the same 21-command catalog as Keyboard shortcuts… → Search commands. query is at most 128 UTF-8 bytes; case-insensitive whitespace-separated literal terms must all match a command label, typed ID, current/form binding display or factory binding display. Empty query returns every command in catalog order. Optional complete typed bindings searches an unsaved form without activating it; duplicate/reserved draft chords remain searchable, unsupported key names and unknown fields reject. Returns query, shared preference snapshot, total, matched and command rows. Reads current in-memory settings only; no command execution, storage read/write, map/history/camera or preference changes.",
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
        description = "Inspect the shared viewport shortcut configuration: undo, redo, frame_selection, delete_selection, duplicate_selection, perspective, top, front, side, objects, faces, show_grid, snap_to_grid, tool_textures, selection_outlines, select_tool, move_tool, rotate_tool, scale_tool, draw_brush and clip_mode, with exact primary/alternate nullable chord slots, factory defaults, command labels, scope/reserved keys, preference snapshot, storage path and load errors. Projection, selection-mode, viewport-toggle, transform-tool and authoring-mode commands are unbound by default, including older saved configurations. Text fields retain native editing. Saved shortcuts load at startup; malformed storage stays untouched and current valid bindings remain usable. No map/history/camera mutation.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_keyboard_shortcuts(&self) -> ToolResult {
        ok(self.session().keyboard_shortcuts_state())
    }

    #[tool(
        description = "Configure shortcuts through the same Session operation as Keyboard shortcuts…. Requires current get_keyboard_shortcuts.snapshot. action=save takes bindings keyed by command, each exactly two nullable slots. The original command fields remain required; missing perspective/top/front/side/objects/faces/show_grid/snap_to_grid/tool_textures/selection_outlines/select_tool/move_tool/rotate_tool/scale_tool/draw_brush/clip_mode slots default to unbound for version1 compatibility. A chord is {key,control,shift}; keys are logical A–Z/F1–F12/Delete/Backspace, case normalized. No Alt/Meta, sequences or arbitrary commands. Duplicate chords and reserved camera/file/clipboard/navigation/tool keys reject without changing bindings or disk. Save persists atomically and activates immediately; reload activates only fully valid storage and preserves current bindings on failure; reset explicitly writes/activates factory defaults, including recovery from malformed storage. Save is blocked after load/source errors until Reload succeeds or explicit Reset. All writes compare the inspected file identity/bytes; stale concurrent changes reject. write_committed reports completed writes even if later verification failed. Save/Reset coordinate participating Linux writers through a nonblocking lock and retain the original directory binding. Changed directories require Reload; programs ignoring the lock remain outside that guarantee. No map/history/source/camera changes."
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
        description = "Execute a shared viewport command through the same typed Session dispatcher as configured human shortcuts. command=draw_brush activates the planar drawing mode through set_spatial_settings and returns draw_block; entering from Perspective selects Top without creating a brush. command=clip_mode activates set_clip_tool and returns clip_tool; entering requires selected brushes or brush entities. Repeated activation preserves the drawing/cut preview and active plane drag. Leave modes through existing human controls or explicit setters. Both return the actual projection and camera_changed boolean, preserve spatial dimensions/grid and normal setter retirement, and write no map/history/preferences; aspect is unused. command=select_tool/move_tool/rotate_tool/scale_tool uses set_transform_tool and the viewport's paint-disable operation, returning tool=select/move/rotate/scale. It preserves axes, pivot, texture policies, angle snap, camera and Objects/Faces mode; actual tool changes retire transform/area/component work and held paint strokes before late release. Repeating unchanged settings preserves pending transform work. aspect is unused. command=show_grid/snap_to_grid toggles the originating Session's spatial setting and returns grid_visible/snap; tool_textures/selection_outlines toggles the live view setting and returns the corresponding boolean. These use the same checkbox operations and draft/drag cleanup, without map/history or saved-default writes; aspect is unused. command=perspective/top/front/side changes only the originating Session's projection through the existing camera controls, preserving its per-projection camera behavior; aspect is unused for these commands. command=objects/faces calls the same selection-mode operation as the viewport and set_selection_mode: a mode change clears object/face selection and obsolete gestures/previews, entering faces also disables clipping/draw-block; component editing clears even when the mode is already active. Returns mode and mode_changed, which reports only the mode transition. No map/history/camera change. undo/redo performs one history step (label=null when unavailable); frame_selection frames selected objects/faces or the map using optional positive finite aspect (default1); delete_selection deletes current selected objects; duplicate_selection copies the current selection by [grid,grid,0] and selects the copies. Object commands reject component-editing scope and empty selection. Existing delete_objects/duplicate_objects retain explicit ID/offset support through the same Session helpers. This does not synthesize keyboard input, change shortcut preferences, access system clipboard or open dialogs."
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

#[cfg(test)]
mod tests;
