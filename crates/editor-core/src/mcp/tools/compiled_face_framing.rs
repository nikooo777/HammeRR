use super::super::*;
use crate::session::baked_lightmaps::Channel;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct FrameCompiledFaceArgs {
    pub build_token: String,
    pub channel: String,
    pub face: usize,
    pub aspect: Option<f32>,
}

#[tool_router(router = compiled_face_framing_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Frame one compiled flat world/detail face from the current successful owned BSP. Require build_token, explicit channel ldr/hdr and zero-based face index from inspect_baked_lightmaps; optional aspect defaults to 1 and must be finite 0.01–100. Bounds come from that channel's actual Model0 face/surfedge/edge/vertex records. Multiple or unavailable authored contributors do not prevent valid compiled geometry framing. Terrain, model-local faces, stale/replaced/malformed output and active viewport work refuse before camera mutation. Preserves projection, map, selection, history, settings and ordinary drafts. Returns build, channel, face, framed_bounds, aspect and camera. UI: Baked lightmaps → Inspect face → Frame face."
    )]
    pub(in crate::mcp) fn frame_compiled_face(
        &self,
        Parameters(args): Parameters<FrameCompiledFaceArgs>,
    ) -> ToolResult {
        run(|| {
            let aspect = args.aspect.unwrap_or(1.0);
            let frame = self.session().frame_compiled_face(
                Actor::Agent,
                args.build_token,
                Channel::parse(&args.channel)?,
                args.face,
                aspect,
            )?;
            Ok(json!({
                "build":frame.build,"channel":frame.channel,"face":frame.face,
                "framed_bounds":frame.bounds,"aspect":aspect,"camera":camera_json(&frame.camera),
            }))
        })
    }
}
