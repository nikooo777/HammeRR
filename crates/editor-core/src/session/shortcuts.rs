use super::{Actor, SelectMode, Session};
use crate::{
    identity::id_string,
    shortcuts::{Authority, Bindings, Command, CommandSearch, KeyInput, Resolution, Snapshot},
};
use glam::{DVec3, Vec3};
use map_core::{ObjectId, edit};
use render::Projection;
use serde_json::{Value, json};
use std::path::PathBuf;

pub const MAX_CAMERA_STEP_SECONDS: f32 = 1.0;
pub const MAX_CAMERA_DIRECTIONS: usize = 6;

pub fn step_camera_navigation(
    mut camera: render::Camera,
    direction: [f32; 3],
    seconds: f32,
    fly_speed: f32,
    fast: bool,
) -> Result<render::Camera, String> {
    if !seconds.is_finite() || !(0.0..=MAX_CAMERA_STEP_SECONDS).contains(&seconds) {
        return Err(format!(
            "Camera movement duration must be finite and between 0 and {MAX_CAMERA_STEP_SECONDS} seconds"
        ));
    }
    let mut direction = Vec3::from_array(direction);
    if !direction.is_finite() || direction.abs().max_element() > 1.0 {
        return Err("Camera movement axes must be finite and between -1 and 1".into());
    }
    crate::navigation_preferences::Navigation { fly_speed }.validate()?;
    if !camera.position.is_finite() {
        return Err("Camera movement requires a finite current position".into());
    }
    if camera.projection != Projection::Perspective {
        direction.z = 0.0;
    }
    if direction == Vec3::ZERO || seconds == 0.0 {
        return Ok(camera);
    }
    let multiplier = if fast {
        crate::navigation_preferences::FAST_MULTIPLIER
    } else {
        1.0
    };
    let amount = direction.normalize() * fly_speed * multiplier * seconds;
    if camera.projection == Projection::Perspective {
        camera.fly(amount);
    } else {
        camera.position += camera.up() * amount.x + camera.right() * amount.y;
    }
    if !camera.position.is_finite() {
        return Err("Camera movement would produce a nonfinite position".into());
    }
    Ok(camera)
}

pub(super) enum Binding {
    Independent(Authority),
    Application(Authority),
}

impl Default for Binding {
    fn default() -> Self {
        Self::Independent(Authority::default())
    }
}

impl Binding {
    fn authority(&self) -> &Authority {
        match self {
            Self::Independent(authority) | Self::Application(authority) => authority,
        }
    }
}

impl Session {
    pub fn with_keyboard_shortcuts(
        game: Result<source_formats::gamefs::GameFs, String>,
        authority: Authority,
    ) -> Self {
        let mut session = Self::new(game);
        session.use_application_shortcuts(authority);
        session
    }

    pub(crate) fn use_application_shortcuts(&mut self, authority: Authority) {
        self.shortcuts = Binding::Application(authority);
        if let Some(error) = self.shortcuts_error_message() {
            self.log(
                Actor::System,
                format!("Saved keyboard shortcuts unavailable: {error}"),
            );
        }
    }

    pub(super) fn configure_shortcuts(&mut self, directory: Result<PathBuf, String>) {
        if let Binding::Independent(authority) = &self.shortcuts
            && let Err(error) = authority.configure(directory)
        {
            self.log(
                Actor::System,
                format!("Saved keyboard shortcuts unavailable: {error}"),
            );
        }
    }

    pub fn keyboard_shortcuts_snapshot(&self) -> Snapshot {
        self.shortcuts.authority().snapshot()
    }
    pub fn shortcuts_stamp(&self) -> String {
        self.shortcuts.authority().serial().to_string()
    }
    pub fn shortcuts_error_message(&self) -> Option<String> {
        self.keyboard_shortcuts_snapshot().error
    }
    pub fn shortcut_bindings(&self) -> Bindings {
        self.keyboard_shortcuts_snapshot().current
    }

    pub fn resolve_editor_shortcut_sequence(
        &self,
        inputs: &[KeyInput],
    ) -> Result<Resolution, String> {
        self.shortcuts.authority().resolve_sequence(inputs)
    }

    pub fn navigate_camera(
        &mut self,
        actor: Actor,
        commands: &[Command],
        seconds: f32,
        fast: bool,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        if commands.is_empty() || commands.len() > MAX_CAMERA_DIRECTIONS {
            return Err(format!(
                "Camera movement requires 1–{MAX_CAMERA_DIRECTIONS} distinct direction commands"
            ));
        }
        let mut direction = Vec3::ZERO;
        for (index, command) in commands.iter().enumerate() {
            if commands[..index].contains(command) {
                return Err("Camera movement direction commands must be distinct".into());
            }
            direction += match command {
                Command::CameraForward => Vec3::X,
                Command::CameraBackward => Vec3::NEG_X,
                Command::CameraRight => Vec3::Y,
                Command::CameraLeft => Vec3::NEG_Y,
                Command::CameraUp => Vec3::Z,
                Command::CameraDown => Vec3::NEG_Z,
                _ => return Err("Camera movement accepts only camera direction commands".into()),
            };
        }
        let before = self.camera();
        let camera = step_camera_navigation(
            before,
            direction.to_array(),
            seconds,
            self.fly_speed(),
            fast,
        )?;
        let changed = camera != before;
        if changed {
            self.set_camera(camera);
            self.log(actor, "Moved camera");
        }
        Ok(json!({
            "commands":commands,
            "seconds":seconds,
            "fast":fast,
            "fly_speed":self.fly_speed(),
            "camera_changed":changed,
            "projection":camera.projection.name(),
            "position":camera.position.to_array()
        }))
    }

    pub fn keyboard_shortcuts_state(&self) -> Value {
        self.keyboard_shortcuts_snapshot().report()
    }

    pub fn find_editor_commands(
        &self,
        query: &str,
        bindings: Option<&Bindings>,
    ) -> Result<CommandSearch, String> {
        let snapshot = self.keyboard_shortcuts_snapshot();
        let commands = bindings.unwrap_or(&snapshot.current).find_commands(query)?;
        Ok(CommandSearch {
            query: query.trim().into(),
            snapshot: snapshot.stamp(),
            total: Command::ALL.len(),
            matched: commands.len(),
            commands,
        })
    }

    pub fn change_keyboard_shortcuts(
        &mut self,
        actor: Actor,
        snapshot: &str,
        action: &str,
        bindings: Option<Bindings>,
    ) -> Result<Value, String> {
        let result = self
            .shortcuts
            .authority()
            .change(snapshot, action, bindings)?;
        self.log(
            actor,
            match action {
                "save" => "Saved keyboard shortcuts",
                "reset" => "Reset saved keyboard shortcuts",
                _ => "Reloaded saved keyboard shortcuts",
            },
        );
        Ok(result)
    }

    pub fn delete_objects(&mut self, actor: Actor, ids: &[ObjectId]) -> Result<(), String> {
        self.edit(actor, &format!("delete {} objects", ids.len()), |tx| {
            edit::delete(tx, ids)
        })
    }

    pub fn duplicate_objects(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
        offset: DVec3,
    ) -> Result<Vec<ObjectId>, String> {
        let copies = self.edit(actor, &format!("duplicate {} objects", ids.len()), |tx| {
            edit::duplicate(tx, ids, offset)
        })?;
        self.select(&copies, SelectMode::Replace)?;
        Ok(copies)
    }

    pub fn execute_editor_command(
        &mut self,
        actor: Actor,
        command: Command,
        aspect: f32,
    ) -> Result<Value, String> {
        let object_command = matches!(
            command,
            Command::DeleteSelection | Command::DuplicateSelection
        );
        if object_command
            && (self.component_picking().is_some() || !self.component_selection().is_empty())
        {
            return Err("Use Reshape brushes or its move handles. Finish component editing before editing whole objects.".into());
        }
        let mut result = json!({"command":command});
        match command {
            Command::NewMap => {
                return Err("Use new_map with discard_changes and optional expected_review from get_map_info; the native shortcut opens the shared New workflow".into());
            }
            Command::OpenMap => {
                return Err("Use open_map with an explicit path and replacement inputs; the native shortcut opens the file chooser".into());
            }
            Command::SaveMap | Command::SaveMapAs => {
                return Err("Use save_map with an explicit path for Save as, or omit path to save the retained current source; native shortcuts use the shared Save workflows".into());
            }
            Command::Copy | Command::Cut => {
                return Err("Use copy_objects or cut_objects with get_clipboard.snapshot and an explicit session/system destination; native shortcuts use the shared system clipboard workflow".into());
            }
            Command::Paste => {
                return Err("Use get_clipboard, import_clipboard when needed, then preview_paste and apply_paste; the native shortcut opens the shared Paste workflow".into());
            }
            Command::CameraForward
            | Command::CameraBackward
            | Command::CameraLeft
            | Command::CameraRight
            | Command::CameraUp
            | Command::CameraDown => {
                return Err("Use step_camera_navigation with direction commands and an explicit duration; native camera shortcuts run while held".into());
            }
            Command::Undo => result["label"] = json!(self.undo(actor)),
            Command::Redo => result["label"] = json!(self.redo(actor)),
            Command::HideSelectionInEditor | Command::ShowAllEditorHidden => {
                let action = if command == Command::HideSelectionInEditor {
                    super::editor_visibility::Action::Hide(self.selected_ids())
                } else {
                    super::editor_visibility::Action::ShowAll
                };
                result["editor_visibility"] =
                    self.set_editor_visibility(actor, action, &self.working_region_stamp())?;
            }
            Command::Perspective => self.set_projection(actor, Projection::Perspective),
            Command::Top => self.set_projection(actor, Projection::Top),
            Command::Front => self.set_projection(actor, Projection::Front),
            Command::Side => self.set_projection(actor, Projection::Side),
            Command::Objects | Command::Faces => {
                let enabled = command == Command::Faces;
                result["mode_changed"] = json!(self.face_mode() != enabled);
                self.set_face_mode(enabled);
                result["mode"] = json!(if self.face_mode() { "faces" } else { "objects" });
            }
            Command::SelectTool | Command::MoveTool | Command::RotateTool | Command::ScaleTool => {
                let (mode, name) = match command {
                    Command::MoveTool => (Some(render::handles::Mode::Move), "move"),
                    Command::RotateTool => (Some(render::handles::Mode::Rotate), "rotate"),
                    Command::ScaleTool => (Some(render::handles::Mode::Scale), "scale"),
                    _ => (None, "select"),
                };
                self.set_transform_tool(
                    actor,
                    super::TransformTool {
                        mode,
                        ..self.transform_tool()
                    },
                )?;
                self.set_paint_operation(
                    actor,
                    false,
                    self.paint_radius(),
                    super::paint::Operation::Selection,
                )?;
                result["tool"] = json!(name);
            }
            Command::ShowGrid => {
                self.set_spatial(
                    actor,
                    crate::spatial::Update {
                        grid_visible: Some(!self.spatial().grid_visible),
                        ..Default::default()
                    },
                )?;
                result["grid_visible"] = json!(self.spatial().grid_visible);
            }
            Command::DrawBrush | Command::ClipMode => {
                let camera = self.camera();
                if command == Command::DrawBrush {
                    self.set_spatial(
                        actor,
                        crate::spatial::Update {
                            draw_block: Some(true),
                            ..Default::default()
                        },
                    )?;
                    result["draw_block"] = json!(self.spatial().draw_block);
                } else {
                    self.set_clip_tool(actor, true)?;
                    result["clip_tool"] = json!(self.clip_tool());
                }
                result["projection"] = json!(self.camera().projection.name());
                result["camera_changed"] = json!(self.camera() != camera);
            }
            Command::SnapToGrid => {
                self.set_spatial(
                    actor,
                    crate::spatial::Update {
                        snap: Some(!self.spatial().snap),
                        ..Default::default()
                    },
                )?;
                result["snap"] = json!(self.spatial().snap);
            }
            Command::ToolTextures => {
                self.set_view(render::ViewOptions {
                    tool_textures: !self.view().tool_textures,
                    ..self.view()
                });
                result["tool_textures"] = json!(self.view().tool_textures);
            }
            Command::SelectionOutlines => {
                self.set_view(render::ViewOptions {
                    selection_outlines: !self.view().selection_outlines,
                    ..self.view()
                });
                result["selection_outlines"] = json!(self.view().selection_outlines);
            }
            Command::FrameSelection => {
                self.frame_selection(actor, aspect)?;
                result["camera_changed"] = json!(true);
            }
            Command::DeleteSelection | Command::DuplicateSelection => {
                let ids = self.selected_ids();
                if ids.is_empty() {
                    return Err("Nothing selected".into());
                }
                if command == Command::DeleteSelection {
                    self.delete_objects(actor, &ids)?;
                    result["deleted"] = json!(ids.into_iter().map(id_string).collect::<Vec<_>>());
                } else {
                    let grid = self.spatial.grid_size;
                    let copies =
                        self.duplicate_objects(actor, &ids, DVec3::new(grid, grid, 0.0))?;
                    result["copies"] = json!(copies.into_iter().map(id_string).collect::<Vec<_>>());
                }
            }
        }
        if matches!(
            command,
            Command::Perspective | Command::Top | Command::Front | Command::Side
        ) {
            result["projection"] = json!(self.camera().projection.name());
            result["camera_changed"] = json!(true);
        }
        Ok(result)
    }
}
