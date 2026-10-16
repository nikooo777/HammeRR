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
        description = "Manage Measurements through the same owned Session draft as the UI. action=preview requires name, exact start/end [x,y,z] and latest get_measurements.snapshot; optional id edits a saved measurement. Up to 128 measurements, case-insensitively unique names of 1–80 characters, distinct finite endpoints within ±65536. Preview changes no map/history; invalid requests preserve the draft. Review with token explicitly transfers ownership and returns a new token; Apply revalidates and stores editor-only VMF metadata in one Undo step. Cancel accepts only its matching owned token, including stale cleanup. Foreign/replaced tokens reject. Delete requires id and snapshot with no open measurement draft. Frame requires exactly one id or preview token, snapshot and optional aspect from 0.01 to 100 (default 16/9), and changes only the camera. View toggles show_guides using snapshot. Numeric endpoints are unsnapped, unattached to objects, and remain independent of hidden objects. Geometry previews must be applied or cancelled before changing measurements. UI: Measurements… → Preview / Review preview / Apply / Cancel / Delete / Frame / Show guides."
    )]
    pub(in crate::mcp) fn set_measurement(
        &self,
        Parameters(args): Parameters<MeasurementArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            match args {
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
