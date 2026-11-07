use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use compiler::{Control, asset_sources::AssetSourceGuard};
use glam::DVec3;
use map_core::{
    Document, EditError, ObjectId, Transaction, edit,
    query::{Ray, SurfaceHit},
};
use render::{Camera, SceneMesh, ropes};
use serde_json::{Value, json};
use source_formats::{fgd::ClassKind, gamefs::GameFs, vmf::Entity};

use super::{Actor, Pointer, SelectMode, Session, viewport_pick};

const SOURCE_METADATA_BYTES: usize = 8 * 1024 * 1024;
const LIMITS: &str = "Two static world-space anchors on visible authored world, func_detail or terrain surfaces. No model, instance or runtime parent attachment. The pending span uses the existing five-second settled rope preview with gravity 1500, zero wind and no collision; the rope tube may cross surfaces. Slack is the native signed integer: nominal length is truncated float32 endpoint distance + Slack - 100, with integer segment rounding.";

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub material: String,
    pub slack: i32,
    pub width: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Endpoint {
    Start,
    End,
}

impl Endpoint {
    fn index(self) -> usize {
        match self {
            Self::Start => 0,
            Self::End => 1,
        }
    }
}

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    draft: Option<Draft>,
}

struct Draft {
    token: u64,
    owner: Actor,
    generation: u64,
    revision: u64,
    scene: u64,
    content: u64,
    definitions: u64,
    visibility: u64,
    models: u64,
    tools: bool,
    selection: Vec<ObjectId>,
    request: Request,
    endpoints: [Option<SurfaceHit>; 2],
    picking: Option<Picking>,
    plan: Option<Plan>,
    error: Option<String>,
}

struct Picking {
    endpoint: Endpoint,
    context: viewport_pick::Context,
}

struct Plan {
    document: Document,
    objects: [ObjectId; 2],
    names: Vec<String>,
    nodes: [Node; 2],
    preview: Arc<ropes::Preview>,
    appearance: Arc<ropes::materials::Prepared>,
    sources: Arc<AssetSourceGuard>,
}

#[derive(Clone)]
struct Node {
    classname: String,
    properties: Vec<(String, String)>,
    origin: DVec3,
}

#[derive(Clone)]
pub struct RenderGuard {
    scope: String,
    token: u64,
    sources: Arc<AssetSourceGuard>,
    game: Arc<GameFs>,
}

impl RenderGuard {
    pub fn verify_original_sources(&self) -> Result<(), String> {
        let control = Control::default();
        self.sources
            .verify_original_metadata(&control, SOURCE_METADATA_BYTES)
            .and_then(|_| self.sources.verify_sources(&self.game, &control))
            .map_err(|error| format!("Rope material sources changed: {error}"))
    }
}

fn apply_nodes(tx: &mut Transaction<'_>, nodes: &[Node; 2]) -> Result<[ObjectId; 2], EditError> {
    let mut objects = Vec::with_capacity(2);
    for node in nodes {
        objects.push(edit::create_entity(
            tx,
            &node.classname,
            Some(node.origin),
            &node.properties,
            Vec::new(),
        )?);
    }
    Ok(objects.try_into().unwrap())
}

fn surface_report(surface: SurfaceHit) -> Value {
    json!({"point":surface.hit.point.to_array(),"normal":surface.normal.to_array(),
        "target":crate::identity::id_string(surface.hit.object),"solid":surface.hit.solid,"side":surface.hit.side})
}

impl Session {
    pub fn rope_placement_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{}:{}:{:?}:{}",
            self.buffer_review_scope,
            self.document_generation,
            self.document.revision(),
            self.scene_serial,
            self.content_epoch,
            self.definitions_serial,
            self.working_region.serial,
            self.geometry.rope_placement.serial,
            self.selected_ids(),
            self.view.tool_textures
        )
    }

    pub fn rope_placement_pick_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{:?}",
            self.rope_placement_stamp(),
            self.view_serial,
            self.camera_serial,
            self.camera
        )
    }

    fn rope_placement_context(&self) -> Result<(), String> {
        self.ensure_not_closing()?;
        let own = self
            .geometry
            .rope_placement
            .draft
            .as_ref()
            .and_then(|draft| draft.plan.as_ref())
            .is_some_and(|plan| std::ptr::eq(self.render_document(), &plan.document));
        if !std::ptr::eq(self.render_document(), &self.document) && !own {
            return Err("Apply or cancel the geometry preview before placing a rope".into());
        }
        Ok(())
    }

    fn checked_rope_placement(&self, token: u64) -> Result<&Draft, String> {
        let draft = self
            .geometry
            .rope_placement
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Rope placement was closed or replaced")?;
        if let Some(error) = &draft.error {
            return Err(error.clone());
        }
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
            || draft.scene != self.scene_serial
            || draft.content != self.content_epoch
            || draft.definitions != self.definitions_serial
            || draft.visibility != self.working_region.serial
            || draft.models != self.models.borrow().reload_generation()
            || draft.tools != self.view.tool_textures
            || draft.selection != self.selected_ids()
        {
            return Err("Map, selection, definitions, visibility or content changed; begin rope placement again".into());
        }
        self.rope_placement_context()?;
        if let Some(plan) = &draft.plan {
            if plan.appearance.generation != self.materials.generation() {
                return Err("Rope material library changed; begin rope placement again".into());
            }
            plan.sources
                .verify_original_metadata(&Control::default(), SOURCE_METADATA_BYTES)
                .map_err(|error| format!("Rope material sources changed: {error}"))?;
        }
        Ok(draft)
    }

    fn owned_rope_placement(&self, actor: Actor, token: u64) -> Result<&Draft, String> {
        let draft = self.checked_rope_placement(token)?;
        if draft.owner != actor {
            return Err("Review the other actor's rope placement before changing it".into());
        }
        Ok(draft)
    }

    fn rope_node(
        &self,
        classname: &str,
        request: &Request,
        name: &str,
        next: &str,
        origin: DVec3,
    ) -> Result<Node, String> {
        let class = self
            .definitions
            .class(classname)
            .filter(|class| class.kind == ClassKind::Point)
            .ok_or_else(|| {
                format!("Load point-entity definitions for {classname} before placing a rope")
            })?;
        let properties = BTreeMap::from([
            ("targetname".into(), name.into()),
            ("NextKey".into(), next.into()),
            ("RopeMaterial".into(), request.material.clone()),
            ("Slack".into(), request.slack.to_string()),
            ("Width".into(), request.width.to_string()),
            ("Type".into(), "0".into()),
            ("Subdiv".into(), "2".into()),
            ("TextureScale".into(), "4".into()),
            ("Collide".into(), "0".into()),
            ("NoWind".into(), "0".into()),
            ("Dangling".into(), "0".into()),
            ("Breakable".into(), "0".into()),
            ("parentname".into(), String::new()),
            ("StartAttachment".into(), "0".into()),
            ("EndAttachment".into(), "0".into()),
        ]);
        let (classname, properties) =
            self.entity_properties(&class.name, ClassKind::Point, properties)?;
        let mut entity = Entity::new(0, &classname);
        entity.properties.0.extend(properties.clone());
        ropes::settings(&entity)?;
        Ok(Node {
            classname,
            properties,
            origin,
        })
    }

    fn validate_rope_request(&self, request: &Request) -> Result<(), String> {
        if request.material.trim().is_empty() {
            return Err("Choose an explicit mounted rope material".into());
        }
        self.rope_node("move_rope", request, "rope_start", "rope_end", DVec3::ZERO)?;
        self.rope_node("keyframe_rope", request, "rope_end", "", DVec3::ZERO)?;
        Ok(())
    }

    fn plan_rope_placement(
        &mut self,
        request: &Request,
        endpoints: [Option<SurfaceHit>; 2],
    ) -> Result<Option<Plan>, String> {
        let [Some(start), Some(end)] = endpoints else {
            return Ok(None);
        };
        if start.hit.point == end.hit.point {
            return Err("Rope endpoints must be distinct".into());
        }
        let names = self.linked_path_names("rope", 2)?;
        let nodes = [
            self.rope_node("move_rope", request, &names[0], &names[1], start.hit.point)?,
            self.rope_node("keyframe_rope", request, &names[1], "", end.hit.point)?,
        ];
        let mut document = self.document.preview_snapshot();
        let objects = document
            .edit("preview rope placement", |tx| apply_nodes(tx, &nodes))
            .map_err(|error| error.to_string())?;
        let preview = ropes::prepare(document.vmf(), |id| objects.contains(&id));
        if !preview.complete
            || preview.links.len() != 1
            || !preview.issues.is_empty()
            || preview.omitted != 0
        {
            return Err(preview.issues.first().map_or_else(
                || "Rope span could not be prepared".into(),
                |issue| issue.message.clone(),
            ));
        }
        let control = Control::default();
        let assets = super::ropes::materials::prepare(&self.game, &preview, &control)?;
        assets.verify_sources(&self.game, &control)?;
        let (appearance, sources) = assets.bind(&mut self.materials)?;
        Ok(Some(Plan {
            document,
            objects,
            names,
            nodes,
            preview: Arc::new(preview),
            appearance: Arc::new(appearance),
            sources: Arc::new(sources),
        }))
    }

    fn rope_placement_changed(&mut self) {
        self.geometry.rope_placement.serial += 1;
        self.scene_serial += 1;
        self.view_serial += 1;
        if let Some(draft) = &mut self.geometry.rope_placement.draft {
            draft.token = self.geometry.rope_placement.serial;
            draft.scene = self.scene_serial;
        }
    }

    pub fn get_rope_placement(&self) -> Value {
        let draft = self.geometry.rope_placement.draft.as_ref().map(|draft| {
            let error = self.checked_rope_placement(draft.token).err();
            let plan = draft.plan.as_ref().map(|plan| {
                let link = &plan.preview.links[0];
                let bounds = link.bounds();
                json!({"entities":plan.objects.map(crate::identity::id_string),"names":plan.names,
                    "nodes":plan.nodes.iter().zip(plan.objects).map(|(node,id)|json!({
                        "entity":crate::identity::id_string(id),"classname":node.classname,
                        "origin":node.origin.to_array(),"properties":node.properties})).collect::<Vec<_>>(),
                    "points":link.points.iter().map(|point|point.to_array()).collect::<Vec<_>>(),
                    "segment_count":link.points.len()-1,"bounds":{"mins":bounds.0.to_array(),"maxs":bounds.1.to_array()},
                    "nominal_length":link.texture_length,"initial_hang_seconds":ropes::INITIAL_HANG_SECONDS,
                    "collision":false,"gravity":ropes::Environment::default().gravity,"wind":[0,0,0]})
            });
            let picking_error = draft.picking.as_ref().filter(|pick| !pick.context.is_current(self))
                .map(|_| "Scene, view or camera changed; choose Pick start or Pick end again");
            json!({"token":draft.token,"owner":draft.owner.label(),"valid":error.is_none(),"error":error,
                "material":draft.request.material,"slack":draft.request.slack,"width":draft.request.width,
                "start":draft.endpoints[0].map(surface_report),"end":draft.endpoints[1].map(surface_report),
                "picking":draft.picking.is_some() && error.is_none() && picking_error.is_none(),
                "picking_endpoint":draft.picking.as_ref().map(|pick|pick.endpoint),"picking_error":picking_error,
                "ready":error.is_none() && plan.is_some() && draft.picking.is_none(),"plan":plan})
        });
        json!({"snapshot":self.rope_placement_stamp(),"pick_snapshot":self.rope_placement_pick_stamp(),
            "generation":self.document_generation,"revision":self.document.revision(),"draft":draft,"limits":LIMITS})
    }

    pub fn begin_rope_placement(
        &mut self,
        actor: Actor,
        request: Request,
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.rope_placement_stamp() {
            return Err("Rope placement context changed; inspect it again".into());
        }
        self.rope_placement_context()?;
        if self
            .geometry
            .rope_placement
            .draft
            .as_ref()
            .is_some_and(|draft| draft.owner != actor)
        {
            return Err("Review the other actor's rope placement before changing it".into());
        }
        self.validate_rope_request(&request)?;
        self.clear_geometry_previews_except(&[super::previews::Kind::RopePlacement]);
        self.cancel_block_preview();
        self.cancel_area_selection();
        self.clear_volume_selection();
        self.spatial.draw_block = false;
        self.clip_tool = false;
        self.geometry.rope_placement.draft = Some(Draft {
            token: 0,
            owner: actor,
            generation: self.document_generation,
            revision: self.document.revision(),
            scene: self.scene_serial,
            content: self.content_epoch,
            definitions: self.definitions_serial,
            visibility: self.working_region.serial,
            models: self.models.borrow().reload_generation(),
            tools: self.view.tool_textures,
            selection: self.selected_ids(),
            request,
            endpoints: [None; 2],
            picking: None,
            plan: None,
            error: None,
        });
        self.rope_placement_changed();
        Ok(self.get_rope_placement())
    }

    pub fn update_rope_placement(
        &mut self,
        actor: Actor,
        token: u64,
        request: Request,
    ) -> Result<Value, String> {
        let endpoints = self.owned_rope_placement(actor, token)?.endpoints;
        self.validate_rope_request(&request)?;
        let plan = self.plan_rope_placement(&request, endpoints)?;
        let draft = self.geometry.rope_placement.draft.as_mut().unwrap();
        draft.request = request;
        draft.plan = plan;
        draft.picking = None;
        self.rope_placement_changed();
        Ok(self.get_rope_placement())
    }

    pub fn begin_rope_placement_pick(
        &mut self,
        actor: Actor,
        token: u64,
        endpoint: Endpoint,
        camera: Camera,
        size: (u32, u32),
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.rope_placement_pick_stamp() {
            return Err("Rope picking context changed; inspect it again".into());
        }
        self.owned_rope_placement(actor, token)?;
        if self.measurement_picking()
            || self.viewport_pick_conflict_without_rope()
            || self.linked_path_picking()
        {
            return Err("Finish the active viewport tool before picking a rope anchor".into());
        }
        let context = viewport_pick::Context::capture(self, camera, size, "Rope surface picking")?;
        self.geometry.rope_placement.draft.as_mut().unwrap().picking = None;
        self.rope_placement_changed();
        let mut context = context;
        context.refresh_scene(self);
        self.geometry.rope_placement.draft.as_mut().unwrap().picking =
            Some(Picking { endpoint, context });
        Ok(self.get_rope_placement())
    }

    fn checked_rope_pick(&self, actor: Actor, token: u64) -> Result<&Picking, String> {
        let draft = self.owned_rope_placement(actor, token)?;
        let pick = draft
            .picking
            .as_ref()
            .ok_or("Choose Pick start or Pick end first")?;
        if !pick.context.is_current(self) {
            return Err(
                "Scene, view or camera changed; choose Pick start or Pick end again".into(),
            );
        }
        Ok(pick)
    }

    pub fn pick_rope_placement_ray(
        &mut self,
        actor: Actor,
        token: u64,
        ray: Ray,
    ) -> Result<Value, String> {
        let endpoint = self.checked_rope_pick(actor, token)?.endpoint;
        let request = self.owned_rope_placement(actor, token)?.request.clone();
        let surface = self.placement_surface(&[], ray, true)?;
        let solid = surface.hit.solid.ok_or("Rope placement supports visible authored world/detail/terrain surfaces, not model or instance anchors")?;
        let (owner, _) =
            map_core::subobject::solid(&self.document, solid).map_err(|error| error.to_string())?;
        if let ObjectId::Entity(entity) = owner {
            let entity = self
                .document
                .entity(entity)
                .ok_or("Rope surface owner disappeared")?;
            if !entity
                .classname()
                .is_some_and(|name| name.eq_ignore_ascii_case("func_detail"))
            {
                return Err("Rope placement supports static world/detail/terrain surfaces; moving brush entities need explicit runtime attachment".into());
            }
        }
        let mut endpoints = self.owned_rope_placement(actor, token)?.endpoints;
        endpoints[endpoint.index()] = Some(surface);
        let plan = self.plan_rope_placement(&request, endpoints)?;
        let draft = self.geometry.rope_placement.draft.as_mut().unwrap();
        draft.endpoints = endpoints;
        draft.plan = plan;
        draft.picking = None;
        self.rope_placement_changed();
        Ok(self.get_rope_placement())
    }

    pub fn pick_rope_placement_pixel(
        &mut self,
        actor: Actor,
        token: u64,
        pointer: Pointer,
    ) -> Result<Value, String> {
        let ray = self.checked_rope_pick(actor, token)?.context.ray(pointer)?;
        self.pick_rope_placement_ray(actor, token, ray)
    }

    pub fn cancel_rope_placement_pick(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Value, String> {
        let error = self.checked_rope_placement(token).err();
        let draft = self
            .geometry
            .rope_placement
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Rope placement was closed or replaced")?;
        if draft.owner != actor {
            return Err("Review the other actor's rope placement before cancelling picking".into());
        }
        if draft.picking.is_none() {
            return Err("Rope surface picking is not active".into());
        }
        let draft = self.geometry.rope_placement.draft.as_mut().unwrap();
        draft.picking = None;
        draft.error = error;
        self.rope_placement_changed();
        Ok(self.get_rope_placement())
    }

    pub fn review_rope_placement(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let error = self.checked_rope_placement(token).err();
        self.geometry
            .rope_placement
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Rope placement was closed or replaced")?;
        self.rope_placement_context()?;
        let draft = self.geometry.rope_placement.draft.as_mut().unwrap();
        draft.owner = actor;
        draft.picking = None;
        draft.error = error;
        self.rope_placement_changed();
        Ok(self.get_rope_placement())
    }

    pub fn apply_rope_placement(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self.owned_rope_placement(actor, token)?;
        if draft.picking.is_some() {
            return Err("Finish or cancel rope surface picking before Apply".into());
        }
        let plan = draft
            .plan
            .as_ref()
            .ok_or("Pick both rope endpoints before Apply")?;
        let nodes = plan.nodes.clone();
        let expected_objects = plan.objects;
        let expected = expected_objects.map(|id| {
            let ObjectId::Entity(entity) = id else {
                unreachable!()
            };
            plan.document.entity(entity).unwrap().clone()
        });
        let guard = self
            .rope_placement_render_guard()?
            .ok_or("Rope preview is unavailable")?;
        let verified = guard.verify_original_sources();
        self.verify_rope_placement_render_guard(&guard, verified)?;
        let result = self.edit(actor, "place rope", |tx| {
            let objects = apply_nodes(tx, &nodes)?;
            for (id, expected) in objects.iter().zip(&expected) {
                let ObjectId::Entity(entity) = id else {
                    unreachable!()
                };
                if tx.document().entity(*entity) != Some(expected) {
                    return Err(EditError::Invalid(
                        "Rope placement replay changed; preview again".into(),
                    ));
                }
            }
            if objects != expected_objects {
                return Err(EditError::Invalid(
                    "Rope endpoint identities changed; preview again".into(),
                ));
            }
            guard
                .verify_original_sources()
                .map_err(EditError::Invalid)?;
            Ok(objects)
        });
        let objects = match result {
            Ok(objects) => objects,
            Err(error) => {
                self.verify_rope_placement_render_guard(&guard, Err(error.clone()))?;
                return Err(error);
            }
        };
        self.select(&objects, SelectMode::Replace)?;
        Ok(
            json!({"created":objects.map(crate::identity::id_string),"state":self.get_rope_placement()}),
        )
    }

    pub fn cancel_rope_placement(&mut self, actor: Actor, token: u64) -> Result<(), String> {
        let draft = self
            .geometry
            .rope_placement
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Rope placement was closed or replaced")?;
        if draft.owner != actor {
            return Err("Review the other actor's rope placement before cancelling it".into());
        }
        self.clear_rope_placement();
        Ok(())
    }

    pub(super) fn clear_rope_placement(&mut self) {
        if self.geometry.rope_placement.draft.take().is_some() {
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub(super) fn rope_surface_picking(&self) -> bool {
        self.geometry
            .rope_placement
            .draft
            .as_ref()
            .is_some_and(|draft| draft.picking.is_some())
    }

    pub fn ensure_rope_placement_resolved(&self) -> Result<(), String> {
        if self.geometry.rope_placement.draft.is_some() {
            return Err(
                "Apply or cancel rope placement before saving, compiling or playing".into(),
            );
        }
        Ok(())
    }

    pub fn rope_placement_render_guard(&self) -> Result<Option<RenderGuard>, String> {
        let Some(draft) = &self.geometry.rope_placement.draft else {
            return Ok(None);
        };
        let Some(plan) = &draft.plan else {
            return Ok(None);
        };
        self.checked_rope_placement(draft.token)?;
        Ok(Some(RenderGuard {
            scope: self.buffer_review_scope.clone(),
            token: draft.token,
            sources: plan.sources.clone(),
            game: self.game.clone(),
        }))
    }

    pub fn verify_rope_placement_render_guard(
        &mut self,
        guard: &RenderGuard,
        verified: Result<(), String>,
    ) -> Result<(), String> {
        if guard.scope != self.buffer_review_scope
            || !self
                .geometry
                .rope_placement
                .draft
                .as_ref()
                .is_some_and(|draft| {
                    draft.token == guard.token
                        && draft
                            .plan
                            .as_ref()
                            .is_some_and(|plan| Arc::ptr_eq(&plan.sources, &guard.sources))
                })
        {
            return Err("Rope placement render source was closed or replaced".into());
        }
        let result = verified.and_then(|_| self.checked_rope_placement(guard.token).map(|_| ()));
        if let Err(error) = &result {
            self.fail_rope_placement_render(error.clone());
        }
        result
    }

    pub(super) fn preflight_rope_placement(&mut self) {
        let error = self
            .geometry
            .rope_placement
            .draft
            .as_ref()
            .filter(|draft| draft.plan.is_some())
            .and_then(|draft| self.checked_rope_placement(draft.token).err());
        if let Some(error) = error {
            self.fail_rope_placement_render(error);
        }
    }

    pub(super) fn fail_rope_placement_render(&mut self, error: String) {
        if let Some(draft) = &mut self.geometry.rope_placement.draft {
            draft.plan = None;
            draft.picking = None;
            draft.error = Some(error);
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub(super) fn append_rope_placement_preview(&self, mesh: &mut SceneMesh) -> Result<(), String> {
        let Some(draft) = self.geometry.rope_placement.draft.as_ref() else {
            return Ok(());
        };
        if self.checked_rope_placement(draft.token).is_err() {
            return Ok(());
        }
        if let Some(plan) = &draft.plan {
            mesh.append_pending_rope_preview(
                &plan.preview,
                &plan.appearance,
                &HashSet::from(plan.objects),
                true,
            )?;
        }
        for endpoint in draft.endpoints.iter().flatten() {
            mesh.add_bounds_lines(
                endpoint.hit.point - DVec3::splat(3.0),
                endpoint.hit.point + DVec3::splat(3.0),
                [80, 200, 255, 255],
            );
        }
        Ok(())
    }
}

impl State {
    pub(super) fn document(&self) -> Option<&Document> {
        self.draft
            .as_ref()?
            .plan
            .as_ref()
            .map(|plan| &plan.document)
    }
    pub(super) fn has_draft(&self) -> bool {
        self.draft.is_some()
    }
    pub(super) fn close_draft(&self) -> Option<Value> {
        self.draft.as_ref().map(|draft| {
            json!([
                self.serial,
                draft.token,
                draft.owner.label(),
                draft.plan.is_some(),
                draft
                    .endpoints
                    .map(|point| point.map(|point| point.hit.point.to_array()))
            ])
        })
    }
}
