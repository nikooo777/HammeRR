use crate::identity::id_string;
use glam::DVec3;
use map_core::{Document, EditError, ObjectId, repeat};
use serde_json::{Value, json};

pub use super::transform_pivot::PivotSource;
use super::{Actor, SelectMode, Session};

mod policies;
mod scatter;
pub use policies::{ExternalReferences, InternalReferences, NameStyle, Naming, Policies};
pub use scatter::ScatterRequest;

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub count: u32,
    pub translation: DVec3,
    pub rotation: DVec3,
    pub scale: DVec3,
    pub local: bool,
    pub texture_lock: bool,
    pub pivot_source: Option<PivotSource>,
    pub pivot: Option<DVec3>,
    pub policies: Policies,
}

impl Request {
    fn report(&self) -> Value {
        json!({
            "count":self.count,
            "translation":self.translation.to_array(),
            "rotation":self.rotation.to_array(),
            "scale":self.scale.to_array(),
            "space":if self.local {"local"} else {"world"},
            "texture_lock":self.texture_lock,
            "pivot_source":self.pivot_source,
            "pivot":self.pivot.map(|point|point.to_array()),
            "policies":self.policies,
        })
    }
}

enum Intent {
    Array {
        options: repeat::Options,
        request: Request,
    },
    SurfaceScatter(scatter::Description),
}

impl Intent {
    fn kind(&self) -> &'static str {
        match self {
            Self::Array { .. } => "array",
            Self::SurfaceScatter(_) => "surface_scatter",
        }
    }

    fn policies(&self) -> &Policies {
        match self {
            Self::Array { request, .. } => &request.policies,
            Self::SurfaceScatter(scatter) => &scatter.request.policies,
        }
    }
}

pub struct Preview {
    pub document: Document,
    pub result: repeat::Result,
    token: u64,
    sources: Vec<ObjectId>,
    plan: repeat::Plan,
    intent: Intent,
    generation: u64,
    revision: u64,
    definitions: u64,
    content: u64,
    scene: u64,
}

#[derive(Default)]
pub struct State {
    serial: u64,
    pub preview: Option<Preview>,
    last_settings: Option<Request>,
}

fn pose_report(placement: &repeat::Placement) -> Value {
    json!({"translation":placement.translation.to_array(),"rotation":placement.rotation.to_array(),
        "scale":placement.scale.to_array(),"pivot":placement.pivot.to_array(),
        "basis":placement.basis.to_cols_array_2d(),"texture_lock":placement.texture_lock})
}

impl Session {
    pub fn repeat_stamp(&self) -> String {
        format!(
            "repeat:{}:{}:{}:{}:{}:{}:{}:{}",
            self.buffer_review_scope,
            self.document_generation,
            self.document.revision(),
            self.definitions_serial,
            self.content_epoch,
            self.scene_serial,
            self.geometry.repeat.serial,
            self.pivot_preferences.local_serial
        )
    }

    pub fn repeat_array(&self) -> Value {
        let preview = self.geometry.repeat.preview.as_ref().map(|preview| {
            let objects = preview.result.objects();
            let stale = self.checked_repeat(preview.token).err();
            let mut report = json!({
                "kind":preview.intent.kind(),
                "token": preview.token,
                "sources": preview.sources.iter().copied().map(id_string).collect::<Vec<_>>(),
                "count": preview.plan.placements.len(),
                "policies":preview.intent.policies(),
                "poses":preview.plan.placements.iter().map(pose_report).collect::<Vec<_>>(),
                "bounds": self.object_bounds_in(&preview.document, &objects).map(|(mins, maxs)| [mins.to_array(), maxs.to_array()]),
                "copies": preview.result.copies.iter().map(|copy| json!({
                    "index": copy.index,
                    "scale": copy.placement.scale.to_array(),
                    "pose":pose_report(&copy.placement),
                    "objects": copy.objects.iter().copied().map(id_string).collect::<Vec<_>>(),
                    "mapping":copy.mapping.iter().map(|(source,copy)|json!({"source":id_string(*source),"copy":id_string(*copy)})).collect::<Vec<_>>(),
                    "names": copy.names.iter().map(|name| json!({"source":id_string(ObjectId::Entity(name.source)), "copy":id_string(ObjectId::Entity(name.copy)), "before":name.before, "after":name.after})).collect::<Vec<_>>(),
                    "remapped_references": copy.remapped_references,
                })).collect::<Vec<_>>(),
                "entities": objects.iter().filter_map(|id| {
                    let ObjectId::Entity(id) = id else { return None };
                    let entity = preview.document.entity(*id)?;
                    Some(json!({"id":id_string(ObjectId::Entity(*id)),"origin":entity.origin().map(|origin| origin.to_array()),"properties":entity.properties.0,"connections":entity.connections.iter().flatten().map(|connection|json!({"output":connection.output,"value":connection.value})).collect::<Vec<_>>()}))
                }).collect::<Vec<_>>(),
                "object_count": objects.len(),
                "valid":stale.is_none(),"stale_error":stale,
            });
            match &preview.intent {
                Intent::Array { options, request } => {
                    report["translation"] = json!(options.translation.to_array());
                    report["rotation"] = json!(options.rotation.to_array());
                    report["scale"] = json!(options.scale.to_array());
                    report["pivot"] = json!(options.pivot.to_array());
                    report["pivot_source"] = json!(request.pivot_source);
                    report["request"] = request.report();
                    report["space"] = json!(if request.local { "local" } else { "world" });
                    report["basis"] = json!(options.basis.to_cols_array_2d());
                    report["frame_source"] = json!(request.local.then(|| preview.sources.iter().copied().min().map(id_string)).flatten());
                    report["texture_lock"] = json!(options.texture_lock);
                }
                Intent::SurfaceScatter(scatter) => scatter.report(&mut report),
            }
            report
        });
        json!({
            "handle": self.repeat_array_handle(),
            "snapshot": self.repeat_stamp(),
            "document_generation":self.document_generation,
            "preview": preview,
            "last_settings":self.last_repeat_array_settings().as_ref().map(Request::report),
            "limits": {"sources":repeat::MAX_SOURCES,"copies":repeat::MAX_COPIES,"objects_including_owned_brushes":repeat::MAX_OBJECTS,"sides":repeat::MAX_SIDES,"name_bytes":repeat::MAX_NAME_BYTES,"name_affix_bytes":repeat::MAX_AFFIX_BYTES,"name_index":repeat::MAX_NAME_INDEX,"scatter":scatter::limits()},
            "transform_policy":"Array copy n uses the original objects, scales each XYZ axis by its per-copy factor raised to n, then rotates by n × XYZ degrees, then translates by n × the move step. Scale and rotation share one fixed world pivot and captured source-local frame when requested. Scatter places the resolved source pivot at an area-weighted sample plus its normal offset. Count excludes originals. Every copy reports its actual pose.",
            "reference_policy":"Naming uses the requested bounded source suffix or indexed prefix, with collision suffixes. Internal declared exact FGD/I/O/parent references can follow the copy or keep their originals. External declared references can be preserved or rejected; procedural ! references retain their intrinsic behavior. Shared names, ambiguous internal matches, wildcard captures of new names, overlay face-ID references and node IDs reject. Named source sets with scripts or dynamic I/O also reject.",
            "metadata_policy":"Supported object and owned-brush context notes copy with their existing authorship/observations and full map budgets. Saved boundary/free-cut plans copy with fresh identities and locked projections within their shared storage budgets. Rotated or scaled placement rejects unsupported opaque/spatial metadata through the shared prefab rules. Pure translation preserves prior opaque custom metadata and embedded text; their semantics are not inferred or remapped.",
            "reuse_policy":"Last settings retain the most recently applied array's steps, space, texture policy, pivot intent and naming/reference policies. Preview last array resolves current sources, pivot and local frame afresh. Custom world points remain literal coordinates. Scatter, failed previews and Cancel do not replace array settings.",
        })
    }

    pub(super) fn repeat_array_handle(&self) -> Value {
        Self::edit_handle(
            "repeat_array",
            self.geometry.repeat.preview.as_ref().and_then(|preview| {
                self.checked_repeat(preview.token)
                    .is_ok()
                    .then_some(preview.token)
            }),
        )
    }

    pub fn clear_repeat_preview(&mut self) {
        if self.geometry.repeat.preview.take().is_some() {
            self.geometry.repeat.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    fn checked_repeat(&self, token: u64) -> Result<&Preview, String> {
        self.ensure_not_closing()?;
        let preview = self
            .geometry
            .repeat
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Repeated-placement preview was closed or replaced")?;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
            || preview.definitions != self.definitions_serial
            || preview.content != self.content_epoch
            || preview.scene != self.scene_serial
        {
            return Err("Map, selection, content, visibility or entity definitions changed; preview the placement again".into());
        }
        Ok(preview)
    }

    fn check_repeat_request(
        &self,
        sources: &[ObjectId],
        count: u32,
        snapshot: &str,
    ) -> Result<(), String> {
        if snapshot != self.repeat_stamp() {
            return Err("Map, selection, content, pivot or preview changed; inspect repeated placement again".into());
        }
        self.ensure_not_closing()?;
        self.require_root_objects(sources)?;
        repeat::validate_sources(&self.document, sources, count, &self.definitions)
            .map_err(|error| error.to_string())?;
        self.ensure_partitions_resolved()
    }

    pub fn preview_repeat_array(
        &mut self,
        sources: &[ObjectId],
        options: repeat::Options,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.preview_repeat_array_in_space(sources, options, false, snapshot)
    }

    pub fn preview_repeat_array_in_space(
        &mut self,
        sources: &[ObjectId],
        options: repeat::Options,
        local: bool,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.preview_repeat_array_request(
            sources,
            Request {
                count: options.count,
                translation: options.translation,
                rotation: options.rotation,
                scale: options.scale,
                local,
                texture_lock: options.texture_lock,
                pivot_source: None,
                pivot: Some(options.pivot),
                policies: Policies::default(),
            },
            snapshot,
        )
    }

    pub fn last_repeat_array_settings(&self) -> Option<Request> {
        self.geometry.repeat.last_settings.clone()
    }

    pub fn preview_last_repeat_array(&mut self, snapshot: &str) -> Result<Value, String> {
        let request = self
            .last_repeat_array_settings()
            .ok_or("Apply a repeat array before reusing its settings")?;
        let sources = self.selected_ids();
        self.preview_repeat_array_request(&sources, request, snapshot)
    }

    pub fn preview_repeat_array_request(
        &mut self,
        sources: &[ObjectId],
        mut request: Request,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.check_repeat_request(sources, request.count, snapshot)?;
        let pivot =
            self.resolve_transform_pivot_request(sources, request.pivot_source, request.pivot)?;
        request.pivot_source = Some(PivotSource::from_request(
            request.pivot_source,
            request.pivot,
        )?);
        let options = repeat::Options {
            count: request.count,
            translation: request.translation,
            rotation: request.rotation,
            scale: request.scale,
            pivot,
            basis: self.transform_basis(sources, request.local)?,
            texture_lock: request.texture_lock,
        };
        let plan = repeat::Plan::array(options, request.policies.core())
            .map_err(|error| error.to_string())?;
        self.prepare_repeat_preview(sources, plan, Intent::Array { options, request })
    }

    fn prepare_repeat_preview(
        &mut self,
        sources: &[ObjectId],
        plan: repeat::Plan,
        intent: Intent,
    ) -> Result<Value, String> {
        let mut document = self.document.preview_snapshot();
        let result = document
            .edit("preview repeated placement", |tx| {
                repeat::create_plan(tx, sources, &plan, &self.definitions)
            })
            .map_err(|error| error.to_string())?;
        self.select(sources, SelectMode::Replace)?;
        self.cancel_block_preview();
        self.clear_geometry_previews_except(&[]);
        self.cancel_area_selection();
        self.clear_volume_selection();
        self.clip_tool = false;
        self.geometry.paint.enabled = false;
        self.spatial.draw_block = false;
        self.geometry.repeat.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        self.geometry.repeat.preview = Some(Preview {
            document,
            result,
            token: self.geometry.repeat.serial,
            sources: sources.to_vec(),
            plan,
            intent,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            content: self.content_epoch,
            scene: self.scene_serial,
        });
        Ok(self.repeat_array())
    }

    pub fn apply_repeat_array(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let preview = self.checked_repeat(token)?;
        let expected = preview.document.vmf().clone();
        let sources = preview.sources.clone();
        let plan = preview.plan.clone();
        let kind = preview.intent.kind();
        let last_settings = match &preview.intent {
            Intent::Array { request, .. } => Some(request.clone()),
            Intent::SurfaceScatter(_) => None,
        };
        let expected_result = preview.result.clone();
        let definitions = self.definitions.clone();
        let result = self.edit(
            actor,
            if kind == "array" {
                "repeat array"
            } else {
                "surface scatter"
            },
            |tx| {
                let result = repeat::create_plan(tx, &sources, &plan, &definitions)?;
                if result != expected_result || tx.document().vmf() != &expected {
                    return Err(EditError::Invalid(
                        "Placement differs from its preview; preview the placement again".into(),
                    ));
                }
                Ok(result)
            },
        )?;
        if let Some(request) = last_settings {
            self.geometry.repeat.last_settings = Some(request);
        }
        let copies = result.objects();
        self.select(&copies, SelectMode::Replace)?;
        Ok(
            json!({"handle":self.repeat_array_handle(),"kind":kind,"objects":copies.iter().copied().map(id_string).collect::<Vec<_>>(),"count":result.copies.len(),"state":self.repeat_array()}),
        )
    }

    pub fn cancel_repeat_array(&mut self, token: u64) -> Result<(), String> {
        self.geometry
            .repeat
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Repeated-placement preview was closed or replaced")?;
        self.clear_paste_preview();
        self.clear_repeat_preview();
        Ok(())
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<Value> {
        self.preview
            .as_ref()
            .map(|preview| json!([self.serial, preview.token]))
    }
}
