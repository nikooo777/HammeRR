use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::{Actor, Session};
use crate::{BulkRenameNameRow, BulkRenameReferenceRow, BulkRenameWindow};

pub struct Controller {
    pub window: BulkRenameWindow,
}

fn token(window: &BulkRenameWindow) -> Result<u64, String> {
    window
        .get_draft_token()
        .parse()
        .map_err(|_| "Capture the entity rename set again".into())
}

fn failed(window: &BulkRenameWindow, error: String) {
    window.set_can_apply(false);
    window.set_status(error.into());
}

fn show_plan(window: &BulkRenameWindow, session: &Session, reset_names: bool) {
    window.set_can_apply(false);
    let plan = match token(window).and_then(|token| session.entity_rename_batch_preview(token)) {
        Ok(plan) => plan,
        Err(error) => {
            window.set_ready(false);
            window.set_references(ModelRc::default());
            failed(window, error);
            return;
        }
    };
    window.set_ready(true);
    if reset_names {
        window.set_names(ModelRc::new(VecModel::from(
            plan.names
                .iter()
                .map(|name| BulkRenameNameRow {
                    entity: format!("entity:{}", name.entity).into(),
                    label: session
                        .document()
                        .entity(name.entity)
                        .and_then(|entity| entity.classname())
                        .map(crate::inspector::title)
                        .unwrap_or_else(|| "Custom entity".into())
                        .into(),
                    old_name: name.old_name.clone().unwrap_or_default().into(),
                    new_name: name.new_name.clone().into(),
                })
                .collect::<Vec<_>>(),
        )));
        window.set_dirty(false);
    }
    let renamed = plan
        .names
        .iter()
        .filter(|name| name.old_name.as_deref() != Some(name.new_name.as_str()))
        .count();
    let rewrites = plan
        .references
        .iter()
        .filter(|change| change.changed)
        .count();
    let review = if plan.error.is_some() {
        "reference review blocked".to_owned()
    } else {
        format!("{rewrites} reference field updates")
    };
    window.set_summary(
        format!(
            "{} captured entities · {renamed} name changes · {review}",
            plan.names.len()
        )
        .into(),
    );
    window.set_references(ModelRc::new(VecModel::from(
        plan.references
            .iter()
            .map(|change| {
                use map_core::references::Location;
                let field = match &change.reference.location {
                    Location::Property { key, .. } => format!("property {key}"),
                    Location::Connection {
                        index,
                        output,
                        field,
                    } => format!(
                        "{output} #{} {}",
                        index + 1,
                        if *field == 0 { "target" } else { "parameter" }
                    ),
                };
                BulkRenameReferenceRow {
                    label: format!(
                        "entity:{} · {field}{}",
                        change.reference.source,
                        if change.reference.hidden {
                            " · hidden"
                        } else {
                            ""
                        }
                    )
                    .into(),
                    before: change.reference.value.clone().into(),
                    after: change.after.clone().into(),
                    reason: change.reason.clone().into(),
                    changed: change.changed,
                }
            })
            .collect::<Vec<_>>(),
    )));
    window.set_can_apply(plan.error.is_none() && renamed > 0 && !window.get_dirty());
    window.set_status(plan.error.unwrap_or_default().into());
}

fn with_session(
    window: &slint::Weak<BulkRenameWindow>,
    access: &DocumentContinuation,
    action: impl FnOnce(&BulkRenameWindow, &mut Session) -> Result<(), String>,
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = access
        .upgrade()
        .and_then(|access| access.with_session(|session| action(&window, session)));
    if let Err(error) = result {
        failed(&window, error);
    }
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = BulkRenameWindow::new()?;
        window.on_names_edited({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.set_dirty(true);
                    window.set_can_apply(false);
                    window.set_references(ModelRc::default());
                    window.set_status("Preview all names to review this proposal.".into());
                }
            }
        });
        window.on_preview({
            let (window, access) = (window.as_weak(), access.downgrade());
            move || {
                with_session(&window, &access, |window, session| {
                    let names = window
                        .get_names()
                        .iter()
                        .map(|row| {
                            let id = row
                                .entity
                                .strip_prefix("entity:")
                                .ok_or("Capture the entity rename set again")?
                                .parse()
                                .map_err(|_| "Invalid captured entity identity")?;
                            Ok((id, row.new_name.to_string()))
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    let token = session.update_entity_rename_batch(token(window)?, &names)?;
                    window.set_draft_token(token.to_string().into());
                    show_plan(window, session, true);
                    Ok(())
                })
            }
        });
        window.on_apply({
            let (window, access) = (window.as_weak(), access.downgrade());
            move || {
                with_session(&window, &access, |window, session| {
                    if window.get_dirty() {
                        return Err("Preview all names before Apply renames".into());
                    }
                    session.commit_entity_rename_batch(Actor::User, token(window)?)?;
                    window.set_draft_token("".into());
                    window.hide().map_err(|error| error.to_string())
                })
            }
        });
        window.on_cancel({
            let (window, access) = (window.as_weak(), access.downgrade());
            move || {
                with_session(&window, &access, |window, session| {
                    if !window.get_draft_token().is_empty() {
                        session.cancel_entity_rename_batch(token(window)?)?;
                        window.set_draft_token("".into());
                    }
                    window.hide().map_err(|error| {
                        format!(
                            "The rename plan is closed, but the window could not be hidden: {error}"
                        )
                    })
                });
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_cancel();
                }
                slint::CloseRequestResponse::KeepWindowShown
            }
        });
        Ok(Self { window })
    }

    pub fn initialize(&self, session: &mut Session) -> Result<(), String> {
        let token = session.begin_entity_rename_batch(&[])?;
        self.window.set_draft_token(token.to_string().into());
        show_plan(&self.window, session, true);
        Ok(())
    }

    pub fn refresh(&self, session: &Session) {
        show_plan(&self.window, session, false);
    }
}
