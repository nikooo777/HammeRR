use map_core::document::{ChangeKind, Step};
use serde::Serialize;

use super::*;

pub const PAGE_SIZE: usize = 50;
pub const MAX_PAGE_SIZE: usize = 100;
pub const MAX_NAVIGATION_STEPS: usize = 256;
pub const MAX_OBJECT_SAMPLES: usize = 16;
pub const MAX_LABEL_CHARS: usize = 256;
pub const INITIAL_STATE: &str = "0";

#[derive(Debug, Serialize)]
pub struct ObjectChange {
    pub id: String,
    pub kind: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Entry {
    pub state: String,
    pub position: usize,
    pub label: String,
    pub label_truncated: bool,
    pub actor: &'static str,
    pub applied: bool,
    pub current: bool,
    pub added: usize,
    pub modified: usize,
    pub removed: usize,
    pub objects: Vec<ObjectChange>,
    pub objects_total: usize,
    pub sections: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub snapshot: String,
    pub title: String,
    pub current_state: String,
    pub undo_count: usize,
    pub redo_count: usize,
    pub unsaved_changes: bool,
    pub navigation_blocked: Option<String>,
    pub offset: usize,
    pub total: usize,
    pub next_offset: Option<usize>,
    pub entries: Vec<Entry>,
    pub max_page_size: usize,
    pub max_navigation_steps: usize,
    pub max_object_samples: usize,
    pub max_label_chars: usize,
}

#[derive(Debug, Serialize)]
pub struct Plan {
    pub snapshot: String,
    pub from: String,
    pub to: String,
    pub direction: &'static str,
    pub steps: usize,
    pub unsaved_changes: bool,
}

#[derive(Debug, Serialize)]
pub struct Navigation {
    pub plan: Plan,
    pub snapshot: String,
    pub current_state: String,
    pub unsaved_changes: bool,
}

fn entry(
    step: &Step,
    position: usize,
    applied: bool,
    current: bool,
    actor: Option<Actor>,
) -> Entry {
    let mut result = Entry {
        state: step.id().to_string(),
        position,
        label: step.label.chars().take(MAX_LABEL_CHARS).collect(),
        label_truncated: step.label.chars().nth(MAX_LABEL_CHARS).is_some(),
        actor: actor.map_or("unknown", Actor::label),
        applied,
        current,
        added: 0,
        modified: 0,
        removed: 0,
        objects: Vec::new(),
        objects_total: 0,
        sections: step.sections().collect(),
    };
    for (id, kind) in step.object_changes() {
        let kind = match kind {
            ChangeKind::Added => {
                result.added += 1;
                "added"
            }
            ChangeKind::Modified => {
                result.modified += 1;
                "modified"
            }
            ChangeKind::Removed => {
                result.removed += 1;
                "removed"
            }
        };
        result.objects_total += 1;
        if result.objects.len() < MAX_OBJECT_SAMPLES {
            result.objects.push(ObjectChange {
                id: crate::identity::id_string(id),
                kind,
            });
        }
    }
    result
}

impl Session {
    pub fn history_snapshot(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            usize::from(self.document.is_dirty()),
            self.history_save_serial
        )
    }

    fn history_current_state(&self) -> String {
        self.document
            .undo_steps()
            .last()
            .map_or_else(|| INITIAL_STATE.into(), |step| step.id().to_string())
    }

    pub fn history_navigation_blocked(&self) -> Option<String> {
        if self.compiling() || self.launching() || self.closing {
            return Some(
                "Wait for the build, launch or shutdown to finish before navigating history."
                    .into(),
            );
        }
        if self.geometry.entity_placement.has_draft()
            || self.measurements.has_draft()
            || !std::ptr::eq(self.render_document(), &self.document)
            || self.block_preview.is_some()
            || self.area_preview.is_some()
            || self.volume_preview().is_some()
            || self.geometry.paint.stroke.is_some()
            || self.transform_drag.is_some()
            || self.clip_drag.is_some()
        {
            return Some("Apply or cancel the active preview before navigating history.".into());
        }
        None
    }

    pub fn inspect_history(&self, offset: usize, limit: usize) -> Result<Report, String> {
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(format!("History page size must be 1–{MAX_PAGE_SIZE}"));
        }
        let undo = self.document.undo_steps();
        let redo = self.document.redo_steps();
        let total = undo.len() + redo.len();
        if offset > total {
            return Err("History offset is beyond the retained transactions".into());
        }
        let entries = undo
            .iter()
            .chain(redo.iter().rev())
            .rev()
            .skip(offset)
            .take(limit)
            .enumerate()
            .map(|(index, step)| {
                let position = total - offset - index;
                entry(
                    step,
                    position,
                    position <= undo.len(),
                    position == undo.len(),
                    self.history_actors.get(&step.id()).copied(),
                )
            })
            .collect::<Vec<_>>();
        let next = offset + entries.len();
        Ok(Report {
            snapshot: self.history_snapshot(),
            title: self.title(),
            current_state: self.history_current_state(),
            undo_count: undo.len(),
            redo_count: redo.len(),
            unsaved_changes: self.document.is_dirty(),
            navigation_blocked: self.history_navigation_blocked(),
            offset,
            total,
            next_offset: (next < total).then_some(next),
            entries,
            max_page_size: MAX_PAGE_SIZE,
            max_navigation_steps: MAX_NAVIGATION_STEPS,
            max_object_samples: MAX_OBJECT_SAMPLES,
            max_label_chars: MAX_LABEL_CHARS,
        })
    }

    pub fn plan_history_navigation(&self, snapshot: &str, target: &str) -> Result<Plan, String> {
        if snapshot != self.history_snapshot() {
            return Err(
                "Map or history changed. Refresh history and select the state again.".into(),
            );
        }
        let undo = self.document.undo_steps();
        let position = if target == INITIAL_STATE {
            0
        } else {
            let id = target
                .parse::<u64>()
                .ok()
                .filter(|id| id.to_string() == target)
                .ok_or("Select an exact transaction state returned by history inspection")?;
            undo.iter()
                .chain(self.document.redo_steps().iter().rev())
                .position(|step| step.id() == id)
                .map(|position| position + 1)
                .ok_or("The selected transaction is no longer in this document's history")?
        };
        let steps = undo.len().abs_diff(position);
        if steps > MAX_NAVIGATION_STEPS {
            return Err(format!(
                "History navigation is limited to {MAX_NAVIGATION_STEPS} transactions at a time; select an intermediate state."
            ));
        }
        if steps > 0
            && let Some(reason) = self.history_navigation_blocked()
        {
            return Err(reason);
        }
        Ok(Plan {
            snapshot: snapshot.into(),
            from: self.history_current_state(),
            to: target.into(),
            direction: if position < undo.len() {
                "undo"
            } else if position > undo.len() {
                "redo"
            } else {
                "none"
            },
            steps,
            unsaved_changes: self.document.is_dirty(),
        })
    }

    pub fn navigate_history(
        &mut self,
        actor: Actor,
        snapshot: &str,
        target: &str,
    ) -> Result<Navigation, String> {
        let plan = self.plan_history_navigation(snapshot, target)?;
        if plan.steps > 0 {
            for _ in 0..plan.steps {
                if plan.direction == "undo" {
                    self.document.undo().expect("validated history position");
                } else {
                    self.document.redo().expect("validated history position");
                }
            }
            self.prune_selection();
            self.changed();
            self.log(
                actor,
                format!(
                    "history: {} {} transactions; state {}",
                    plan.direction, plan.steps, plan.to
                ),
            );
        }
        Ok(Navigation {
            plan,
            snapshot: self.history_snapshot(),
            current_state: self.history_current_state(),
            unsaved_changes: self.document.is_dirty(),
        })
    }
}
