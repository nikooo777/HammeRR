use std::collections::BTreeMap;
use std::sync::Arc;

use glam::DVec3;
use map_core::{Document, ObjectId, query};
use render::ropes::{self, Preview};
use serde_json::{Value, json};

use super::Session;

pub(super) mod collision;
mod controls;
pub(super) mod endpoints;
pub(super) mod materials;
mod playback;
mod state;

pub(crate) use controls::Update;
pub(crate) use playback::RenderGuard;
pub use playback::{Completed, Request};
pub(crate) use state::ClockLease;
pub(super) use state::State;
pub use state::{Clock, CollisionSummary, Command, Frame, Settings};

impl Session {
    pub(crate) fn presentation_bounds_in(
        &self,
        document: &Document,
        rope_preview: Option<bool>,
    ) -> BTreeMap<ObjectId, (DVec3, DVec3)> {
        let mut bounds = self.display_bounds_in(document);
        if !rope_preview.unwrap_or(self.view.rope_preview) {
            return bounds;
        }
        for link in &self.rope_preview_geometry_in(document).links {
            let source = ObjectId::Entity(link.source);
            let span = link.bounds();
            let helper = bounds
                .get(&source)
                .copied()
                .or_else(|| query::bounds(document, source));
            let combined = helper.map_or(span, |(mins, maxs)| (mins.min(span.0), maxs.max(span.1)));
            bounds.insert(source, combined);
        }
        bounds
    }

    pub(crate) fn presentation_object_bounds_in(
        &self,
        document: &Document,
        ids: &[ObjectId],
        rope_preview: Option<bool>,
    ) -> Option<(DVec3, DVec3)> {
        if ids.is_empty() {
            return None;
        }
        let bounds = self.presentation_bounds_in(document, rope_preview);
        super::models::combined_bounds(document, ids, &bounds)
    }

    pub(super) fn rope_preview_geometry(&self) -> Arc<Preview> {
        self.rope_preview_geometry_in(self.render_document())
    }

    pub fn rope_preview_state(&self) -> Value {
        self.rope_preview_report(self.view.rope_preview)
    }

    pub fn rope_preview_report(&self, enabled: bool) -> Value {
        let clock = self.rope_preview_clock();
        let frame = self.rope_preview_frame();
        let ready = frame.is_some() && self.rope_preview.machine.is_some() && !clock.busy;
        let error = if clock.status == "stale" {
            Some("Rope source changed; prepare the preview again")
        } else {
            self.rope_preview.error.as_deref()
        };
        let summary = self.rope_preview_summary_for(enabled);
        let links = frame
            .into_iter()
            .flat_map(|frame| {
                frame.preview.links.iter().map(move |link| {
                    let span = frame.prepared.spans.iter().find(|span| span.source == link.source);
                    let appearance = frame.appearance.by_source.get(&link.source);
                    json!({
                        "source":format!("entity:{}",link.source),
                        "target":link.target.map(|id|format!("entity:{id}")),
                        "width":link.width,"slack":link.slack,"length":link.length(),
                        "points":link.points.iter().map(|point|point.to_array()).collect::<Vec<_>>(),
                        "material":link.material,"material_status":appearance.map(|appearance|&appearance.report),
                        "physical_nodes":link.physical_nodes,"subdiv":link.subdiv,
                        "subdiv_authored":link.subdiv_authored,
                        "texture_scale":link.texture_scale,"texture_length":link.texture_length,
                        "scroll":link.scroll,"barbed":link.barbed,"breakable":link.breakable,
                        "scroll_speed":span.map(|span|span.settings.scroll_speed),
                        "inert_keys":link.inert_keys,
                        "anchors":span.map(|span|json!({
                            "start":span.endpoints.start.position.to_array(),
                            "start_forward":span.endpoints.start.forward.to_array(),
                            "end":span.endpoints.end.map(|end|end.position.to_array()),
                            "end_forward":span.endpoints.end.map(|end|end.forward.to_array()),
                            "rope_length":span.endpoints.rope_length,
                            "binding":if link.target.is_none(){"native_absent_end_world_zero"}else{"authored_target"},
                        })),
                        "collision":span.map(|span|span.settings.collide),
                        "no_wind":span.map(|span|span.settings.no_wind),
                        "attachment_status":"Resolved static authored anchors; runtime parent motion and model animation are not simulated",
                        "physics_status":"Published complete fixed-step pose; source entity owns its outgoing span",
                    })
                })
            })
            .collect::<Vec<_>>();
        let issues = self
            .rope_preview
            .issues
            .iter()
            .map(|issue| {
                json!({
                    "entity":format!("entity:{}",issue.entity),"message":issue.message,
                })
            })
            .collect::<Vec<_>>();
        let last_actor = self.rope_preview.last_actor.map(|actor| match actor {
            super::Actor::User => "user",
            super::Actor::Agent => "agent",
            super::Actor::System => "system",
        });
        json!({
            "enabled":enabled,"applied":enabled && frame.is_some_and(|frame|!frame.preview.links.is_empty()),
            "ready":ready,"published":frame.is_some(),"static":false,"preview_only":true,
            "status":clock.status,"summary":summary,"error":error,"playing":clock.playing,"busy":clock.busy,
            "presentation_active":clock.presentation_active,
            "worker_actor":self.rope_preview.pending.as_ref().map(|pending|match pending.actor {
                super::Actor::User=>"user",super::Actor::Agent=>"agent",super::Actor::System=>"system",
            }),
            "snapshot":clock.snapshot,"serial":clock.serial,"frame_serial":clock.frame_serial,
            "time_seconds":clock.time_seconds,"published_time_seconds":frame.map(|frame|frame.preview.time),
            "requested_time_seconds":clock.requested_time_seconds,"max_time_seconds":ropes::MAX_TIME_SECONDS,
            "settings":self.rope_preview.settings,"last_actor":last_actor,
            "links":links,"issues":issues,"omitted_nodes":self.rope_preview.omitted,
            "collision":frame.map(|frame|&frame.collision),
            "parent_model_sources":frame.map(|frame|frame.parent_models),
            "simulation_work":frame.map(|frame|frame.simulation_work),
            "collision_work":frame.map(|frame|frame.collision_work),
            "warmup_steps":frame.map(|frame|frame.warmup_steps),
            "snapshot_bytes":frame.map(|frame|frame.snapshot_bytes),
            "last_source_check_age_seconds":frame.map(|frame|frame.verified.elapsed().as_secs_f64()),
            "limits":{
                "nodes":ropes::MAX_LINKS,"physical_nodes_per_span":ropes::MAX_PHYSICS_NODES,
                "segments_per_span":ropes::MAX_CURVE_SEGMENTS,"time_seconds":ropes::MAX_TIME_SECONDS,
                "fixed_step_seconds":ropes::STEP_SECONDS,"initial_hang_seconds":ropes::INITIAL_HANG_SECONDS,
                "steps_per_worker_batch":ropes::MAX_BATCH_STEPS,"advance_seconds":state::MAX_ADVANCE_SECONDS,
                "simulation_work":ropes::MAX_SIMULATION_WORK,"source_snapshot_bytes":state::MAX_SOURCE_BYTES,
                "identifier_bytes":ropes::MAX_IDENTIFIER_BYTES,
                "diagnostic_excerpt_bytes":ropes::MAX_DIAGNOSTIC_EXCERPT_BYTES,
                "coordinate_extent":ropes::MAX_EXTENT,"slack":[ropes::MIN_SLACK,ropes::MAX_SLACK],
                "width":[1.0,ropes::MAX_WIDTH],"gravity":[0.0,ropes::MAX_GRAVITY],
                "wind_component":[-ropes::MAX_WIND,ropes::MAX_WIND],
                "collision_brushes":map_core::rope_collision::MAX_BRUSHES,
                "collision_triangles":map_core::rope_collision::MAX_TRIANGLES,
                "collision_trace_work":map_core::rope_collision::MAX_TRACE_WORK,
                "collision_request_work":playback::MAX_REQUEST_COLLISION_WORK,
                "active_workers":1,"published_frames":1,
            },
            "appearance":render::ropes::materials::LIMITS,
            "motion":"Deterministic editor preview: native-shaped fixed-step springs, three constraint passes, initial five-second hang without wind, then explicit preview gravity and direct world wind acceleration in units per second squared where the authored flags allow it. Reset and backward Seek replay from the same source. Authored ScrollSpeed is retained but does not animate this admitted path. No runtime I/O, break events, parent motion, model animation, env_wind or native pixel-parity claim.",
            "playback_policy":"Play uses one shared runtime clock in desktop and headless sessions. The UI only refreshes the published state. Presentation suspension freezes wall time without catch-up; explicit Seek and Advance remain available. Expensive scenes may play slower than wall time. Seek and Reset pause. Settings require preparation again. Disabling visibility pauses and preserves only a current published pose.",
            "collision_policy":"Authored compile-visible world, func_detail and qualified terrain are prepared for fixed ±2 hull traces independently of the transient working region. Brush models, model surfaces and dynamic native collision are excluded and counted. Rope admission itself uses the captured visible source set.",
            "source_check_policy":"Current map/render-document, visibility, definitions, model and material/content reload epochs are checked without I/O. Original material/collision metadata and actual used parent-model sources are rechecked outside the Session lock on preparation and due one-second polling. Explicit render/pick verification uses the same retained original sources.",
            "selection":"Rendering, picking, framing and conservative area/volume bounds use this same published immutable pose. Each span belongs to its source entity and enlarges its ordinary helper by the full sampled curve plus half Width; incoming targets retain independent ownership. Publishing or retiring a pose expires area/volume reviews. Authored transforms, pivots, placement and paint retain ordinary helper bounds.",
            "persistence":"Preview environment, time and playback are Session view state only. They do not edit VMF properties, create Undo steps or persist as rope entity settings. The existing rope_preview visibility default remains separate.",
        })
    }

    pub fn rope_preview_summary(&self) -> String {
        self.rope_preview_summary_for(self.view.rope_preview)
    }

    fn rope_preview_summary_for(&self, enabled: bool) -> String {
        let frame = self.rope_preview_frame();
        if self.rope_preview.frame.is_some() && frame.is_none() {
            return "Rope source changed; prepare the preview again".into();
        }
        if let Some(error) = &self.rope_preview.error {
            return error.clone();
        }
        if let Some(frame) = frame {
            format!(
                "{} rope spans · {:.3} s · {}{}",
                frame.preview.links.len(),
                frame.preview.time,
                self.rope_preview.status,
                if enabled { "" } else { " · preview off" }
            )
        } else {
            format!(
                "Rope preview {} · prepare a complete pose",
                self.rope_preview.status
            )
        }
    }
}
