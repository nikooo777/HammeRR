use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::mcp::{SharedSession, lock};
use crate::session::{Actor, Session};
use crate::{RenameRow, RenameWindow};

fn guard(window: &RenameWindow, session: &Session) -> Result<(), String> {
    session.check_rename_draft(
        window
            .get_draft_token()
            .parse()
            .map_err(|_| "Reopen References and rename")?,
    )
}

pub fn refresh(window: &RenameWindow, session: &Session) {
    window.set_can_apply(false);
    let Some(draft) = session.rename_draft() else {
        window.set_summary("No active rename preview".into());
        clear_rows(window);
        window.set_requires_review(false);
        window.set_message(
            "Rename preview closed. Reopen References and rename to start again.".into(),
        );
        return;
    };
    window.set_draft_token(draft.token.to_string().into());
    window.set_new_name(draft.new_name.clone().into());
    window.set_rename_group(draft.rename_group);
    window.set_accept_review(draft.accept_review);
    window.set_show_details(draft.show_details);
    let plan = match session.rename_preview() {
        Ok(plan) => plan,
        Err(error) => {
            window.set_message(error.into());
            window.set_summary("This preview is stale. Cancel and reopen it.".into());
            clear_rows(window);
            window.set_requires_review(false);
            return;
        }
    };
    let entity = session.document().entity(draft.entity).unwrap();
    window.set_entity_label(
        format!(
            "{} · {}",
            session.rename_entity_label(entity.id),
            crate::inspector::title(entity.classname().unwrap_or("Custom entity")),
        )
        .into(),
    );
    window.set_old_name(plan.old_name.clone().into());
    window.set_shared_count(
        session
            .document()
            .vmf()
            .entities
            .iter()
            .filter(|entity| {
                !plan.old_name.is_empty()
                    && entity
                        .properties
                        .get("targetname")
                        .is_some_and(|name| name.eq_ignore_ascii_case(&plan.old_name))
            })
            .count() as i32,
    );
    window.set_members(
        plan.entities
            .iter()
            .map(|id| {
                let entity = session.document().entity(*id).unwrap();
                let label = format!(
                    "{} · {}",
                    session.rename_entity_label(*id),
                    crate::inspector::title(entity.classname().unwrap_or("Custom entity"))
                );
                if draft.show_details {
                    format!("{label} · entity:{id}")
                } else {
                    label
                }
            })
            .collect::<Vec<_>>()
            .join("; ")
            .into(),
    );
    let changes = plan
        .references
        .iter()
        .filter(|change| change.changed)
        .count();
    let review = plan
        .references
        .iter()
        .filter(|change| change.review)
        .count();
    window.set_requires_review(review > 0);
    let proposal = plan.new_name != plan.old_name
        || plan.entities.iter().any(|id| {
            session.document().entity(*id).is_some_and(|entity| {
                entity.properties.get("targetname").unwrap_or_default() != plan.new_name
            })
        });
    window.set_has_proposal(proposal);
    let mut summary = format!(
        "Rename {} {} and update {changes} {}.",
        plan.entities.len(),
        if plan.entities.len() == 1 {
            "entity"
        } else {
            "entities"
        },
        if changes == 1 { "link" } else { "links" },
    );
    if review > 0 {
        summary.push_str(&format!(
            " {review} {} need{} your attention.",
            if review == 1 { "field" } else { "fields" },
            if review == 1 { "s" } else { "" }
        ));
    }
    window.set_summary(summary.into());
    if !proposal {
        window.set_summary(
            format!(
                "{} references and text matches found.",
                plan.references.len()
            )
            .into(),
        );
    }
    window.set_can_apply(session.rename_can_apply(&plan));
    window.set_message(
        plan.error
            .unwrap_or_else(|| {
                if plan.new_name == plan.old_name && !window.get_can_apply() && review == 0 {
                    "Inspect references below, or enter a new name.".into()
                } else if review > 0 && !draft.accept_review {
                    "Check the fields below, then choose whether to keep them unchanged.".into()
                } else {
                    "You can undo this rename and its link updates in one step.".into()
                }
            })
            .into(),
    );
    let rows = plan
        .references
        .iter()
        .map(|change| {
            let view = session.rename_reference_view(change, &plan.new_name);
            RenameRow {
                section: Default::default(),
                section_note: Default::default(),
                label: view.label.into(),
                change: view.change.into(),
                reason: view.explanation.into(),
                review: change.review,
                changed: change.changed,
                details: view.details.into(),
            }
        })
        .collect::<Vec<_>>();
    window.set_changes(ModelRc::new(VecModel::from(
        rows.iter()
            .filter(|row| row.changed)
            .cloned()
            .collect::<Vec<_>>(),
    )));
    window.set_attention(ModelRc::new(VecModel::from(
        rows.iter()
            .filter(|row| row.review)
            .cloned()
            .collect::<Vec<_>>(),
    )));
    window.set_other(ModelRc::new(VecModel::from(
        rows.iter()
            .filter(|row| !row.changed && !row.review)
            .cloned()
            .collect::<Vec<_>>(),
    )));
    let mut visible = Vec::new();
    if proposal {
        for (section, note, group) in [
            ("What will change", "", window.get_changes()),
            (
                "Needs your attention",
                "These fields will stay as they are. Check their effects before applying.",
                window.get_attention(),
            ),
            (
                "Other references · unchanged",
                "",
                if draft.show_details {
                    window.get_other()
                } else {
                    ModelRc::default()
                },
            ),
        ] {
            for (index, mut row) in group.iter().enumerate() {
                if index == 0 {
                    row.section = section.into();
                    row.section_note = note.into();
                }
                visible.push(row);
            }
        }
    } else {
        visible = rows;
        if let Some(row) = visible.first_mut() {
            row.section = "References to this entity".into();
        }
    }
    window.set_references(ModelRc::new(VecModel::from(visible)));
}

fn clear_rows(window: &RenameWindow) {
    window.set_references(ModelRc::default());
    window.set_changes(ModelRc::default());
    window.set_attention(ModelRc::default());
    window.set_other(ModelRc::default());
}

pub fn bind(window: &RenameWindow, shared: &SharedSession) {
    window.on_fields_changed({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let result = guard(&window, &session).and_then(|()| {
                session.update_entity_rename(
                    window.get_new_name().into(),
                    window.get_rename_group(),
                    window.get_accept_review(),
                )?;
                session.set_rename_details(window.get_show_details())
            });
            refresh(&window, &session);
            if let Err(error) = result {
                window.set_message(error.into());
            }
        }
    });
    window.on_apply({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            let result =
                guard(&window, &session).and_then(|()| session.commit_entity_rename(Actor::User));
            refresh(&window, &session);
            match result {
                Ok(_) => {
                    let _ = window.hide();
                }
                Err(error) => window.set_message(error.into()),
            }
        }
    });
    window.on_cancel({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let mut session = lock(&shared);
            if guard(&window, &session).is_ok() {
                session.cancel_entity_rename();
            }
            let _ = window.hide();
        }
    });
    window.window().on_close_requested({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let mut session = lock(&shared);
            if guard(&window.unwrap(), &session).is_ok() {
                session.cancel_entity_rename();
            }
            slint::CloseRequestResponse::HideWindow
        }
    });
}
