use super::super::*;

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectCompiledDetailsArgs {
    token: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Prepare,
    Cancel,
    Clear,
    Show,
    Hide,
    Lighting,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetCompiledDetailsArgs {
    action: Action,
    token: Option<String>,
    background: Option<bool>,
    lighting: Option<String>,
    channel: Option<String>,
    #[schemars(range(min = render::compiled_details::MIN_EXPOSURE_EV, max = render::compiled_details::MAX_EXPOSURE_EV))]
    exposure_ev: Option<f64>,
    #[schemars(length(max = 63))]
    style_gains: Option<Vec<render::compiled_details::StyleGain>>,
}

#[tool_router(router=compiled_details_tools,vis="pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the explicitly prepared final native detail-prop records from the retained successful BSP without loading assets or initializing graphics. Returns preparation token, native leaf-ordered ordinal/kind/dictionary/model/origin/angles/scale/orientation/sway/shape, exact base RGBExp32 and normalized linear lighting, native style ranges, preview lighting mode and Baked LDR availability/refusal reason, complete layer coverage, separate build freshness and preview observations, explicit loading/prepared/ready/stale/cancelled/unavailable/context_suppressed states and 1–64-row pages. Baked LDR requires zero runtime styles and keeps its existing 8-bit display. Baked radiance reports exact selected LDR/HDR extra-table availability, shared base provenance, static style gains, independent exposure and effective normalized linear colors. Dual builds keep their shared HDR base in both choices. No native tone-map, runtime-style or dynamic-light parity. No authored source identity is inferred. Nonzero pages require the returned preparation token. Output replacement invalidates retained records. Does not select, pick, edit or frame authored objects."
    )]
    pub(in crate::mcp) fn inspect_compiled_details(
        &self,
        Parameters(args): Parameters<InspectCompiledDetailsArgs>,
    ) -> ToolResult {
        run(|| {
            let token = args.token.as_deref().map(parse_detail_token).transpose()?;
            self.session().inspect_compiled_details(
                token,
                args.offset.unwrap_or(0),
                args.limit.unwrap_or(64),
            )
        })
    }

    #[tool(
        description = "Manage the read-only Compiled details… editor layer through the same Session operations. prepare requires a current successful owned build, retires the previous layer, resets preview lighting to editor_shading and reads exact owned BSP bytes on one cancellable worker; background defaults false. cancel/clear/show/hide/lighting require the returned preparation token and reject background. Only action lighting accepts lighting: editor_shading, baked_ldr or baked_radiance. Baked radiance requires explicit channel ldr/hdr, exposure_ev -8..8 and style_gains [{style,gain}] with distinct present styles 1..63, finite gains 0..2 and omitted styles at zero. The full settings update is atomic even before Show or while hidden; invalid or unavailable channels/tables preserve the previous settings. Linear base texture times shared native base plus selected-channel stored extras times gains, divided by 255 and multiplied by 2^EV, receives per-channel Reinhard and one editor sRGB conversion. Dual builds retain their shared HDR base in both extra-channel choices; zero-style lit builds need no extra table, while styled records require the selected table. No LDR/HDR substitution or recovered independent base. Legacy modes reject channel/exposure/gains. Baked LDR requires every final record to have zero runtime styles and atomically refuses unsupported data, preserving the previous mode. It modulates models and all card forms with native base RGBExp32 / 255, clamped to 0–1 and rounded to 8-bit linear color with editor sRGB output, style 0 fixed at unity and no additional editor directional/authored lights. clear cancels pending work and releases retained geometry; hide preserves preparation and lighting mode. show validates original compiler/output identities and separately observes verified shared model/material assets before atomically enabling the complete set. Missing assets, unsupported material effects and resource limits refuse the whole layer. Full/vertical cards face the actual render camera. Pole/coincident views use the Source VectorAngles singular basis. Dynamic lights, runtime style patterns, sway, avoidance, runtime random card mirroring, fades, native gamma and native appearance parity are not simulated. Layer display is suppressed during editing tools/drafts and outside Objects mode. Working region intersects whole compiled-prop geometry bounds, with conservative rotation envelopes for camera-facing cards. Temporary authored-ID hiding is independent of this compiler-owned layer; no authored owner is guessed. No authored geometry, selection, picking, camera or Undo changes."
    )]
    pub(in crate::mcp) fn set_compiled_details(
        &self,
        Parameters(args): Parameters<SetCompiledDetailsArgs>,
    ) -> ToolResult {
        run(|| {
            if !matches!(args.action, Action::Lighting)
                && (args.lighting.is_some()
                    || args.channel.is_some()
                    || args.exposure_ev.is_some()
                    || args.style_gains.is_some())
            {
                return Err(
                    "Only the lighting action accepts lighting, channel, exposure and style gains"
                        .into(),
                );
            }
            match args.action {
                Action::Prepare => {
                    if args.token.is_some() {
                        return Err("Prepare accepts only action and optional background".into());
                    }
                    let request = self.session().begin_compiled_details(Actor::Agent)?;
                    let token = request.token();
                    if args.background.unwrap_or(false) {
                        let shared = self.session.clone();
                        let spawned = std::thread::Builder::new()
                            .name("compiled-details".into())
                            .spawn(self.worker(move || {
                                let prepared = request.run();
                                let _ =
                                    lock(&shared).finish_compiled_details(Actor::Agent, prepared);
                            }));
                        if let Err(error) = spawned {
                            return self.session().fail_compiled_details(
                                Actor::Agent,
                                token,
                                format!("Could not start compiled detail preparation: {error}"),
                            );
                        }
                        Ok(json!({"status":"loading","token":token.to_string()}))
                    } else {
                        let prepared = request.run();
                        self.session()
                            .finish_compiled_details(Actor::Agent, prepared)
                    }
                }
                action => {
                    if args.background.is_some() {
                        return Err("Only Prepare accepts background".into());
                    }
                    let token = parse_detail_token(
                        args.token
                            .as_deref()
                            .ok_or("Use the returned compiled detail token")?,
                    )?;
                    let mut session = self.session();
                    match action {
                        Action::Cancel => session.cancel_compiled_details(Actor::Agent, token),
                        Action::Clear => session.clear_compiled_details(Actor::Agent, token),
                        Action::Show => {
                            session.set_compiled_details_visible(Actor::Agent, token, true)
                        }
                        Action::Hide => {
                            session.set_compiled_details_visible(Actor::Agent, token, false)
                        }
                        Action::Lighting => {
                            use render::compiled_details::{Lighting, LightingSettings};
                            let mode = Lighting::parse(args.lighting.as_deref().ok_or(
                                "Lighting requires editor_shading, baked_ldr or baked_radiance",
                            )?)?;
                            let settings = if mode == Lighting::BakedRadiance {
                                LightingSettings {
                                    mode,
                                    channel: render::LightingChannel::parse(
                                        args.channel
                                            .as_deref()
                                            .ok_or("Baked radiance requires channel")?,
                                    )?,
                                    exposure_ev: args
                                        .exposure_ev
                                        .ok_or("Baked radiance requires exposure_ev")?,
                                    style_gains: args
                                        .style_gains
                                        .ok_or("Baked radiance requires style_gains")?,
                                }
                            } else {
                                if args.channel.is_some()
                                    || args.exposure_ev.is_some()
                                    || args.style_gains.is_some()
                                {
                                    return Err(
                                        "Channel, exposure and style gains require Baked radiance"
                                            .into(),
                                    );
                                }
                                LightingSettings {
                                    mode,
                                    ..Default::default()
                                }
                            };
                            session.set_compiled_details_lighting_options(
                                Actor::Agent,
                                token,
                                settings,
                            )
                        }
                        Action::Prepare => unreachable!(),
                    }
                }
            }
        })
    }
}

fn parse_detail_token(raw: &str) -> Result<u64, String> {
    if raw.is_empty()
        || raw.len() > 20
        || raw.starts_with('0')
        || !raw.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("Detail token must be the returned opaque decimal string".into());
    }
    raw.parse::<u64>()
        .map_err(|_| "Detail token must be the returned opaque decimal string".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(result: ToolResult) -> Value {
        let result = result.unwrap();
        assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
        serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
    }

    #[test]
    fn compiled_detail_mcp_uses_owned_empty_preparation_and_guards_tokens_without_graphics() {
        let (_directory, session, _source) =
            crate::session::compiled_world_lights::test_support::compiled(false);
        let before = (
            session.document().vmf().clone(),
            session.camera(),
            session.history_snapshot(),
        );
        let server = EditorServer::new(Arc::new(Mutex::new(session)), None);
        let report = value(
            server.inspect_compiled_details(Parameters(InspectCompiledDetailsArgs::default())),
        );
        assert_eq!(report["status"], "not_loaded");
        let prepared = value(server.set_compiled_details(Parameters(
            serde_json::from_value(json!({"action":"prepare"})).unwrap(),
        )));
        assert_eq!(prepared["status"], "prepared");
        assert_eq!(prepared["page"]["total"], 0);
        assert_eq!(prepared["lighting"]["mode"], "editor_shading");
        let token = prepared["token"].as_str().unwrap();
        let lit = value(
            server.set_compiled_details(Parameters(
                serde_json::from_value(
                    json!({"action":"lighting","token":token,"lighting":"baked_ldr"}),
                )
                .unwrap(),
            )),
        );
        assert_eq!(lit["lighting"]["mode"], "baked_ldr");
        assert_eq!(lit["displayed"], false);
        let shown = value(server.set_compiled_details(Parameters(
            serde_json::from_value(json!({"action":"show","token":token})).unwrap(),
        )));
        assert_eq!(shown["complete"], true);
        assert_eq!(shown["displayed"], true);
        assert_eq!(shown["lighting"]["mode"], "baked_ldr");
        assert_eq!(
            value(server.get_map_info())["rendering"]["initialized"],
            false
        );
        for args in [
            json!({"action":"show","token":"0"}),
            json!({"action":"prepare","token":token}),
            json!({"action":"hide","token":token,"background":false}),
            json!({"action":"lighting","token":token}),
            json!({"action":"lighting","token":token,"lighting":"hdr"}),
            json!({"action":"lighting","token":token,"lighting":"editor_shading","background":false}),
            json!({"action":"show","token":token,"lighting":"editor_shading"}),
            json!({"action":"prepare","lighting":"baked_ldr"}),
            json!({"action":"prepare","channel":"hdr"}),
            json!({"action":"lighting","token":token,"lighting":"baked_radiance"}),
            json!({"action":"lighting","token":token,"lighting":"baked_radiance","channel":"hdr","exposure_ev":0}),
            json!({"action":"lighting","token":token,"lighting":"baked_radiance","channel":"ldr","exposure_ev":9,"style_gains":[]}),
            json!({"action":"lighting","token":token,"lighting":"baked_radiance","channel":"hdr","exposure_ev":0,"style_gains":[]}),
            json!({"action":"lighting","token":token,"lighting":"baked_ldr","channel":"ldr","exposure_ev":0,"style_gains":[]}),
        ] {
            assert!(
                server
                    .set_compiled_details(Parameters(serde_json::from_value(args).unwrap()))
                    .unwrap()
                    .is_error
                    .unwrap_or(false)
            );
        }
        let radiance = value(server.set_compiled_details(Parameters(serde_json::from_value(json!({"action":"lighting","token":token,"lighting":"baked_radiance","channel":"ldr","exposure_ev":1,"style_gains":[]})).unwrap())));
        assert_eq!(radiance["lighting"]["mode"], "baked_radiance");
        assert_eq!(radiance["lighting"]["base_source_channel"], "ldr");
        assert_eq!(radiance["lighting"]["exposure_ev"], 1.0);
        assert_eq!(radiance["lighting"]["style_gains"], json!([]));
        let cleared = value(server.set_compiled_details(Parameters(
            serde_json::from_value(json!({"action":"clear","token":token})).unwrap(),
        )));
        assert_eq!(cleared["status"], "not_loaded");
        assert_eq!(cleared["lighting"]["mode"], "editor_shading");
        assert!(cleared["token"].is_null());
        assert!(
            server
                .inspect_compiled_details(Parameters(
                    serde_json::from_value(json!({"token":token})).unwrap()
                ))
                .unwrap()
                .is_error
                .unwrap_or(false)
        );
        let session = server.session();
        assert_eq!(
            (
                session.document().vmf().clone(),
                session.camera(),
                session.history_snapshot()
            ),
            before
        );
    }

    #[test]
    fn radiance_mcp_reports_shared_hdr_base_and_requires_the_complete_atomic_settings_shape() {
        let (_directory, session, _source) =
            crate::session::compiled_world_lights::test_support::compiled(true);
        let server = EditorServer::new(Arc::new(Mutex::new(session)), None);
        let prepared = value(server.set_compiled_details(Parameters(
            serde_json::from_value(json!({"action":"prepare"})).unwrap(),
        )));
        let token = prepared["token"].as_str().unwrap();
        for channel in ["hdr", "ldr"] {
            let report=value(server.set_compiled_details(Parameters(serde_json::from_value(json!({"action":"lighting","token":token,"lighting":"baked_radiance","channel":channel,"exposure_ev":-1,"style_gains":[]})).unwrap())));
            assert_eq!(report["lighting"]["channel"], channel);
            assert_eq!(report["lighting"]["base_source_channel"], "hdr");
            assert_eq!(report["lighting"]["base_policy"], "shared_native_base");
            assert_eq!(report["lighting"]["exposure_ev"], -1.0);
            assert!(
                report["lighting"]["channels"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|entry| entry["available"] == true)
            );
        }
        let schema = serde_json::to_value(schemars::schema_for!(SetCompiledDetailsArgs)).unwrap();
        assert_eq!(schema["$defs"]["StyleGain"]["additionalProperties"], false);
        assert!(serde_json::from_value::<SetCompiledDetailsArgs>(json!({"action":"lighting","token":token,"lighting":"baked_radiance","channel":"hdr","exposure_ev":0,"style_gains":[{"style":1,"gain":1,"other":0}]})).is_err());
    }
}
