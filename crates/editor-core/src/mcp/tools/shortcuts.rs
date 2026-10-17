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

#[tool_router(router = shortcut_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the shared five-command viewport shortcut configuration: undo, redo, frame_selection, delete_selection and duplicate_selection, with exact primary/alternate nullable chord slots, factory defaults, command labels, scope/reserved keys, preference snapshot, storage path and load errors. Text fields retain native editing. Saved shortcuts load at startup; malformed storage stays untouched and current valid bindings remain usable. No map/history/camera mutation.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_keyboard_shortcuts(&self) -> ToolResult {
        ok(self.session().keyboard_shortcuts_state())
    }

    #[tool(
        description = "Configure shortcuts through the same Session operation as Keyboard shortcuts…. Requires current get_keyboard_shortcuts.snapshot. action=save requires complete bindings {undo,redo,frame_selection,delete_selection,duplicate_selection}, each exactly two nullable slots. A chord is {key,control,shift}; keys are logical A–Z/F1–F12/Delete/Backspace, case normalized. No Alt/Meta, sequences or arbitrary commands. Duplicate chords and reserved camera/file/clipboard/navigation/tool keys reject without changing bindings or disk. Save persists atomically and activates immediately; reload activates only fully valid storage and preserves current bindings on failure; reset explicitly writes/activates factory defaults, including recovery from malformed storage. Save is blocked after load/source errors until Reload succeeds or explicit Reset. All writes compare the inspected file identity/bytes; stale concurrent changes reject. write_committed reports completed writes even if later reload failed. Independent writers do not lock the file. No map/history/source/camera changes."
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
        description = "Execute one of the five shared viewport commands through the same typed Session dispatcher as configured human shortcuts. command=undo/redo performs one history step (label=null when unavailable); frame_selection frames selected objects/faces or the map using optional positive finite aspect (default1); delete_selection deletes current selected objects; duplicate_selection copies the current selection by [grid,grid,0] and selects the copies. Object commands reject component-editing scope and empty selection. Existing delete_objects/duplicate_objects retain explicit ID/offset support through the same Session helpers. This does not synthesize keyboard input, change shortcut preferences, access system clipboard or open dialogs."
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
