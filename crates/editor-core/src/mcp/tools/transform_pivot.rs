use super::super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum TransformPivotPickArgs {
    Inspect,
    Begin {
        snapshot: String,
        width: u32,
        height: u32,
        camera: Option<CameraSpec>,
    },
    Ray {
        token: u64,
        ray_origin: [f64; 3],
        ray_direction: [f64; 3],
    },
    Pixel {
        token: u64,
        x: f64,
        y: f64,
        width: u32,
        height: u32,
    },
    Cancel {
        token: u64,
    },
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum TransformPivotGridArgs {
    Inspect {},
    Snap { snapshot: String },
}

#[tool_router(router = transform_pivot_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Snap the current effective shared transform pivot to the current grid, using the same Session operation as Snap pivot to grid in the UI. action=inspect returns snapshot, resolved before point, candidate after point, grid, changed=false and error without mutation. action=snap requires that latest snapshot and returns before/after/grid/changed plus the next snapshot. Selection-center mode requires a nonempty object selection with valid bounds for every object; world-origin and custom pivots can snap without a selection. Each world coordinate rounds to the nearest grid multiple, with half steps away from zero, even when Snap is off. Both points must be finite and within ±65536 units. Success fixes the pivot at the snapped world point, preserves all other transform settings and clears old surface-pick provenance even for an aligned point. changed reports a pivot coordinate or selection-center-mode change. Active tools, picks, previews, gestures, paint/component editing, builds, launches and closing reject without retiring their work. Context changes reject stale snapshots. No map, Undo history, selection, camera, grid or saved-startup-default writes. Numeric UI transforms and handles use the result; pass after explicitly to transform_objects when needed."
    )]
    pub(in crate::mcp) fn transform_pivot_grid(
        &self,
        Parameters(args): Parameters<TransformPivotGridArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let report = match args {
                TransformPivotGridArgs::Inspect {} => session.transform_pivot_grid_state(),
                TransformPivotGridArgs::Snap { snapshot } => {
                    session.snap_transform_pivot_to_grid(Actor::Agent, &snapshot)?
                }
            };
            serde_json::to_value(report).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Pick the shared custom transform pivot from an exact visible brush, terrain sheet or supported model surface, using the same operations as Pick pivot… in the UI. inspect returns snapshot, current_pivot, owned pending token/context validity and last_pick target/solid/side/point/normal provenance. begin requires latest snapshot, a nonempty object selection, width/height1–8192 and optional temporary camera matching render_view; rejects active previews/viewport tools/builds. ray requires the owned token and finite ray_origin/ray_direction; pixel requires the token and continuous x/y pixel positions with captured width/height (no implicit half-pixel offset). The first valid hit sets only session TransformTool.pivot, records provenance and ends picking. Selected objects remain valid targets. Misses/invalid/stale/foreign requests preserve the existing pivot and armed pick. cancel disarms an owned matching token even after stale context changes; it never overwrites later settings. Generation/revision/definitions/selection/tool/view/camera/viewport guards reject stale input. Desktop input is captured only for a fresh user-owned context. No snapping, workplane fallback, offsets, pivot entities, attachment, persistence, map/history/selection/camera/framing changes. Numeric UI transforms and shared handles use the resulting custom pivot. Pass current_pivot explicitly to transform_objects; its omitted-pivot selection-center behavior is unchanged. UI: Pivot → Pick pivot… → click a surface; Cancel pivot pick or Escape cancels an interactive pick."
    )]
    pub(in crate::mcp) fn transform_pivot_pick(
        &self,
        Parameters(args): Parameters<TransformPivotPickArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let state = match args {
                TransformPivotPickArgs::Inspect => session.transform_pivot_pick_state(),
                TransformPivotPickArgs::Begin {
                    snapshot,
                    width,
                    height,
                    camera,
                } => {
                    let maximum = crate::session::transform_pivot::MAX_VIEWPORT_DIMENSION;
                    if width == 0 || height == 0 || width > maximum || height > maximum {
                        return Err(
                            "Transform pivot viewport dimensions must be between 1 and 8192".into(),
                        );
                    }
                    let camera = camera
                        .unwrap_or_default()
                        .resolve(&session, width as f32 / height as f32)?;
                    session.begin_transform_pivot_pick(
                        Actor::Agent,
                        camera,
                        (width, height),
                        &snapshot,
                    )?
                }
                TransformPivotPickArgs::Ray {
                    token,
                    ray_origin,
                    ray_direction,
                } => session.pick_transform_pivot_ray(
                    Actor::Agent,
                    token,
                    Ray {
                        origin: vec3(ray_origin),
                        direction: vec3(ray_direction),
                    },
                )?,
                TransformPivotPickArgs::Pixel {
                    token,
                    x,
                    y,
                    width,
                    height,
                } => session.pick_transform_pivot_pixel(
                    Actor::Agent,
                    token,
                    crate::session::Pointer {
                        position: DVec2::new(x, y),
                        size: (width, height),
                    },
                )?,
                TransformPivotPickArgs::Cancel { token } => {
                    session.cancel_transform_pivot_pick(Actor::Agent, token)?
                }
            };
            serde_json::to_value(state).map_err(|error| error.to_string())
        })
    }
}
