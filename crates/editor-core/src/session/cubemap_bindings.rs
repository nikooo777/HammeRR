use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use compiler::cubemap_assets::{
    CubemapBindingInspection, CubemapBindingQuery, CubemapBindingReport, CubemapBindingSources,
    CubemapHelperSource,
};
use map_core::{ObjectId, surface::FaceId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use source_formats::vmf::{Entity, Vmf};

use super::{Actor, Session};

pub const MAX_FACES: usize = 1024;
pub const MAX_PAGE: usize = 256;
const MAX_HELPERS: usize = 4096;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Bind,
    Remove,
}

impl Operation {
    fn label(self) -> &'static str {
        match self {
            Self::Bind => "bind",
            Self::Remove => "remove",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub helper: u32,
    pub faces: Vec<FaceId>,
    pub operation: Operation,
}

#[derive(Default)]
pub struct Query {
    pub helper: Option<u32>,
    pub offset: usize,
    pub limit: usize,
    pub query: String,
}

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    context_serial: u64,
    draft: Option<Draft>,
    inspection_sources: RefCell<Option<Arc<CubemapBindingSources>>>,
    inspection_serial: Cell<u64>,
}

impl State {
    pub(super) fn context_changed(&mut self) {
        self.context_serial += 1;
    }

    pub(super) fn texture_shadows_changed(&mut self) {
        self.context_changed();
        self.inspection_serial.set(self.inspection_serial.get() + 1);
    }
}

struct Draft {
    token: u64,
    owner: Actor,
    source: String,
    request: Request,
    sides: Option<String>,
    report: Value,
    sources: Arc<CubemapBindingSources>,
}

fn is_helper(entity: &Entity) -> bool {
    entity
        .classname()
        .is_some_and(|name| name.eq_ignore_ascii_case("env_cubemap"))
}

fn helper(vmf: &Vmf, id: u32) -> Result<&Entity, String> {
    let mut entities = vmf.entities.iter().filter(|entity| entity.id == id);
    let entity = entities
        .next()
        .filter(|entity| is_helper(entity))
        .ok_or_else(|| format!("entity:{id} is not an existing env_cubemap helper"))?;
    if entities.next().is_some() {
        return Err(format!("entity:{id} has an ambiguous duplicate entity ID"));
    }
    Ok(entity)
}

fn manual_sides(entity: &Entity) -> Result<BTreeSet<u32>, String> {
    if entity.properties.get_all("sides").take(2).count() > 1 {
        return Err(format!(
            "entity:{} has repeated sides properties",
            entity.id
        ));
    }
    let mut sides = BTreeSet::new();
    for word in entity
        .properties
        .get("sides")
        .unwrap_or("")
        .split_whitespace()
    {
        let id = word
            .parse::<u32>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(|| format!("entity:{}: sides must contain positive face IDs", entity.id))?;
        if !sides.insert(id) {
            return Err(format!("entity:{}: side:{id} is listed twice", entity.id));
        }
        if sides.len() > MAX_FACES {
            return Err(format!(
                "entity:{} exceeds the {MAX_FACES}-face manual binding limit",
                entity.id
            ));
        }
    }
    Ok(sides)
}

fn source_faces(vmf: &Vmf) -> BTreeMap<u32, Vec<(FaceId, bool)>> {
    let mut faces: BTreeMap<u32, Vec<(FaceId, bool)>> = BTreeMap::new();
    for (entity_hidden, solid) in
        vmf.world
            .solids
            .iter()
            .map(|solid| (false, solid))
            .chain(vmf.entities.iter().flat_map(|entity| {
                entity
                    .solids
                    .iter()
                    .map(move |solid| (entity.hidden, solid))
            }))
    {
        for side in &solid.sides {
            faces.entry(side.id).or_default().push((
                FaceId {
                    solid: solid.id,
                    side: side.id,
                },
                entity_hidden || solid.hidden,
            ));
        }
    }
    faces
}

fn face_json(face: FaceId) -> Value {
    json!({"solid":face.solid,"side":face.side})
}

fn helpers_json(vmf: &Vmf, report: Option<&CubemapBindingReport>) -> Vec<Value> {
    let faces = source_faces(vmf);
    vmf.entities
        .iter()
        .filter(|entity| is_helper(entity))
        .take(MAX_HELPERS)
        .map(|entity| {
            let status = report.and_then(|report| {
                report
                    .helpers
                    .iter()
                    .find(|item| item.helper_id == entity.id)
            });
            let (manual, error) = match manual_sides(entity) {
                Ok(sides) => (
                    sides
                        .iter()
                        .filter_map(|id| {
                            let [(face, _)] = faces.get(id)?.as_slice() else {
                                return None;
                            };
                            Some(face_json(*face))
                        })
                        .collect::<Vec<_>>(),
                    None,
                ),
                Err(error) => (Vec::new(), Some(error)),
            };
            json!({
                "helper":entity.id,"entity":format!("entity:{}",entity.id),
                "position":entity.origin().map(|position|position.to_array()),
                "hidden":entity.hidden,"excluded":status.map(|status|status.excluded),
                "sides":entity.properties.get("sides").unwrap_or(""),
                "manual_faces":manual,"manual_error":error,
            })
        })
        .collect()
}

fn report_json(vmf: &Vmf, inspection: &CubemapBindingInspection) -> Value {
    let report = &inspection.report;
    let faces = source_faces(vmf);
    let assignments = report
        .assignments
        .iter()
        .map(|assignment| {
            let face = faces.get(&assignment.face_id).and_then(|values| {
                let [(face, _)] = values.as_slice() else {
                    return None;
                };
                Some(face_json(*face))
            });
            let sample = report
                .samples
                .iter()
                .find(|sample| sample.helper_id == assignment.helper_id);
            let source = inspection.helper_sources.get(&assignment.helper_id);
            let helper = match source {
                Some(CubemapHelperSource::Root { helper }) => Some(*helper),
                _ => None,
            };
            json!({
                "face":face,"side":assignment.face_id,"helper":helper,"helper_source":source,
                "position":sample.map(|sample|sample.origin),"manual":assignment.manual,
                "material":assignment.material,"alias":assignment.alias,
            })
        })
        .collect::<Vec<_>>();
    let helpers = helpers_json(vmf, Some(report));
    let diagnostics = helpers
        .iter()
        .filter_map(|helper| helper["manual_error"].as_str())
        .collect::<Vec<_>>();
    json!({
        "available":true,"namespace":report.namespace,"total":report.total,"assignments":assignments,
        "helpers":helpers,"diagnostics":diagnostics,
        "face_scope":"root_authored_faces","sample_scope":"assembled_map",
        "child_faces_editable":false,
    })
}

impl Session {
    fn cubemap_binding_source(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{}:{:?}:{:?}",
            self.document_generation,
            self.document.revision(),
            self.build_content_epoch(),
            self.cubemap_bindings.context_serial,
            self.compile_options().brush_texture_shadows,
            self.cubemap_binding_namespace()
                .unwrap_or_else(|error| error),
            self.face_mode,
            self.selection,
            self.face_selection,
        )
    }

    pub fn cubemap_bindings_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.cubemap_binding_source(),
            self.cubemap_bindings.serial,
            self.cubemap_binding_context().is_ok(),
            self.cubemap_binding_report_stamp(),
            self.cubemap_bindings
                .draft
                .as_ref()
                .is_none_or(|draft| draft.sources.verify_metadata().is_ok()),
        )
    }

    pub fn cubemap_binding_report_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.build_content_epoch(),
            self.compile_options().brush_texture_shadows,
            self.cubemap_binding_namespace()
                .unwrap_or_else(|error| error),
            self.cubemap_bindings
                .inspection_sources
                .borrow()
                .as_ref()
                .is_none_or(|sources| sources.verify_metadata().is_ok()),
            self.cubemap_bindings.inspection_serial.get(),
        )
    }

    fn cubemap_binding_namespace(&self) -> Result<String, String> {
        self.binding_output_namespace()
            .map(|namespace| namespace.unwrap_or_else(|| "untitled".into()))
    }

    fn cubemap_binding_context(&self) -> Result<(), String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        if compiler::instances::requires_source(self.document.vmf()) && self.path.is_none() {
            return Err("Save the parent VMF before inspecting or editing cubemap bindings with visible instances".into());
        }
        if !std::ptr::eq(self.render_document(), &self.document) {
            return Err(
                "Apply or cancel the geometry preview before editing cubemap bindings".into(),
            );
        }
        Ok(())
    }

    pub fn selected_cubemap_helper(&self) -> Option<u32> {
        let ids = self.selected_ids();
        let [ObjectId::Entity(id)] = ids.as_slice() else {
            return None;
        };
        helper(self.document.vmf(), *id)
            .ok()
            .filter(|entity| !entity.hidden)
            .map(|entity| entity.id)
    }

    fn checked_cubemap_binding(&self, token: u64) -> Result<&Draft, String> {
        self.cubemap_binding_context()?;
        let draft = self
            .cubemap_bindings
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Cubemap binding draft was closed or replaced")?;
        if draft.source != self.cubemap_binding_source() {
            return Err(
                "Map, selection, output name or content changed; preview bindings again".into(),
            );
        }
        draft.sources.verify_metadata()?;
        Ok(draft)
    }

    pub fn cubemap_binding_state(&self) -> Value {
        json!({
            "snapshot":self.cubemap_bindings_stamp(),
            "generation":self.document_generation,"revision":self.document.revision(),
            "content_epoch":self.build_content_epoch(),
            "brush_texture_shadows":self.compile_options().brush_texture_shadows,
            "selected_helper":self.selected_cubemap_helper(),
            "selected_faces":self.selected_faces().into_iter().map(face_json).collect::<Vec<_>>(),
            "context_error":self.cubemap_binding_context().err(),
            "draft":self.cubemap_bindings.draft.as_ref().map(|draft|json!({
                "token":draft.token,"owner":draft.owner.label(),
                "valid":self.checked_cubemap_binding(draft.token).is_ok(),
                "helper":draft.request.helper,"operation":draft.request.operation,
                "faces":draft.request.faces.iter().copied().map(face_json).collect::<Vec<_>>(),
                "sides":draft.sides,"report":draft.report,
            })),
        })
    }

    fn inspect_cubemap_binding_map(
        &self,
        vmf: &Vmf,
        query: &CubemapBindingQuery,
    ) -> Result<CubemapBindingInspection, String> {
        if let Some(error) = &self.game_error {
            return Err(format!(
                "Cubemap binding inspection needs mounted game content: {error}"
            ));
        }
        let control = compiler::Control::default();
        let source = if compiler::instances::requires_source(vmf) {
            if let Some(claim) = &self.file_claims.source {
                claim.destination().verify().map_err(|error| {
                    format!(
                        "Map source binding changed; reopen or Save As before inspecting: {error}"
                    )
                })?;
            }
            Some(
                compiler::instances::SourceContext::capture(
                    self.path
                        .as_deref()
                        .ok_or("Save the parent VMF before inspecting instances")?,
                    &control,
                )
                .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        compiler::cubemap_assets::inspect_root_cubemap_bindings(
            vmf,
            &self.game,
            source.as_ref(),
            query,
            &control,
        )
    }

    pub fn get_cubemap_bindings(&self, query: Query) -> Result<Value, String> {
        let limit = if query.limit == 0 { 50 } else { query.limit };
        if limit > MAX_PAGE || query.query.len() > 256 {
            return Err(format!(
                "Use a cubemap search up to 256 bytes and a page of 1 to {MAX_PAGE} rows"
            ));
        }
        if let Some(id) = query.helper {
            helper(self.document.vmf(), id)?;
        }
        self.cubemap_bindings
            .inspection_serial
            .set(self.cubemap_bindings.inspection_serial.get() + 1);
        let namespace = self
            .cubemap_binding_context()
            .and_then(|()| self.cubemap_binding_namespace());
        let mut value = match namespace.and_then(|namespace| {
            self.inspect_cubemap_binding_map(
                self.document.vmf(),
                &CubemapBindingQuery {
                    brush_texture_shadows: self.compile_options().brush_texture_shadows,
                    helper_id: query.helper,
                    namespace: Some(namespace),
                    offset: query.offset,
                    limit,
                    query: Some(query.query),
                    ..Default::default()
                },
            )
        }) {
            Ok(inspection) => {
                *self.cubemap_bindings.inspection_sources.borrow_mut() =
                    Some(inspection.sources.clone());
                report_json(self.document.vmf(), &inspection)
            }
            Err(error) => json!({
                "available":false,"namespace":self.cubemap_binding_namespace().ok(),"total":0,"assignments":[],
                "helpers":helpers_json(self.document.vmf(),None),"diagnostics":[error],
            }),
        };
        value["state"] = self.cubemap_binding_state();
        value["report_snapshot"] = json!(self.cubemap_binding_report_stamp());
        value["offset"] = json!(query.offset);
        value["limit"] = json!(limit);
        value["provisional_namespace"] = json!(self.path.is_none());
        value["policy"] = json!(
            "Assignments list root-authored faces only; sample choices use the complete assembled map. Child samples carry source file and occurrence; child faces and helpers cannot be edited here. Manual sides override automatic assignment. Remove bindings to use the compiler's nearest active sample, which may belong to a child. Aliases are provisional for an unnamed map. Inspection does not capture reflections or change source properties."
        );
        Ok(value)
    }

    fn prepare_cubemap_binding(
        &self,
        request: &Request,
    ) -> Result<(Option<String>, Value, Arc<CubemapBindingSources>), String> {
        self.cubemap_binding_context()?;
        let vmf = self.document.vmf();
        let entity = helper(vmf, request.helper)?;
        if entity.hidden {
            return Err(format!("entity:{} is hidden", request.helper));
        }
        if request.faces.is_empty() || request.faces.len() > MAX_FACES {
            return Err(format!("Pass 1 to {MAX_FACES} explicit cubemap faces"));
        }
        let faces = source_faces(vmf);
        let mut requested = BTreeSet::new();
        for face in &request.faces {
            if !requested.insert(face.side) {
                return Err(format!("side:{} is repeated in the request", face.side));
            }
            let Some(values) = faces.get(&face.side) else {
                return Err(format!(
                    "solid:{} side:{} is missing",
                    face.solid, face.side
                ));
            };
            let [(actual, hidden)] = values.as_slice() else {
                return Err(format!(
                    "side:{} has an ambiguous duplicate side ID",
                    face.side
                ));
            };
            if actual != face || *hidden {
                return Err(format!(
                    "solid:{} side:{} is missing or hidden",
                    face.solid, face.side
                ));
            }
        }
        let mut owners = BTreeMap::new();
        for (index, owner) in vmf
            .entities
            .iter()
            .filter(|entity| is_helper(entity))
            .take(MAX_HELPERS + 1)
            .enumerate()
        {
            if index == MAX_HELPERS {
                return Err("Cubemap helper inspection limit exceeded".into());
            }
            for side in manual_sides(owner)? {
                if !requested.contains(&side) {
                    continue;
                }
                if owners.insert(side, owner.id).is_some() {
                    return Err(format!(
                        "side:{side} has duplicate manual cubemap ownership"
                    ));
                }
                if owner.id != request.helper && request.operation == Operation::Bind {
                    return Err(format!(
                        "side:{side} has manual cubemap ownership by entity:{}; remove its binding first (unhide the owner or restore its compile region if needed)",
                        owner.id
                    ));
                }
            }
        }
        let mut sides = manual_sides(entity)?;
        for side in &requested {
            match request.operation {
                Operation::Bind => {
                    sides.insert(*side);
                }
                Operation::Remove => {
                    if !sides.remove(side) {
                        return Err(format!(
                            "entity:{} does not manually own side:{side}",
                            request.helper
                        ));
                    }
                }
            }
        }
        if sides.len() > MAX_FACES {
            return Err(format!(
                "Manual bindings are limited to {MAX_FACES} faces per helper"
            ));
        }
        let sides = (!sides.is_empty()).then(|| {
            sides
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        });
        if sides.as_deref().unwrap_or("") == entity.properties.get("sides").unwrap_or("") {
            return Err("The requested cubemap bindings are already current".into());
        }
        let mut candidate = vmf.clone();
        let entity = candidate
            .entities
            .iter_mut()
            .find(|entity| entity.id == request.helper)
            .unwrap();
        if let Some(sides) = &sides {
            entity.properties.set("sides", sides);
        } else {
            entity.properties.remove("sides");
        }
        let report = self.inspect_cubemap_binding_map(
            &candidate,
            &CubemapBindingQuery {
                brush_texture_shadows: self.compile_options().brush_texture_shadows,
                namespace: Some(self.cubemap_binding_namespace()?),
                face_ids: requested.into_iter().collect(),
                limit: MAX_FACES,
                ..Default::default()
            },
        )?;
        let status = report
            .report
            .helpers
            .iter()
            .find(|helper| helper.helper_id == request.helper)
            .ok_or("Cubemap helper is missing from the authoritative plan")?;
        if status.hidden || status.excluded {
            return Err(format!(
                "entity:{} is hidden or outside the committed compile region",
                request.helper
            ));
        }
        Ok((sides, report_json(&candidate, &report), report.sources))
    }

    pub fn preview_cubemap_bindings(
        &mut self,
        actor: Actor,
        request: Request,
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.cubemap_bindings_stamp() {
            return Err(
                "Map, selection or binding draft changed; inspect cubemap bindings again".into(),
            );
        }
        let (sides, report, sources) = self.prepare_cubemap_binding(&request)?;
        self.cubemap_bindings.serial += 1;
        self.cubemap_bindings.draft = Some(Draft {
            token: self.cubemap_bindings.serial,
            owner: actor,
            source: self.cubemap_binding_source(),
            request,
            sides,
            report,
            sources,
        });
        self.view_serial += 1;
        self.log(actor, "Preview cubemap face bindings");
        Ok(self.cubemap_binding_state())
    }

    pub fn review_cubemap_bindings(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.checked_cubemap_binding(token)?;
        self.cubemap_bindings.serial += 1;
        let draft = self.cubemap_bindings.draft.as_mut().unwrap();
        draft.token = self.cubemap_bindings.serial;
        draft.owner = actor;
        self.view_serial += 1;
        self.log(actor, "Review cubemap face bindings");
        Ok(self.cubemap_binding_state())
    }

    pub fn apply_cubemap_bindings(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self.checked_cubemap_binding(token)?;
        if draft.owner != actor {
            return Err("Review the current cubemap binding draft before applying it".into());
        }
        let control = compiler::Control::default();
        draft.sources.verify(&self.game, &control)?;
        let (sides, report, _) = self.prepare_cubemap_binding(&draft.request)?;
        if sides != draft.sides || report != draft.report {
            return Err(
                "Cubemap assignments or material assets changed; preview bindings again".into(),
            );
        }
        draft.sources.verify(&self.game, &control)?;
        let helper = draft.request.helper;
        let operation = draft.request.operation;
        self.edit(actor, "Edit cubemap face bindings", |tx| {
            let properties = &mut tx.entity_mut(helper)?.properties;
            if let Some(sides) = &sides {
                properties.set("sides", sides);
            } else {
                properties.remove("sides");
            }
            Ok(())
        })?;
        self.cubemap_bindings.draft = None;
        self.cubemap_bindings.serial += 1;
        self.view_serial += 1;
        Ok(
            json!({"helper":helper,"operation":operation.label(),"report":report,"state":self.cubemap_binding_state()}),
        )
    }

    pub fn cancel_cubemap_bindings(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self
            .cubemap_bindings
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Cubemap binding draft was closed or replaced")?;
        if draft.owner != actor {
            return Err("Review the current cubemap binding draft before cancelling it".into());
        }
        self.cubemap_bindings.draft = None;
        self.cubemap_bindings.serial += 1;
        self.view_serial += 1;
        self.log(actor, "Cancel cubemap face bindings");
        Ok(self.cubemap_binding_state())
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.draft
            .as_ref()
            .map(|draft| serde_json::json!([self.serial, draft.token, draft.owner.label()]))
    }
}

#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
