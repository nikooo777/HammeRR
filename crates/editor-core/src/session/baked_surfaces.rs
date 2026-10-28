use std::sync::Arc;

use compiler::Control;
use render::{
    LightingChannel, SceneMesh, ViewOptions,
    compiled_faces::{LightingData, LightingSettings, Scene},
};
use serde_json::{Value, json};
use source_formats::{bsp::Bsp, gamefs::GameFs};

use super::{
    Actor, Session, baked_build, inspection_framing,
    worker::{Slot, Ticket},
};

mod assets;

pub const MAX_ROWS: usize = 64;
pub const MAX_METADATA_BYTES: usize = 32 * 1024 * 1024;
const SOURCE_MODE: baked_build::SourceVerification =
    baked_build::SourceVerification::OriginalMetadata {
        maximum_bytes: MAX_METADATA_BYTES,
    };
const DISPLAY_POLICY: &str = "Selected final BSP FACE and LIGHTING channel, without substitution. Stored style 0 defaults to unity; other omitted stored IDs default to zero. Linear RGBExp32/255 maps use face-isolated bilinear sampling at native luxel centers and static material directional reconstruction. Resolved compiled diffuse/blend and static tint multiply linear lighting times 2^EV, then one Reinhard transform and one sRGB output. No editor lighting or native appearance/adaptation parity.";
const VIEW_POLICY: &str = "The compiled surface layer replaces authored brush and terrain draw surfaces. Working regions include whole final faces whose initial-pose world AABB intersects the region, without clipping triangles. Authored hiding is independent of this immutable compiled layer; no contributor-derived surface subsets are inferred. Authored selection outlines, picking, entities and helpers remain authored; native FACE ordinals are not authored identities.";

struct Pending {
    token: u64,
    build: Arc<baked_build::RetainedBuild>,
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
    options: LightingSettings,
}

struct Ready {
    build: Arc<baked_build::RetainedBuild>,
    scene: Scene,
    assets: assets::Prepared,
    lighting: Option<Arc<LightingData>>,
}

impl Ready {
    fn lighting_capacity(&self, options: &LightingSettings) -> Result<(), String> {
        self.scene.validate_settings(options)?;
        let old = self
            .lighting
            .as_ref()
            .map(|lighting| lighting.cpu_byte_len())
            .transpose()?
            .unwrap_or(0);
        let new = self.scene.lighting_cpu_bytes(options.channel)?;
        let bytes = self
            .scene
            .retained_bytes()
            .checked_add(self.assets.bytes)
            .and_then(|bytes| bytes.checked_add(old))
            .and_then(|bytes| bytes.checked_add(new));
        if bytes.is_none_or(|bytes| bytes > render::compiled_faces::MAX_CPU_BYTES) {
            return Err(
                "Atomic baked surface lighting replacement exceeds the 256 MiB CPU working budget"
                    .into(),
            );
        }
        Ok(())
    }
}

pub struct Request {
    token: u64,
    build: Arc<baked_build::RetainedBuild>,
    game: Arc<GameFs>,
    control: Control,
    worker: Ticket,
}

pub struct Preparation {
    token: u64,
    build: Arc<baked_build::RetainedBuild>,
    control: Control,
    decoded: Result<Decoded, String>,
    _worker: Ticket,
}

struct Decoded {
    scene: Scene,
    assets: assets::Prepared,
    lighting: Option<Arc<LightingData>>,
}

pub struct RenderGuard {
    token: u64,
    build: Arc<baked_build::RetainedBuild>,
    assets: compiler::asset_sources::AssetSourceGuard,
    options: LightingSettings,
    scene_key: render::SceneKey,
}

impl Request {
    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn run(self) -> Preparation {
        let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            decode(&self.build, &self.game, &self.control)
        }))
        .unwrap_or_else(|_| Err("Baked surface preparation panicked".into()));
        Preparation {
            token: self.token,
            build: self.build,
            control: self.control,
            decoded,
            _worker: self.worker,
        }
    }
}

fn decode(
    build: &baked_build::RetainedBuild,
    game: &GameFs,
    control: &Control,
) -> Result<Decoded, String> {
    build.verify(control, MAX_METADATA_BYTES)?;
    let bytes = baked_build::read_owned_bytes_controlled(&build.output, control)?;
    baked_build::decoded_bsp_bound(&bytes)?;
    let bsp = Bsp::parse(&bytes).map_err(|error| error.to_string())?;
    drop(bytes);
    for kind in [
        source_formats::bsp::LightingKind::Ldr,
        source_formats::bsp::LightingKind::Hdr,
    ] {
        if !bsp
            .lighting_faces(kind)
            .map_err(|error| error.to_string())?
            .is_empty()
        {
            build.origins.validate_channel(&bsp, kind)?;
        }
        control.checkpoint()?;
    }
    let transforms = compiler::initial_brush_model_transforms_controlled(&bsp, &mut || {
        control.checkpoint().map_err(std::io::Error::other)
    })?;
    let scene = Scene::decode_controlled(&bsp, &transforms, &mut || {
        control.checkpoint().map_err(std::io::Error::other)
    })?;
    let lighting_reserve = scene
        .available_channels()
        .into_iter()
        .map(|channel| scene.lighting_cpu_bytes(channel))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .max()
        .unwrap_or(0);
    let remaining = render::compiled_faces::MAX_CPU_BYTES
        .checked_sub(scene.retained_bytes())
        .and_then(|bytes| bytes.checked_sub(lighting_reserve))
        .ok_or("Baked surface geometry exceeds the CPU working budget")?;
    let assets = assets::prepare(game, &bsp, scene.materials(), remaining, control)?;
    let lighting = if scene.available_channels().contains(&LightingChannel::Ldr) {
        Some(scene.lighting(&LightingSettings::default())?)
    } else {
        None
    };
    let lighting_bytes = lighting
        .as_ref()
        .map(|lighting| lighting.cpu_byte_len())
        .transpose()?
        .unwrap_or(0);
    if scene
        .retained_bytes()
        .checked_add(assets.bytes)
        .and_then(|bytes| bytes.checked_add(lighting_bytes))
        .is_none_or(|bytes| bytes > render::compiled_faces::MAX_CPU_BYTES)
    {
        return Err("Baked surface retained geometry, materials and lighting exceed the 256 MiB CPU working budget".into());
    }
    build.verify(control, MAX_METADATA_BYTES)?;
    assets.verify(control)?;
    Ok(Decoded {
        scene,
        assets,
        lighting,
    })
}

impl Session {
    pub fn begin_baked_surfaces(&mut self, actor: Actor) -> Result<Request, String> {
        self.ensure_not_closing()?;
        if self.baked_surfaces.worker.busy() {
            return Err("Wait for the baked surface worker to finish".into());
        }
        let control = Control::default();
        let state = baked_build::build_state_controlled(self, SOURCE_MODE, &control)?;
        if state["current"] != true {
            return Err(
                "Baked surfaces require a current successful owned build; compile again".into(),
            );
        }
        let build = baked_build::RetainedBuild::capture(
            self.compiled
                .as_ref()
                .ok_or("Compile the map before preparing baked surfaces")?,
        )?;
        build.verify(&control, MAX_METADATA_BYTES)?;
        let worker = self
            .baked_surfaces
            .worker
            .begin()
            .ok_or("Baked surface worker is busy")?;
        self.reset_baked_surfaces();
        let token = self.baked_surfaces.serial;
        self.baked_surfaces.status = "loading";
        self.baked_surfaces.pending = Some(Pending {
            token,
            build: build.clone(),
            control: control.clone(),
        });
        self.log(actor, "Prepare baked surfaces");
        Ok(Request {
            token,
            build,
            game: self.game.clone(),
            control,
            worker,
        })
    }

    pub fn finish_baked_surfaces(
        &mut self,
        actor: Actor,
        prepared: Preparation,
    ) -> Result<Value, String> {
        let matches = self.baked_surfaces.pending.as_ref().is_some_and(|pending| {
            pending.token == prepared.token && Arc::ptr_eq(&pending.build, &prepared.build)
        });
        if !matches {
            return Err("Baked surface result belongs to an expired request".into());
        }
        let cancelled = self.baked_surfaces.status == "cancelling" && prepared.control.cancelled();
        self.baked_surfaces.pending = None;
        let current = !self.closing
            && self
                .compiled
                .as_ref()
                .is_some_and(|build| build.lightmap_identity.token == prepared.build.token)
            && baked_build::build_state_controlled(self, SOURCE_MODE, &Control::default())
                .is_ok_and(|state| state["current"] == true);
        let result = if cancelled {
            Err((
                "cancelled",
                "Baked surface preparation cancelled".to_owned(),
            ))
        } else if prepared.control.cancelled() || !current {
            Err((
                "stale",
                "The owned build or sources changed during baked surface preparation".to_owned(),
            ))
        } else {
            prepared
                .decoded
                .and_then(|decoded| {
                    prepared
                        .build
                        .verify(&prepared.control, MAX_METADATA_BYTES)?;
                    decoded.assets.verify(&prepared.control)?;
                    Ok(decoded)
                })
                .map_err(|error| ("unavailable", error))
        };
        match result {
            Ok(decoded) => {
                self.baked_surfaces.ready = Some(Ready {
                    build: prepared.build,
                    scene: decoded.scene,
                    assets: decoded.assets,
                    lighting: decoded.lighting,
                });
                self.baked_surfaces.status = "prepared";
                self.baked_surfaces.error = None;
            }
            Err((status, error)) => {
                self.baked_surfaces.status = status;
                self.baked_surfaces.error = Some(baked_build::bounded_message(&error));
            }
        }
        self.scene_serial += 1;
        self.log(
            actor,
            format!("Baked surfaces {}", self.baked_surfaces.status),
        );
        self.inspect_baked_surfaces(None, 0, MAX_ROWS)
    }

    fn baked_surface_token(&self) -> Option<String> {
        (!self.baked_surfaces.status.is_empty() && self.baked_surfaces.status != "not_loaded")
            .then(|| self.baked_surfaces.serial.to_string())
    }

    fn check_baked_surface_token(&self, token: u64) -> Result<(), String> {
        if token != self.baked_surfaces.serial || self.baked_surface_token().is_none() {
            return Err("Baked surface token is expired; inspect again".into());
        }
        Ok(())
    }

    pub fn fail_baked_surfaces(
        &mut self,
        actor: Actor,
        token: u64,
        message: String,
    ) -> Result<Value, String> {
        self.check_baked_surface_token(token)?;
        let pending = self
            .baked_surfaces
            .pending
            .take()
            .ok_or("No baked surface preparation is active")?;
        let _ = pending.control.cancel();
        self.baked_surfaces.status = "unavailable";
        self.baked_surfaces.error = Some(baked_build::bounded_message(&message));
        self.scene_serial += 1;
        self.log(actor, "Baked surface worker could not start");
        self.inspect_baked_surfaces(None, 0, MAX_ROWS)
    }

    pub fn cancel_baked_surfaces(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.check_baked_surface_token(token)?;
        self.baked_surfaces
            .pending
            .as_ref()
            .ok_or("No baked surface preparation is active")?
            .control
            .cancel()?;
        self.baked_surfaces.status = "cancelling";
        self.log(actor, "Cancel baked surface preparation");
        self.inspect_baked_surfaces(None, 0, MAX_ROWS)
    }

    pub fn clear_baked_surfaces(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.check_baked_surface_token(token)?;
        self.reset_baked_surfaces();
        self.log(actor, "Clear baked surfaces");
        self.inspect_baked_surfaces(None, 0, MAX_ROWS)
    }

    pub(super) fn reset_baked_surfaces(&mut self) {
        if let Some(pending) = self.baked_surfaces.pending.take() {
            let _ = pending.control.cancel();
        }
        self.baked_surfaces.serial = self.baked_surfaces.serial.wrapping_add(1);
        self.baked_surfaces.ready = None;
        self.baked_surfaces.requested = false;
        self.baked_surfaces.displayed = false;
        self.baked_surfaces.options = LightingSettings::default();
        self.baked_surfaces.status = "not_loaded";
        self.baked_surfaces.error = None;
        self.scene_serial += 1;
    }

    fn baked_surface_context_ready(&self) -> bool {
        !self.face_mode
            && !self.viewport_pick_conflict()
            && !self.measurement_picking()
            && !self.linked_path_picking()
            && !self.paint_enabled()
            && self.component_selection().is_empty()
            && std::ptr::eq(self.render_document(), self.document())
            && self.block_preview.is_none()
            && self.property_draft().is_none()
            && self.ensure_partitions_resolved().is_ok()
    }

    fn baked_surface_current(&self, control: &Control) -> Result<(Value, Value, bool), String> {
        let mut build = baked_build::build_state_controlled(self, SOURCE_MODE, control)?;
        let mut assets =
            json!({"status":"not_observed","compiler_provenance":false,"diagnostics":[]});
        if let Some(ready) = &self.baked_surfaces.ready {
            if build["token"].as_str() != Some(ready.build.token.to_string().as_str()) {
                build["current"] = json!(false);
                build["status"] = json!("unavailable");
            }
            match ready.assets.verify(control) {
                Ok(()) => assets["status"] = json!("current"),
                Err(error) => {
                    assets["status"] = json!("stale");
                    assets["diagnostics"] = json!([baked_build::bounded_message(&error)]);
                }
            }
        }
        let current = build["current"] == true && assets["status"] != "stale";
        Ok((build, assets, current))
    }

    pub fn preflight_baked_surfaces(&mut self) {
        if self.baked_surfaces.pending.is_none() && self.baked_surfaces.ready.is_none() {
            return;
        }
        let observed = self.baked_surface_current(&Control::default());
        let current = observed.as_ref().is_ok_and(|(_, _, current)| *current);
        if !current {
            if let Some(pending) = &self.baked_surfaces.pending {
                let _ = pending.control.cancel();
            }
            self.baked_surfaces.requested = false;
            if observed
                .as_ref()
                .is_ok_and(|(build, _, _)| build["status"] == "unavailable")
            {
                self.baked_surfaces.ready = None;
                self.baked_surfaces.status = "unavailable";
                self.baked_surfaces.error = Some(
                    "The compiled output changed or is no longer owned; compile and prepare again"
                        .into(),
                );
            }
        }
        let displayed = current
            && self.baked_surface_context_ready()
            && self.baked_surfaces.requested
            && self
                .baked_surfaces
                .ready
                .as_ref()
                .is_some_and(|ready| ready.lighting.is_some());
        if displayed != self.baked_surfaces.displayed {
            self.baked_surfaces.displayed = displayed;
            self.scene_serial += 1;
        }
    }

    pub fn baked_surfaces_observation(&self) -> Value {
        json!({"token":self.baked_surface_token(),"status":if self.baked_surfaces.status.is_empty(){"not_loaded"}else{self.baked_surfaces.status},
            "worker_active":self.baked_surfaces.worker.busy(),"requested":self.baked_surfaces.requested,"displayed":self.baked_surfaces.displayed,
            "complete":self.baked_surfaces.ready.is_some(),"error":self.baked_surfaces.error,"lighting_options":self.baked_surfaces.options,
            "context_ready":self.baked_surface_context_ready(),"build":self.compiled_world_light_observation()})
    }

    pub fn baked_surfaces_options_report(&self) -> Value {
        let ready = self.baked_surfaces.ready.as_ref();
        let options = &self.baked_surfaces.options;
        json!({"settings":options,"channel":options.channel,"exposure_ev":options.exposure_ev,"style_gains":options.style_gains,
            "available_styles":ready.and_then(|ready|ready.scene.available_styles(options.channel).ok()),
            "effective_style_gains":ready.and_then(|ready|ready.lighting.as_ref()).map(|lighting|&lighting.effective_style_gains),
            "channels":([LightingChannel::Ldr,LightingChannel::Hdr].map(|channel|json!({"channel":channel,"available":ready.is_some_and(|ready|ready.scene.available_styles(channel).is_ok())}))),
            "available":ready.is_some_and(|ready|ready.lighting.is_some()),"unavailable_reason":ready.and_then(|ready|ready.scene.validate_settings(options).err()),
            "reconstruction":"auto_material_directional","sampling":"face_isolated_bilinear_native_luxel_centers",
            "material":render::compiled_faces::materials::POLICY,
            "style_zero_policy":"implicit_unity_unless_explicit_gain","display_policy":DISPLAY_POLICY})
    }

    pub fn baked_surfaces_render_report(&self, view: ViewOptions) -> Value {
        let surfaces = self.baked_surfaces.ready.as_ref().and_then(|ready| {
            ready
                .scene
                .surfaces(self.baked_surfaces.options.channel)
                .ok()
        });
        json!({"state":self.baked_surfaces_observation(),"lighting":self.baked_surfaces_options_report(),"applied":self.baked_surfaces_visible(),"surface_view":view.surface.name(),
            "surface_count":surfaces.map(<[_]>::len),"visible_surface_count":surfaces.map(|surfaces|surfaces.iter().filter(|surface|self.baked_surface_in_region(surface)).count()),
            "view_policy":VIEW_POLICY})
    }

    pub fn inspect_baked_surfaces(
        &mut self,
        token: Option<u64>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
        if !(1..=MAX_ROWS).contains(&limit) || offset.checked_add(limit).is_none() {
            return Err("Baked surface page must have 1–64 rows without overflow".into());
        }
        if let Some(token) = token {
            self.check_baked_surface_token(token)?;
        } else if offset != 0 {
            return Err("Retained surface pages require the returned preparation token".into());
        }
        self.preflight_baked_surfaces();
        let (build, assets, current) = self.baked_surface_current(&Control::default())?;
        let ready = self.baked_surfaces.ready.as_ref();
        let surfaces = ready.and_then(|ready| {
            ready
                .scene
                .surfaces(self.baked_surfaces.options.channel)
                .ok()
        });
        let total = surfaces.map(<[_]>::len);
        if offset > total.unwrap_or(0) {
            return Err(
                "Baked surface page offset exceeds the selected retained face table".into(),
            );
        }
        let rows = surfaces.unwrap_or_default().iter().skip(offset).take(limit).map(|surface| {
            let material = ready.and_then(|ready|ready.scene.materials().get(surface.material));
            let mut origins = [0usize;4];
            if let Some(contributors) = ready.and_then(|ready|ready.build.origins.contributors(surface.ordinal)) {
                for origin in contributors {
                    let index = match origin {
                        compiler::face_origins::Origin::Root{..} => 0,
                        compiler::face_origins::Origin::Instance{..} => 1,
                        compiler::face_origins::Origin::GeneratedRegion{..} => 2,
                        compiler::face_origins::Origin::Unresolved{..} => 3,
                    };
                    origins[index]+=1;
                }
            }
            json!({"face":surface.ordinal,"model":surface.model,"material":material.map(|material|material.name.as_str()),
                "receiver":if surface.terrain{"terrain"}else if surface.model==0{"world_detail"}else{"brush_model"},
                "bounds":[surface.mins,surface.maxs],"vertices":surface.vertices,"triangles":surface.triangles,
                "lightmap_dimensions":surface.dimensions,"maps":surface.maps,"styles":surface.styles,"frame_available":current,
                "missing_surface_samples":surface.missing_surface_samples,"in_working_region":self.baked_surface_in_region(surface),
                "authored_identity":null,"source_contributors":{"root":origins[0],"instance":origins[1],"generated_region":origins[2],"unresolved":origins[3]},"source_policy":"many_to_many_contributors_without_authored_footprint_or_vertex_correspondence"})
        }).collect::<Vec<_>>();
        let status = if ready.is_some() && !current {
            "stale"
        } else if self.baked_surfaces.requested && !self.baked_surface_context_ready() {
            "context_suppressed"
        } else if self.baked_surfaces.status.is_empty() {
            "not_loaded"
        } else {
            self.baked_surfaces.status
        };
        let mut report = json!({"status":status,"token":self.baked_surface_token(),"build":build,"preview_assets":assets,"current":current,
            "requested":self.baked_surfaces.requested,"displayed":self.baked_surfaces.displayed,"worker_active":self.baked_surfaces.worker.busy(),
            "complete":ready.is_some(),"error":self.baked_surfaces.error,"lighting_options":self.baked_surfaces.options,"lighting":self.baked_surfaces_options_report(),
            "prepare_available":build["current"]==true && !self.baked_surfaces.worker.busy(),"prepare_unavailable_reason":if self.baked_surfaces.worker.busy(){Some("Wait for the baked surface worker")}else if build["current"]!=true{Some("Compile a current successful owned build first")}else{None},
            "suppression_reason":if self.baked_surface_context_ready(){None}else{Some("Use Objects mode and finish editing tools/drafts")},
            "page":{"offset":offset,"limit":limit,"total":total,"returned":rows.len(),"next_offset":total.filter(|total|offset+rows.len()<*total).map(|_|offset+rows.len()),"rows":rows},
            "counts":surfaces.map(|surfaces|json!({"faces":surfaces.len(),"world_detail":surfaces.iter().filter(|surface|surface.model==0&&!surface.terrain).count(),"terrain":surfaces.iter().filter(|surface|surface.terrain).count(),"brush_model":surfaces.iter().filter(|surface|surface.model!=0&&!surface.terrain).count()})),
            "retained_geometry_bytes":ready.map(|ready|ready.scene.retained_bytes()),"preview_resources_bytes":ready.map(|ready|ready.assets.bytes),
            "region_faces":surfaces.map(|surfaces|surfaces.iter().filter(|surface|self.baked_surface_in_region(surface)).count()),
            "view_policy":VIEW_POLICY,"working_region":self.working_region_summary(),
            "limits":{"rows":MAX_ROWS,"bsp_bytes":baked_build::MAX_BSP_BYTES,"metadata_bytes":MAX_METADATA_BYTES,"preview_source_bytes":assets::MAX_ASSET_BYTES,"decoded_texture_bytes":assets::MAX_DECODED_BYTES,"asset_file_bytes":assets::MAX_ASSET_FILE_BYTES,"cpu_scene_bytes":render::compiled_faces::MAX_CPU_BYTES,"gpu_scene_bytes":render::compiled_faces::MAX_GPU_BYTES,"active_workers":1}});
        let (build, assets, current) = self.baked_surface_current(&Control::default())?;
        if !current {
            self.preflight_baked_surfaces();
            report["displayed"] = json!(false);
            report["status"] = json!(if self.baked_surfaces.ready.is_some() {
                "stale"
            } else if self.baked_surfaces.status.is_empty() {
                "not_loaded"
            } else {
                self.baked_surfaces.status
            });
            if self.baked_surfaces.ready.is_none() {
                report["complete"] = json!(false);
                report["page"] = json!({"offset":offset,"limit":limit,"total":null,"returned":0,"next_offset":null,"rows":[]});
                for field in [
                    "counts",
                    "retained_geometry_bytes",
                    "preview_resources_bytes",
                ] {
                    report[field] = Value::Null;
                }
            }
        }
        report["build"] = build;
        report["preview_assets"] = assets;
        report["current"] = json!(current);
        Ok(report)
    }

    pub fn set_baked_surfaces_options(
        &mut self,
        actor: Actor,
        token: u64,
        mut options: LightingSettings,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.check_baked_surface_token(token)?;
        options.validate()?;
        options.style_gains.sort_by_key(|gain| gain.style);
        if self.baked_surfaces.worker.busy() {
            return Err("Wait for the baked surface worker before changing lighting".into());
        }
        let ready = self
            .baked_surfaces
            .ready
            .as_ref()
            .ok_or("Prepare baked surfaces first")?;
        ready.lighting_capacity(&options)?;
        let lighting = ready.scene.lighting(&options)?;
        let control = Control::default();
        if !self.baked_surface_current(&control)?.2 {
            return Err("Baked surface settings require the current owned build and unchanged preview assets".into());
        }
        ready.build.verify(&control, MAX_METADATA_BYTES)?;
        ready.assets.verify(&control)?;
        let ready = self
            .baked_surfaces
            .ready
            .as_mut()
            .ok_or("Prepare baked surfaces first")?;
        ready.lighting = Some(lighting);
        self.baked_surfaces.options = options;
        self.scene_serial += 1;
        self.log(actor, "Set baked surface lighting");
        self.inspect_baked_surfaces(None, 0, MAX_ROWS)
    }

    pub fn set_baked_surfaces_visible(
        &mut self,
        actor: Actor,
        token: u64,
        visible: bool,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.check_baked_surface_token(token)?;
        if visible {
            self.inspection_viewport_ready("showing baked surfaces")?;
            if !self.baked_surface_context_ready() {
                return Err(
                    "Show baked surfaces requires Objects mode and completed editing tools/drafts"
                        .into(),
                );
            }
            let control = Control::default();
            if !self.baked_surface_current(&control)?.2 {
                return Err("Show baked surfaces requires the current owned build and unchanged preview assets".into());
            }
            let ready = self
                .baked_surfaces
                .ready
                .as_ref()
                .ok_or("Prepare baked surfaces first")?;
            ready
                .scene
                .validate_settings(&self.baked_surfaces.options)?;
            if ready.lighting.is_none() {
                return Err("Choose an available baked surface channel first".into());
            }
            ready.build.verify(&control, MAX_METADATA_BYTES)?;
            ready.assets.verify(&control)?;
        }
        self.baked_surfaces.requested = visible;
        self.baked_surfaces.displayed = visible;
        if visible {
            self.baked_surfaces.status = "ready";
        }
        self.scene_serial += 1;
        self.log(
            actor,
            if visible {
                "Show baked surfaces"
            } else {
                "Hide baked surfaces"
            },
        );
        self.inspect_baked_surfaces(None, 0, MAX_ROWS)
    }

    pub fn frame_baked_surface(
        &mut self,
        actor: Actor,
        token: u64,
        face: usize,
        aspect: f32,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        self.check_baked_surface_token(token)?;
        inspection_framing::validate_aspect(aspect)?;
        self.inspection_viewport_ready("framing a baked surface")?;
        let control = Control::default();
        if !self.baked_surface_current(&control)?.2 {
            return Err(
                "Framing requires the current owned build and unchanged preview assets".into(),
            );
        }
        let ready = self
            .baked_surfaces
            .ready
            .as_ref()
            .ok_or("Prepare baked surfaces first")?;
        let surface = ready
            .scene
            .surfaces(self.baked_surfaces.options.channel)?
            .iter()
            .find(|surface| surface.ordinal == face)
            .ok_or("Baked surface FACE ordinal is unavailable in this channel")?;
        let mins = glam::Vec3::from_array(surface.mins);
        let maxs = glam::Vec3::from_array(surface.maxs);
        let camera = self.inspection_frame_camera(mins, maxs, aspect, "Baked surface")?;
        let bounds = [surface.mins, surface.maxs];
        ready.build.verify(&control, MAX_METADATA_BYTES)?;
        ready.assets.verify(&control)?;
        self.set_camera(camera);
        self.log(actor, format!("Frame baked surface FACE {face}"));
        Ok(
            json!({"token":token.to_string(),"face":face,"channel":self.baked_surfaces.options.channel,"bounds":bounds,"camera":{"position":camera.position.to_array(),"yaw_degrees":camera.yaw_degrees,"pitch_degrees":camera.pitch_degrees}}),
        )
    }

    pub(super) fn baked_surfaces_visible(&self) -> bool {
        self.baked_surfaces.displayed
    }

    pub fn baked_surfaces_render_guard(
        &self,
        view: ViewOptions,
    ) -> Result<Option<RenderGuard>, String> {
        if !self.baked_surfaces_visible() {
            return Ok(None);
        }
        let ready = self
            .baked_surfaces
            .ready
            .as_ref()
            .ok_or("Prepared baked surfaces were retired")?;
        let control = Control::default();
        ready.build.verify_metadata(&control, MAX_METADATA_BYTES)?;
        ready.assets.verify(&control)?;
        Ok(Some(RenderGuard {
            token: self.baked_surfaces.serial,
            build: ready.build.clone(),
            assets: ready.assets.guard.clone(),
            options: self.baked_surfaces.options.clone(),
            scene_key: self.render_scene_key(view),
        }))
    }

    pub fn verify_baked_surfaces_render(
        &mut self,
        guard: Option<&RenderGuard>,
        view: ViewOptions,
    ) -> Result<(), String> {
        let Some(guard) = guard else {
            return Ok(());
        };
        let control = Control::default();
        let result = (|| {
            guard.build.verify_metadata(&control, MAX_METADATA_BYTES)?;
            guard
                .assets
                .verify_original_metadata(&control, MAX_METADATA_BYTES)
                .map_err(|error| error.to_string())?;
            if self.baked_surfaces.serial != guard.token
                || self.render_scene_key(view) != guard.scene_key
                || self.baked_surfaces.options != guard.options
                || !self.baked_surfaces_visible()
                || !self.baked_surface_context_ready()
                || !self.baked_surface_current(&control)?.2
                || self
                    .baked_surfaces
                    .ready
                    .as_ref()
                    .is_none_or(|ready| !Arc::ptr_eq(&ready.build, &guard.build))
            {
                return Err("Baked surface state changed while rendering; render again".into());
            }
            Ok(())
        })();
        if result.is_err() {
            self.preflight_baked_surfaces();
        }
        result
    }

    fn baked_surface_in_region(&self, surface: &render::compiled_faces::SurfaceInfo) -> bool {
        self.working_region
            .enabled
            .then_some(self.working_region.bounds)
            .flatten()
            .is_none_or(|region| {
                let mins = glam::DVec3::from_array(surface.mins.map(f64::from));
                let maxs = glam::DVec3::from_array(surface.maxs.map(f64::from));
                mins.cmple(region[1]).all() && maxs.cmpge(region[0]).all()
            })
    }

    pub(super) fn append_baked_surfaces(&mut self, mesh: &mut SceneMesh) -> Result<(), String> {
        let ready = self
            .baked_surfaces
            .ready
            .as_ref()
            .ok_or("Prepared baked surfaces were retired")?;
        let lighting = ready
            .lighting
            .as_ref()
            .ok_or("Baked surface lighting is unavailable")?;
        let result = ready.scene.append_prepared_filtered(
            mesh,
            &ready.assets.materials,
            lighting.clone(),
            |surface| self.baked_surface_in_region(surface),
        );
        if let Err(error) = &result {
            self.baked_surfaces.requested = false;
            self.baked_surfaces.displayed = false;
            self.baked_surfaces.status = "unavailable";
            self.baked_surfaces.error = Some(baked_build::bounded_message(error));
            self.scene_serial += 1;
        }
        result
    }
}
