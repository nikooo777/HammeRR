use std::collections::BTreeSet;

use glam::DVec3;
use map_core::context::{self, Metadata, Note, Region, Scope};
use map_core::{ObjectId, query};
use serde::Serialize;
use serde_json::{Value, json};
use source_formats::vmf::format_number;

use super::{Actor, SelectMode, Session};

const DEFAULT_REGION_COLOR: [u8; 3] = [83, 196, 240];
const MIN_RESPONSE_BYTES: usize = 2048;
const MAX_RESPONSE_BYTES: usize = 65_536;

#[derive(Default)]
pub struct State {
    pub(super) serial: u64,
    pub draft: Option<Draft>,
    pub show_regions: bool,
    pub focused: Option<u32>,
}

#[derive(Clone, Serialize)]
pub struct Fields {
    pub summary: String,
    pub body: String,
    pub mins: [String; 3],
    pub maxs: [String; 3],
    pub color: [u8; 3],
}

#[derive(Clone)]
pub struct Draft {
    pub token: u64,
    pub scope: Scope,
    pub generation: u64,
    pub revision: u64,
    pub fields: Fields,
    pub existing: bool,
}

#[derive(Debug, Clone)]
pub struct Query {
    pub object: Option<ObjectId>,
    pub bounds: Option<[DVec3; 2]>,
    pub point: Option<DVec3>,
    pub include_map: bool,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            object: None,
            bounds: None,
            point: None,
            include_map: true,
        }
    }
}

fn number(value: &str) -> Result<f64, String> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| "Region coordinates must be finite numbers".into())
}

fn region(fields: &Fields) -> Result<Region, String> {
    let mut mins = DVec3::ZERO;
    let mut maxs = DVec3::ZERO;
    for axis in 0..3 {
        mins[axis] = number(&fields.mins[axis])?;
        maxs[axis] = number(&fields.maxs[axis])?;
    }
    let region = Region {
        mins,
        maxs,
        color: fields.color,
    };
    region.validate()?;
    Ok(region)
}

fn object_bounds(session: &Session, id: ObjectId) -> Option<(DVec3, DVec3)> {
    match id {
        ObjectId::Entity(_) => query::bounds(session.document(), id),
        ObjectId::Solid(id) => map_core::surface::solid(session.document(), id)
            .and_then(|solid| map_core::BrushGeometry::from_solid(solid).ok())
            .map(|geometry| (geometry.mins, geometry.maxs)),
    }
}

fn author(actor: Actor) -> &'static str {
    match actor {
        Actor::User => "human",
        Actor::Agent => "agent",
        Actor::System => "system",
    }
}

impl Session {
    pub fn context_serial(&self) -> u64 {
        self.context.serial
    }
    pub fn context_draft(&self) -> Option<&Draft> {
        self.context.draft.as_ref()
    }

    fn context_metadata(&self) -> Result<&Metadata, String> {
        self.document.context().map_err(|error| error.to_string())
    }

    pub fn selected_context_scope(&self) -> Option<Scope> {
        if self.face_mode {
            let brushes = self
                .face_selection
                .iter()
                .map(|face| face.solid)
                .collect::<BTreeSet<_>>();
            if brushes.len() == 1 {
                return Some(Scope::Object(ObjectId::Solid(*brushes.first()?)));
            }
        } else if let [id] = self.selected_ids().as_slice() {
            return Some(Scope::Object(*id));
        }
        None
    }

    fn context_note_value(&self, scope: Scope, note: &Note, full: bool) -> Value {
        let current = context::fingerprint(&self.document, scope, note.region);
        let ambiguous = matches!(scope, Scope::Object(id) if context::object_exists(&self.document, id) && !context::object_unique(&self.document, id));
        json!({"scope":scope.to_string(),"summary":note.summary,"body":if full {Some(&note.body)} else {None},"body_omitted":!full,
            "author":note.author,"stale":ambiguous || (current.as_ref() != Ok(&note.observed)),
            "orphaned":matches!(scope,Scope::Object(id) if !context::object_exists(&self.document,id)),
            "ambiguous":ambiguous,
            "text_bytes":note.text_bytes(),"region":note.region.map(|region|json!({"mins":region.mins.to_array(),"maxs":region.maxs.to_array(),"color":region.color})),
            "observation_scope":if matches!(scope,Scope::Region(_)) {"whole_map_and_region_bounds"} else if scope==Scope::Map {"whole_map"} else {"attached_object"}})
    }

    pub fn get_context_note(&self, scope: Scope) -> Result<Value, String> {
        let note = self
            .context_metadata()?
            .notes
            .get(&scope)
            .ok_or("Context note does not exist")?;
        Ok(
            json!({"note":self.context_note_value(scope,note,true),"generation":self.document_generation,"revision":self.document.revision()}),
        )
    }

    pub fn list_context_notes(
        &self,
        query: &str,
        kind: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        if kind.is_some_and(|kind| !matches!(kind, "map" | "object" | "region")) {
            return Err("Context kind must be map, object or region".into());
        }
        let metadata = self.context_metadata()?;
        let query = query.to_lowercase();
        let notes = metadata
            .notes
            .iter()
            .filter(|(scope, _)| {
                kind.is_none_or(|kind| {
                    matches!(
                        (kind, scope),
                        ("map", Scope::Map)
                            | ("object", Scope::Object(_))
                            | ("region", Scope::Region(_))
                    )
                })
            })
            .filter(|(scope, note)| {
                format!("{scope} {} {}", note.summary, note.body)
                    .to_lowercase()
                    .contains(&query)
            })
            .collect::<Vec<_>>();
        let page = notes
            .iter()
            .skip(offset)
            .take(limit.clamp(1, 100))
            .map(|(scope, note)| self.context_note_value(**scope, note, false))
            .collect::<Vec<_>>();
        Ok(
            json!({"notes":page,"matching":notes.len(),"offset":offset,"total":metadata.notes.len(),"text_bytes":metadata.text_bytes(),"generation":self.document_generation,"revision":self.document.revision()}),
        )
    }

    pub fn object_context_summary(&self, id: ObjectId) -> Value {
        let Ok(metadata) = self.context_metadata() else {
            return Value::Null;
        };
        if metadata.notes.is_empty() {
            return Value::Null;
        }
        let attached=metadata.notes.get(&Scope::Object(id)).map(|note|json!({"scope":Scope::Object(id).to_string(),"summary":note.summary,"author":note.author}));
        let map = metadata
            .notes
            .get(&Scope::Map)
            .map(|note| json!({"scope":"map","summary":note.summary}));
        let bounds = object_bounds(self, id);
        let regions = metadata
            .notes
            .iter()
            .filter(|(_, note)| {
                note.region
                    .is_some_and(|region| bounds.is_some_and(|(lo, hi)| region.intersects(lo, hi)))
            })
            .collect::<Vec<_>>();
        let regions_page=regions.iter().take(8).map(|(scope,note)|json!({"scope":scope.to_string(),"summary":note.summary,"author":note.author})).collect::<Vec<_>>();
        let children = match id {
            ObjectId::Entity(id) => self
                .document
                .entity(id)
                .map(|entity| {
                    entity
                        .solids
                        .iter()
                        .filter(|solid| {
                            metadata
                                .notes
                                .contains_key(&Scope::Object(ObjectId::Solid(solid.id)))
                        })
                        .count()
                })
                .unwrap_or_default(),
            _ => 0,
        };
        json!({"attached":attached,"map":map,"regions":regions_page,"region_count":regions.len(),"noted_owned_brushes":children,"full_context_tool":"get_context"})
    }

    pub fn retrieve_context(
        &self,
        query: &Query,
        max_bytes: usize,
        cursor: Option<&str>,
    ) -> Result<Value, String> {
        if !(MIN_RESPONSE_BYTES..=MAX_RESPONSE_BYTES).contains(&max_bytes) {
            return Err(format!(
                "Context response budget must be {MIN_RESPONSE_BYTES}–{MAX_RESPONSE_BYTES} bytes"
            ));
        }
        if usize::from(query.object.is_some())
            + usize::from(query.bounds.is_some())
            + usize::from(query.point.is_some())
            > 1
        {
            return Err("Choose one object, bounds or point for relevant context".into());
        }
        if let Some(point) = query.point
            && !point.is_finite()
        {
            return Err("Context query point must be finite".into());
        }
        if let Some([mins, maxs]) = query.bounds {
            Region {
                mins,
                maxs,
                color: DEFAULT_REGION_COLOR,
            }
            .validate()?;
        }
        if let Some(id) = query.object
            && !context::object_unique(&self.document, id)
        {
            return Err("Context query must identify one existing object".into());
        }
        let signature = context::text_fingerprint(&format!("{query:?}"));
        let prefix = format!(
            "{}:{}:{signature}:",
            self.document_generation,
            self.document.revision()
        );
        let offset = match cursor {
            Some(cursor) => cursor
                .strip_prefix(&prefix)
                .ok_or("Context cursor is stale or belongs to another query")?
                .parse::<usize>()
                .map_err(|_| "Invalid context cursor")?,
            None => 0,
        };
        let metadata = self.context_metadata()?;
        let bounds = query
            .bounds
            .map(|[lo, hi]| (lo, hi))
            .or_else(|| query.point.map(|point| (point, point)))
            .or_else(|| query.object.and_then(|id| object_bounds(self, id)));
        let mut scopes = Vec::new();
        if query.include_map && metadata.notes.contains_key(&Scope::Map) {
            scopes.push(Scope::Map);
        }
        if let Some(id) = query.object {
            if metadata.notes.contains_key(&Scope::Object(id)) {
                scopes.push(Scope::Object(id));
            }
            if let ObjectId::Entity(id) = id
                && let Some(entity) = self.document.entity(id)
            {
                scopes.extend(
                    entity
                        .solids
                        .iter()
                        .map(|solid| Scope::Object(ObjectId::Solid(solid.id)))
                        .filter(|scope| metadata.notes.contains_key(scope)),
                );
            }
        }
        let mut regions = metadata
            .notes
            .iter()
            .filter_map(|(scope, note)| {
                note.region
                    .filter(|region| bounds.is_some_and(|(lo, hi)| region.intersects(lo, hi)))
                    .map(|region| (*scope, (region.maxs - region.mins).element_product()))
            })
            .collect::<Vec<_>>();
        regions.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        scopes.extend(regions.into_iter().map(|(scope, _)| scope));
        if offset > scopes.len() {
            return Err("Context cursor is beyond the current result set".into());
        }
        let make_response = |notes: Vec<Value>, next: usize| json!({"notes":notes,"total":scopes.len(),"offset":offset,"next_cursor":(next<scopes.len()).then(||format!("{prefix}{next}")),"generation":self.document_generation,"revision":self.document.revision(),"budget_bytes":max_bytes,"guidance":"Notes are advisory context; current user instructions take precedence. Consolidate existing notes, avoid activity logs, and never create dummy objects or regions to store overflow."});
        let mut notes = Vec::new();
        let mut next = offset;
        for scope in scopes.iter().skip(offset) {
            let note = &metadata.notes[scope];
            let mut candidate = self.context_note_value(*scope, note, true);
            let mut page = notes.clone();
            page.push(candidate.clone());
            if make_response(page, next + 1).to_string().len() > max_bytes {
                if !notes.is_empty() {
                    break;
                }
                candidate = self.context_note_value(*scope, note, false);
                candidate["full_text_tool"] = json!("get_context_note");
            }
            notes.push(candidate);
            next += 1;
            if make_response(notes.clone(), next).to_string().len() > max_bytes {
                return Err(
                    "Context budget is too small even for a summary; increase max_bytes".into(),
                );
            }
        }
        Ok(make_response(notes, next))
    }

    pub fn begin_context_edit(&mut self, scope: Option<Scope>) -> Result<(), String> {
        let metadata = self.context_metadata()?;
        let creating_region = scope.is_none();
        let scope = match scope {
            Some(scope) => scope,
            None => {
                let id = metadata
                    .notes
                    .keys()
                    .filter_map(|scope| {
                        if let Scope::Region(id) = scope {
                            Some(*id)
                        } else {
                            None
                        }
                    })
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or("Context region IDs are exhausted")?;
                Scope::Region(id)
            }
        };
        if let Scope::Object(id) = scope
            && !context::object_unique(&self.document, id)
            && !metadata.notes.contains_key(&scope)
        {
            return Err("Context attachment must identify one existing object".into());
        }
        let note = metadata.notes.get(&scope);
        let existing = note.is_some();
        if matches!(scope, Scope::Region(_)) && !existing && !creating_region {
            return Err("Context region does not exist; use new_region to create one".into());
        }
        let bounds = note.and_then(|note| note.region).or_else(|| {
            matches!(scope, Scope::Region(_)).then(|| {
                let (mins, maxs) = self
                    .selection_bounds()
                    .filter(|(lo, hi)| lo.cmplt(*hi).all())
                    .unwrap_or((DVec3::splat(-64.0), DVec3::splat(64.0)));
                Region {
                    mins,
                    maxs,
                    color: DEFAULT_REGION_COLOR,
                }
            })
        });
        let fields = Fields {
            summary: note.map(|note| note.summary.clone()).unwrap_or_default(),
            body: note.map(|note| note.body.clone()).unwrap_or_default(),
            mins: bounds.map_or([String::new(), String::new(), String::new()], |region| {
                region.mins.to_array().map(format_number)
            }),
            maxs: bounds.map_or([String::new(), String::new(), String::new()], |region| {
                region.maxs.to_array().map(format_number)
            }),
            color: bounds.map_or(DEFAULT_REGION_COLOR, |region| region.color),
        };
        self.context.serial += 1;
        self.context.draft = Some(Draft {
            token: self.context.serial,
            scope,
            generation: self.document_generation,
            revision: self.document.revision(),
            fields,
            existing,
        });
        if matches!(scope, Scope::Region(_)) {
            self.context.show_regions = true;
        }
        self.scene_serial += 1;
        Ok(())
    }

    fn check_context_draft(&self, token: u64) -> Result<&Draft, String> {
        let draft = self
            .context
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Context edit was closed or replaced")?;
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
        {
            return Err("Map changed; reopen the context note before applying".into());
        }
        Ok(draft)
    }

    pub fn update_context_edit(&mut self, token: u64, fields: Fields) -> Result<(), String> {
        let draft = self.check_context_draft(token)?;
        if !matches!(draft.scope, Scope::Region(_))
            && (fields.mins != draft.fields.mins
                || fields.maxs != draft.fields.maxs
                || fields.color != draft.fields.color)
        {
            return Err("Only context regions have bounds and guide colors".into());
        }
        if fields.summary.len() + fields.body.len() > context::MAX_NOTE_BYTES * 4 {
            return Err(
                "Draft is too large; keep long context in a linked project document".into(),
            );
        }
        self.context.draft.as_mut().unwrap().fields = fields;
        self.context.serial += 1;
        self.scene_serial += 1;
        Ok(())
    }

    pub fn context_draft_feedback(&self) -> Result<usize, String> {
        let draft = self
            .context
            .draft
            .as_ref()
            .ok_or("No context edit is open")?;
        self.check_context_draft(draft.token)?;
        if let Scope::Object(id) = draft.scope
            && !context::object_unique(&self.document, id)
        {
            return Err(
                "Context attachment is missing or ambiguous; this note can be read or deleted"
                    .into(),
            );
        }
        let region = matches!(draft.scope, Scope::Region(_))
            .then(|| region(&draft.fields))
            .transpose()?;
        let note = Note {
            summary: draft.fields.summary.clone(),
            body: draft.fields.body.clone(),
            author: "human".into(),
            observed: String::new(),
            region,
            extra: vec![],
        };
        note.validate(draft.scope)?;
        let mut metadata = self.context_metadata()?.clone();
        metadata.notes.insert(draft.scope, note);
        metadata.validate()?;
        Ok(metadata.text_bytes())
    }

    pub fn apply_context_edit(&mut self, actor: Actor, token: u64) -> Result<Scope, String> {
        let draft = self.check_context_draft(token)?.clone();
        self.context_draft_feedback()?;
        let region = matches!(draft.scope, Scope::Region(_))
            .then(|| region(&draft.fields))
            .transpose()?;
        self.edit(actor, "update design context", |tx| {
            context::set_note(
                tx,
                draft.scope,
                draft.fields.summary,
                draft.fields.body,
                author(actor),
                region,
            )
        })?;
        self.context.draft = None;
        self.context.serial += 1;
        self.scene_serial += 1;
        Ok(draft.scope)
    }

    pub fn cancel_context_edit(&mut self, token: u64) -> Result<(), String> {
        if self
            .context
            .draft
            .as_ref()
            .is_none_or(|draft| draft.token != token)
        {
            return Err("Context edit was closed or replaced".into());
        }
        self.context.draft = None;
        self.context.serial += 1;
        self.scene_serial += 1;
        Ok(())
    }

    pub fn delete_context_note(
        &mut self,
        actor: Actor,
        scope: Scope,
        generation: u64,
        revision: u64,
    ) -> Result<(), String> {
        if generation != self.document_generation || revision != self.document.revision() {
            return Err("Map changed; read the note again before deleting".into());
        }
        self.edit(actor, "delete design context", |tx| {
            context::delete_note(tx, scope)
        })?;
        if self
            .context
            .draft
            .as_ref()
            .is_some_and(|draft| draft.scope == scope)
        {
            self.context.draft = None;
        }
        if matches!(scope,Scope::Region(id) if self.context.focused==Some(id)) {
            self.context.focused = None;
        }
        self.context.serial += 1;
        self.scene_serial += 1;
        Ok(())
    }

    pub fn set_context_view(&mut self, show_regions: bool) {
        if self.context.show_regions != show_regions {
            self.context.show_regions = show_regions;
            self.context.serial += 1;
            self.scene_serial += 1;
        }
    }

    pub fn focus_context(&mut self, actor: Actor, scope: Scope, aspect: f32) -> Result<(), String> {
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("View aspect must be finite and positive".into());
        }
        let bounds = match scope {
            Scope::Map => query::union(
                self.document
                    .objects()
                    .filter_map(|id| query::bounds(&self.document, id)),
            ),
            Scope::Object(id) => {
                if !context::object_unique(&self.document, id) {
                    return Err("Context attachment is missing or ambiguous".into());
                }
                object_bounds(self, id)
            }
            Scope::Region(_) => self
                .context_metadata()?
                .notes
                .get(&scope)
                .and_then(|note| note.region)
                .map(|region| (region.mins, region.maxs)),
        }
        .ok_or("No bounds are available for this context scope")?;
        if let Scope::Object(id) = scope {
            let owner = match id {
                ObjectId::Solid(id) => self.document.owner_of_solid(id).unwrap(),
                _ => id,
            };
            self.set_face_mode(false);
            self.select(&[owner], SelectMode::Replace)?;
        }
        if let Scope::Region(id) = scope {
            self.context.focused = Some(id);
            self.context.show_regions = true;
            self.context.serial += 1;
            self.scene_serial += 1;
        }
        self.set_camera(
            self.camera
                .frame_bounds(bounds.0.as_vec3(), bounds.1.as_vec3(), aspect),
        );
        self.log(actor, format!("focused context {scope}"));
        Ok(())
    }

    pub fn context_state(&self) -> Value {
        let (count, bytes, error) = match self.context_metadata() {
            Ok(metadata) => (metadata.notes.len(), metadata.text_bytes(), None),
            Err(error) => (0, 0, Some(error)),
        };
        let draft=self.context.draft.as_ref().map(|draft|json!({"token":draft.token.to_string(),"scope":draft.scope.to_string(),"generation":draft.generation,"revision":draft.revision,"fields":draft.fields,"existing":draft.existing,"error":self.context_draft_feedback().err(),"resulting_text_bytes":self.context_draft_feedback().ok()}));
        json!({"notes":count,"text_bytes":bytes,"error":error,"draft":draft,"show_regions":self.context.show_regions,"focused_region":self.context.focused,
            "limits":{"summary_characters":context::MAX_SUMMARY_CHARS,"note_text_bytes":context::MAX_NOTE_BYTES,"map_text_bytes":context::MAX_MAP_TEXT_BYTES,"regions":context::MAX_REGIONS,"default_response_bytes":context::DEFAULT_CONTEXT_BYTES},
            "selected_scope":self.selected_context_scope().map(|scope|scope.to_string()),"generation":self.document_generation,"revision":self.document.revision()})
    }

    pub fn append_context_guides(&self, mesh: &mut render::SceneMesh) {
        if !self.context.show_regions {
            return;
        }
        let mut regions = self
            .context_metadata()
            .ok()
            .into_iter()
            .flat_map(|metadata| metadata.notes.iter())
            .filter_map(|(scope, note)| note.region.map(|region| (*scope, region)))
            .collect::<Vec<_>>();
        if let Some(draft) = &self.context.draft
            && matches!(draft.scope, Scope::Region(_))
            && draft.generation == self.document_generation
            && draft.revision == self.document.revision()
            && let Ok(region) = region(&draft.fields)
        {
            regions.retain(|(scope, _)| *scope != draft.scope);
            regions.push((draft.scope, region));
        }
        for (scope, region) in regions {
            let color = if matches!(scope,Scope::Region(id) if self.context.focused==Some(id)) {
                [255, 207, 107, 255]
            } else {
                [region.color[0], region.color[1], region.color[2], 220]
            };
            let corners: Vec<DVec3> = (0..8)
                .map(|bits| {
                    DVec3::new(
                        if bits & 1 == 0 {
                            region.mins.x
                        } else {
                            region.maxs.x
                        },
                        if bits & 2 == 0 {
                            region.mins.y
                        } else {
                            region.maxs.y
                        },
                        if bits & 4 == 0 {
                            region.mins.z
                        } else {
                            region.maxs.z
                        },
                    )
                })
                .collect();
            for index in 0..8 {
                for bit in [1, 2, 4] {
                    let other = index ^ bit;
                    if index < other {
                        for point in [corners[index], corners[other]] {
                            mesh.lines.push(render::scene::LineVertex {
                                position: point.as_vec3().to_array(),
                                color,
                            });
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use map_core::edit;

    fn fixture() -> (Session, ObjectId) {
        let mut session = Session::new(Err("no assets needed".into()));
        let id = session
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        (session, id)
    }

    fn save_note(
        session: &mut Session,
        actor: Actor,
        scope: Option<Scope>,
        summary: &str,
        body: &str,
    ) -> Scope {
        session.begin_context_edit(scope).unwrap();
        let draft = session.context_draft().unwrap().clone();
        let mut fields = draft.fields;
        fields.summary = summary.into();
        fields.body = body.into();
        session.update_context_edit(draft.token, fields).unwrap();
        session.apply_context_edit(actor, draft.token).unwrap()
    }

    #[test]
    fn both_actors_share_drafts_cancel_apply_and_undo() {
        for actor in [Actor::User, Actor::Agent] {
            let (mut session, id) = fixture();
            let before = session.document.vmf().clone();
            session.begin_context_edit(Some(Scope::Object(id))).unwrap();
            let draft = session.context_draft().unwrap().clone();
            assert!(session.context_draft_feedback().is_err());
            let mut fields = draft.fields;
            fields.summary = "Keep this sightline blocked".into();
            fields.body = "Cover must remain at least 64 units tall.".into();
            session.update_context_edit(draft.token, fields).unwrap();
            assert!(session.context_draft_feedback().is_ok());
            assert_eq!(session.document.vmf(), &before);
            session.cancel_context_edit(draft.token).unwrap();
            assert_eq!(session.document.vmf(), &before);
            save_note(
                &mut session,
                actor,
                Some(Scope::Object(id)),
                "Blocker",
                "Keep it tall.",
            );
            let note = session.get_context_note(Scope::Object(id)).unwrap();
            assert_eq!(note["note"]["author"], author(actor));
            assert_eq!(note["note"]["stale"], false);
            session.undo(actor).unwrap();
            assert_eq!(session.document.vmf(), &before);
            session.redo(actor).unwrap();
            assert_eq!(
                session.list_context_notes("tall", None, 0, 30).unwrap()["matching"],
                1
            );
        }
    }

    #[test]
    fn stale_replaced_and_invalid_drafts_never_change_the_map() {
        let (mut session, id) = fixture();
        session.begin_context_edit(Some(Scope::Map)).unwrap();
        let old = session.context_draft().unwrap().token;
        session.begin_context_edit(Some(Scope::Object(id))).unwrap();
        assert!(session.cancel_context_edit(old).is_err());
        assert!(session.apply_context_edit(Actor::User, old).is_err());
        let draft = session.context_draft().unwrap().clone();
        let mut fields = draft.fields;
        fields.summary = "Valid".into();
        fields.body = "x".repeat(context::MAX_NOTE_BYTES);
        session.update_context_edit(draft.token, fields).unwrap();
        let before = session.document.vmf().clone();
        assert!(
            session
                .apply_context_edit(Actor::Agent, draft.token)
                .is_err()
        );
        assert_eq!(session.document.vmf(), &before);
        session
            .edit(Actor::Agent, "move", |tx| {
                edit::translate(tx, &[id], DVec3::X, true)
            })
            .unwrap();
        assert!(
            session
                .context_draft_feedback()
                .unwrap_err()
                .contains("Map changed")
        );
        session.cancel_context_edit(draft.token).unwrap();
        assert!(
            session
                .begin_context_edit(Some(Scope::Region(123)))
                .is_err()
        );
        assert!(
            session
                .begin_context_edit(Some(Scope::Object(ObjectId::Entity(999))))
                .is_err()
        );
    }

    #[test]
    fn context_responses_obey_encoded_byte_budgets_and_guard_cursors() {
        let (mut session, id) = fixture();
        save_note(
            &mut session,
            Actor::Agent,
            Some(Scope::Map),
            "Map brief",
            &"\"".repeat(3500),
        );
        save_note(
            &mut session,
            Actor::Agent,
            Some(Scope::Object(id)),
            "Object intent",
            &"é".repeat(1700),
        );
        let query = Query {
            object: Some(id),
            ..Default::default()
        };
        let first = session.retrieve_context(&query, 2048, None).unwrap();
        assert!(first.to_string().len() <= 2048);
        assert_eq!(first["notes"][0]["body_omitted"], true);
        let cursor = first["next_cursor"].as_str().unwrap();
        let second = session
            .retrieve_context(&query, 2048, Some(cursor))
            .unwrap();
        assert!(second.to_string().len() <= 2048);
        assert_ne!(first["notes"][0]["scope"], second["notes"][0]["scope"]);
        assert!(second["next_cursor"].is_null());
        assert!(
            session
                .retrieve_context(&Query::default(), 2048, Some(cursor))
                .is_err()
        );
        assert!(session.retrieve_context(&query, 1, None).is_err());
        session
            .edit(Actor::Agent, "move", |tx| {
                edit::translate(tx, &[id], DVec3::X, true)
            })
            .unwrap();
        assert!(
            session
                .retrieve_context(&query, 2048, Some(cursor))
                .is_err()
        );
        assert_eq!(
            session.get_context_note(Scope::Object(id)).unwrap()["note"]["stale"],
            true
        );
        session.undo(Actor::Agent).unwrap();
        assert_eq!(
            session.get_context_note(Scope::Object(id)).unwrap()["note"]["stale"],
            false
        );
    }

    #[test]
    fn context_regions_are_only_guides_and_retrieve_by_overlap() {
        let (mut session, id) = fixture();
        let before = session.document.vmf().clone();
        let base = session.build_scene();
        session.begin_context_edit(None).unwrap();
        let draft = session.context_draft().unwrap().clone();
        let mut fields = draft.fields;
        fields.summary = "Courtyard".into();
        fields.body = "Keep the entry open.".into();
        fields.mins = ["-32".into(), "-32".into(), "-32".into()];
        fields.maxs = ["96".into(), "96".into(), "96".into()];
        session.update_context_edit(draft.token, fields).unwrap();
        let preview = session.build_scene();
        assert_eq!(preview.vertices, base.vertices);
        assert_eq!(preview.indices, base.indices);
        assert_eq!(preview.lines.len(), base.lines.len() + 24);
        assert_eq!(session.document.vmf(), &before);
        let scope = session
            .apply_context_edit(Actor::Agent, draft.token)
            .unwrap();
        assert_eq!(session.document.objects().count(), 1);
        let relevant = session
            .retrieve_context(
                &Query {
                    object: Some(id),
                    ..Default::default()
                },
                8192,
                None,
            )
            .unwrap();
        assert_eq!(relevant["notes"].as_array().unwrap().len(), 1);
        assert_eq!(relevant["notes"][0]["scope"], scope.to_string());
        assert_eq!(
            session
                .retrieve_context(
                    &Query {
                        point: Some(DVec3::splat(1000.0)),
                        ..Default::default()
                    },
                    8192,
                    None
                )
                .unwrap()["total"],
            0
        );
        session.set_context_view(false);
        assert_eq!(session.build_scene().lines.len(), base.lines.len());
        let selected = session.selected_ids();
        session.focus_context(Actor::Agent, scope, 1.6).unwrap();
        assert_eq!(session.selected_ids(), selected);
        assert!(session.context.show_regions);
        let before_delete = session.document.vmf().clone();
        assert!(
            session
                .delete_context_note(
                    Actor::Agent,
                    scope,
                    session.document_generation,
                    session.document.revision() + 1
                )
                .is_err()
        );
        session
            .delete_context_note(
                Actor::Agent,
                scope,
                session.document_generation,
                session.document.revision(),
            )
            .unwrap();
        assert!(session.get_context_note(scope).is_err());
        session.undo(Actor::Agent).unwrap();
        assert_eq!(session.document.vmf(), &before_delete);
    }

    #[test]
    fn region_bounds_reject_nonfinite_reversed_and_unrenderable_coordinates() {
        let (mut session, _) = fixture();
        session.begin_context_edit(None).unwrap();
        let draft = session.context_draft().unwrap().clone();
        for invalid in ["NaN", "inf", "1000001", "unfinished"] {
            let mut fields = draft.fields.clone();
            fields.summary = "Area".into();
            fields.maxs[0] = invalid.into();
            session.update_context_edit(draft.token, fields).unwrap();
            assert!(session.context_draft_feedback().is_err());
        }
        session.begin_context_edit(Some(Scope::Map)).unwrap();
        let draft = session.context_draft().unwrap().clone();
        let mut fields = draft.fields;
        fields.mins[0] = "12".into();
        assert!(session.update_context_edit(draft.token, fields).is_err());
    }

    #[test]
    fn import_rejects_over_limit_metadata_without_replacing_the_current_map() {
        let (mut session, id) = fixture();
        save_note(
            &mut session,
            Actor::Agent,
            Some(Scope::Object(id)),
            "Current",
            "Preserve this map.",
        );
        let before = session.document.vmf().clone();
        let mut malformed = before.clone();
        let node = malformed
            .extra
            .iter_mut()
            .find(|node| node.key == context::METADATA_KEY)
            .unwrap();
        let source_formats::keyvalues::Value::Block(children) = &mut node.value else {
            panic!()
        };
        let source_formats::keyvalues::Value::Block(fields) = &mut children[1].value else {
            panic!()
        };
        fields
            .iter_mut()
            .find(|field| field.key == "body_base64")
            .unwrap()
            .value = source_formats::keyvalues::Value::String("YQ==".repeat(5000));
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("oversized.vmf");
        std::fs::write(
            &path,
            malformed.write(&source_formats::keyvalues::WriteOptions::default()),
        )
        .unwrap();
        assert!(
            session
                .open(Actor::Agent, &path, true)
                .unwrap_err()
                .contains("per-note limit")
        );
        assert_eq!(session.document.vmf(), &before);
    }
    #[test]
    fn orphan_notes_remain_readable_and_deletable_but_cannot_be_applied() {
        let mut session = Session::new(Err("no assets needed".into()));
        let scope = Scope::Object(ObjectId::Entity(999));
        session
            .document
            .edit("import context", |tx| {
                let mut metadata = Metadata::default();
                metadata.notes.insert(
                    scope,
                    Note {
                        summary: "Old light".into(),
                        body: "Keep a warm light here.".into(),
                        author: "agent".into(),
                        observed: String::new(),
                        region: None,
                        extra: vec![],
                    },
                );
                tx.set_context(metadata)
            })
            .unwrap();
        assert_eq!(
            session.get_context_note(scope).unwrap()["note"]["orphaned"],
            true
        );
        session.begin_context_edit(Some(scope)).unwrap();
        let draft = session.context_draft().unwrap().clone();
        assert_eq!(draft.fields.body, "Keep a warm light here.");
        assert!(
            session
                .apply_context_edit(Actor::User, draft.token)
                .is_err()
        );
        assert!(session.focus_context(Actor::Agent, scope, 1.6).is_err());
        session
            .delete_context_note(Actor::User, scope, draft.generation, draft.revision)
            .unwrap();
        assert_eq!(session.context_state()["notes"], 0);
        session.undo(Actor::Agent).unwrap();
        assert_eq!(session.context_state()["notes"], 1);
    }
}
