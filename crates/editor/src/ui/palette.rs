use editor_core::shortcuts::{Bindings, Chord, Command};
use slint::{Model, ModelRc, VecModel};

use crate::identity::parse_id;
use crate::runtime::DocumentAccess;
use crate::session::{Actor, ObjectFilter, SelectMode, Session};
use crate::{AppWindow, PaletteRow};

const OBJECT_ROWS: usize = 8;

fn shortcut_command(id: &str) -> Option<Command> {
    Some(match id {
        "undo" => Command::Undo,
        "redo" => Command::Redo,
        "frame-selection" => Command::FrameSelection,
        "delete" => Command::DeleteSelection,
        "perspective" => Command::Perspective,
        "top" => Command::Top,
        "front" => Command::Front,
        "side" => Command::Side,
        "objects" => Command::Objects,
        "faces" => Command::Faces,
        "show-grid" => Command::ShowGrid,
        "snap" => Command::SnapToGrid,
        "tool-textures" => Command::ToolTextures,
        "selection-outlines" => Command::SelectionOutlines,
        "select-tool" => Command::SelectTool,
        "move-tool" => Command::MoveTool,
        "rotate-tool" => Command::RotateTool,
        "scale-tool" => Command::ScaleTool,
        "block" => Command::DrawBrush,
        "clip" => Command::ClipMode,
        "hide-editor" => Command::HideSelectionInEditor,
        "show-editor-hidden" => Command::ShowAllEditorHidden,
        _ => return None,
    })
}

fn command_keys(bindings: &Bindings, id: &str) -> String {
    let fixed = match id {
        "new-map" => "Ctrl+N",
        "open-map" => "Ctrl+O",
        "save-map" => "Ctrl+S",
        "save-map-as" => "Ctrl+Shift+S",
        "copy" => "Ctrl+C",
        "cut" => "Ctrl+X",
        "paste" => "Ctrl+V",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.to_owned();
    }
    shortcut_command(id).map_or_else(String::new, |command| {
        bindings
            .slots(command)
            .iter()
            .flatten()
            .map(Chord::label)
            .collect::<Vec<_>>()
            .join(" · ")
    })
}

fn command_rows(app: &AppWindow, session: &Session, query: &str) -> Vec<PaletteRow> {
    let bindings = session.keyboard_shortcuts_snapshot().current;
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut ranked: Vec<(bool, PaletteRow)> = app
        .get_palette_commands()
        .iter()
        .filter_map(|command| {
            let title = command.title.to_lowercase();
            let text = format!("{title} {}", command.group.to_lowercase());
            words
                .iter()
                .all(|word| text.contains(word.as_str()))
                .then(|| {
                    let leading = words
                        .first()
                        .is_some_and(|word| title.starts_with(word.as_str()));
                    let row = PaletteRow {
                        object: false,
                        keys: command_keys(&bindings, &command.id).into(),
                        id: command.id,
                        title: command.title,
                        detail: command.group,
                        enabled: command.enabled,
                    };
                    (leading, row)
                })
        })
        .collect();
    ranked.sort_by_key(|(leading, row)| (!leading, !row.enabled));
    ranked.into_iter().map(|(_, row)| row).collect()
}

fn object_rows(session: &Session, query: &str) -> Vec<PaletteRow> {
    let query = query.trim();
    if query.chars().count() < 2 {
        return Vec::new();
    }
    let filter = ObjectFilter {
        query: Some(query.to_owned()),
        ..ObjectFilter::default()
    };
    match session.object_report(&filter) {
        Ok(objects) => objects
            .into_iter()
            .take(OBJECT_ROWS)
            .map(|object| {
                let class = if object.classname.is_empty() {
                    "Brush".to_owned()
                } else {
                    object.classname
                };
                let title = if object.targetname.is_empty() {
                    class
                } else {
                    format!("{class} · {}", object.targetname)
                };
                PaletteRow {
                    object: true,
                    id: object.id.into(),
                    title: title.into(),
                    detail: object.kind.into(),
                    keys: Default::default(),
                    enabled: true,
                }
            })
            .collect(),
        Err(error) => vec![PaletteRow {
            object: true,
            id: Default::default(),
            title: error.into(),
            detail: Default::default(),
            keys: Default::default(),
            enabled: false,
        }],
    }
}

pub(super) fn pick_object(app: &AppWindow, session: &mut Session, id: &str) -> Result<(), String> {
    session.select(&[parse_id(id)?], SelectMode::Replace)?;
    let aspect = app.get_viewport_width() / app.get_viewport_height().max(1.0);
    session.frame_selection(Actor::User, aspect)
}

pub(super) fn bind(app: &AppWindow, access: &DocumentAccess) {
    app.on_palette_search({
        let callback = super::admission::SessionCallback::new(app, access);
        move |query| {
            callback.input(|app, session| {
                let mut rows = command_rows(&app, session, &query);
                rows.extend(object_rows(session, &query));
                app.set_palette_rows(ModelRc::new(VecModel::from(rows)));
            });
        }
    });
    app.on_palette_pick_object({
        let callback = super::admission::SessionCallback::new(app, access);
        move |id| {
            callback.input(|app, session| {
                if let Err(error) = pick_object(&app, session, &id) {
                    app.set_message(error.into());
                }
            });
        }
    });
}
