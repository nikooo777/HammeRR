use super::super::*;

#[tool_router(router=view_layout_tools,vis="pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the shared Single, Two columns, Two rows or Quad viewport layout, all four independent pane cameras, visible indices, active pane, optional orthographic pan/zoom linking, Session-scoped snapshot and switch blockers. Optional width+height returns exact physical-pixel rectangles within the shared canvas budget. Single uses pane0; two-pane layouts use0/1; Quad uses0top-left,1top-right,2bottom-left,3bottom-right. The legacy get_camera/set_camera alias the active pane. Inspection never activates or publishes another camera.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_view_layout(
        &self,
        Parameters(args): Parameters<ViewLayoutGetArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let mut report = session.view_layout_snapshot();
            match (args.width, args.height) {
                (Some(width), Some(height)) => {
                    report["rectangles"] =
                        json!(session.view_layout().rectangles((width, height))?);
                    report["canvas"] = json!({"width":width,"height":height});
                }
                (None, None) => {}
                _ => return Err("Supply both width and height for pane rectangles".into()),
            }
            Ok(report)
        })
    }

    #[tool(
        description = "Change shared viewport state through the same Session operations as the editor. action=update accepts layout single|two_columns|two_rows|quad and/or linked_orthographic. activate requires a visible pane index. camera applies a strict CameraSpec to any configured pane0–3 without activating it; aspect defaults1. projection chooses perspective|top|front|side for one pane. frame fits the current selection or visible map in every visible pane; aspects optionally supplies four pane aspect ratios (hidden slots ignored), default1. Optional latest get_view_layout.snapshot rejects stale or foreign requests. Pane/layout and active-camera changes refuse captured viewport interactions while preserving standalone owned geometry previews. Inactive camera changes leave the active camera and common working-region anchor unchanged. Linking affects only other orthographic cameras when the active orthographic camera pans/zooms, preserving relative zoom with existing height limits; perspective cameras remain independent. State is Session-local. Accepted New/Open reseeds cameras while retaining layout, active index, linking and inactive projections; the active projection follows the accepted map camera."
    )]
    pub(in crate::mcp) fn set_view_layout(
        &self,
        Parameters(args): Parameters<ViewLayoutArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            match args {
                ViewLayoutArgs::Update {
                    layout,
                    linked_orthographic,
                    snapshot,
                } => session.update_view_layout(
                    Actor::Agent,
                    layout,
                    linked_orthographic,
                    snapshot.as_deref(),
                ),
                ViewLayoutArgs::Activate { pane, snapshot } => {
                    session.activate_view_pane(Actor::Agent, pane, snapshot.as_deref())
                }
                ViewLayoutArgs::Camera {
                    pane,
                    camera,
                    aspect,
                    snapshot,
                } => {
                    let aspect = aspect.unwrap_or(1.0);
                    let camera = camera.resolve_for_pane(&session, pane, aspect, None, None)?;
                    session.set_view_pane_camera(
                        Actor::Agent,
                        pane,
                        camera,
                        aspect,
                        snapshot.as_deref(),
                    )
                }
                ViewLayoutArgs::Projection {
                    pane,
                    projection,
                    snapshot,
                } => {
                    let projection = Projection::parse(&projection)
                        .ok_or("projection must be perspective, top, front or side")?;
                    session.set_view_pane_projection(
                        Actor::Agent,
                        pane,
                        projection,
                        snapshot.as_deref(),
                    )
                }
                ViewLayoutArgs::Frame { aspects, snapshot } => session.frame_all_views(
                    Actor::Agent,
                    aspects.unwrap_or([1.0; crate::session::view_layout::MAX_PANES]),
                    snapshot.as_deref(),
                ),
            }
        })
    }
}
