use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Actor, Document, HashSet, ObjectId, Session, paint};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectionFilters {
    pub world_brushes: bool,
    pub brush_entities: bool,
    pub point_entities: bool,
}

impl Default for SelectionFilters {
    fn default() -> Self {
        Self {
            world_brushes: true,
            brush_entities: true,
            point_entities: true,
        }
    }
}

impl SelectionFilters {
    pub(super) fn allows(self, document: &Document, id: ObjectId) -> bool {
        match id {
            ObjectId::Solid(_) => self.world_brushes,
            ObjectId::Entity(id) => document.entity(id).is_some_and(|entity| {
                if entity.solids.is_empty() {
                    self.point_entities
                } else {
                    self.brush_entities
                }
            }),
        }
    }

    pub(super) fn allowed_solids(self, document: &Document) -> Option<HashSet<u32>> {
        if self.world_brushes && self.brush_entities {
            return None;
        }
        Some(
            document
                .vmf()
                .world
                .solids
                .iter()
                .filter(|_| self.world_brushes)
                .map(|solid| solid.id)
                .chain(
                    document
                        .vmf()
                        .entities
                        .iter()
                        .filter(|_| self.brush_entities)
                        .flat_map(|entity| entity.solids.iter().map(|solid| solid.id)),
                )
                .collect(),
        )
    }
}

#[derive(Default)]
pub(super) struct State {
    pub(super) filters: SelectionFilters,
    pub(super) revision: u64,
}

impl Session {
    pub fn selection_filters(&self) -> SelectionFilters {
        self.selection_filters.filters
    }

    pub fn selection_filter_revision(&self) -> u64 {
        self.selection_filters.revision
    }

    pub fn selection_filter_state(&self) -> Value {
        json!({
            "filters":self.selection_filters(),
            "revision":self.selection_filter_revision(),
            "scope":"Object click, overlap cycling, marquee, volume and paint selection. Objects remain visible; faces, components, geometric queries and explicit IDs bypass these filters.",
            "groups":"Enabled hits seed complete visible groups. Fully inside still requires every visible group member inside. Bypass groups selects individual objects.",
        })
    }

    pub fn set_selection_filters(
        &mut self,
        actor: Actor,
        filters: SelectionFilters,
    ) -> Result<Value, String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        if self.selection_filters() == filters {
            return Ok(self.selection_filter_state());
        }
        let revision = self
            .selection_filter_revision()
            .checked_add(1)
            .ok_or("Selection filter revision exhausted")?;
        if self
            .area_preview
            .as_ref()
            .is_some_and(|preview| !preview.faces)
        {
            self.area_preview = None;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
        if self.paint_stroke().is_some_and(|stroke| {
            !stroke.spec.faces && stroke.operation == paint::Operation::Selection
        }) {
            self.clear_paint_stroke();
        }
        if self
            .volume_preview()
            .is_some_and(|preview| !preview.spec.faces)
        {
            self.clear_volume_selection();
        }
        self.selection_filters = State { filters, revision };
        self.log(actor, "changed object selection filters");
        Ok(self.selection_filter_state())
    }
}
