use super::super::*;

#[tool_router(router = measurement_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect persistent named measurement guides and the shared owned preview, including exact resolved world endpoints, Euclidean distance and signed XYZ components in Hammer units. Endpoint attachments identify authored point-entity origins or rigid local points by stable ID, with local_offset for entity_local attachments, availability and errors; unavailable references return null resolved points/distance, retain stored_start/stored_end and suppress guides. Includes snapshot, visibility, limits and malformed-import errors. Invalid metadata is preserved and blocks edits; no measurement is silently repaired or dropped. Guides appear in UI and render_view, are never pickable, ignore object visibility, and never enter BSP geometry or entities. UI: Measurements….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_measurements(&self) -> ToolResult {
        ok(self.session().measurement_state())
    }

    #[tool(
        description = "Manage Measurements through the same owned Session draft as the UI. action=preview requires name, exact start/end [x,y,z] and latest get_measurements.snapshot; optional id edits a saved measurement. Up to 128 measurements, case-insensitively unique names of 1–80 characters, distinct finite endpoints within ±65536. Preview changes no map/history; invalid requests preserve the draft. Review with token explicitly transfers ownership and returns a new token; Apply revalidates and stores editor-only VMF metadata in one Undo step. Cancel accepts only its matching owned token, including stale cleanup. Foreign/replaced tokens reject. attach_origin requires owned token, endpoint start|end and object entity:<id>; it attaches exactly to an existing point entity's authored finite origin. attach_local uses the same token/endpoint/object fields and captures the current resolved world endpoint in a rigid unit-axis entity frame; it follows authored Source orientation and translation, with no model scale, parenting or mesh association. Absent angles mean zero; malformed, nonfinite or ambiguous active orientation properties make local attachments unavailable. References follow origin changes; duplication leaves the reference on the original. Missing/deleted, invalid-origin and brush-entity targets remain unavailable without freezing or detaching; Apply and Frame reject. detach_origin freezes either resolvable attachment as a world point. world_endpoint requires owned token, endpoint and exact point [x,y,z], explicitly replacing even an unavailable reference. Attachment actions return a new token without map/history edits. Preview preserves attachments and rejects numeric changes to attached endpoints; detach or world_endpoint first. Delete requires id and snapshot with no open measurement draft. Frame requires exactly one id or preview token, snapshot and optional aspect from 0.01 to 100 (default 16/9), and changes only the camera. View toggles show_guides using snapshot. begin_pick requires owned token, unattached endpoint, exact width/height (1–8192), latest pick_snapshot and optional temporary camera matching render_view. pick_ray uses finite ray_origin/ray_direction; pick_pixel uses integer x/y and captured width/height at pixel center. Both pick the first visible brush, terrain triangle or supported model mesh, retain exact world coordinates and return a new token with last_pick provenance. Misses, stale context and coincident endpoints preserve the draft; no plane fallback or snapping. Apply rejects while picking. cancel_pick preserves endpoints and accepts owned stale cleanup. Surface picks and ordinary numeric endpoints stay world points. Brush/surface attachments are unsupported. Local attachments require editor metadata version 2; older builds preserve it and disable measurement edits. Version 1 remains unchanged until a local attachment is applied. Geometry previews must be applied or cancelled first. UI: Measurements… → Preview / Review preview / Attach start origin / Attach end origin / Attach start local / Attach end local / Detach start / Detach end / Use start world / Use end world / Apply / Cancel / Delete / Frame / Pick start / Pick end / Cancel picking / Show guides."
    )]
    pub(in crate::mcp) fn set_measurement(
        &self,
        Parameters(args): Parameters<MeasurementArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            match args {
                MeasurementArgs::AttachLocal {
                    token,
                    endpoint,
                    object,
                } => session.attach_measurement_local(
                    Actor::Agent,
                    token,
                    endpoint,
                    parse_id(&object)?,
                ),
                MeasurementArgs::AttachOrigin {
                    token,
                    endpoint,
                    object,
                } => session.attach_measurement_origin(
                    Actor::Agent,
                    token,
                    endpoint,
                    parse_id(&object)?,
                ),
                MeasurementArgs::DetachOrigin { token, endpoint } => {
                    session.detach_measurement_origin(Actor::Agent, token, endpoint)
                }
                MeasurementArgs::WorldEndpoint {
                    token,
                    endpoint,
                    point,
                } => session.set_measurement_world_endpoint(
                    Actor::Agent,
                    token,
                    endpoint,
                    vec3(point),
                ),
                MeasurementArgs::BeginPick {
                    token,
                    endpoint,
                    width,
                    height,
                    camera,
                    snapshot,
                } => {
                    let camera = camera
                        .unwrap_or_default()
                        .resolve(&session, width as f32 / height as f32)?;
                    session.begin_measurement_pick(
                        Actor::Agent,
                        token,
                        endpoint,
                        camera,
                        (width, height),
                        &snapshot,
                    )
                }
                MeasurementArgs::PickRay {
                    token,
                    ray_origin,
                    ray_direction,
                } => session.pick_measurement_ray(
                    Actor::Agent,
                    token,
                    Ray {
                        origin: vec3(ray_origin),
                        direction: vec3(ray_direction),
                    },
                ),
                MeasurementArgs::PickPixel {
                    token,
                    x,
                    y,
                    width,
                    height,
                } => session.pick_measurement_pixel(
                    Actor::Agent,
                    token,
                    crate::session::Pointer {
                        position: glam::DVec2::new(f64::from(x) + 0.5, f64::from(y) + 0.5),
                        size: (width, height),
                    },
                ),
                MeasurementArgs::CancelPick { token } => {
                    session.cancel_measurement_pick(Actor::Agent, token)
                }
                MeasurementArgs::Preview {
                    id,
                    name,
                    start,
                    end,
                    snapshot,
                } => session.preview_measurement(
                    Actor::Agent,
                    crate::session::measurements::Request {
                        id,
                        name,
                        start: DVec3::from_array(start),
                        end: DVec3::from_array(end),
                    },
                    &snapshot,
                ),
                MeasurementArgs::Review { token } => {
                    session.review_measurement(Actor::Agent, token)
                }
                MeasurementArgs::Apply { token } => session.apply_measurement(Actor::Agent, token),
                MeasurementArgs::Cancel { token } => {
                    session.cancel_measurement(Actor::Agent, token)
                }
                MeasurementArgs::Delete { id, snapshot } => {
                    session.delete_measurement(Actor::Agent, id, &snapshot)
                }
                MeasurementArgs::Frame {
                    id,
                    token,
                    aspect,
                    snapshot,
                } => {
                    session.frame_measurement(
                        id,
                        token,
                        aspect.unwrap_or(16.0 / 9.0),
                        &snapshot,
                    )?;
                    Ok(session.measurement_state())
                }
                MeasurementArgs::View {
                    show_guides,
                    snapshot,
                } => session.set_measurement_guides(show_guides, &snapshot),
            }
        })
    }
}
