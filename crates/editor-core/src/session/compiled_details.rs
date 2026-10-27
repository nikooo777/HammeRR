use std::sync::Arc;

use compiler::{Control, FileIdentity, asset_sources::AssetSourceGuard};
use render::{SceneMesh, compiled_details::Lighting};
use serde_json::{Value, json};
use source_formats::{
    bsp::{Bsp, DetailProps, lump},
    gamefs::asset_relative,
    keyvalues::{FlatBlockLimits, parse_flat_blocks_controlled},
};

use super::{
    Actor, Session, baked_build,
    worker::{Slot, Ticket},
};

mod assets;
#[cfg(test)]
mod tests;

pub const MAX_ROWS: usize = 64;
pub const MAX_METADATA_BYTES: usize = 32 * 1024 * 1024;
const SOURCE_MODE: baked_build::SourceVerification =
    baked_build::SourceVerification::OriginalMetadata {
        maximum_bytes: MAX_METADATA_BYTES,
    };
const NOTES: &str = "Final native dprp v4 records from the retained successful BSP, including explicit and scattered details; native ordinals have no authored identity. Editor shading is the default preview lighting. Baked LDR uses the native base RGBExp32 divided by 255 as linear modulation, clamped to 0–1 and rounded to 8-bit vertex color, with the editor's sRGB output. It requires zero-style records and applies no editor directional or authored lighting. Runtime styles refuse the complete Baked LDR layer. Fixed native card/cross/tri forms and live full/vertical camera-facing cards remain static. Dynamic lights, sway, avoidance, runtime random card mirroring, fades, HDR, animation, native gamma and native appearance parity are not simulated. Exact pole/coincident views use the Source VectorAngles singular basis. Original compiler source guards and separately captured preview asset observations are checked independently. Preview texture reads do not acquire compiler provenance. Preparation verifies exact MDL/VVD/VTX identities in the shared model cache and refreshes requested stable material IDs. Currentness covers the committed map/settings/content epoch and tracked guards, not every world material. Unapplied drafts are not compiled. The layer never participates in authored picking, bounds, selection or Undo.";

struct Pending {
    token: u64,
    build: u64,
    identity: Arc<()>,
    control: Control,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) worker: Slot,
    serial: u64,
    pending: Option<Pending>,
    ready: Option<Ready>,
    requested: bool,
    displayed: bool,
    status: &'static str,
    error: Option<String>,
    lighting: Lighting,
}

struct Ready {
    build: u64,
    records: DetailProps,
    material: String,
    ldr_style_records: Option<usize>,
    geometry: Option<SceneMesh>,
    asset_guard: Option<AssetSourceGuard>,
    asset_bytes: usize,
}

struct Freshness {
    build: Value,
    preview_assets: Value,
    current: bool,
}

pub struct Request {
    token: u64,
    build: u64,
    identity: Arc<()>,
    control: Control,
    output: FileIdentity,
    worker: Ticket,
}

pub struct Preparation {
    token: u64,
    build: u64,
    identity: Arc<()>,
    control: Control,
    records: Result<Decoded, String>,
    _worker: Ticket,
}

struct Decoded {
    records: DetailProps,
    material: String,
    ldr_style_records: Option<usize>,
}

impl Request {
    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn run(self) -> Preparation {
        let records = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            decode(&self.output, &self.control)
        }))
        .unwrap_or_else(|_| Err("Compiled detail preparation panicked".into()));
        Preparation {
            token: self.token,
            build: self.build,
            identity: self.identity,
            control: self.control,
            records,
            _worker: self.worker,
        }
    }
}

fn decode(output: &FileIdentity, control: &Control) -> Result<Decoded, String> {
    let bytes = baked_build::read_owned_bytes_controlled(output, control)?;
    baked_build::decoded_bsp_bound(&bytes)?;
    control.checkpoint()?;
    let bsp = Bsp::parse(&bytes).map_err(|error| error.to_string())?;
    drop(bytes);
    if bsp.lumps[lump::LEAFS].data.len() > 65_536 * 56 {
        return Err("Detail inspection native leaf data exceeds its 3,670,016-byte bound".into());
    }
    control.checkpoint()?;
    let records = bsp
        .detail_props()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let ldr_style_records = bsp
        .detail_prop_lighting(false)
        .map_err(|error| error.to_string())?
        .map(|records| records.len());
    let material = if records.props.iter().any(|prop| prop.kind != 0) {
        let blocks = parse_flat_blocks_controlled(
            bsp.entities().map_err(|error| error.to_string())?,
            FlatBlockLimits {
                max_bytes: 1024 * 1024,
                max_blocks: 8192,
                max_pairs: 65_536,
            },
            &mut || control.checkpoint().map_err(std::io::Error::other),
        )
        .map_err(|error| error.to_string())?;
        let world = blocks
            .iter()
            .filter(|block| {
                block
                    .iter()
                    .any(|(key, value)| key == "classname" && value == "worldspawn")
            })
            .collect::<Vec<_>>();
        if world.len() != 1 {
            return Err("Compiled details require exactly one native worldspawn".into());
        }
        let entries = world[0]
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case("detailmaterial"))
            .collect::<Vec<_>>();
        if entries.len() > 1 {
            return Err("Compiled detail atlas declaration is ambiguous".into());
        }
        let path =
            DetailProps::sprite_material_name(entries.first().map(|(_, value)| value.as_str()));
        if path.len() > 1024 || path.chars().any(char::is_control) {
            return Err("Compiled detail atlas path exceeds its bound".into());
        }
        asset_relative(path).map_err(|error| error.to_string())?
    } else {
        String::new()
    };
    control.checkpoint()?;
    output.verify(control).map_err(|error| error.to_string())?;
    Ok(Decoded {
        records,
        material,
        ldr_style_records,
    })
}

impl Session {
    pub fn begin_compiled_details(&mut self, actor: Actor) -> Result<Request, String> {
        self.ensure_not_closing()?;
        if self.compiled_details.worker.busy() {
            return Err("Wait for the compiled detail worker to finish".into());
        }
        let control = Control::default();
        let state = baked_build::build_state_controlled(self, SOURCE_MODE, &control)?;
        if state["current"] != true {
            return Err("Compiled detail preparation requires a current successful owned build; compile again".into());
        }
        let build = self
            .compiled
            .as_ref()
            .ok_or("Compile the map before preparing final details")?;
        let output = build
            .lightmap_identity
            .output
            .as_ref()
            .map_err(|error| baked_build::bounded_message(error))?
            .clone();
        let build = build.lightmap_identity.token;
        let worker = self
            .compiled_details
            .worker
            .begin()
            .ok_or("Compiled detail worker is busy")?;
        self.compiled_details.serial = self.compiled_details.serial.wrapping_add(1);
        let token = self.compiled_details.serial;
        let identity = Arc::new(());
        self.compiled_details.ready = None;
        self.compiled_details.requested = false;
        self.compiled_details.status = "loading";
        self.compiled_details.error = None;
        self.compiled_details.displayed = false;
        self.compiled_details.lighting = Lighting::EditorShading;
        self.compiled_details.pending = Some(Pending {
            token,
            build,
            identity: identity.clone(),
            control: control.clone(),
        });
        self.scene_serial += 1;
        self.log(actor, "Prepare compiled details");
        Ok(Request {
            token,
            build,
            identity,
            control,
            output,
            worker,
        })
    }

    pub fn finish_compiled_details(
        &mut self,
        actor: Actor,
        prepared: Preparation,
    ) -> Result<Value, String> {
        let matches = self
            .compiled_details
            .pending
            .as_ref()
            .is_some_and(|pending| {
                pending.token == prepared.token
                    && pending.build == prepared.build
                    && Arc::ptr_eq(&pending.identity, &prepared.identity)
            });
        if !matches {
            return Err("Compiled detail result belongs to an expired request".into());
        }
        self.compiled_details.pending = None;
        let current = self
            .compiled
            .as_ref()
            .is_some_and(|build| build.lightmap_identity.token == prepared.build)
            && baked_build::build_state_controlled(self, SOURCE_MODE, &Control::default())
                .is_ok_and(|state| state["current"] == true);
        if prepared.control.cancelled() && self.compiled_details.status == "cancelling" {
            self.compiled_details.status = "cancelled";
            self.compiled_details.error = Some("Compiled detail preparation cancelled".into());
        } else if prepared.control.cancelled() || self.closing || !current {
            self.compiled_details.status = "stale";
            self.compiled_details.error =
                Some("The owned build or its sources changed during detail preparation".into());
        } else {
            match prepared.records {
                Ok(decoded) => {
                    self.compiled_details.status = "prepared";
                    self.compiled_details.ready = Some(Ready {
                        build: prepared.build,
                        records: decoded.records,
                        material: decoded.material,
                        ldr_style_records: decoded.ldr_style_records,
                        geometry: None,
                        asset_guard: None,
                        asset_bytes: 0,
                    });
                    self.compiled_details.error = None;
                }
                Err(error) => {
                    self.compiled_details.status = "unavailable";
                    self.compiled_details.error = Some(baked_build::bounded_message(&error));
                }
            }
        }
        self.scene_serial += 1;
        self.log(
            actor,
            format!("Compiled details {}", self.compiled_details.status),
        );
        self.inspect_compiled_details(None, 0, MAX_ROWS)
    }

    pub fn fail_compiled_details(
        &mut self,
        actor: Actor,
        token: u64,
        message: String,
    ) -> Result<Value, String> {
        self.check_detail_token(token)?;
        if let Some(pending) = self.compiled_details.pending.take() {
            let _ = pending.control.cancel();
        }
        self.compiled_details.status = "unavailable";
        self.compiled_details.error = Some(baked_build::bounded_message(&message));
        self.scene_serial += 1;
        self.log(actor, "Compiled detail worker could not start");
        self.inspect_compiled_details(None, 0, MAX_ROWS)
    }

    fn detail_token(&self) -> Option<String> {
        (self.compiled_details.serial != 0
            && !self.compiled_details.status.is_empty()
            && self.compiled_details.status != "not_loaded")
            .then(|| self.compiled_details.serial.to_string())
    }

    fn check_detail_token(&self, token: u64) -> Result<(), String> {
        if token != self.compiled_details.serial || self.detail_token().is_none() {
            return Err("Compiled detail token is expired; inspect again".into());
        }
        Ok(())
    }

    pub fn cancel_compiled_details(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.check_detail_token(token)?;
        let pending = self
            .compiled_details
            .pending
            .as_ref()
            .ok_or("No compiled detail preparation is active")?;
        pending.control.cancel()?;
        self.compiled_details.status = "cancelling";
        self.log(actor, "Cancel compiled detail preparation");
        self.inspect_compiled_details(None, 0, MAX_ROWS)
    }

    pub fn clear_compiled_details(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.check_detail_token(token)?;
        self.reset_compiled_details();
        self.log(actor, "Clear compiled details");
        self.inspect_compiled_details(None, 0, MAX_ROWS)
    }

    pub(super) fn reset_compiled_details(&mut self) {
        if let Some(pending) = self.compiled_details.pending.take() {
            let _ = pending.control.cancel();
        }
        self.compiled_details.ready = None;
        self.compiled_details.requested = false;
        self.compiled_details.displayed = false;
        self.compiled_details.status = "not_loaded";
        self.compiled_details.error = None;
        self.compiled_details.lighting = Lighting::EditorShading;
        self.compiled_details.serial = self.compiled_details.serial.wrapping_add(1);
        self.scene_serial += 1;
    }

    fn detail_context_ready(&self) -> bool {
        !self.face_mode
            && !self.viewport_pick_conflict()
            && !self.measurement_picking()
            && !self.linked_path_picking()
            && !self.paint_enabled()
            && self.component_selection().is_empty()
            && std::ptr::eq(self.render_document(), self.document())
            && self.block_preview.is_none()
    }

    fn detail_current(&self, control: &Control) -> Result<Freshness, String> {
        let mut build = baked_build::build_state_controlled(self, SOURCE_MODE, control)?;
        if self.compiled.is_none() {
            build["diagnostics"] = json!([
                "Compile the map successfully in this session before preparing final details"
            ]);
        }
        let mut preview_assets =
            json!({"status":"not_observed", "compiler_provenance":false,"diagnostics":[]});
        if let Some(ready) = &self.compiled_details.ready {
            if self
                .compiled
                .as_ref()
                .is_none_or(|build| build.lightmap_identity.token != ready.build)
            {
                build["current"] = json!(false);
                build["status"] = json!("unavailable");
            }
            if let Some(guard) = &ready.asset_guard {
                match guard.verify_original_metadata(control, MAX_METADATA_BYTES) {
                    Ok(()) => preview_assets["status"] = json!("current"),
                    Err(error) => {
                        preview_assets["status"] = json!("stale");
                        preview_assets["diagnostics"] = json!([baked_build::bounded_message(
                            &format!("Preview asset observations changed: {error}")
                        )]);
                    }
                }
            }
        }
        let current = build["current"] == true && preview_assets["status"] != "stale";
        Ok(Freshness {
            build,
            preview_assets,
            current,
        })
    }

    pub fn compiled_details_observation(&self) -> Value {
        json!({"build":self.compiled_world_light_observation(),"token":self.detail_token(),
            "status":if self.compiled_details.status.is_empty(){"not_loaded"}else{self.compiled_details.status},
            "worker_active":self.compiled_details.worker.busy(),"requested":self.compiled_details.requested,
            "displayed":self.compiled_details.displayed,"error":self.compiled_details.error,
            "lighting":self.compiled_details.lighting.name(),
            "complete":self.compiled_details.ready.as_ref().is_some_and(|ready|ready.geometry.is_some()),
            "context_ready":self.detail_context_ready(),"working_region":{"enabled":self.working_region.enabled,"bounds":self.working_region.bounds.map(|bounds|bounds.map(|point|point.to_array()))},"total":self.compiled_details.ready.as_ref().map(|ready|ready.records.props.len())})
    }

    pub fn preflight_compiled_details(&mut self) {
        if self.compiled_details.pending.is_none() && !self.compiled_details.requested {
            return;
        }
        self.preflight_compiled_details_ownership();
    }

    fn preflight_compiled_details_ownership(&mut self) {
        if self.compiled_details.pending.is_none() && self.compiled_details.ready.is_none() {
            return;
        }
        let state = self.detail_current(&Control::default());
        let current = state.as_ref().is_ok_and(|state| state.current);
        if !current && let Some(pending) = &self.compiled_details.pending {
            let _ = pending.control.cancel();
        }
        if state
            .as_ref()
            .is_ok_and(|state| state.build["status"] == "unavailable")
        {
            self.compiled_details.ready = None;
            self.compiled_details.status = "unavailable";
            self.compiled_details.error = Some(
                "Compiled output changed or is no longer owned; prepare again after compiling"
                    .into(),
            );
        }
        let displayed = current
            && self.detail_context_ready()
            && self.compiled_details.requested
            && self
                .compiled_details
                .ready
                .as_ref()
                .is_some_and(|ready| ready.geometry.is_some());
        if displayed != self.compiled_details.displayed {
            self.compiled_details.displayed = displayed;
            self.scene_serial += 1;
        }
    }

    pub fn inspect_compiled_details(
        &mut self,
        token: Option<u64>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        if !(1..=MAX_ROWS).contains(&limit) || offset.checked_add(limit).is_none() {
            return Err("Compiled detail page must have 1–64 rows without overflow".into());
        }
        if let Some(token) = token {
            self.check_detail_token(token)?;
        } else if offset != 0 {
            return Err("Retained detail pages require the returned preparation token".into());
        }
        self.preflight_compiled_details_ownership();
        let state = self.detail_current(&Control::default())?;
        let ready = self.compiled_details.ready.as_ref();
        let total = ready.map(|ready| ready.records.props.len());
        if total.is_some_and(|total| offset > total) || total.is_none() && offset != 0 {
            return Err("Compiled detail page offset exceeds its retained table".into());
        }
        let rows = ready.map(|ready| ready.records.props.iter().enumerate().skip(offset).take(limit).map(|(ordinal,prop)| json!({
            "ordinal":ordinal,"kind":prop.kind,"kind_name":match prop.kind {0=>"model",1=>"card",2=>"cross",3=>"tri",_=>"unknown"},
            "dictionary_index":prop.model,"model":if prop.kind == 0 {ready.records.model_name(usize::from(prop.model)).ok()} else {None},
            "leaf":prop.leaf,"origin":prop.origin,"angles":prop.angles,"scale":if prop.kind == 0 {1.0} else {prop.scale},
            "orientation":prop.orientation,"sway":prop.sway,"shape_angle":prop.shape_angle,"shape_size":prop.shape_size,
            "lighting":{"rgb":prop.lighting.rgb,"exponent":prop.lighting.exponent,"linear":render::compiled_details::baked_ldr_linear(prop.lighting)},"light_styles":prop.light_styles,"light_style_count":prop.light_style_count,
            "source_association":null,
            "bounds":ready.geometry.as_ref().and_then(|mesh|mesh.detail_parts.iter().find(|part|part.ordinal==ordinal)).map(|part|part.bounds.map(|point|point.to_array())),
            "bounds_policy":if prop.kind==1 && prop.orientation!=0 {"conservative_camera_rotation_envelope"}else{"posed_geometry"}
        })).collect::<Vec<_>>()).unwrap_or_default();
        let status = if ready.is_some() && !state.current {
            "stale"
        } else if self.compiled_details.requested
            && ready.is_some_and(|ready| ready.geometry.is_some())
            && !self.detail_context_ready()
        {
            "context_suppressed"
        } else if self.compiled_details.status.is_empty() {
            "not_loaded"
        } else {
            self.compiled_details.status
        };
        let mut report = json!({"status":status,"token":self.detail_token(),
            "build":state.build,"preview_assets":state.preview_assets,"current":state.current,"requested":self.compiled_details.requested,"displayed":self.compiled_details.displayed,"worker_active":self.compiled_details.worker.busy(),
            "complete":ready.is_some_and(|ready|ready.geometry.is_some()),"error":self.compiled_details.error,
            "lighting":{"mode":self.compiled_details.lighting.name(),"baked_ldr_available":ready.map(|ready|render::compiled_details::baked_ldr_issue(&ready.records).is_none()),"unavailable_reason":ready.and_then(|ready|render::compiled_details::baked_ldr_issue(&ready.records)),"ldr_style_records":ready.and_then(|ready|ready.ldr_style_records),"display_policy":"native base linear RGB / 255; clamp 0–1; round to 8-bit; editor sRGB output; style 0 fixed at unity; no editor lighting; native gamma parity unqualified"},
            "suppression_reason":if self.detail_context_ready(){None}else{Some("Use Objects mode and finish editing tools/drafts to show the compiled detail layer")},
            "page":{"offset":offset,"limit":limit,"total":total,"returned":rows.len(),"next_offset":total.filter(|total|offset+rows.len()<*total).map(|_|offset+rows.len()),"rows":rows},
            "sprite_material":ready.map(|ready|ready.material.as_str()),"counts":ready.map(|ready|{
                let mut counts=[0usize;4]; for prop in &ready.records.props { counts[usize::from(prop.kind)]+=1; }
                json!({"models":counts[0],"cards":counts[1],"cross":counts[2],"tri":counts[3]})}),
            "geometry":ready.and_then(|ready|ready.geometry.as_ref()).map(|mesh|json!({"vertices":mesh.vertices.len(),"indices":mesh.indices.len(),"wire_vertices":mesh.wire_lines.len()+mesh.detail_billboards.len()*8,"billboard_cards":mesh.detail_billboards.len(),"retained_bytes":mesh.compiled_detail_bytes()})),
            "preview_resources_bytes":ready.map(|ready|ready.asset_bytes),"notes":NOTES,
            "view_policy":{"working_region":"whole_compiled_prop_bounds; camera-facing cards use a conservative rotation envelope","authored_editor_hiding":"independent; native records have no authored owner","eligible_records":ready.and_then(|ready|ready.geometry.as_ref()).map(|mesh|mesh.detail_parts.len()),"region_records":ready.and_then(|ready|ready.geometry.as_ref()).map(|mesh|mesh.detail_parts.iter().filter(|part|!self.working_region.enabled || self.working_region.bounds.is_none_or(|region|part.intersects(region))).count())},
            "limits":{"bsp_bytes":baked_build::MAX_BSP_BYTES,"decoded_bsp_bytes":baked_build::MAX_BSP_BYTES,"rows":MAX_ROWS,"props":source_formats::bsp::MAX_DETAIL_PROPS,"geometry_bytes":render::compiled_details::MAX_BYTES,"preview_source_bytes":assets::MAX_ASSET_BYTES,"model_asset_bytes":assets::MAX_ASSET_BYTES,"texture_payload_bytes":assets::MAX_ASSET_BYTES,"metadata_bytes":MAX_METADATA_BYTES,"active_workers":1}});
        let observed = self.detail_current(&Control::default())?;
        if observed.build["status"] == "unavailable" {
            self.preflight_compiled_details_ownership();
            report["status"] = json!(if self.compiled_details.status.is_empty()
                || self.compiled_details.status == "not_loaded"
            {
                "not_loaded"
            } else {
                "unavailable"
            });
            report["view_policy"]["eligible_records"] = Value::Null;
            report["view_policy"]["region_records"] = Value::Null;
            report["complete"] = json!(false);
            report["displayed"] = json!(false);
            report["lighting"]["baked_ldr_available"] = Value::Null;
            report["lighting"]["unavailable_reason"] = Value::Null;
            report["lighting"]["ldr_style_records"] = Value::Null;
            report["page"] = json!({"offset":offset,"limit":limit,"total":null,"returned":0,"next_offset":null,"rows":[]});
            for field in [
                "counts",
                "geometry",
                "sprite_material",
                "preview_resources_bytes",
            ] {
                report[field] = Value::Null;
            }
        } else if !observed.current && self.compiled_details.ready.is_some() {
            if self.compiled_details.displayed {
                self.compiled_details.displayed = false;
                self.scene_serial += 1;
            }
            report["status"] = json!("stale");
            report["displayed"] = json!(false);
        }
        report["build"] = observed.build;
        report["preview_assets"] = observed.preview_assets;
        report["current"] = json!(observed.current);
        Ok(report)
    }

    pub fn set_compiled_details_visible(
        &mut self,
        actor: Actor,
        token: u64,
        visible: bool,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.check_detail_token(token)?;
        if !visible {
            self.compiled_details.requested = false;
            self.compiled_details.displayed = false;
            self.scene_serial += 1;
            return self.inspect_compiled_details(None, 0, MAX_ROWS);
        }
        self.inspection_viewport_ready("showing compiled details")?;
        if !self.detail_context_ready() {
            return Err("Show compiled details requires Objects mode with no active editing tool or geometry draft".into());
        }
        let control = Control::default();
        if !self.detail_current(&control)?.current {
            return Err("Show compiled details requires the current owned build and unchanged preview assets".into());
        }
        let mut ready = self
            .compiled_details
            .ready
            .take()
            .ok_or("Prepare compiled details first")?;
        let result = (|| {
            let build = self
                .compiled
                .as_ref()
                .ok_or("The successful build is no longer retained")?;
            let output = build
                .lightmap_identity
                .output
                .as_ref()
                .map_err(Clone::clone)?;
            output.verify(&control).map_err(|error| error.to_string())?;
            if ready.geometry.is_none() {
                let (mut mesh, guard, bytes) =
                    self.prepare_detail_assets(&ready.records, &ready.material, &control)?;
                mesh.set_compiled_detail_lighting(&ready.records, self.compiled_details.lighting)?;
                ready.geometry = Some(mesh);
                ready.asset_guard = Some(guard);
                ready.asset_bytes = bytes;
            }
            let build = self
                .compiled
                .as_ref()
                .ok_or("The successful build is no longer retained")?;
            if let Some(error) = baked_build::source_issue_with_mode(build, SOURCE_MODE, &control) {
                return Err(error);
            }
            build
                .lightmap_identity
                .output
                .as_ref()
                .map_err(Clone::clone)?
                .verify(&control)
                .map_err(|error| error.to_string())?;
            if let Some(guard) = &ready.asset_guard {
                guard
                    .verify_original_metadata(&control, MAX_METADATA_BYTES)
                    .map_err(|error| error.to_string())?;
            }
            control.checkpoint()
        })();
        self.compiled_details.ready = Some(ready);
        if let Err(error) = result {
            self.compiled_details.requested = false;
            self.compiled_details.displayed = false;
            self.compiled_details.status = "unavailable";
            self.compiled_details.error = Some(baked_build::bounded_message(&error));
            if let Some(ready) = &mut self.compiled_details.ready {
                ready.geometry = None;
                ready.asset_guard = None;
            }
            self.scene_serial += 1;
            return Err(error);
        }
        self.compiled_details.requested = true;
        self.compiled_details.displayed = true;
        self.compiled_details.status = "ready";
        self.compiled_details.error = None;
        self.scene_serial += 1;
        self.log(actor, "Show compiled details");
        self.inspect_compiled_details(None, 0, MAX_ROWS)
    }

    pub fn set_compiled_details_lighting(
        &mut self,
        actor: Actor,
        token: u64,
        lighting: &str,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.check_detail_token(token)?;
        let lighting = Lighting::parse(lighting)?;
        if self.compiled_details.worker.busy() {
            return Err(
                "Wait for the compiled detail worker before changing preview lighting".into(),
            );
        }
        let ready = self
            .compiled_details
            .ready
            .as_ref()
            .ok_or("Prepare compiled details first")?;
        if lighting == Lighting::BakedLdr
            && let Some(issue) = render::compiled_details::baked_ldr_issue(&ready.records)
        {
            return Err(issue);
        }
        let control = Control::default();
        if !self.detail_current(&control)?.current {
            return Err("Compiled detail lighting requires the current owned build and unchanged preview assets".into());
        }
        let build = self
            .compiled
            .as_ref()
            .ok_or("The successful build is no longer retained")?;
        build
            .lightmap_identity
            .output
            .as_ref()
            .map_err(Clone::clone)?
            .verify(&control)
            .map_err(|error| error.to_string())?;
        if let Some(error) = baked_build::source_issue_with_mode(build, SOURCE_MODE, &control) {
            return Err(error);
        }
        let ready = self
            .compiled_details
            .ready
            .as_mut()
            .ok_or("Prepare compiled details first")?;
        if let Some(mesh) = &mut ready.geometry {
            mesh.set_compiled_detail_lighting(&ready.records, lighting)?;
        }
        self.compiled_details.lighting = lighting;
        self.scene_serial += 1;
        self.log(
            actor,
            format!("Compiled detail preview lighting {}", lighting.name()),
        );
        self.inspect_compiled_details(None, 0, MAX_ROWS)
    }

    pub(super) fn append_compiled_details(&mut self, mesh: &mut SceneMesh) {
        self.preflight_compiled_details();
        if !self.compiled_details.displayed {
            return;
        }
        if let Some(details) = self
            .compiled_details
            .ready
            .as_ref()
            .and_then(|ready| ready.geometry.as_ref())
            && let Err(error) = mesh.append_compiled_details(
                details,
                self.working_region
                    .enabled
                    .then_some(self.working_region.bounds)
                    .flatten(),
            )
        {
            self.compiled_details.requested = false;
            self.compiled_details.displayed = false;
            self.compiled_details.status = "unavailable";
            self.compiled_details.error = Some(baked_build::bounded_message(&error));
            if let Some(ready) = &mut self.compiled_details.ready {
                ready.geometry = None;
                ready.asset_guard = None;
            }
            self.scene_serial += 1;
        }
    }
}
