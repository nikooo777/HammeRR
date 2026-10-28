use super::super::*;
use crate::session::baked_surfaces::{FrameTarget, MAX_ROWS, PreparationOptions, RecordKind};
use render::compiled_faces::{LightingSettings, StyleGain};

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectBakedSurfacesArgs {
    token: Option<String>,
    record_kind: Option<RecordKind>,
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
        compiled_static_props: Option<bool>,
        #[schemars(range(min = 0, max = 7))]
        draw_lod: Option<u8>,
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
        record_kind: Option<RecordKind>,
        face: Option<usize>,
        prop: Option<u32>,
        #[schemars(range(min = 0.01, max = 100))]
        aspect: Option<f32>,
    },
}

#[tool_router(router = baked_surfaces_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect Baked surfaces through Build settings… / Build details… → Baked surfaces… → Inspect and the shared Session operation. Available before the first build; reports preparation availability, token, loading/prepared/ready/stale/cancelled/unavailable/context-suppressed state, exact owned build/output/source and display-asset freshness, shared channel/exposure, FACE stored-ID gains and preparation options. record_kind defaults faces; static_props pages expose prepared native prop ordinals, selected hardware Draw LOD, available LODs and transformed mesh bounds. limit is 1–64; nonzero pages require the returned opaque token. Native ordinals belong to this preparation, never authored picking identities. Static VHV weight is fixed at1 with no style decomposition; FACE density and smoothing are inapplicable. inspect_baked_prop_vertices retains exact stored BGRA/section inspection. Does not prepare, initialize graphics, change options, select, frame or edit."
    )]
    pub(in crate::mcp) fn inspect_baked_surfaces(
        &self,
        Parameters(args): Parameters<InspectBakedSurfacesArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().inspect_baked_surface_records(
                args.token.as_deref().map(parse_token).transpose()?,
                args.record_kind.unwrap_or_default(),
                args.offset.unwrap_or(0),
                args.limit.unwrap_or(MAX_ROWS),
            )
        })
    }

    #[tool(
        description = "Manage Baked surfaces through the shared Session lifecycle. prepare requires a current successful owned lit build; background defaults false, compiled_static_props defaults false and draw_lod defaults0 (explicit0–7). Compiled static props require exact published VHV/model/material inputs and the requested hardware LOD on every receiver; missing inputs/LODs fail the complete preparation without substitution. Changing Draw LOD requires Prepare. Prepare resets lighting settings; Clear cancels/releases and resets all options. Hide retains preparation/settings. cancel/clear/show/hide require the opaque token. lighting atomically sets channel ldr/hdr, exposure_ev -8..8 and style_gains [{style,gain}], distinct stored FACE IDs0–63 with finite gains0–2. FACE style0 defaults1, omitted other styles0; VHV has fixed static weight1 and no style decomposition. Channel/EV apply to both receiver kinds. VHV RGB decodes per hardware vertex as pow(2*byte/255,2.2_f32) before interpolation, then resolved diffuse/coverage, EV, Reinhard and one sRGB output; alpha coverage comes from materials. No native appearance/adaptation parity is claimed. Show validates original build/output/source identities and independent display assets. Working regions include whole final faces/props; only exact sealed source occurrences lose replaced authored draw surfaces, preserving authored picking/helpers/outlines and opted-out siblings. frame requires exactly face or prop from inspection; optional record_kind faces/static_props must match. It uses transformed mesh bounds and optional aspect (default1, finite0.01–100). Only Frame changes the camera."
    )]
    pub(in crate::mcp) fn set_baked_surfaces(
        &self,
        Parameters(args): Parameters<SetBakedSurfacesArgs>,
    ) -> ToolResult {
        run(|| match args {
            SetBakedSurfacesArgs::Prepare {
                background,
                compiled_static_props,
                draw_lod,
            } => {
                let request = self.session().begin_baked_surfaces_with_options(
                    Actor::Agent,
                    PreparationOptions {
                        compiled_static_props: compiled_static_props.unwrap_or(false),
                        draw_lod: draw_lod.unwrap_or(0),
                    },
                )?;
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
                record_kind,
                face,
                prop,
                aspect,
            } => {
                let kind = record_kind.unwrap_or(if prop.is_some() {
                    RecordKind::StaticProps
                } else {
                    RecordKind::Faces
                });
                let target = match (kind, face, prop) {
                    (RecordKind::Faces, Some(face), None) => FrameTarget::Face(face),
                    (RecordKind::StaticProps, None, Some(prop)) => FrameTarget::StaticProp(prop),
                    _ => return Err(
                        "Frame requires exactly one matching native face or static-prop ordinal"
                            .into(),
                    ),
                };
                self.session().frame_baked_surface_record(
                    Actor::Agent,
                    parse_token(&token)?,
                    target,
                    aspect.unwrap_or(1.0),
                )
            }
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
