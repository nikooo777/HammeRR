use super::super::*;

#[tool_router(router = measurement_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect saved named rulers and the owned preview: resolved world endpoints, distance, signed XYZ components, attachments, availability/errors, snapshot and pick_snapshot. Attachments include point-entity origins/local points and durable brush/terrain surface points. Unavailable anchors return null resolved points/distance and retain stored coordinates; guides are suppressed. Malformed metadata stays preserved and blocks edits. Saved rulers are editor-only VMF metadata with Undo and Save/reopen support. UI: Measurements….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_measurements(
        &self,
        Parameters(_args): Parameters<MeasurementsInspectArgs>,
    ) -> ToolResult {
        ok(self.session().measurement_state())
    }

    #[tool(
        description = "Edit the shared ruler draft. preview takes name, start/end, snapshot and optional saved id. Endpoints are finite, distinct and within ±65536; up to 128 rulers with unique 1–80-character names. review transfers ownership; attachment edits and picks return a new token. apply revalidates and saves one Undo step; cancel accepts owned stale cleanup. attach_origin/attach_local take token, endpoint and object; attach_surface takes token/endpoint from the last owned surface pick and preserves that exact picked brush/terrain surface point using durable authored topology and triangle weights. Changed topology or missing anchors remain explicitly unavailable. detach (legacy detach_origin) freezes a resolvable anchor; world_endpoint replaces any anchor, including unavailable ones, with an explicit point. begin_pick takes token, unattached endpoint, mode surface|plane, width/height and latest pick_snapshot; optional camera matches render_view. Surface is default and picks visible geometry without snapping; plane explicitly intersects the reference plane and uses its snapping, without surface fallback. pick_ray/pick_pixel complete the captured mode; pixels use captured dimensions at pixel center. Failed or stale picks preserve the draft. cancel_pick ends picking; Apply rejects while armed. Surface attachments persist through Save/reopen and Undo; model-mesh attachments are unsupported. delete/view/frame retain snapshot guards; Frame changes only camera. UI: Measurements…."
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
                MeasurementArgs::AttachSurface { token, endpoint } => {
                    session.attach_measurement_surface(Actor::Agent, token, endpoint)
                }
                MeasurementArgs::Detach { token, endpoint }
                | MeasurementArgs::DetachOrigin { token, endpoint } => {
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
                    mode,
                    width,
                    height,
                    camera,
                    snapshot,
                } => {
                    let camera = camera
                        .unwrap_or_default()
                        .resolve(&session, width as f32 / height as f32)?;
                    session.begin_measurement_pick_mode(
                        Actor::Agent,
                        token,
                        endpoint,
                        mode.unwrap_or(crate::session::measurements::PickMode::Surface),
                        (camera, (width, height)),
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
