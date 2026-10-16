use super::super::*;

#[tool_router(router = measurement_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect persistent named measurement guides and the shared owned preview, including exact world endpoints, Euclidean distance and signed XYZ components in Hammer units. Includes snapshot, visibility, limits and explicit malformed-import errors. Invalid metadata is preserved and blocks edits; no measurement is silently repaired or dropped. Guides appear in UI and render_view, are never pickable, ignore object visibility, and never enter BSP geometry or entities. UI: Measurements….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_measurements(&self) -> ToolResult {
        ok(self.session().measurement_state())
    }

    #[tool(
        description = "Manage Measurements through the same owned Session draft as the UI. action=preview requires name, exact start/end [x,y,z] and latest get_measurements.snapshot; optional id edits a saved measurement. Up to 128 measurements, case-insensitively unique names of 1–80 characters, distinct finite endpoints within ±65536. Preview changes no map/history; invalid requests preserve the draft. Review with token explicitly transfers ownership and returns a new token; Apply revalidates and stores editor-only VMF metadata in one Undo step. Cancel accepts only its matching owned token, including stale cleanup. Foreign/replaced tokens reject. Delete requires id and snapshot with no open measurement draft. Frame requires exactly one id or preview token, snapshot and optional aspect from 0.01 to 100 (default 16/9), and changes only the camera. View toggles show_guides using snapshot. action=begin_pick requires owned token, endpoint start|end, exact width/height (1–8192), latest get_measurements.pick_snapshot, and optional temporary camera matching render_view. It returns a new token and captures the scene, camera and viewport. action=pick_ray requires token and finite ray_origin/ray_direction; pick_pixel requires token, integer x/y and the captured width/height and uses the pixel center. Both pick the first visible brush, terrain triangle or supported model mesh, retain double precision world coordinates, and return a new token with last_pick provenance. Misses, hidden/helper-only targets, stale scene/camera/viewport and coincident endpoints reject without changing the draft; no plane fallback or snapping. Apply rejects while picking. cancel_pick stops only endpoint picking, preserves endpoints and accepts owned stale cleanup. Numeric coordinates remain exact, unsnapped and unattached; later object changes do not move them. Geometry previews must be applied or cancelled before changing measurements. UI: Measurements… → Preview / Review preview / Apply / Cancel / Delete / Frame / Pick start / Pick end / Cancel picking / Show guides."
    )]
    pub(in crate::mcp) fn set_measurement(
        &self,
        Parameters(args): Parameters<MeasurementArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            match args {
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

#[cfg(test)]
mod tests;
