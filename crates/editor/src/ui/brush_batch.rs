use super::*;
use session::brush_batch::{Batch, MAX_IMPORT_BYTES};
use std::io::Read;

fn read_batch(path: &Path) -> Result<Batch, String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("Cannot inspect brush batch: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_IMPORT_BYTES as u64 {
        return Err(format!(
            "Choose a JSON file of at most {MAX_IMPORT_BYTES} bytes"
        ));
    }
    let file =
        std::fs::File::open(path).map_err(|error| format!("Cannot open brush batch: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("Cannot inspect brush batch: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_IMPORT_BYTES as u64 {
        return Err(format!(
            "Choose a JSON file of at most {MAX_IMPORT_BYTES} bytes"
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_IMPORT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Cannot read brush batch: {error}"))?;
    Batch::from_json(&bytes)
}

pub(super) fn import_dialog(app: &AppWindow, ui: &SharedUi) {
    let access = file_access(ui);
    let context = access.with_session(|session| {
        Ok((
            session.document_generation(),
            session
                .path()
                .and_then(|path| path.parent())
                .map(Path::to_owned),
        ))
    });
    let (generation, directory) = match context {
        Ok(context) => context,
        Err(error) => {
            app.set_message(error.into());
            return;
        }
    };
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title("Import brush batch")
        .add_filter("Brush batch JSON", &["json"]);
    if let Some(directory) = directory {
        dialog = dialog.set_directory(directory);
    }
    let continuation = access.downgrade();
    let (app, owner) = (app.as_weak(), Rc::downgrade(ui));
    slint::spawn_local(async move {
        let Some(file) = dialog.pick_file().await else {
            return;
        };
        let (Some(_ui), Some(window)) = (owner.upgrade(), app.upgrade()) else {
            return;
        };
        let result = read_batch(file.path()).and_then(|batch| {
            continuation.upgrade()?.with_session(|session| {
                if session.document_generation() != generation {
                    return Err(
                        "Map changed while choosing the brush batch; import it again".into(),
                    );
                }
                session
                    .create_blocks(Actor::User, &batch.blocks, batch.snap)
                    .map(|ids| ids.len())
            })
        });
        match result {
            Ok(count) => {
                window.set_message(format!("Imported {count} brushes in one Undo step").into())
            }
            Err(error) => window.set_message(error.into()),
        }
    })
    .expect("event loop is running");
}
