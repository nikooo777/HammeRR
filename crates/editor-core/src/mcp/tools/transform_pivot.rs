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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum TransformPivotAttachmentArgs {
    Inspect {},
    AttachOrigin {
        object: Option<String>,
        snapshot: String,
    },
    AttachLocal {
        object: Option<String>,
        snapshot: String,
    },
    Detach {
        snapshot: String,
    },
    WorldPoint {
        point: [f64; 3],
    },
}

#[tool_router(router = transform_pivot_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect or change the shared transform pivot's point-entity attachment. inspect returns attachment, resolved world point or explicit error, and snapshot. attach_origin and attach_local require the latest snapshot and optional canonical object ID; omitted object requires exactly one selected valid point entity. Origin follows that entity's origin; local captures the current resolved world pivot in that entity's Source orientation frame and follows its later origin/orientation. detach requires snapshot and fixes the current resolved point in world space. Missing or invalid anchors reject resolution and dependent transforms without a selection-center or cached-point fallback. world_point requires explicit finite point coordinates and replaces the attachment through shared transform controls, including recovery when it cannot resolve. Actions use the same Session operations as Attach origin, Attach local point, Detach pivot and Use world point. Strict action fields reject incompatible arguments. No geometry, Undo, camera or saved-default writes; broader surface, brush and general anchors remain unavailable."
    )]
    pub(in crate::mcp) fn transform_pivot_attachment(
        &self,
        Parameters(args): Parameters<TransformPivotAttachmentArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let report = match args {
                TransformPivotAttachmentArgs::Inspect {} => {
                    session.transform_pivot_attachment_state()
                }
                TransformPivotAttachmentArgs::AttachOrigin { object, snapshot } => session
                    .attach_transform_pivot_origin(
                        Actor::Agent,
                        object.as_deref().map(parse_id).transpose()?,
                        &snapshot,
                    )?,
                TransformPivotAttachmentArgs::AttachLocal { object, snapshot } => session
                    .attach_transform_pivot_local(
                        Actor::Agent,
                        object.as_deref().map(parse_id).transpose()?,
                        &snapshot,
                    )?,
                TransformPivotAttachmentArgs::Detach { snapshot } => {
                    session.detach_transform_pivot(Actor::Agent, &snapshot)?
                }
                TransformPivotAttachmentArgs::WorldPoint { point } => {
                    let mut tool = session.transform_tool();
                    tool.pivot = Some(vec3(point));
                    session.set_transform_tool_with_pivot(Actor::Agent, tool, true)?;
                    session.transform_pivot_attachment_state()
                }
            };
            serde_json::to_value(report).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Snap the current effective shared transform pivot to the current grid, using the same Session operation as Snap pivot to grid in the UI. action=inspect returns snapshot, resolved before point, candidate after point, grid, changed=false and error without mutation. action=snap requires that latest snapshot and returns before/after/grid/changed plus the next snapshot. Selection-center mode requires a nonempty object selection with valid bounds for every object; world-origin and custom pivots can snap without a selection. Each world coordinate rounds to the nearest grid multiple, with half steps away from zero, even when Snap is off. Both points must be finite and within ±65536 units. Success fixes the pivot at the snapped world point, detaches any entity anchor, preserves other transform settings and clears old surface-pick provenance even for an aligned point. changed reports a coordinate, selection-center-mode or attachment change. Active tools, picks, previews, gestures, paint/component editing, builds, launches and closing reject without retiring their work. Context changes reject stale snapshots. No map, Undo history, selection, camera, grid or saved-startup-default writes. Numeric UI transforms and handles use the result; use pivot_source=current_transform_pivot with transform_objects to resolve the current tool pivot explicitly."
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
        description = "Pick the shared custom transform pivot from an exact visible brush, terrain sheet or supported model surface, using the same operations as Pick pivot… in the UI. inspect returns snapshot, current_pivot, owned pending token/context validity and last_pick target/solid/side/point/normal provenance. begin requires latest snapshot, a nonempty object selection, width/height1–8192 and optional temporary camera matching render_view; rejects active previews/viewport tools/builds. ray requires the owned token and finite ray_origin/ray_direction; pixel requires the token and continuous x/y pixel positions with captured width/height (no implicit half-pixel offset). The first valid hit replaces any attachment with a fixed world pivot, records provenance and ends picking. Selected objects remain valid targets. Misses/invalid/stale/foreign requests preserve the existing pivot and armed pick. cancel disarms an owned matching token even after stale context changes; it never overwrites later settings. Generation/revision/definitions/selection/tool/view/camera/viewport guards reject stale input. Desktop input is captured only for a fresh user-owned context. The surface pick remains a fixed world point; use transform_pivot_attachment to attach it to a point entity. No snapping, workplane fallback, persistence or map/history/selection/camera/framing changes. Numeric UI transforms and shared handles use the resulting custom pivot. Use pivot_source=current_transform_pivot with transform_objects; omitted source/pivot retains its selection-center behavior. UI: Pivot → Pick pivot… → click a surface; Cancel pivot pick or Escape cancels an interactive pick."
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
