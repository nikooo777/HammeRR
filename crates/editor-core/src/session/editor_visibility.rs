use std::{collections::HashSet, sync::Arc};

use map_core::ObjectId;
use serde_json::{Value, json};

use super::{Actor, Session};

pub enum Action {
    Hide(Vec<ObjectId>),
    ShowAll,
}

impl Session {
    pub fn editor_hidden_count(&self) -> usize {
        self.working_region.editor_hidden.len()
    }

    pub fn can_hide_selected_in_editor(&self) -> bool {
        !self.closing
            && !self.selection.is_empty()
            && self.selection.iter().all(|id| self.document.contains(*id))
            && std::ptr::eq(self.render_document(), &self.document)
            && self
                .selection
                .iter()
                .any(|id| !self.editor_object_hidden(*id))
    }

    pub fn get_editor_visibility(&self, offset: usize, limit: usize) -> Result<Value, String> {
        if !(1..=200).contains(&limit) {
            return Err("Editor visibility page limit must be between 1 and 200".into());
        }
        let mut ids = self
            .working_region
            .editor_hidden
            .iter()
            .copied()
            .collect::<Vec<_>>();
        ids.sort();
        Ok(json!({
            "snapshot":self.working_region_stamp(),
            "hidden_total":ids.len(),
            "hidden":ids.iter().skip(offset).take(limit).map(|id|crate::identity::id_string(*id)).collect::<Vec<_>>(),
            "selected_hidden":self.selection.iter().filter(|id|self.editor_object_hidden(**id)).count(),
            "offset":offset,"limit":limit,
            "next_offset":(offset.saturating_add(limit)<ids.len()).then_some(offset.saturating_add(limit)),
            "can_hide_selection":self.can_hide_selected_in_editor(),
            "policy":"Temporary committed-root exclusions. Saved visibility, source, selection, Undo and builds are unchanged. Hide requires committed geometry; Show all clears only these exclusions and remains available during previews. New/Open resets them; removed source IDs are pruned after edits, Undo and Redo. Explicit IDs remain available; no additional group expansion."
        }))
    }

    pub fn set_editor_visibility(
        &mut self,
        actor: Actor,
        action: Action,
        snapshot: &str,
    ) -> Result<Value, String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        if snapshot != self.working_region_stamp() {
            return Err("Map, selection or view changed; inspect editor visibility again".into());
        }
        let mut next = self.working_region.editor_hidden.as_ref().clone();
        let hiding = matches!(&action, Action::Hide(_));
        match action {
            Action::ShowAll => next.clear(),
            Action::Hide(ids) => {
                if ids.is_empty() {
                    return Err(
                        "Select objects or provide committed root IDs to hide in the editor".into(),
                    );
                }
                for id in &ids {
                    if !self.document.contains(*id) {
                        return Err(format!(
                            "{} is not a committed root object",
                            crate::identity::id_string(*id)
                        ));
                    }
                }
                next.extend(ids);
            }
        }
        if next == *self.working_region.editor_hidden {
            return self.get_editor_visibility(0, 50);
        }
        if hiding && !std::ptr::eq(self.render_document(), &self.document) {
            return Err("Apply or cancel the displayed geometry preview before hiding objects in the editor; Show all editor-hidden remains available".into());
        }
        let serial = self
            .working_region
            .serial
            .checked_add(1)
            .ok_or("Editor visibility revision exhausted")?;
        self.invalidate_display_visibility_gestures();
        self.working_region.editor_hidden = Arc::new(next);
        self.working_region.serial = serial;
        self.preflight_sky3d_preview();
        self.scene_serial += 1;
        self.view_serial += 1;
        self.log(
            actor,
            if hiding {
                "Hid objects in the editor"
            } else {
                "Showed all editor-hidden objects"
            },
        );
        self.get_editor_visibility(0, 50)
    }

    pub(super) fn prune_editor_visibility(&mut self) {
        if self
            .working_region
            .editor_hidden
            .iter()
            .any(|id| !self.document.contains(*id))
        {
            self.working_region.editor_hidden = Arc::new(
                self.working_region
                    .editor_hidden
                    .iter()
                    .copied()
                    .filter(|id| self.document.contains(*id))
                    .collect::<HashSet<_>>(),
            );
        }
    }
}
