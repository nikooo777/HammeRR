use std::sync::Arc;

use compiler::Control;
use render::{
    LightingChannel, SceneMesh, ViewOptions,
    compiled_faces::{LightingData, LightingSettings, Scene},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use source_formats::{bsp::Bsp, gamefs::GameFs};

use super::{
    Actor, Session, baked_build, inspection_framing,
    worker::{Slot, Ticket},
};

mod assets;
mod props;

pub const MAX_ROWS: usize = 64;
pub const MAX_METADATA_BYTES: usize = 32 * 1024 * 1024;
const SOURCE_MODE: baked_build::SourceVerification =
    baked_build::SourceVerification::OriginalMetadata {
        maximum_bytes: MAX_METADATA_BYTES,
    };
const DISPLAY_POLICY: &str = "Selected final BSP FACE and LIGHTING channel, without substitution. Stored style 0 defaults to unity; other omitted stored IDs default to zero. Linear RGBExp32/255 maps use face-isolated bilinear sampling at native luxel centers and static material directional reconstruction. Resolved compiled diffuse/blend and static tint multiply linear lighting times 2^EV, then one Reinhard transform and one sRGB output. No editor lighting or native appearance/adaptation parity.";
const VIEW_POLICY: &str = "The compiled surface layer replaces authored brush and terrain draw surfaces. Working regions include whole final faces whose initial-pose world AABB intersects the region, without clipping triangles. Authored hiding is independent of this immutable compiled layer; no contributor-derived surface subsets are inferred. Authored selection outlines, picking, entities and helpers remain authored; native FACE ordinals are not authored identities.";
const PROP_POLICY: &str = "Final static-prop geometry uses the exact selected hardware draw LOD and native placement/skin. VHV RGB is decoded per hardware vertex as pow(2*byte/255,2.2_f32) before interpolation. Shared channel and exposure apply; static weight is fixed at 1 and FACE style gains do not affect VHV. Runtime point/spot style contributions are not simulated. Material coverage is independent of VHV alpha. Inspection is double-sided and does not claim native appearance/adaptation parity.";

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct PreparationOptions {
    #[serde(default)]
    pub compiled_static_props: bool,
    #[serde(default)]
    #[schemars(range(min = 0, max = 7))]
    pub draw_lod: u8,
}

impl PreparationOptions {
    fn validate(self) -> Result<(), String> {
        if self.draw_lod > 7 {
            return Err("Draw LOD must be 0–7; prepare again to change it".into());
        }
        Ok(())
    }
}

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    #[default]
    Faces,
    StaticProps,
}

#[derive(Clone, Copy)]
pub enum FrameTarget {
    Face(usize),
    StaticProp(u32),
}

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
    preparation: PreparationOptions,
}

struct Ready {
    build: Arc<baked_build::RetainedBuild>,
    scene: Scene,
    assets: assets::Prepared,
    lighting: Option<Arc<LightingData>>,
    props: Option<props::Prepared>,
    preparation: PreparationOptions,
}

impl Ready {
    fn verify_assets(&self, control: &Control) -> Result<(), String> {
        self.assets.verify(control)?;
        if let Some(props) = &self.props {
            props.verify(control)?;
        }
        Ok(())
    }

    fn validate_settings(&self, options: &LightingSettings) -> Result<(), String> {
        self.scene.validate_settings(options)?;
        if let Some(props) = &self.props {
            props
                .scene
                .validate_settings(options, self.preparation.draw_lod)?;
        }
        Ok(())
    }

    fn lighting_capacity(&self, options: &LightingSettings) -> Result<(), String> {
        self.validate_settings(options)?;
        let old = self
            .lighting
            .as_ref()
            .map(|lighting| lighting.cpu_byte_len())
            .transpose()?
            .unwrap_or(0);
        let new = self.scene.lighting_cpu_bytes(options.channel)?;
        let props = self
            .props
            .as_ref()
            .map(|props| {
                props
                    .scene
                    .retained_bytes()
                    .checked_add(props.bytes)
                    .and_then(|bytes| {
                        bytes.checked_add(
                            props
                                .scene
                                .mesh_bytes(options.channel, self.preparation.draw_lod)
                                .ok()?,
                        )
                    })
                    .ok_or("Compiled static-prop working size overflow")
            })
            .transpose()?
            .unwrap_or(0);
        let bytes = self
            .scene
            .retained_bytes()
            .checked_add(self.assets.bytes)
            .and_then(|bytes| bytes.checked_add(old))
            .and_then(|bytes| bytes.checked_add(new));
        let bytes = bytes.and_then(|bytes| bytes.checked_add(props));
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
    options: PreparationOptions,
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
    props: Option<props::Prepared>,
    preparation: PreparationOptions,
}

pub struct RenderGuard {
    token: u64,
    build: Arc<baked_build::RetainedBuild>,
    assets: compiler::asset_sources::AssetSourceGuard,
    prop_assets: Option<compiler::asset_sources::AssetSourceGuard>,
    preparation: PreparationOptions,
    options: LightingSettings,
    scene_key: render::SceneKey,
}

impl Request {
    pub fn token(&self) -> u64 {
        self.token
    }

    pub fn run(self) -> Preparation {
        let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            decode(&self.build, &self.game, self.options, &self.control)
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
    options: PreparationOptions,
    control: &Control,
) -> Result<Decoded, String> {
    options.validate()?;
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
    let other_cpu_bytes = scene
        .retained_bytes()
        .checked_add(assets.bytes)
        .and_then(|bytes| bytes.checked_add(lighting_bytes))
        .ok_or("Baked surface working size overflow")?;
    let props = if options.compiled_static_props {
        let prepared = props::prepare(
            &bsp,
            game,
            build.prop_lighting_guard.as_ref().ok_or(
                "Compile static-prop vertex lighting before preparing compiled static props",
            )?,
            build
                .static_prop_origins
                .as_ref()
                .ok_or("Compiled static-prop source associations are unavailable; compile again")?,
            other_cpu_bytes,
            control,
        )?;
        prepared
            .scene
            .validate_settings(&LightingSettings::default(), options.draw_lod)?;
        Some(prepared)
    } else {
        None
    };
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
    if let Some(props) = &props {
        props.verify(control)?;
    }
    Ok(Decoded {
        scene,
        assets,
        lighting,
        props,
        preparation: options,
    })
}

impl Session {
    pub fn begin_baked_surfaces(&mut self, actor: Actor) -> Result<Request, String> {
        self.begin_baked_surfaces_with_options(actor, PreparationOptions::default())
    }

    pub fn begin_baked_surfaces_with_options(
        &mut self,
        actor: Actor,
        options: PreparationOptions,
    ) -> Result<Request, String> {
        options.validate()?;
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
        self.baked_surfaces.preparation = options;
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
            options,
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
                    if let Some(props) = &decoded.props {
                        props.verify(&prepared.control)?;
                    }
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
                    props: decoded.props,
                    preparation: decoded.preparation,
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
        self.baked_surfaces.preparation = PreparationOptions::default();
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
            match ready.verify_assets(control) {
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
            "complete":self.baked_surfaces.ready.is_some(),"error":self.baked_surfaces.error,"lighting_options":self.baked_surfaces.options,"preparation_options":self.baked_surfaces.preparation,
            "context_ready":self.baked_surface_context_ready(),"build":self.compiled_world_light_observation()})
    }

    pub fn baked_surfaces_options_report(&self) -> Value {
        let ready = self.baked_surfaces.ready.as_ref();
        let options = &self.baked_surfaces.options;
        json!({"settings":options,"channel":options.channel,"exposure_ev":options.exposure_ev,"style_gains":options.style_gains,
            "available_styles":ready.and_then(|ready|ready.scene.available_styles(options.channel).ok()),
            "effective_style_gains":ready.and_then(|ready|ready.lighting.as_ref()).map(|lighting|&lighting.effective_style_gains),
            "channels":([LightingChannel::Ldr,LightingChannel::Hdr].map(|channel|json!({"channel":channel,"available":ready.is_some_and(|ready|ready.scene.available_styles(channel).is_ok() && ready.props.as_ref().is_none_or(|props|props.scene.available_channels().contains(&channel)))}))),
            "available":ready.is_some_and(|ready|ready.lighting.is_some()),"unavailable_reason":ready.and_then(|ready|ready.validate_settings(options).err()),
            "reconstruction":"auto_material_directional","sampling":"face_isolated_bilinear_native_luxel_centers",
            "material":render::compiled_faces::materials::POLICY,
            "style_zero_policy":"implicit_unity_unless_explicit_gain","styles_apply_to":"faces","display_policy":DISPLAY_POLICY,
            "static_props":{"enabled":self.baked_surfaces.preparation.compiled_static_props,"draw_lod":self.baked_surfaces.preparation.draw_lod,"static_weight":1,"style_decomposition":false,"runtime_direct_lighting":"not_simulated","policy":PROP_POLICY}})
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
        self.inspect_baked_surface_records(token, RecordKind::Faces, offset, limit)
    }

    pub fn inspect_baked_surface_records(
        &mut self,
        token: Option<u64>,
        record_kind: RecordKind,
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
        let surfaces = ready
            .filter(|ready| {
                ready
                    .scene
                    .available_channels()
                    .contains(&self.baked_surfaces.options.channel)
            })
            .map(|ready| ready.scene.surfaces(self.baked_surfaces.options.channel))
            .transpose()?;
        let prop_scene = ready
            .and_then(|ready| ready.props.as_ref())
            .map(|props| &props.scene);
        let total = match record_kind {
            RecordKind::Faces => surfaces.map(<[_]>::len),
            RecordKind::StaticProps => prop_scene.map(|scene| scene.props().len()),
        };
        if offset > total.unwrap_or(0) {
            return Err(
                "Baked surface page offset exceeds the selected retained record table".into(),
            );
        }
        let rows = if record_kind == RecordKind::Faces {
            surfaces.unwrap_or_default().iter().skip(offset).take(limit).map(|surface| {
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
            json!({"record_kind":"faces","face":surface.ordinal,"model":surface.model,"material":material.map(|material|material.name.as_str()),
                "receiver":if surface.terrain{"terrain"}else if surface.model==0{"world_detail"}else{"brush_model"},
                "bounds":[surface.mins,surface.maxs],"vertices":surface.vertices,"triangles":surface.triangles,
                "lightmap_dimensions":surface.dimensions,"maps":surface.maps,"styles":surface.styles,"frame_available":current,
                "missing_surface_samples":surface.missing_surface_samples,"in_working_region":self.baked_surface_in_region(surface),
                "authored_identity":null,"source_contributors":{"root":origins[0],"instance":origins[1],"generated_region":origins[2],"unresolved":origins[3]},"source_policy":"many_to_many_contributors_without_authored_footprint_or_vertex_correspondence"})
        }).collect::<Vec<_>>()
        } else {
            prop_scene.map(|scene| scene.props().iter().skip(offset).take(limit).map(|prop| {
                let bounds = scene.bounds(self.baked_surfaces.preparation.draw_lod, Some(prop.ordinal), |_|true)?;
                let sample = scene.draw_lod_sample(prop.ordinal, self.baked_surfaces.preparation.draw_lod, &self.baked_surfaces.options)?;
                Ok(json!({"record_kind":"static_props","prop":prop.ordinal,"model":prop.model_name,"receiver":"static_prop",
                    "bounds":bounds,"draw_lod":self.baked_surfaces.preparation.draw_lod,"available_lods":prop.lods,"frame_available":current && bounds.is_some(),
                    "skin":prop.skin,"origin":prop.origin,"angles":prop.angles,"all_lod_vertices":prop.vertices,"all_lod_triangles":prop.triangles,
                    "sample":sample,"sample_scope":"first_hardware_vertex_of_selected_draw_lod",
                    "in_working_region":self.baked_prop_in_region(prop),"authored_identity":null,"static_weight":1,"style_decomposition":false,
                    "lighting_channel":self.baked_surfaces.options.channel,"lighting_policy":PROP_POLICY,
                    "face_density":null,"face_smoothing":null,"raw_colors":"inspect_baked_prop_vertices retains exact stored BGRA bytes and sections"}))
            }).collect::<Result<Vec<_>,String>>()).transpose()?.unwrap_or_default()
        };
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
            "complete":ready.is_some(),"error":self.baked_surfaces.error,"lighting_options":self.baked_surfaces.options,"preparation_options":self.baked_surfaces.preparation,"lighting":self.baked_surfaces_options_report(),
            "prepare_available":build["current"]==true && !self.baked_surfaces.worker.busy(),"prepare_unavailable_reason":if self.baked_surfaces.worker.busy(){Some("Wait for the baked surface worker")}else if build["current"]!=true{Some("Compile a current successful owned build first")}else{None},
            "suppression_reason":if self.baked_surface_context_ready(){None}else{Some("Use Objects mode and finish editing tools/drafts")},
            "record_kind":record_kind,"page":{"record_kind":record_kind,"offset":offset,"limit":limit,"total":total,"returned":rows.len(),"next_offset":total.filter(|total|offset+rows.len()<*total).map(|_|offset+rows.len()),"rows":rows},
            "record_unavailable_reason":if record_kind == RecordKind::StaticProps && prop_scene.is_none(){Some("Prepare with Compiled static props enabled to inspect static props")}else{None},
            "counts":surfaces.map(|surfaces|json!({"faces":surfaces.len(),"world_detail":surfaces.iter().filter(|surface|surface.model==0&&!surface.terrain).count(),"terrain":surfaces.iter().filter(|surface|surface.terrain).count(),"brush_model":surfaces.iter().filter(|surface|surface.model!=0&&!surface.terrain).count()})),
            "retained_geometry_bytes":ready.map(|ready|ready.scene.retained_bytes()+ready.props.as_ref().map_or(0,|props|props.scene.retained_bytes())),"preview_resources_bytes":ready.map(|ready|ready.assets.bytes+ready.props.as_ref().map_or(0,|props|props.bytes)),
            "region_faces":surfaces.map(|surfaces|surfaces.iter().filter(|surface|self.baked_surface_in_region(surface)).count()),
            "static_props":{"total":prop_scene.map(|scene|scene.props().len()),"region_props":prop_scene.map(|scene|scene.props().iter().filter(|prop|self.baked_prop_in_region(prop)).count()),"policy":PROP_POLICY},
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
                    "static_props",
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
        ready.verify_assets(&control)?;
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
            ready.validate_settings(&self.baked_surfaces.options)?;
            if ready.lighting.is_none() {
                return Err("Choose an available baked surface channel first".into());
            }
            ready.build.verify(&control, MAX_METADATA_BYTES)?;
            ready.verify_assets(&control)?;
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
        self.frame_baked_surface_record(actor, token, FrameTarget::Face(face), aspect)
    }

    pub fn frame_baked_surface_record(
        &mut self,
        actor: Actor,
        token: u64,
        target: FrameTarget,
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
        let (bounds, kind, ordinal) = match target {
            FrameTarget::Face(face) => {
                let surface = ready
                    .scene
                    .surfaces(self.baked_surfaces.options.channel)?
                    .iter()
                    .find(|surface| surface.ordinal == face)
                    .ok_or("Baked surface FACE ordinal is unavailable in this channel")?;
                ([surface.mins, surface.maxs], RecordKind::Faces, face)
            }
            FrameTarget::StaticProp(prop) => {
                let props = ready
                    .props
                    .as_ref()
                    .ok_or("Prepare with Compiled static props enabled first")?;
                let bounds = props
                    .scene
                    .bounds(ready.preparation.draw_lod, Some(prop), |_| true)?
                    .ok_or("Baked static-prop ordinal has no prepared mesh bounds")?;
                (bounds, RecordKind::StaticProps, prop as usize)
            }
        };
        let mins = glam::Vec3::from_array(bounds[0]);
        let maxs = glam::Vec3::from_array(bounds[1]);
        let camera = self.inspection_frame_camera(mins, maxs, aspect, "Baked surface")?;
        ready.build.verify(&control, MAX_METADATA_BYTES)?;
        ready.verify_assets(&control)?;
        self.set_camera(camera);
        self.log(actor, format!("Frame baked surface {kind:?} {ordinal}"));
        let mut report = json!({"token":token.to_string(),"record_kind":kind,"channel":self.baked_surfaces.options.channel,"bounds":bounds,"camera":{"position":camera.position.to_array(),"yaw_degrees":camera.yaw_degrees,"pitch_degrees":camera.pitch_degrees}});
        report[if kind == RecordKind::Faces {
            "face"
        } else {
            "prop"
        }] = json!(ordinal);
        Ok(report)
    }

    pub(super) fn baked_surfaces_visible(&self) -> bool {
        self.baked_surfaces.displayed
    }

    pub(super) fn baked_static_prop_replaces(
        &self,
        matches: impl Fn(&compiler::entity_origins::Origin) -> bool,
    ) -> bool {
        self.baked_surfaces_visible()
            && self
                .baked_surfaces
                .ready
                .as_ref()
                .and_then(|ready| ready.props.as_ref())
                .is_some_and(|props| {
                    props.replacements.iter().any(|(ordinal, origin)| {
                        matches(origin)
                            && props
                                .scene
                                .props()
                                .iter()
                                .find(|prop| prop.ordinal == *ordinal)
                                .is_some_and(|prop| self.baked_prop_in_region(prop))
                    })
                })
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
        ready.verify_assets(&control)?;
        Ok(Some(RenderGuard {
            token: self.baked_surfaces.serial,
            build: ready.build.clone(),
            assets: ready.assets.guard.clone(),
            prop_assets: ready.props.as_ref().map(|props| props.guard.clone()),
            preparation: self.baked_surfaces.preparation,
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
            if let Some(assets) = &guard.prop_assets {
                assets
                    .verify_original_metadata(&control, MAX_METADATA_BYTES)
                    .map_err(|error| error.to_string())?;
            }
            if self.baked_surfaces.serial != guard.token
                || self.render_scene_key(view) != guard.scene_key
                || self.baked_surfaces.options != guard.options
                || self.baked_surfaces.preparation != guard.preparation
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
        self.baked_bounds_in_region(surface.mins, surface.maxs)
    }

    fn baked_prop_in_region(&self, prop: &render::compiled_faces::props::PropInfo) -> bool {
        self.baked_bounds_in_region(prop.mins, prop.maxs)
    }

    fn baked_bounds_in_region(&self, mins: [f32; 3], maxs: [f32; 3]) -> bool {
        self.working_region
            .enabled
            .then_some(self.working_region.bounds)
            .flatten()
            .is_none_or(|region| {
                let mins = glam::DVec3::from_array(mins.map(f64::from));
                let maxs = glam::DVec3::from_array(maxs.map(f64::from));
                mins.cmple(region[1]).all() && maxs.cmpge(region[0]).all()
            })
    }

    pub(super) fn append_baked_surfaces(
        &mut self,
        mesh: &mut SceneMesh,
        authored_models: bool,
    ) -> Result<(), String> {
        let ready = self
            .baked_surfaces
            .ready
            .as_ref()
            .ok_or("Prepared baked surfaces were retired")?;
        let lighting = ready
            .lighting
            .as_ref()
            .ok_or("Baked surface lighting is unavailable")?;
        if ready.props.is_some() {
            ready.lighting_capacity(&self.baked_surfaces.options)?;
        }
        let result = ready
            .scene
            .append_prepared_filtered(mesh, &ready.assets.materials, lighting.clone(), |surface| {
                self.baked_surface_in_region(surface)
            })
            .and_then(|()| {
                if let Some(props) = &ready.props
                    && authored_models
                {
                    let decoded = props
                        .materials
                        .iter()
                        .flatten()
                        .try_fold(0usize, |bytes, material| {
                            bytes.checked_add(material.decoded_bytes())
                        })
                        .ok_or("Compiled prop material byte size overflow")?;
                    let metadata = props
                        .bytes
                        .checked_sub(decoded)
                        .ok_or("Compiled prop retained material size is inconsistent")?;
                    let extra = ready
                        .scene
                        .retained_bytes()
                        .checked_add(ready.assets.bytes)
                        .and_then(|bytes| bytes.checked_add(lighting.cpu_byte_len().ok()?))
                        .and_then(|bytes| bytes.checked_add(metadata))
                        .ok_or("Baked surface retained working size overflow")?;
                    props.scene.append_prepared_filtered(
                        mesh,
                        &props.materials,
                        &self.baked_surfaces.options,
                        ready.preparation.draw_lod,
                        extra,
                        |prop| self.baked_prop_in_region(prop),
                    )?;
                }
                Ok(())
            });
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
