use map_core::groups::{self, Edit, Graph, Member};
use serde_json::{Value, json};

use super::*;

#[derive(Default)]
pub(super) struct State {
    pub draft: Option<Draft>,
    serial: u64,
    bypass: bool,
}

#[derive(Clone)]
pub(super) struct Draft {
    token: u64,
    generation: u64,
    revision: u64,
    edit: Edit,
}

pub(super) struct SelectionIndex {
    roots: BTreeMap<ObjectId, u32>,
    members: BTreeMap<u32, Vec<ObjectId>>,
}

impl SelectionIndex {
    pub(super) fn key(&self, id: ObjectId) -> Member {
        self.roots
            .get(&id)
            .map_or(Member::Object(id), |&root| Member::Group(root))
    }

    pub(super) fn members(&self, id: ObjectId) -> Vec<ObjectId> {
        let mut members = self
            .roots
            .get(&id)
            .and_then(|root| self.members.get(root))
            .cloned()
            .unwrap_or_default();
        if !members.contains(&id) {
            members.push(id);
        }
        members
    }

    fn units(&self, ids: &[ObjectId]) -> Vec<Vec<ObjectId>> {
        let mut units: BTreeMap<Member, BTreeSet<ObjectId>> = BTreeMap::new();
        for &id in ids {
            let key = self.key(id);
            units.entry(key).or_default().extend(self.members(id));
        }
        units
            .into_values()
            .map(|members| members.into_iter().collect())
            .collect()
    }
}

pub fn member_id(member: Member) -> String {
    match member {
        Member::Object(id) => crate::identity::id_string(id),
        Member::Group(id) => format!("group:{id}"),
    }
}

pub fn parse_member(value: &str) -> Result<Member, String> {
    if let Some(id) = value.strip_prefix("group:") {
        return id
            .parse()
            .map(Member::Group)
            .map_err(|_| "Expected group:<number>".into());
    }
    crate::identity::parse_id(value).map(Member::Object)
}

impl Session {
    fn group_graph(&self) -> Result<Graph, String> {
        Graph::read(&self.document).map_err(|error| error.to_string())
    }

    fn group_object_hidden(&self, id: ObjectId) -> bool {
        match id {
            ObjectId::Solid(id) => self.document.solid(id).is_some_and(|solid| solid.hidden),
            ObjectId::Entity(id) => self.document.entity(id).is_some_and(|entity| entity.hidden),
        }
    }

    fn group_object_selectable(&self, id: ObjectId) -> bool {
        if !self.working_object_visible(&self.document, id) {
            return false;
        }
        let visible = |solid: &source_formats::vmf::Solid| {
            !solid.hidden
                && (self.view.tool_textures
                    || !solid.sides.iter().all(|side| {
                        self.materials
                            .find(&side.material)
                            .is_some_and(|material| material.tool)
                    }))
        };
        match id {
            ObjectId::Solid(id) => self.document.solid(id).is_some_and(visible),
            ObjectId::Entity(id) => self.document.entity(id).is_some_and(|entity| {
                !entity.hidden && (entity.solids.is_empty() || entity.solids.iter().any(visible))
            }),
        }
    }

    pub(super) fn group_selection_index(&self) -> Option<SelectionIndex> {
        if self.groups.bypass || self.document.vmf().world.groups.is_empty() {
            return None;
        }
        let graph = self.group_graph().ok()?;
        let mut index = SelectionIndex {
            roots: BTreeMap::new(),
            members: BTreeMap::new(),
        };
        for (&id, &parent) in &graph.objects {
            if let Some(root) = parent.and_then(|parent| graph.root(parent).ok()) {
                index.roots.insert(id, root);
                if self.group_object_selectable(id) {
                    index.members.entry(root).or_default().push(id);
                }
            }
        }
        Some(index)
    }

    pub(super) fn group_selection_units(&self, ids: &[ObjectId]) -> Vec<Vec<ObjectId>> {
        self.group_selection_index().map_or_else(
            || ids.iter().map(|&id| vec![id]).collect(),
            |index| index.units(ids),
        )
    }

    pub(super) fn group_region_matches(
        &self,
        ids: &[ObjectId],
        rule: map_core::region::Match,
    ) -> Vec<ObjectId> {
        let matched: BTreeSet<_> = ids.iter().copied().collect();
        self.group_selection_units(ids)
            .into_iter()
            .filter(|unit| {
                (rule != map_core::region::Match::Contained
                    || unit.iter().all(|id| matched.contains(id)))
                    && unit.iter().any(|id| {
                        matched.contains(id) && self.selection_filters().allows(&self.document, *id)
                    })
            })
            .flatten()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(super) fn grouped_preview_selection(
        &self,
        selected: &HashSet<ObjectId>,
        ids: &[ObjectId],
        mode: SelectMode,
    ) -> HashSet<ObjectId> {
        let units = self.group_selection_units(ids);
        let mut result = if mode == SelectMode::Replace {
            HashSet::new()
        } else {
            selected.clone()
        };
        for unit in units {
            if mode == SelectMode::Remove
                || mode == SelectMode::Toggle && unit.iter().all(|id| selected.contains(id))
            {
                for id in unit {
                    result.remove(&id);
                }
            } else {
                result.extend(unit);
            }
        }
        result
    }

    pub fn group_selection_bypass(&self) -> bool {
        self.groups.bypass
    }

    pub fn set_group_selection_bypass(&mut self, actor: Actor, bypass: bool) {
        if self.groups.bypass != bypass {
            self.cancel_area_selection();
            self.clear_volume_selection();
            self.clear_paint_stroke();
            self.groups.bypass = bypass;
            self.view_serial += 1;
            self.log(
                actor,
                if bypass {
                    "bypass object groups"
                } else {
                    "select outer object groups"
                },
            );
        }
    }

    fn group_member_json(&self, graph: &Graph, member: Member) -> Value {
        let (label, hidden, count) = match member {
            Member::Object(id) => {
                let label = match id {
                    ObjectId::Solid(_) => "World brush".to_string(),
                    ObjectId::Entity(id) => self
                        .document
                        .entity(id)
                        .map(|entity| {
                            let class = entity.classname().unwrap_or("entity");
                            let name = entity.properties.get("targetname").unwrap_or("");
                            if name.is_empty() {
                                class.into()
                            } else {
                                format!("{class} · {name}")
                            }
                        })
                        .unwrap_or_default(),
                };
                (label, self.group_object_hidden(id), 1)
            }
            Member::Group(id) => {
                let label = self
                    .document
                    .vmf()
                    .world
                    .groups
                    .iter()
                    .find(|group| group.id == id)
                    .map(groups::name)
                    .filter(|name| !name.is_empty())
                    .unwrap_or("Unnamed group")
                    .to_string();
                (label, false, graph.counts(id).1)
            }
        };
        json!({"id":member_id(member), "label":label, "hidden":hidden, "objects":count,
            "kind":if matches!(member, Member::Group(_)) {"group"} else {"object"}})
    }

    pub fn object_groups(
        &self,
        group: Option<u32>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        let graph = self.group_graph()?;
        let limit = limit.clamp(1, 200);
        let owned_brush_links = groups::owned_brush_memberships(&self.document);
        let rows = self.document.vmf().world.groups.iter();
        let rows: Vec<_> = rows.map(|group| {
            json!({"id":group.id, "name":groups::name(group), "parent":graph.parents[&group.id],
                "depth":graph.depth(group.id).unwrap_or(0), "direct_members":graph.counts(group.id).0,
                "objects":graph.counts(group.id).1,
                "owned_brush_links":owned_brush_links.get(&group.id).cloned().unwrap_or_default()})
        }).collect();
        let detail = if let Some(id) = group {
            let members = graph
                .direct_members(id)
                .map_err(|error| error.to_string())?;
            let record = rows
                .iter()
                .find(|row| row["id"].as_u64() == Some(u64::from(id)))
                .cloned()
                .unwrap();
            json!({"group":record, "members":members.iter().skip(offset).take(limit)
                .map(|&member| self.group_member_json(&graph, member)).collect::<Vec<_>>(),
                "total":members.len(), "offset":offset, "next_offset":if offset.saturating_add(limit) < members.len() {Some(offset + limit)} else {None}})
        } else {
            Value::Null
        };
        Ok(
            json!({"generation":self.document_generation, "revision":self.document.revision(),
            "bypass":self.groups.bypass, "groups":rows.iter().skip(offset).take(limit).collect::<Vec<_>>(),
            "total":rows.len(), "offset":offset, "next_offset":if offset.saturating_add(limit) < rows.len() {Some(offset + limit)} else {None},
            "detail":detail, "draft":self.group_edit_report(),
            "limits":{"groups":groups::MAX_GROUPS,"members":groups::MAX_MEMBERS,"depth":groups::MAX_DEPTH,"name_characters":groups::MAX_NAME_CHARS}}),
        )
    }

    fn check_group_draft(&self, token: u64, stale_allowed: bool) -> Result<&Draft, String> {
        let draft = self
            .groups
            .draft
            .as_ref()
            .ok_or("No group draft; create or edit a group first")?;
        if draft.token != token {
            return Err("Group draft was replaced; reopen it".into());
        }
        if !stale_allowed
            && (draft.generation != self.document_generation
                || draft.revision != self.document.revision())
        {
            return Err("Map changed; cancel and reopen the group draft".into());
        }
        Ok(draft)
    }

    pub fn group_edit_report(&self) -> Value {
        let Some(draft) = &self.groups.draft else {
            return Value::Null;
        };
        let plan = self.check_group_draft(draft.token, false).and_then(|draft| {
            let mut preview = self.document.preview_snapshot();
            let id = preview.edit("group preview", |tx| groups::apply(tx, &draft.edit)).map_err(|error| error.to_string())?;
            let before = self.group_graph()?;
            let after = Graph::read(&preview).map_err(|error| error.to_string())?;
            let changed_objects = before.objects.iter().filter(|&(id, parent)| after.objects.get(id) != Some(parent)).count();
            Ok(json!({"group":id,"changed_objects":changed_objects,"groups_after":after.parents.len()}))
        });
        let graph = self.group_graph().ok();
        json!({"token":draft.token,"generation":draft.generation,"revision":draft.revision,
            "group":draft.edit.group,"name":draft.edit.name,"parent":draft.edit.parent,
            "ungroup":draft.edit.ungroup,"valid":plan.is_ok(),"error":plan.as_ref().err(),"plan":plan.ok(),
            "members":draft.edit.members.iter().map(|&member| graph.as_ref().map_or_else(
                || json!({"id":member_id(member),"label":"Unavailable"}),
                |graph| self.group_member_json(graph, member))).collect::<Vec<_>>()})
    }

    pub fn begin_group_edit(&mut self, group: Option<u32>, ungroup: bool) -> Result<Value, String> {
        let graph = self.group_graph()?;
        let edit = if let Some(id) = group {
            let members = graph
                .direct_members(id)
                .map_err(|error| error.to_string())?;
            let record = self
                .document
                .vmf()
                .world
                .groups
                .iter()
                .find(|group| group.id == id)
                .unwrap();
            Edit {
                group,
                name: groups::name(record).into(),
                parent: graph.parents[&id],
                members,
                ungroup,
            }
        } else {
            if ungroup {
                return Err("Choose a group to ungroup".into());
            }
            Edit {
                group: None,
                name: String::new(),
                parent: None,
                members: graph.selection_members(&self.selected_ids()),
                ungroup: false,
            }
        };
        self.groups.serial += 1;
        self.groups.draft = Some(Draft {
            token: self.groups.serial,
            generation: self.document_generation,
            revision: self.document.revision(),
            edit,
        });
        self.view_serial += 1;
        Ok(self.group_edit_report())
    }

    pub fn update_group_edit(
        &mut self,
        token: u64,
        name: Option<String>,
        parent: Option<Option<u32>>,
        members: Option<Vec<Member>>,
    ) -> Result<Value, String> {
        let mut draft = self.check_group_draft(token, false)?.clone();
        if draft.edit.ungroup {
            return Err(
                "Ungroup drafts cannot edit membership; cancel and choose Edit group".into(),
            );
        }
        if let Some(name) = name {
            draft.edit.name = name;
        }
        if let Some(parent) = parent {
            draft.edit.parent = parent;
        }
        if let Some(members) = members {
            if members.len() > groups::MAX_MEMBERS {
                return Err(format!(
                    "At most {} direct members per edit",
                    groups::MAX_MEMBERS
                ));
            }
            draft.edit.members = members;
        }
        self.groups.draft = Some(draft);
        self.view_serial += 1;
        Ok(self.group_edit_report())
    }

    pub fn add_group_edit_members(
        &mut self,
        token: u64,
        members: &[Member],
    ) -> Result<Value, String> {
        let mut wanted = self.check_group_draft(token, false)?.edit.members.clone();
        for &member in members {
            if !wanted.contains(&member) {
                wanted.push(member);
            }
        }
        self.update_group_edit(token, None, None, Some(wanted))
    }

    pub fn add_selection_to_group_edit(&mut self, token: u64) -> Result<Value, String> {
        let graph = self.group_graph()?;
        let draft = self.check_group_draft(token, false)?;
        let mut members = graph.selection_members(&self.selected_ids());
        if let Some(id) = draft.edit.group
            && members.contains(&Member::Group(id))
        {
            members.retain(|member| *member != Member::Group(id));
            members.extend(
                graph
                    .direct_members(id)
                    .map_err(|error| error.to_string())?,
            );
        }
        self.add_group_edit_members(token, &members)
    }

    pub fn remove_group_edit_member(
        &mut self,
        token: u64,
        member: Member,
    ) -> Result<Value, String> {
        let mut members = self.check_group_draft(token, false)?.edit.members.clone();
        members.retain(|candidate| *candidate != member);
        self.update_group_edit(token, None, None, Some(members))
    }

    pub fn apply_group_edit(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self.check_group_draft(token, false)?.clone();
        let id = self.edit(
            actor,
            if draft.edit.ungroup {
                "ungroup objects"
            } else {
                "edit object group"
            },
            |tx| groups::apply(tx, &draft.edit),
        )?;
        self.groups.draft = None;
        self.view_serial += 1;
        Ok(json!({"group":id,"ungrouped":draft.edit.ungroup}))
    }

    pub fn cancel_group_edit(&mut self, token: u64) -> Result<(), String> {
        self.check_group_draft(token, true)?;
        self.groups.draft = None;
        self.view_serial += 1;
        Ok(())
    }

    pub fn select_object_group(
        &mut self,
        group: u32,
        mode: SelectMode,
        include_hidden: bool,
    ) -> Result<Vec<ObjectId>, String> {
        let graph = self.group_graph()?;
        let ids: Vec<_> = graph
            .descendants(group)
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|&id| include_hidden || !self.group_object_hidden(id))
            .collect();
        self.select_exact(&ids, mode)?;
        Ok(ids)
    }

    pub fn select_group_member(
        &mut self,
        member: Member,
        mode: SelectMode,
        include_hidden: bool,
    ) -> Result<Vec<ObjectId>, String> {
        match member {
            Member::Group(id) => self.select_object_group(id, mode, include_hidden),
            Member::Object(id) => {
                if !include_hidden && self.group_object_hidden(id) {
                    return Err("Member is hidden; enable Include hidden members first".into());
                }
                self.select_exact(&[id], mode)?;
                Ok(vec![id])
            }
        }
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.draft.as_ref().map(|draft| {
            serde_json::json!([
                self.serial,
                draft.token,
                draft.edit.name,
                draft.edit.parent,
                draft
                    .edit
                    .members
                    .iter()
                    .copied()
                    .map(member_id)
                    .collect::<Vec<_>>()
            ])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn fixture() -> Session {
        let mut session = Session::new(Err("no game assets".into()));
        for x in [0.0, 64.0, 128.0] {
            session
                .create_entity(
                    Actor::User,
                    "info_target",
                    Some(DVec3::new(x, 0.0, 0.0)),
                    BTreeMap::new(),
                )
                .unwrap();
        }
        session
    }

    fn create(session: &mut Session, members: &[Member]) -> u32 {
        let token = session.begin_group_edit(None, false).unwrap()["token"]
            .as_u64()
            .unwrap();
        session
            .update_group_edit(token, Some("Assembly".into()), None, Some(members.to_vec()))
            .unwrap();
        session.apply_group_edit(Actor::Agent, token).unwrap()["group"]
            .as_u64()
            .unwrap() as u32
    }

    #[test]
    fn group_drafts_share_actors_nested_selection_bypass_and_atomic_history() {
        let mut session = fixture();
        let inner = create(
            &mut session,
            &[
                Member::Object(ObjectId::Entity(2)),
                Member::Object(ObjectId::Entity(3)),
            ],
        );
        let outer = create(
            &mut session,
            &[Member::Group(inner), Member::Object(ObjectId::Entity(4))],
        );
        let source = session.document.vmf().clone();
        let revision = session.document.revision();
        session
            .select_grouped(&[ObjectId::Entity(2)], SelectMode::Replace)
            .unwrap();
        assert_eq!(session.selected_ids().len(), 3);
        session
            .select_grouped(&[ObjectId::Entity(3)], SelectMode::Toggle)
            .unwrap();
        assert!(session.selected_ids().is_empty());
        session
            .select_object_group(inner, SelectMode::Replace, false)
            .unwrap();
        assert_eq!(session.selected_ids().len(), 2);
        session.set_group_selection_bypass(Actor::User, true);
        session
            .select_grouped(&[ObjectId::Entity(2)], SelectMode::Replace)
            .unwrap();
        assert_eq!(session.selected_ids(), vec![ObjectId::Entity(2)]);
        session.set_group_selection_bypass(Actor::Agent, false);
        session
            .select_grouped(&[ObjectId::Entity(3)], SelectMode::Toggle)
            .unwrap();
        assert_eq!(session.selected_ids().len(), 3);
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.revision(), revision);
        let draft = session.begin_group_edit(Some(inner), true).unwrap();
        assert!(draft["valid"].as_bool().unwrap());
        assert_eq!(session.document.vmf(), &source);
        session
            .apply_group_edit(Actor::User, draft["token"].as_u64().unwrap())
            .unwrap();
        assert_eq!(
            session.group_graph().unwrap().objects[&ObjectId::Entity(2)],
            Some(outer)
        );
        session.undo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &source);
        session.redo(Actor::User).unwrap();
        assert!(!session.group_graph().unwrap().parents.contains_key(&inner));
    }

    #[test]
    fn tokens_staleness_invalid_cycles_cancellation_and_hidden_members_are_guarded() {
        let mut session = fixture();
        let inner = create(
            &mut session,
            &[
                Member::Object(ObjectId::Entity(2)),
                Member::Object(ObjectId::Entity(3)),
            ],
        );
        let outer = create(&mut session, &[Member::Group(inner)]);
        let token = session.begin_group_edit(Some(outer), false).unwrap()["token"]
            .as_u64()
            .unwrap();
        let invalid = session
            .update_group_edit(token, None, Some(Some(inner)), None)
            .unwrap();
        assert_eq!(invalid["valid"], false);
        let source = session.document.vmf().clone();
        assert!(session.apply_group_edit(Actor::User, token).is_err());
        assert_eq!(session.document.vmf(), &source);
        let next = session.begin_group_edit(Some(inner), false).unwrap()["token"]
            .as_u64()
            .unwrap();
        assert!(session.cancel_group_edit(token).is_err());
        assert_eq!(session.group_edit_report()["token"], next);
        session
            .edit(Actor::Agent, "hide", |tx| {
                tx.entity_mut(3)?.hidden = true;
                Ok(())
            })
            .unwrap();
        assert!(session.apply_group_edit(Actor::User, next).is_err());
        session.cancel_group_edit(next).unwrap();
        session
            .select_object_group(inner, SelectMode::Replace, false)
            .unwrap();
        assert_eq!(session.selected_ids(), vec![ObjectId::Entity(2)]);
        session
            .select_object_group(inner, SelectMode::Replace, true)
            .unwrap();
        assert_eq!(session.selected_ids().len(), 2);
        let next = session.begin_group_edit(None, false).unwrap()["token"]
            .as_u64()
            .unwrap();
        session.new_map(Actor::User, true).unwrap();
        assert!(session.apply_group_edit(Actor::Agent, next).is_err());
        assert_eq!(session.group_edit_report(), Value::Null);
    }

    #[test]
    fn explicit_transform_scope_stays_exact_and_grouped_volume_previews_match_commit() {
        let mut session = fixture();
        let group = create(
            &mut session,
            &[
                Member::Object(ObjectId::Entity(2)),
                Member::Object(ObjectId::Entity(3)),
            ],
        );
        session
            .select_grouped(&[ObjectId::Entity(2)], SelectMode::Replace)
            .unwrap();
        let original = session.document.vmf().clone();
        session
            .preview_transform(
                Actor::Agent,
                &[ObjectId::Entity(2)],
                map_core::edit::Transform {
                    translation: DVec3::X * 8.0,
                    ..Default::default()
                },
                Some(DVec3::ZERO),
            )
            .unwrap();
        assert_eq!(session.selected_ids(), vec![ObjectId::Entity(2)]);
        assert_eq!(
            session.geometry.transform_preview.as_ref().unwrap().ids,
            vec![ObjectId::Entity(2)]
        );
        assert_eq!(session.document.vmf(), &original);
        session.commit_transform_preview(Actor::Agent).unwrap();
        assert_eq!(
            session.document.entity(2).unwrap().origin(),
            Some(DVec3::X * 8.0)
        );
        assert_eq!(
            session.document.entity(3).unwrap().origin(),
            Some(DVec3::X * 64.0)
        );
        session.undo(Actor::User).unwrap();
        assert_eq!(session.document.vmf(), &original);
        let spec = volume::Spec {
            mins: DVec3::splat(-12.0),
            maxs: DVec3::splat(12.0),
            rule: map_core::region::Match::Intersecting,
            mode: SelectMode::Toggle,
            faces: false,
        };
        let token = session.preview_volume_selection(spec).unwrap();
        assert_eq!(
            session.volume_preview().unwrap().matches.objects,
            vec![ObjectId::Entity(2), ObjectId::Entity(3)]
        );
        let preview = session.grouped_preview_selection(
            &session.selection.iter().copied().collect(),
            &session.volume_preview().unwrap().matches.objects,
            SelectMode::Toggle,
        );
        session.commit_volume_selection(Actor::User, token).unwrap();
        assert_eq!(
            session.selection.iter().copied().collect::<HashSet<_>>(),
            preview
        );
        assert_eq!(session.selected_ids().len(), 2);
        let token = session
            .preview_volume_selection(volume::Spec {
                rule: map_core::region::Match::Contained,
                ..spec
            })
            .unwrap();
        assert!(session.volume_preview().unwrap().matches.objects.is_empty());
        session.cancel_volume_selection(token).unwrap();
        session.set_group_selection_bypass(Actor::Agent, true);
        let token = session
            .preview_volume_selection(volume::Spec {
                mode: SelectMode::Replace,
                ..spec
            })
            .unwrap();
        assert_eq!(
            session.volume_preview().unwrap().matches.objects,
            vec![ObjectId::Entity(2)]
        );
        session.set_group_selection_bypass(Actor::User, false);
        assert!(session.commit_volume_selection(Actor::User, token).is_err());
        session
            .select_object_group(group, SelectMode::Replace, false)
            .unwrap();
        assert_eq!(session.selected_ids().len(), 2);
    }

    #[test]
    fn grouped_paint_candidates_commit_once_and_bypass_invalidates_strokes() {
        let mut session = fixture();
        create(
            &mut session,
            &[
                Member::Object(ObjectId::Entity(2)),
                Member::Object(ObjectId::Entity(3)),
            ],
        );
        session
            .select(&[ObjectId::Entity(2)], SelectMode::Replace)
            .unwrap();
        let spec = paint::Spec {
            camera: Camera::planar(Projection::Top, Vec3::ZERO, 256.0),
            pointer: Pointer {
                position: DVec2::new(320.0, 240.0),
                size: (640, 480),
            },
            radius: 2.0,
            mode: SelectMode::Toggle,
            faces: false,
        };
        let token = session.begin_paint_selection(spec).unwrap();
        assert_eq!(
            session.paint_stroke().unwrap().objects,
            [ObjectId::Entity(2), ObjectId::Entity(3)]
                .into_iter()
                .collect()
        );
        session.commit_paint_selection(Actor::User, token).unwrap();
        assert_eq!(
            session.selected_ids(),
            vec![ObjectId::Entity(2), ObjectId::Entity(3)]
        );
        let token = session.begin_paint_selection(spec).unwrap();
        session.set_group_selection_bypass(Actor::Agent, true);
        assert!(session.paint_stroke().is_none());
        assert!(session.commit_paint_selection(Actor::User, token).is_err());
        let token = session.begin_paint_selection(spec).unwrap();
        assert_eq!(
            session.paint_stroke().unwrap().objects,
            [ObjectId::Entity(2)].into_iter().collect()
        );
        session.commit_paint_selection(Actor::User, token).unwrap();
        assert_eq!(session.selected_ids(), vec![ObjectId::Entity(3)]);
    }
}
