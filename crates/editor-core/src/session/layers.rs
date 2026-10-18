use super::{Actor, SelectMode, Session};
use map_core::{
    ObjectId,
    layers::{self, Edit, Graph},
};
use serde_json::{Value, json};

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    draft: Option<Draft>,
}
impl State {
    pub(super) fn clear_document_draft(&mut self) {
        self.draft = None;
        self.serial += 1;
    }
}
struct Draft {
    token: u64,
    version: u64,
    generation: u64,
    revision: u64,
    edit: Edit,
    error: Option<String>,
}

fn objects(ids: &[ObjectId]) -> Vec<String> {
    ids.iter()
        .copied()
        .map(crate::identity::id_string)
        .collect()
}

impl Session {
    pub fn layer_stamp(&self) -> String {
        format!(
            "{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.layers.serial
        )
    }

    pub fn layer_selection(&self) -> Vec<ObjectId> {
        if self.face_mode {
            self.face_selection
                .iter()
                .map(|face| ObjectId::Solid(face.solid))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        } else {
            self.selected_ids()
        }
    }

    pub fn layer_report(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
        selected: Option<u32>,
        member_offset: usize,
    ) -> Value {
        let draft=self.layers.draft.as_ref().map(|draft|json!({"token":draft.token,"version":draft.version,"id":draft.edit.id,"name":draft.edit.name,"color":draft.edit.color,"parent":draft.edit.parent,"members":objects(&draft.edit.members),"delete":draft.edit.delete,"error":draft.error,"valid":draft.error.is_none()&&draft.generation==self.document_generation&&draft.revision==self.document.revision()}));
        let mut result = json!({"snapshot":self.layer_stamp(),"draft":draft,"limits":{"layers":layers::MAX_LAYERS,"depth":layers::MAX_DEPTH,"links":layers::MAX_LINKS,"edit_members":layers::MAX_EDIT_MEMBERS,"name_characters":layers::MAX_NAME_CHARS},"visibility_policy":"Hide/show uses normal map visibility and compiler omission. Owned-brush hits select or hide their whole entity. Separate editor-only layer filters and automatic categories are unavailable."});
        match Graph::read(&self.document) {
            Err(error) => {
                result["error"] = json!(error.to_string());
                result["layers"] = json!([]);
            }
            Ok(graph) => {
                let query = query.to_lowercase();
                let mut records = graph
                    .records
                    .values()
                    .filter(|record| {
                        record.name.to_lowercase().contains(&query)
                            || record.id.to_string() == query
                    })
                    .collect::<Vec<_>>();
                records.sort_by_key(|record| {
                    let mut path = vec![record.id];
                    let mut parent = record.parent;
                    while let Some(id) = parent {
                        path.push(id);
                        parent = graph.records[&id].parent;
                    }
                    path.reverse();
                    path
                });
                result["parent_choices"] = json!(
                    graph
                        .records
                        .values()
                        .map(
                            |record| json!({"id":record.id,"name":record.name,"depth":record.depth})
                        )
                        .collect::<Vec<_>>()
                );
                let limit = limit.clamp(1, 128);
                result["total"] = json!(records.len());
                result["offset"] = json!(offset);
                result["next_offset"] = json!(
                    (offset.saturating_add(limit) < records.len())
                        .then_some(offset.saturating_add(limit))
                );
                result["layers"]=json!(records.into_iter().skip(offset).take(limit).map(|record|json!({"id":record.id,"name":record.name,"parent":record.parent,"color":record.color,"depth":record.depth,"direct_members":graph.memberships.values().filter(|member|member.layers.contains(&record.id)).count(),"objects_with_descendants":graph.members(record.id,true,true).map(|members|members.len()).unwrap_or(0)})).collect::<Vec<_>>());
                if let Some(id) = selected
                    && let Some(record) = graph.records.get(&id)
                {
                    let members = graph
                        .memberships
                        .iter()
                        .filter(|(_, member)| member.layers.contains(&id))
                        .collect::<Vec<_>>();
                    result["selected"] = json!({"id":id,"name":record.name,"parent":record.parent,"color":record.color,"member_total":members.len(),"members":members.iter().skip(member_offset).take(128).map(|(id,member)|json!({"id":crate::identity::id_string(**id),"owner":crate::identity::id_string(member.owner),"hidden":member.hidden})).collect::<Vec<_>>(),"next_member_offset":(member_offset.saturating_add(128)<members.len()).then_some(member_offset.saturating_add(128))});
                }
            }
        }
        result
    }

    fn layer_draft_checked(&self, token: u64, version: u64) -> Result<&Draft, String> {
        let draft = self
            .layers
            .draft
            .as_ref()
            .filter(|draft| draft.token == token && draft.version == version)
            .ok_or("Layer draft was changed or replaced; inspect it again")?;
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
        {
            return Err("Map changed; reopen the layer draft".into());
        }
        Ok(draft)
    }

    pub fn begin_layer_edit(
        &mut self,
        id: Option<u32>,
        delete: bool,
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.layer_stamp() {
            return Err("Map or layer draft changed; inspect layers again".into());
        }
        let graph = Graph::read(&self.document).map_err(|error| error.to_string())?;
        let edit = if let Some(id) = id {
            let record = graph.records.get(&id).ok_or("Layer no longer exists")?;
            Edit {
                id: Some(id),
                name: record.name.clone(),
                color: record.color,
                parent: record.parent,
                members: graph
                    .memberships
                    .iter()
                    .filter(|(_, member)| member.layers.contains(&id))
                    .map(|(id, _)| *id)
                    .collect(),
                delete,
            }
        } else {
            if delete {
                return Err("Choose an existing layer to delete".into());
            }
            Edit {
                id: None,
                name: "New layer".into(),
                color: [70, 160, 220],
                parent: None,
                members: self.layer_selection(),
                delete: false,
            }
        };
        let error = self.check_layer_edit(&edit).err();
        self.layers.serial += 1;
        self.layers.draft = Some(Draft {
            token: self.layers.serial,
            version: 0,
            generation: self.document_generation,
            revision: self.document.revision(),
            edit,
            error,
        });
        self.view_serial += 1;
        Ok(self.layer_report("", 0, 64, id, 0))
    }

    fn check_layer_edit(&self, edit: &Edit) -> Result<(), String> {
        Graph::read(&self.document).map_err(|error| error.to_string())?;
        let mut document = self.document.preview_snapshot();
        document
            .edit("check layer", |tx| layers::apply(tx, edit))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn update_layer_edit(
        &mut self,
        token: u64,
        version: u64,
        name: String,
        color: [u8; 3],
        parent: Option<u32>,
        members: Vec<ObjectId>,
    ) -> Result<Value, String> {
        let draft = self.layer_draft_checked(token, version)?;
        let edit = Edit {
            id: draft.edit.id,
            name,
            color,
            parent,
            members,
            delete: draft.edit.delete,
        };
        let error = self.check_layer_edit(&edit).err();
        let draft = self.layers.draft.as_mut().unwrap();
        draft.edit = edit;
        draft.error = error;
        draft.version += 1;
        self.layers.serial += 1;
        self.view_serial += 1;
        Ok(self.layer_report("", 0, 64, draft_id(self), 0))
    }

    pub fn apply_layer_edit(
        &mut self,
        actor: Actor,
        token: u64,
        version: u64,
    ) -> Result<Value, String> {
        let draft = self.layer_draft_checked(token, version)?;
        if let Some(error) = &draft.error {
            return Err(error.clone());
        }
        let edit = draft.edit.clone();
        let id = self.edit(
            actor,
            if edit.delete {
                "delete layer"
            } else {
                "edit layer"
            },
            |tx| layers::apply(tx, &edit),
        )?;
        self.layers.draft = None;
        self.layers.serial += 1;
        self.view_serial += 1;
        Ok(
            json!({"layer":id,"deleted":edit.delete,"state":self.layer_report("",0,64,(!edit.delete).then_some(id),0)}),
        )
    }

    pub fn cancel_layer_edit(&mut self, token: u64, version: u64) -> Result<Value, String> {
        self.layers
            .draft
            .as_ref()
            .filter(|draft| draft.token == token && draft.version == version)
            .ok_or("Layer draft was changed or replaced")?;
        self.layers.draft = None;
        self.layers.serial += 1;
        self.view_serial += 1;
        Ok(self.layer_report("", 0, 64, None, 0))
    }

    pub fn select_layer_members(
        &mut self,
        id: u32,
        recursive: bool,
        include_hidden: bool,
        mode: SelectMode,
        snapshot: &str,
    ) -> Result<Value, String> {
        if self.layer_stamp() != snapshot {
            return Err("Map or layers changed; inspect them again".into());
        }
        let graph = Graph::read(&self.document).map_err(|error| error.to_string())?;
        let members = graph
            .members(id, recursive, include_hidden)
            .map_err(|error| error.to_string())?;
        self.select(&members, mode)?;
        Ok(json!({"members":objects(&members),"selection":objects(&self.selected_ids())}))
    }

    pub fn set_layer_visibility(
        &mut self,
        actor: Actor,
        id: u32,
        recursive: bool,
        action: &str,
        snapshot: &str,
    ) -> Result<Value, String> {
        if self.layer_stamp() != snapshot {
            return Err("Map or layers changed; inspect them again".into());
        }
        if !["hide", "show"].contains(&action) {
            return Err("Layer visibility action must be hide or show".into());
        }
        let graph = Graph::read(&self.document).map_err(|error| error.to_string())?;
        let members = graph
            .members(id, recursive, true)
            .map_err(|error| error.to_string())?;
        if !members.is_empty() {
            self.set_visibility(actor, action, &members)?;
        }
        Ok(
            json!({"objects":objects(&members),"action":action,"state":self.layer_report("",0,64,Some(id),0)}),
        )
    }
}

fn draft_id(session: &Session) -> Option<u32> {
    session
        .layers
        .draft
        .as_ref()
        .and_then(|draft| draft.edit.id)
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.draft
            .as_ref()
            .map(|draft| serde_json::json!([self.serial, draft.token, draft.version]))
    }
}

#[cfg(test)]
mod tests;
