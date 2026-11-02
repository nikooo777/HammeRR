use super::super::*;

#[tool_router(router = workplane_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the session-local 3D reference plane, canonical frame, bounded displayed grid, snapping, readiness, limits and snapshot. New/Open reset the guide. The plane does not edit map data, history or camera.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_workplane(
        &self,
        Parameters(_args): Parameters<WorkplaneInspectArgs>,
    ) -> ToolResult {
        ok(self.session().workplane_state())
    }

    #[tool(
        description = "Configure the reference plane through the shared Session operation. update takes settings with optional origin, normal, tangent, enabled/show/snap, grid_size and half_extent; omitted fields stay unchanged. A changed normal without tangent chooses a deterministic orientation. from_face uses solid/side; from_ray uses the first visible surface; reset restores defaults. Optional snapshot from get_workplane rejects changed settings. Degenerate/nonfinite axes and invalid limits refuse without changes. Grid spacing is 0.125–4096; half_extent 0.125–65536 bounds only display. Ruler plane picking is explicit and separate from surface picking. No map/history/camera edits."
    )]
    pub(in crate::mcp) fn set_workplane(
        &self,
        Parameters(args): Parameters<WorkplaneArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            match args {
                WorkplaneArgs::Update { settings, snapshot } => {
                    session.set_workplane(Actor::Agent, settings, snapshot.as_deref())
                }
                WorkplaneArgs::FromFace {
                    solid,
                    side,
                    snapshot,
                } => session.workplane_from_face(
                    Actor::Agent,
                    map_core::surface::FaceId { solid, side },
                    snapshot.as_deref(),
                ),
                WorkplaneArgs::FromRay {
                    ray_origin,
                    ray_direction,
                    snapshot,
                } => session.workplane_from_ray(
                    Actor::Agent,
                    Ray {
                        origin: vec3(ray_origin),
                        direction: vec3(ray_direction),
                    },
                    snapshot.as_deref(),
                ),
                WorkplaneArgs::Reset { snapshot } => {
                    session.reset_workplane(Actor::Agent, snapshot.as_deref())
                }
            }
        })
    }

    #[tool(
        description = "Intersect a finite forward ray with the enabled reference plane. Returns world point, plane coordinates, distance and snapped status. Optional snap overrides the plane's setting; half_extent bounds only the displayed grid. Parallel, behind-ray or invalid/nonfinite results refuse explicitly; world coordinates must remain within ±65536. Does not pick scene surfaces or change ruler endpoints.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn intersect_workplane(
        &self,
        Parameters(args): Parameters<WorkplaneIntersectionArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .workplane_intersection(
                    Ray {
                        origin: vec3(args.ray_origin),
                        direction: vec3(args.ray_direction),
                    },
                    args.snap,
                )
                .map(|intersection| json!(intersection))
        })
    }
}
