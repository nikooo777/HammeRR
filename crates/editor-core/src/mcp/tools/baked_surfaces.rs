use super::super::*;
use crate::session::baked_surfaces::MAX_ROWS;
use render::compiled_faces::{LightingSettings, StyleGain};

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectBakedSurfacesArgs {
    token: Option<String>,
    offset: Option<usize>,
    #[schemars(range(min = 1, max = MAX_ROWS))]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum SetBakedSurfacesArgs {
    Prepare {
        background: Option<bool>,
    },
    Cancel {
        token: String,
    },
    Clear {
        token: String,
    },
    Show {
        token: String,
    },
    Hide {
        token: String,
    },
    Lighting {
        token: String,
        channel: String,
        #[schemars(range(min = render::compiled_faces::MIN_EXPOSURE_EV, max = render::compiled_faces::MAX_EXPOSURE_EV))]
        exposure_ev: f64,
        #[schemars(length(max = 64))]
        style_gains: Vec<StyleGain>,
    },
    Frame {
        token: String,
        face: usize,
        #[schemars(range(min = 0.01, max = 100))]
        aspect: Option<f32>,
    },
}

#[tool_router(router = baked_surfaces_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the Baked surfaces viewport layer through the same Session operation as Build settings… / Build details… → Baked surfaces… → Inspect. Available before the first build; reports preparation availability and its refusal reason. Reads the retained successful owned BSP with exact independent LDR/HDR FACE and LIGHTING tables. Returns preparation token, loading/prepared/ready/stale/cancelled/unavailable/context-suppressed state, build/source/output and display-asset freshness, automatic material-directional reconstruction policy, effective stored-ID gains, exposure and bounded native-face pages. limit is 1–64; nonzero pages require the returned opaque token. Rows identify final world/detail, displacement and initial-pose brush-model surfaces, material and framing availability. Face ordinals belong to this preparation and selected channel; no authored picking or source identity is inferred. Does not initialize graphics, prepare assets, change options, select, frame or edit."
    )]
    pub(in crate::mcp) fn inspect_baked_surfaces(
        &self,
        Parameters(args): Parameters<InspectBakedSurfacesArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().inspect_baked_surfaces(
                args.token.as_deref().map(parse_token).transpose()?,
                args.offset.unwrap_or(0),
                args.limit.unwrap_or(MAX_ROWS),
            )
        })
    }

    #[tool(
        description = "Manage Baked surfaces through the shared Session lifecycle. prepare requires a current successful owned lit build, resets lighting options and starts one cancellable preparation; optional background defaults false. cancel/clear/show/hide require the returned opaque token. Hide retains preparation and options; Clear cancels work and releases the layer. lighting requires token plus the complete atomic channel (ldr/hdr), exposure_ev (-8..8) and style_gains [{style,gain}] request. Gains address distinct stored style IDs 0–63 and are finite 0–2. Omitted style0 defaults to unity; explicit style0 is allowed; omitted nonzero styles are off. An absent selected channel or style refuses without changing valid options; there is no channel substitution. Directional normal-map reconstruction is automatic from actual compiled material and face data; exact map1–3 inspection remains in inspect_baked_lightmaps. The viewport linearly samples decoded RGBExp32/255 maps at native luxel centers, reconstructs admitted directional materials, sums weighted styles, multiplies compiled linear diffuse/blend and 2^EV, then applies Reinhard and one sRGB output conversion. No editor lights are added and no native appearance/adaptation parity is claimed. Show validates exact original build/output/source identities and separate guarded display assets before enabling the complete layer; unsupported inputs refuse explicitly. Editing tools and drafts suppress the compiled layer outside Objects mode. Working region intersects whole final-face bounds; authored-ID hiding is independent. frame requires token and selected-channel native face ordinal from inspection, optionally aspect (default1, finite0.01–100); it frames final world/detail, displacement or initial-pose brush geometry through the same checked Session operation. Only explicit frame changes the camera; map, history, authored selection and picking remain unchanged."
    )]
    pub(in crate::mcp) fn set_baked_surfaces(
        &self,
        Parameters(args): Parameters<SetBakedSurfacesArgs>,
    ) -> ToolResult {
        run(|| match args {
            SetBakedSurfacesArgs::Prepare { background } => {
                let request = self.session().begin_baked_surfaces(Actor::Agent)?;
                let token = request.token();
                if !background.unwrap_or(false) {
                    let prepared = request.run();
                    return self.session().finish_baked_surfaces(Actor::Agent, prepared);
                }
                let shared = self.session.clone();
                let spawned = std::thread::Builder::new()
                    .name("baked-surfaces".into())
                    .spawn(self.worker(move || {
                        let prepared = request.run();
                        let _ = lock(&shared).finish_baked_surfaces(Actor::Agent, prepared);
                    }));
                if let Err(error) = spawned {
                    return self.session().fail_baked_surfaces(
                        Actor::Agent,
                        token,
                        format!("Could not start baked surface preparation: {error}"),
                    );
                }
                Ok(json!({"status":"loading","token":token.to_string()}))
            }
            SetBakedSurfacesArgs::Cancel { token } => self
                .session()
                .cancel_baked_surfaces(Actor::Agent, parse_token(&token)?),
            SetBakedSurfacesArgs::Clear { token } => self
                .session()
                .clear_baked_surfaces(Actor::Agent, parse_token(&token)?),
            SetBakedSurfacesArgs::Show { token } => {
                self.session()
                    .set_baked_surfaces_visible(Actor::Agent, parse_token(&token)?, true)
            }
            SetBakedSurfacesArgs::Hide { token } => {
                self.session()
                    .set_baked_surfaces_visible(Actor::Agent, parse_token(&token)?, false)
            }
            SetBakedSurfacesArgs::Lighting {
                token,
                channel,
                exposure_ev,
                style_gains,
            } => self.session().set_baked_surfaces_options(
                Actor::Agent,
                parse_token(&token)?,
                LightingSettings {
                    channel: render::LightingChannel::parse(&channel)?,
                    exposure_ev,
                    style_gains,
                },
            ),
            SetBakedSurfacesArgs::Frame {
                token,
                face,
                aspect,
            } => self.session().frame_baked_surface(
                Actor::Agent,
                parse_token(&token)?,
                face,
                aspect.unwrap_or(1.0),
            ),
        })
    }
}

fn parse_token(raw: &str) -> Result<u64, String> {
    if raw.is_empty()
        || raw.len() > 20
        || raw.starts_with('0')
        || !raw.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("Use the returned opaque baked surface token".into());
    }
    raw.parse()
        .map_err(|_| "Use the returned opaque baked surface token".into())
}
