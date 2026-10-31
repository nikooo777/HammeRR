use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Actor, Document, HashSet, ObjectId, Session, paint};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ObjectSelectionMode {
    #[default]
    Groups,
    Objects,
    Solids,
}

impl ObjectSelectionMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Groups => "Groups",
            Self::Objects => "Objects",
            Self::Solids => "Solids",
        }
    }

    pub(super) fn hit_object(self, hit: map_core::query::Hit) -> ObjectId {
        if self == Self::Solids {
            hit.solid.map_or(hit.object, ObjectId::Solid)
        } else {
            hit.object
        }
    }
}

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
            ObjectId::Solid(id) => document.owner_of_solid(id).is_some_and(|owner| {
                if matches!(owner, ObjectId::Entity(_)) {
                    self.brush_entities
                } else {
                    self.world_brushes
                }
            }),
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
    pub(super) mode: ObjectSelectionMode,
    pub(super) revision: u64,
}

impl Session {
    pub fn selection_contains(&self, id: ObjectId) -> bool {
        match id {
            ObjectId::Solid(solid) => map_core::subobject::solid(&self.document, solid).is_ok(),
            ObjectId::Entity(_) => self.document.contains(id),
        }
    }

    pub fn require_root_objects(&self, ids: &[ObjectId]) -> Result<(), String> {
        for &id in ids {
            if self.document.contains(id) {
                continue;
            }
            if let ObjectId::Solid(solid) = id {
                let (owner, _) = map_core::subobject::solid(&self.document, solid)
                    .map_err(|error| error.to_string())?;
                return Err(format!(
                    "{} is a child brush; select its whole owner {} in Objects or Groups mode",
                    crate::identity::id_string(id),
                    crate::identity::id_string(owner)
                ));
            }
            return Err(format!("{} does not exist", crate::identity::id_string(id)));
        }
        Ok(())
    }

    pub(super) fn selection_in_mode(
        &self,
        selected: &HashSet<ObjectId>,
        mode: ObjectSelectionMode,
    ) -> HashSet<ObjectId> {
        selected
            .iter()
            .flat_map(|&id| match (mode, id) {
                (ObjectSelectionMode::Solids, ObjectId::Entity(entity)) => self
                    .document
                    .entity(entity)
                    .filter(|entity| !entity.solids.is_empty())
                    .map_or_else(
                        || vec![id],
                        |entity| {
                            entity
                                .solids
                                .iter()
                                .filter(|solid| {
                                    self.group_object_selectable(ObjectId::Solid(solid.id))
                                })
                                .map(|solid| ObjectId::Solid(solid.id))
                                .collect()
                        },
                    ),
                (
                    ObjectSelectionMode::Groups | ObjectSelectionMode::Objects,
                    ObjectId::Solid(solid),
                ) => {
                    vec![self.document.owner_of_solid(solid).unwrap_or(id)]
                }
                _ => vec![id],
            })
            .collect()
    }

    pub fn select_picked_objects(
        &mut self,
        ids: &[ObjectId],
        mode: super::SelectMode,
    ) -> Result<(), String> {
        if !self.view.tool_textures {
            self.hidden_brushes_for(false);
        }
        let selected = self.selection.iter().copied().collect();
        let selection = self.grouped_preview_selection(&selected, ids, mode);
        self.select(
            &selection.into_iter().collect::<Vec<_>>(),
            super::SelectMode::Replace,
        )
    }

    pub fn select_solids(
        &mut self,
        solids: &[u32],
        mode: super::SelectMode,
    ) -> Result<Value, String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        let ids = solids
            .iter()
            .map(|&solid| {
                map_core::subobject::solid(&self.document, solid)
                    .map(|_| ObjectId::Solid(solid))
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
        let before = self.feedback_handles();
        if mode != super::SelectMode::Replace && !self.view.tool_textures {
            self.hidden_brushes_for(false);
        }
        let selected = self.selection.iter().copied().collect();
        let mut result = if mode == super::SelectMode::Replace {
            HashSet::new()
        } else {
            self.selection_in_mode(&selected, ObjectSelectionMode::Solids)
        };
        for id in ids {
            if mode == super::SelectMode::Remove
                || mode == super::SelectMode::Toggle && result.contains(&id)
            {
                result.remove(&id);
            } else {
                result.insert(id);
            }
        }
        self.select(
            &result.into_iter().collect::<Vec<_>>(),
            super::SelectMode::Replace,
        )?;
        let after = self.feedback_handles();
        Ok(serde_json::json!({
            "selection":self.selected_ids().into_iter().map(crate::identity::id_string).collect::<Vec<_>>(),
            "cleared_preview_kinds":before.iter().filter(|handle|!after.contains(handle)).map(|handle|handle["kind"].clone()).collect::<Vec<_>>(),
        }))
    }

    pub fn object_selection_mode(&self) -> ObjectSelectionMode {
        self.selection_filters.mode
    }

    pub fn selection_filters(&self) -> SelectionFilters {
        self.selection_filters.filters
    }

    pub fn selection_filter_revision(&self) -> u64 {
        self.selection_filters.revision
    }

    pub fn selection_filter_state(&self) -> Value {
        json!({
            "filters":self.selection_filters(),
            "mode":self.object_selection_mode(),
            "revision":self.selection_filter_revision(),
            "scope":"Object click, overlap cycling, marquee, volume and paint selection. Objects remain visible; faces, components, geometric queries and explicit IDs bypass these filters.",
            "groups":"Groups selects outermost visible nested groups; Objects selects whole world brushes or entities; Solids selects individual world or brush-entity brushes, and individual point entities. Enabled seeds use the owning object's type. Fully inside requires the complete visible selection unit inside. Explicit select retains its root-object contract; select_solids selects exact brush IDs independently of these rules.",
        })
    }

    pub fn set_selection_filters(
        &mut self,
        actor: Actor,
        filters: SelectionFilters,
    ) -> Result<Value, String> {
        self.set_selection_rules(actor, filters, self.object_selection_mode())
    }

    pub fn set_object_selection_mode(
        &mut self,
        actor: Actor,
        mode: ObjectSelectionMode,
    ) -> Result<Value, String> {
        self.set_selection_rules(actor, self.selection_filters(), mode)
    }

    pub(super) fn set_selection_rules(
        &mut self,
        actor: Actor,
        filters: SelectionFilters,
        mode: ObjectSelectionMode,
    ) -> Result<Value, String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        if self.selection_filters() == filters && self.object_selection_mode() == mode {
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
        self.selection_filters = State {
            filters,
            mode,
            revision,
        };
        self.log(actor, format!("object selection: {}", mode.label()));
        Ok(self.selection_filter_state())
    }
}
