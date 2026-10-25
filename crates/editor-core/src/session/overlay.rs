use std::collections::BTreeMap;

use glam::DVec3;
use map_core::overlay::{self, Definition, Options};
use map_core::surface::FaceId;
use serde_json::{Value, json};
use source_formats::vmt::{CompileSemantics, material_path};

use super::{Actor, SelectMode, Session};

mod handles;

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub source: FaceId,
    pub options: Options,
}

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    context_serial: u64,
    draft: Option<Draft>,
}

impl State {
    pub(super) fn context_changed(&mut self) {
        self.context_serial += 1;
    }
}

struct Draft {
    token: u64,
    generation: u64,
    revision: u64,
    selection: String,
    owner: Actor,
    source: FaceId,
    additional_sources: Vec<FaceId>,
    previous_sources: Vec<FaceId>,
    visibility: Option<(u64, u64, bool)>,
    definition: Definition,
    operation: Operation,
    handles: Option<handles::Handles>,
}

#[derive(Clone, Copy)]
enum Operation {
    Create,
    Corners(u32),
    Refit(u32),
    Rebind { id: u32, previous: FaceId },
}

impl Operation {
    fn label(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Corners(_) => "corners",
            Self::Refit(_) => "refit",
            Self::Rebind { .. } => "rebind",
        }
    }

    fn entity(self) -> Option<u32> {
        match self {
            Self::Create => None,
            Self::Refit(id) | Self::Corners(id) | Self::Rebind { id, .. } => Some(id),
        }
    }
}

fn options_json(options: &Options) -> Value {
    json!({
        "material":options.material,"targetname":options.targetname,
        "render_order":options.render_order,"u":options.u,"v":options.v,
        "fade_min":options.fade_min,"fade_max":options.fade_max,
    })
}

fn sources_json(sources: &[FaceId]) -> Value {
    json!(
        sources
            .iter()
            .map(|source| json!({"solid":source.solid,"side":source.side}))
            .collect::<Vec<_>>()
    )
}

fn source_budget(count: usize) -> Result<(), String> {
    if count > source_formats::bsp::MAX_OVERLAY_FACES {
        return Err(format!(
            "Overlay sources exceed their {}-side allocation budget",
            source_formats::bsp::MAX_OVERLAY_FACES
        ));
    }
    Ok(())
}

fn bounds_json(definition: &Definition) -> Value {
    let (mins, maxs) = definition
        .corners()
        .into_iter()
        .map(DVec3::from_array)
        .fold((DVec3::MAX, DVec3::MIN), |(mins, maxs), point| {
            (mins.min(point), maxs.max(point))
        });
    json!({"mins":mins.to_array(),"maxs":maxs.to_array()})
}

impl Session {
    fn overlay_selection_stamp(&self) -> String {
        format!(
            "{}:{}:{:?}:{:?}",
            self.overlay.context_serial, self.face_mode, self.selection, self.face_selection
        )
    }

    pub fn overlay_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.overlay.serial,
            self.overlay_selection_stamp(),
            self.overlay_context().is_ok()
        )
    }

    fn overlay_context(&self) -> Result<(), String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        if !std::ptr::eq(self.render_document(), &self.document) {
            return Err(
                "Apply or cancel the geometry preview before creating or editing an overlay".into(),
            );
        }
        Ok(())
    }

    fn overlay_visibility_stamp(&self) -> (u64, u64, bool) {
        (
            self.working_region.serial,
            self.materials.generation(),
            self.view.tool_textures,
        )
    }

    fn validate_overlay_sources(&self, sources: &[FaceId]) -> Result<(), String> {
        if !self.overlay_sources_need_validation(sources) {
            return Ok(());
        }
        self.validate_overlay_source_set(sources)
    }

    fn overlay_sources_need_validation(&self, sources: &[FaceId]) -> bool {
        sources.len() > 1
            || sources
                .iter()
                .any(|source| overlay::is_terrain(self.document.vmf(), *source))
    }

    fn validate_overlay_source_set(&self, sources: &[FaceId]) -> Result<(), String> {
        let mut materials = BTreeMap::new();
        for source in sources {
            if !self.working_solid_visible(&self.document, source.solid) {
                return Err(format!(
                    "Overlay brush {} face {} is hidden or outside the working region",
                    source.solid, source.side
                ));
            }
            let side = map_core::surface::side(&self.document, *source)
                .ok_or("Overlay face does not belong to the target brush")?;
            let path = material_path(&side.material);
            if let std::collections::btree_map::Entry::Vacant(entry) = materials.entry(path.clone())
            {
                let material = render::materials::resolved_material_limited(
                    self.game(),
                    &path,
                    compiler::materials::MAX_GEOMETRY_MATERIAL_BYTES,
                )?
                .ok_or_else(|| format!("Overlay source material {path} is unavailable"))?;
                let semantics = CompileSemantics::checked_from_material(&material)?;
                let tool = material.is_tool()
                    || matches!(
                        path.as_str(),
                        "materials/tools/toolsskybox.vmt" | "materials/tools/toolsskybox2d.vmt"
                    );
                entry.insert((tool, semantics));
            }
            let (tool, semantics) = materials.get(&path).unwrap();
            if side.displacement.is_some() {
                map_core::displacement::native::validate_material(
                    semantics.draws,
                    semantics.contents,
                    semantics.surface,
                )?;
            }
            if !semantics.draws || !semantics.unsupported_flags.is_empty() {
                return Err(format!(
                    "Overlay brush {} face {} requires a known rendering material with supported compile flags",
                    source.solid, source.side
                ));
            }
            if *tool && !self.view.tool_textures {
                return Err(format!(
                    "Overlay brush {} face {} is hidden by the tool-texture display setting",
                    source.solid, source.side
                ));
            }
        }
        Ok(())
    }

    fn selected_overlay_source(&self) -> Result<(Vec<FaceId>, Definition), String> {
        if !self.face_mode || self.face_selection.is_empty() {
            return Err("Select coplanar world/detail or isolated planar terrain faces".into());
        }
        self.overlay_context()?;
        source_budget(self.face_selection.len())?;
        let mut sources = self.face_selection.iter().copied().collect::<Vec<_>>();
        if sources.len() > 1
            && let Some(index) = sources.iter().position(|source| {
                overlay::fit(
                    self.document.vmf(),
                    *source,
                    Options {
                        material: "dev/dev_measuregeneric01".into(),
                        ..Options::default()
                    },
                )
                .is_ok()
            })
        {
            let first = sources.remove(index);
            sources.insert(0, first);
        }
        let definition = overlay::fit_faces(
            self.document.vmf(),
            &sources,
            Options {
                material: "dev/dev_measuregeneric01".into(),
                ..Options::default()
            },
        )?;
        self.validate_overlay_sources(&sources)?;
        Ok((sources, definition))
    }

    fn selected_overlay_refit(&self) -> Result<(u32, FaceId, Definition), String> {
        self.overlay_context()?;
        let ids = self.selected_ids();
        let [map_core::ObjectId::Entity(id)] = ids.as_slice() else {
            return Err("Select exactly one visible existing overlay to refit".into());
        };
        let (face, definition) = overlay::fit_existing(self.document.vmf(), *id)?;
        self.validate_overlay_sources(&[face])?;
        Ok((*id, face, definition))
    }

    fn selected_overlay_refit_report(&self) -> Result<Value, String> {
        self.overlay_context()?;
        let ids = self.selected_ids();
        let [map_core::ObjectId::Entity(id)] = ids.as_slice() else {
            return Err("Select exactly one visible existing overlay to refit".into());
        };
        let multiple = self
            .document
            .entity(*id)
            .and_then(|entity| overlay::parse(entity).ok())
            .is_some_and(|definition| definition.sides.len() > 1);
        if !multiple {
            let (id, source, definition) = self.selected_overlay_refit()?;
            return Ok(
                json!({"id":id,"entity":format!("entity:{id}"),"source":{"solid":source.solid,"side":source.side},"corners":definition.corners()}),
            );
        }
        let bound = overlay::checked_bound_sources(self.document.vmf(), *id)?;
        self.validate_overlay_sources(&bound.faces)?;
        Ok(
            json!({"id":id,"entity":format!("entity:{id}"),"sources":sources_json(&bound.faces),
            "source_count":bound.faces.len(),"reference_required":true,"references":sources_json(&bound.references),"corners":null}),
        )
    }

    fn selected_overlay_corners(&self) -> Result<(u32, Vec<FaceId>, Definition), String> {
        self.overlay_context()?;
        let ids = self.selected_ids();
        let [map_core::ObjectId::Entity(id)] = ids.as_slice() else {
            return Err("Select exactly one visible existing overlay to refit".into());
        };
        let (sources, definition) = overlay::editable_corner_faces(self.document.vmf(), *id)?;
        self.validate_overlay_sources(&sources)?;
        Ok((*id, sources, definition))
    }

    fn checked_overlay(&self, token: u64) -> Result<&Draft, String> {
        let draft = self
            .overlay
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Overlay draft was closed or replaced")?;
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
            || draft.selection != self.overlay_selection_stamp()
            || draft
                .visibility
                .is_some_and(|stamp| stamp != self.overlay_visibility_stamp())
        {
            return Err("Map or selection changed; preview the overlay again".into());
        }
        self.overlay_context()?;
        Ok(draft)
    }

    pub fn get_overlay_state(&self) -> Value {
        let (selected_overlay, refit_error) = match self.selected_overlay_refit_report() {
            Ok(report) => (report, Value::Null),
            Err(error) => (Value::Null, json!(error)),
        };
        let (selected_corners, corners_error) = match self.selected_overlay_corners() {
            Ok((id, sources, definition)) => {
                let source = sources[0];
                let mut report = json!({
                    "id":id,"entity":format!("entity:{id}"),"source":{"solid":source.solid,"side":source.side},
                    "options":options_json(&definition.options),"corners":definition.corners(),
                    "local_corners":definition.uv_points.map(|point|[point[0],point[1]]),
                    "basis_origin":definition.basis_origin,"basis_u":definition.basis_u,"basis_v":definition.basis_v,
                });
                if sources.len() > 1 {
                    report["sources"] = sources_json(&sources);
                    report["source_count"] = json!(sources.len());
                }
                (report, Value::Null)
            }
            Err(error) => (Value::Null, json!(error)),
        };
        let (selected_source, selection_error) = match self.selected_overlay_source() {
            Ok((sources, definition)) => {
                let source = sources[0];
                let mut report =
                    json!({"solid":source.solid,"side":source.side,"corners":definition.corners()});
                if sources.len() > 1 {
                    report["sources"] = sources_json(&sources);
                    report["source_count"] = json!(sources.len());
                }
                (report, Value::Null)
            }
            Err(error) => (Value::Null, json!(error)),
        };
        let draft = self.overlay.draft.as_ref().map(|draft| {
            let mut report = json!({
                "token":draft.token,"owner":draft.owner.label(),
                "operation":draft.operation.label(),
                "entity":draft.operation.entity().map(|id|format!("entity:{id}")),
                "previous_source":match draft.operation { Operation::Rebind { previous, .. } => Some(json!({"solid":previous.solid,"side":previous.side})), _ => None },
                "valid":self.checked_overlay(draft.token).is_ok(),
                "source":{"solid":draft.source.solid,"side":draft.source.side},
                "options":options_json(&draft.definition.options),
                "local_corners":draft.definition.uv_points.map(|point|[point[0],point[1]]),
                "corners":draft.definition.corners(),"bounds":bounds_json(&draft.definition),
                "basis_origin":draft.definition.basis_origin,"basis_normal":draft.definition.basis_normal,
                "basis_u":draft.definition.basis_u,"basis_v":draft.definition.basis_v,
                "handles":self.overlay_corner_handles_report(),
            });
            if !draft.additional_sources.is_empty() {
                let sources = std::iter::once(draft.source).chain(draft.additional_sources.iter().copied()).collect::<Vec<_>>();
                report["sources"] = sources_json(&sources);
                report["source_count"] = json!(sources.len());
                if matches!(draft.operation, Operation::Refit(_)) {
                    report["reference"] = json!({"solid":draft.source.solid,"side":draft.source.side});
                }
            }
            if draft.previous_sources.len() > 1 || (!draft.previous_sources.is_empty() && !draft.additional_sources.is_empty()) {
                report["previous_sources"] = sources_json(&draft.previous_sources);
                report["previous_source_count"] = json!(draft.previous_sources.len());
                let sources = std::iter::once(draft.source).chain(draft.additional_sources.iter().copied()).collect::<Vec<_>>();
                report["sources"] = sources_json(&sources);
                report["source_count"] = json!(sources.len());
            }
            report
        });
        let multiple = selected_source["source_count"]
            .as_u64()
            .is_some_and(|count| count > 1)
            || selected_overlay["source_count"]
                .as_u64()
                .is_some_and(|count| count > 1)
            || draft.as_ref().is_some_and(|draft| {
                draft["source_count"]
                    .as_u64()
                    .is_some_and(|count| count > 1)
                    || draft["previous_source_count"]
                        .as_u64()
                        .is_some_and(|count| count > 1)
            });
        let mut state = json!({
            "snapshot":self.overlay_stamp(),"generation":self.document_generation,
            "revision":self.document.revision(),"selected_source":selected_source,
            "selection_error":selection_error,"draft":draft,
            "selected_overlay":selected_overlay,"refit_error":refit_error,
            "selected_corners":selected_corners,"corners_error":corners_error,
            "policy":"Create on visible world/detail quads or isolated planar terrain quads, refit an existing overlay, or explicitly rebind it to a different eligible source set. Up to 64 same-facing coplanar sources may combine flat faces and isolated planar terrain; an eligible quad supplies the frame. Every prepared and native terrain sample must lie on the common plane, with four straight convex boundary edges. Each terrain base must have disjoint stored bounds from every other terrain base, including other selected terrain. Touching/joined/overlapping terrain bases, visible instance references and sculpted/nonplanar terrain are unavailable. Refit and rebind preserve the entity ID, authored option strings, connections and metadata. Preview changes no geometry, selection or history. Review transfers ownership; Apply is one Undo step. Corner editing changes four local U/V positions in the existing projection frame while retaining all source bindings, basis and raw settings. Corners must remain convex, native-representable and on the common source plane. Automatic rebinding is unavailable.",
        });
        if multiple {
            state["policy"] = json!(
                "Create one overlay across up to 64 visible coplanar flat world/detail faces or isolated planar terrain quads facing the same direction. The first eligible quad, including terrain, supplies the projection axes; one U/V rectangle spans all actual source boundaries. Every prepared/native terrain sample must lie on the common plane with straight convex borders. Each terrain base must have disjoint stored bounds from every other terrain base, including other selected terrain; touching/joined/overlapping bases, visible instances and sculpted/nonplanar terrain reject. Refit an existing multi-face overlay only with an explicit eligible reference quad from its unchanged bindings; its current edges set the new projection axes. Numeric and viewport corner editing use the stored projection axes without choosing a new reference and retain every existing binding. Native rendering clips to every bound side, including gaps between faces. Preview changes no map, selection or history; Review transfers ownership and Apply is one Undo step. Explicit rebind replaces valid single or multiple bindings with a different bounded coplanar destination set and its explicit first-quad frame. Brush models and automatic rebinding are unavailable."
            );
        }
        state
    }

    pub fn preview_overlay(
        &mut self,
        actor: Actor,
        request: Request,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.preview_overlay_faces(actor, request, &[], snapshot)
    }

    pub fn preview_overlay_faces(
        &mut self,
        actor: Actor,
        request: Request,
        additional_sources: &[FaceId],
        snapshot: &str,
    ) -> Result<Value, String> {
        self.overlay_context()?;
        if snapshot != self.overlay_stamp() {
            return Err("Map, selection or overlay draft changed; inspect overlays again".into());
        }
        source_budget(additional_sources.len().saturating_add(1))?;
        let sources = std::iter::once(request.source)
            .chain(additional_sources.iter().copied())
            .collect::<Vec<_>>();
        let definition = overlay::fit_faces(self.document.vmf(), &sources, request.options)?;
        self.validate_overlay_sources(&sources)?;
        Ok(self.set_overlay_draft(
            actor,
            request.source,
            additional_sources,
            definition,
            Operation::Create,
            &[],
        ))
    }

    pub fn preview_overlay_refit(
        &mut self,
        actor: Actor,
        id: u32,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.preview_overlay_refit_reference(actor, id, None, snapshot)
    }

    pub fn preview_overlay_refit_reference(
        &mut self,
        actor: Actor,
        id: u32,
        reference: Option<FaceId>,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.overlay_context()?;
        if snapshot != self.overlay_stamp() {
            return Err("Map, selection or overlay draft changed; inspect overlays again".into());
        }
        let (sources, definition) =
            overlay::fit_existing_refit(self.document.vmf(), id, reference)?;
        self.validate_overlay_sources(&sources)?;
        Ok(self.set_overlay_draft(
            actor,
            sources[0],
            &sources[1..],
            definition,
            Operation::Refit(id),
            &[],
        ))
    }

    pub fn preview_overlay_corners(
        &mut self,
        actor: Actor,
        id: u32,
        corners: [[f64; 2]; 4],
        snapshot: &str,
    ) -> Result<Value, String> {
        self.overlay_context()?;
        if snapshot != self.overlay_stamp() {
            return Err("Map, selection or overlay draft changed; inspect overlays again".into());
        }
        let (sources, definition) = overlay::with_corner_faces(self.document.vmf(), id, corners)?;
        self.validate_overlay_sources(&sources)?;
        Ok(self.set_overlay_draft(
            actor,
            sources[0],
            &sources[1..],
            definition,
            Operation::Corners(id),
            &[],
        ))
    }

    pub fn browse_overlay_targets(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        if query.len() > 256 || !(1..=50).contains(&limit) {
            return Err("Use an overlay search up to 256 bytes and a page of 1 to 50 rows".into());
        }
        let query = query.to_ascii_lowercase();
        let words = query.split_whitespace().collect::<Vec<_>>();
        let matches = self
            .document
            .vmf()
            .entities
            .iter()
            .filter(|entity| {
                if !entity
                    .classname()
                    .is_some_and(|name| name.eq_ignore_ascii_case("info_overlay"))
                {
                    return false;
                }
                let label = format!(
                    "entity:{} {} {}",
                    entity.id,
                    entity.properties.get("targetname").unwrap_or(""),
                    entity.properties.get("material").unwrap_or("")
                )
                .to_ascii_lowercase();
                words.iter().all(|word| label.contains(word))
            })
            .collect::<Vec<_>>();
        let entries = matches.iter().skip(offset).take(limit).map(|entity| {
            let checked = overlay::rebind_sources(self.document.vmf(), entity.id).and_then(|(sources, definition)| {
                self.validate_overlay_sources(&sources)?;
                Ok((sources, definition))
            });
            let (sources, options, error) = match checked {
                Ok((sources, definition)) => (sources, Some(options_json(&definition.options)), None),
                Err(error) => (Vec::new(), None, Some(error)),
            };
            let mut report = json!({"id":entity.id,"entity":format!("entity:{}",entity.id),"name":entity.properties.get("targetname").unwrap_or(""),"material":entity.properties.get("material").unwrap_or(""),"source":sources.first().map(|source|json!({"solid":source.solid,"side":source.side})),"options":options,"eligible":error.is_none(),"error":error});
            if sources.len() > 1 { report["sources"] = sources_json(&sources); report["source_count"] = json!(sources.len()); }
            report
        }).collect::<Vec<_>>();
        Ok(
            json!({"snapshot":self.overlay_stamp(),"query":query,"offset":offset,"limit":limit,"total":matches.len(),"entries":entries}),
        )
    }

    pub fn preview_overlay_rebind(
        &mut self,
        actor: Actor,
        id: u32,
        source: FaceId,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.preview_overlay_rebind_faces(actor, id, source, &[], snapshot)
    }

    pub fn preview_overlay_rebind_faces(
        &mut self,
        actor: Actor,
        id: u32,
        source: FaceId,
        additional_sources: &[FaceId],
        snapshot: &str,
    ) -> Result<Value, String> {
        self.overlay_context()?;
        if snapshot != self.overlay_stamp() {
            return Err("Map, selection or overlay draft changed; inspect overlays again".into());
        }
        source_budget(additional_sources.len().saturating_add(1))?;
        let sources = std::iter::once(source)
            .chain(additional_sources.iter().copied())
            .collect::<Vec<_>>();
        let (previous, definition) = overlay::fit_rebind_faces(self.document.vmf(), id, &sources)?;
        if self.overlay_sources_need_validation(&previous)
            || self.overlay_sources_need_validation(&sources)
        {
            self.validate_overlay_source_set(&previous)?;
            self.validate_overlay_source_set(&sources)?;
        }
        Ok(self.set_overlay_draft(
            actor,
            source,
            additional_sources,
            definition,
            Operation::Rebind {
                id,
                previous: previous[0],
            },
            &previous,
        ))
    }

    fn set_overlay_draft(
        &mut self,
        actor: Actor,
        source: FaceId,
        additional_sources: &[FaceId],
        definition: Definition,
        operation: Operation,
        previous_sources: &[FaceId],
    ) -> Value {
        self.overlay.serial += 1;
        self.scene_serial += 1;
        self.overlay.draft = Some(Draft {
            token: self.overlay.serial,
            generation: self.document_generation,
            revision: self.document.revision(),
            selection: self.overlay_selection_stamp(),
            owner: actor,
            source,
            additional_sources: additional_sources.to_vec(),
            previous_sources: previous_sources.to_vec(),
            visibility: (!additional_sources.is_empty()
                || self.overlay_sources_need_validation(previous_sources)
                || overlay::is_terrain(self.document.vmf(), source))
            .then(|| self.overlay_visibility_stamp()),
            definition,
            operation,
            handles: None,
        });
        self.view_serial += 1;
        self.log(
            actor,
            match operation {
                Operation::Create => "Preview overlay creation",
                Operation::Corners(_) => "Preview overlay corners",
                Operation::Refit(_) => "Preview overlay refit",
                Operation::Rebind { .. } => "Preview overlay rebind",
            },
        );
        self.get_overlay_state()
    }

    pub fn review_overlay(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self.checked_overlay(token)?;
        let had_handles = draft.handles.is_some();
        self.overlay.serial += 1;
        let draft = self.overlay.draft.as_mut().unwrap();
        draft.token = self.overlay.serial;
        draft.owner = actor;
        draft.handles = None;
        if had_handles {
            self.scene_serial += 1;
        }
        self.view_serial += 1;
        self.log(actor, "Review overlay");
        Ok(self.get_overlay_state())
    }

    pub fn apply_overlay(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self.checked_overlay(token)?;
        if draft.owner != actor {
            return Err("Review the current overlay draft before applying it".into());
        }
        if draft.handles.is_some() {
            return Err("Finish or cancel viewport corner handles before Apply corners".into());
        }
        let source = draft.source;
        let sources = std::iter::once(source)
            .chain(draft.additional_sources.iter().copied())
            .collect::<Vec<_>>();
        let options = draft.definition.options.clone();
        let operation = draft.operation;
        let previous_sources = draft.previous_sources.clone();
        let corners = draft.definition.uv_points.map(|point| [point[0], point[1]]);
        if !previous_sources.is_empty()
            && (self.overlay_sources_need_validation(&previous_sources)
                || self.overlay_sources_need_validation(&sources))
        {
            self.validate_overlay_source_set(&previous_sources)?;
            self.validate_overlay_source_set(&sources)?;
        } else {
            self.validate_overlay_sources(&sources)?;
        }
        let id = match operation {
            Operation::Corners(id) => self.edit(actor, "Edit overlay corners", |tx| {
                overlay::edit_corners(tx, id, corners)
            })?,
            Operation::Refit(id) => self.edit(actor, "Refit overlay", |tx| {
                if sources.len() == 1 {
                    overlay::refit(tx, id)
                } else {
                    overlay::refit_with_reference(tx, id, Some(source))
                }
            })?,
            Operation::Rebind { id, .. } => self.edit(actor, "Rebind overlay", |tx| {
                if previous_sources.len() == 1 && sources.len() == 1 {
                    overlay::rebind(tx, id, source)
                } else {
                    overlay::rebind_faces(tx, id, &sources)
                }
            })?,
            Operation::Create => self.edit(actor, "Create overlay", |tx| {
                overlay::create_faces(tx, &sources, options)
            })?,
        };
        self.overlay.draft = None;
        self.overlay.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        self.select(&[id], SelectMode::Replace)?;
        let mut result = self.get_overlay_state();
        result["operation"] = json!(operation.label());
        result[if operation.entity().is_some() {
            "updated"
        } else {
            "created"
        }] = json!(crate::identity::id_string(id));
        Ok(result)
    }

    pub fn cancel_overlay(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self
            .overlay
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Overlay draft was closed or replaced")?;
        if draft.owner != actor {
            return Err("Review the current overlay draft before cancelling it".into());
        }
        self.overlay.draft = None;
        self.overlay.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        self.log(actor, "Cancel overlay");
        Ok(self.get_overlay_state())
    }

    pub(super) fn append_overlay_guide(&self, mesh: &mut render::SceneMesh) {
        let Some(draft) = self
            .overlay
            .draft
            .as_ref()
            .filter(|draft| self.checked_overlay(draft.token).is_ok())
        else {
            return;
        };
        if !self.working_solid_visible(self.render_document(), draft.source.solid) {
            return;
        }
        let corners = draft.definition.corners();
        for index in 0..corners.len() {
            for point in [corners[index], corners[(index + 1) % corners.len()]] {
                mesh.lines.push(render::scene::LineVertex {
                    position: DVec3::from_array(point).as_vec3().to_array(),
                    color: [80, 220, 255, 255],
                });
            }
        }
        self.append_overlay_corner_handles(mesh);
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
