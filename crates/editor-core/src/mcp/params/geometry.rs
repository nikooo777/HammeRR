use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct HollowArgs {
    /// Source faces to leave open; each source brush must keep at least one wall.
    #[serde(default)]
    pub(in crate::mcp) open_faces: Vec<FaceRef>,
    pub(in crate::mcp) ids: Option<Vec<String>>,
    /// Wall thickness along each face normal, from 0.125 to 4096 units.
    pub(in crate::mcp) thickness: f64,
    /// Grow outside the source (keep its interior size). Default: inset walls.
    #[serde(default)]
    pub(in crate::mcp) outward: bool,
    /// Latest stamp from get_hollow_preview.
    pub(in crate::mcp) stamp: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct HollowInsideArgs {
    pub(in crate::mcp) token: String,
    /// Zero-based cavity index from the preview result. Default: first cavity.
    #[serde(default)]
    pub(in crate::mcp) index: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct MergeArgs {
    /// Explicit numeric brush IDs, including children of one brush entity.
    pub(in crate::mcp) sources: Vec<u32>,
    /// Latest stamp from get_merge_preview.
    pub(in crate::mcp) stamp: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct CarveArgs {
    pub(in crate::mcp) cutter: u32,
    pub(in crate::mcp) targets: Vec<u32>,
    pub(in crate::mcp) stamp: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct CarveManyArgs {
    pub(in crate::mcp) cutters: Vec<u32>,
    pub(in crate::mcp) targets: Vec<u32>,
    pub(in crate::mcp) stamp: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateBlockArgs {
    /// Box minimum corner; give mins+maxs or center+size.
    pub mins: Option<[f64; 3]>,
    pub maxs: Option<[f64; 3]>,
    pub center: Option<[f64; 3]>,
    pub size: Option<[f64; 3]>,
    /// Material for all faces (default dev/dev_measuregeneric01).
    pub material: Option<String>,
    /// Snap corners using the shared grid settings; omitted preserves exact coordinates.
    #[serde(default)]
    pub snap: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DrawBlockArgs {
    /// First footprint corner in plane coordinates: XY for top, XZ for front, YZ for side.
    pub start: [f64; 2],
    pub end: [f64; 2],
    /// "top", "front", or "side"; omitted uses the current viewport projection.
    pub projection: Option<String>,
    pub elevation: Option<f64>,
    pub depth: Option<f64>,
    pub material: Option<String>,
    /// Show a non-mutating brush preview instead of creating it.
    #[serde(default)]
    pub preview: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MoveArgs {
    pub ids: Option<Vec<String>>,
    /// Translation [dx, dy, dz].
    pub offset: [f64; 3],
    /// Keep textures fixed to the brushes (default true).
    pub texture_lock: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TransformArgs {
    /// Object IDs; omitted uses the shared selection. Repeated IDs are applied once.
    pub ids: Option<Vec<String>>,
    /// Exact world offset [dx, dy, dz], applied last; defaults to zero.
    pub translation: Option<[f64; 3]>,
    /// Right-handed degrees around world [X, Y, Z], applied X then Y then Z; defaults to zero.
    pub rotation: Option<[f64; 3]>,
    /// Positive world-axis factors [sx, sy, sz], applied before rotation; defaults to [1,1,1].
    pub scale: Option<[f64; 3]>,
    /// Mirror chosen world/local axes about the pivot. Brushes and brush entities only; displacement brushes and point entities are rejected atomically.
    pub reflection: Option<[bool; 3]>,
    /// Shift one coordinate by factor times another coordinate about the pivot, after scale/reflection and before rotation. Brushes and brush entities only. Factor range -64..64.
    pub shear: Option<ShearArgs>,
    /// World pivot [x,y,z]; omitted uses the combined selection bounds center.
    pub pivot: Option<[f64; 3]>,
    /// Follow translation/rotation about the pivot; default true.
    pub texture_lock: Option<bool>,
    /// Stretch textures with scale about the pivot. Omitted inherits texture_lock for backward compatibility.
    pub texture_scale_lock: Option<bool>,
    /// world (default) or local; local follows the first selected object's frame.
    pub space: Option<String>,
    /// Draft geometry without changing the document or history; commit or cancel separately.
    #[serde(default)]
    pub preview: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TransformToolArgs {
    /// select, move, rotate or scale; omitted preserves the current tool.
    pub mode: Option<String>,
    /// world or local.
    pub space: Option<String>,
    /// selection, world or custom.
    pub pivot_mode: Option<String>,
    pub pivot: Option<[f64; 3]>,
    pub texture_lock: Option<bool>,
    /// Stretch textures with scale independently of movement locking. Omitted preserves the current mode, which initially inherits texture_lock.
    pub texture_scale_lock: Option<bool>,
    /// Rotation snapping in degrees, 0 disables it. Shared Snap also controls snapping.
    pub angle_step: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PaintToolArgs {
    pub enabled: Option<bool>,
    /// selection, material or projection. Texture strokes require a sampled source face.
    pub operation: Option<String>,
    /// Brush radius in screen pixels, from 2 to 64.
    pub radius: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BeginPaintArgs {
    pub pointer: PointerArgs,
    pub radius: Option<f64>,
    /// objects or faces; omitted uses the current selection mode.
    pub selection_mode: Option<String>,
    /// replace, add, remove or toggle; omitted uses the shared selection rule.
    pub mode: Option<String>,
    pub camera: Option<CameraSpec>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdatePaintArgs {
    pub token: String,
    /// Ordered screen-space points. Segments between them are sampled; at most 256 per call.
    pub points: Vec<[f64; 2]>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BeginTransformArgs {
    /// x/y/z, xy/xz/yz or view for movement, x/y/z for rotation, x/y/z or uniform for scaling.
    pub handle: String,
    /// Optional initial viewport point; omit to drive the handle with numeric amounts.
    pub pointer: Option<PointerArgs>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateTransformArgs {
    pub pointer: Option<PointerArgs>,
    /// Total offset from the start, in units for move, degrees for rotation or positive factor for scale.
    pub amount: Option<f64>,
    /// Total offsets along the two plane axes, or screen right/up for the view handle.
    pub plane_offset: Option<[f64; 2]>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct VolumeArgs {
    pub mins: [f64; 3],
    pub maxs: [f64; 3],
    /// objects or faces; defaults to the current selection mode. Preview does not change that mode.
    pub selection_mode: Option<String>,
    /// intersecting (default) or contained. Uses geometry, including enclosed volumes.
    pub rule: Option<String>,
    /// replace (default), add, remove or toggle.
    pub mode: Option<String>,
    #[serde(default)]
    pub preview: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct VolumeTokenArgs {
    pub token: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AreaOptionsArgs {
    /// intersecting or contained.
    pub rule: Option<String>,
    /// replace, add, remove or toggle.
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AreaArgs {
    /// Viewport pixels, with origin at the top left; reverse drags are supported.
    pub start: [f64; 2],
    pub end: [f64; 2],
    pub width: u32,
    pub height: u32,
    pub camera: Option<CameraSpec>,
    pub rule: Option<String>,
    pub mode: Option<String>,
    /// Preview candidate highlights and rectangle without changing selection or history.
    #[serde(default)]
    pub preview: bool,
}

pub(in crate::mcp) fn area_rule(value: &str) -> Result<map_core::region::Match, String> {
    match value {
        "contained" => Ok(map_core::region::Match::Contained),
        "intersecting" => Ok(map_core::region::Match::Intersecting),
        _ => Err("rule must be contained or intersecting".into()),
    }
}

pub(in crate::mcp) fn area_rule_name(rule: map_core::region::Match) -> &'static str {
    match rule {
        map_core::region::Match::Contained => "contained",
        map_core::region::Match::Intersecting => "intersecting",
    }
}

pub(in crate::mcp) fn transform_mode_name(mode: Option<render::handles::Mode>) -> &'static str {
    match mode {
        None => "select",
        Some(render::handles::Mode::Move) => "move",
        Some(render::handles::Mode::Rotate) => "rotate",
        Some(render::handles::Mode::Scale) => "scale",
    }
}

pub(in crate::mcp) fn transform_state(session: &Session) -> Value {
    let tool = session.transform_tool();
    json!({"settings": {"mode": transform_mode_name(tool.mode), "space": if tool.local {"local"} else {"world"}, "pivot": tool.pivot.map(|pivot| pivot.to_array()), "texture_lock": tool.texture_lock, "texture_scale_lock": tool.texture_scale_lock.unwrap_or(tool.texture_lock), "angle_step": tool.angle_step},
        "drag": session.transform_drag().map(|drag| json!({"handle": drag.handle.name(), "mode": transform_mode_name(Some(drag.mode)), "amount": drag.amount.to_array(), "pivot": drag.pivot.to_array(), "basis": drag.basis.to_cols_array_2d(), "camera": camera_json(&drag.camera)})),
        "preview": session.transform_preview().map(|preview| { let presentation = preview.presentation(); json!({"grid_snap": preview.grid_snap, "display": {"primary": if presentation.operations.is_empty() { "identity" } else { presentation.primary.name() }, "operations": presentation.operations.iter().map(|operation| operation.name()).collect::<Vec<_>>(), "summary": presentation.summary}, "ids": preview.ids.iter().copied().map(id_string).collect::<Vec<_>>(), "translation": preview.transform.translation.to_array(), "rotation": preview.transform.rotation.to_array(), "scale": preview.transform.scale.to_array(), "reflection": preview.transform.reflection, "shear": preview.transform.shear.map(|shear| json!({"axis": (["x", "y", "z"][shear.axis]), "reference": (["x", "y", "z"][shear.reference]), "factor": shear.factor})), "basis": preview.transform.basis.to_cols_array_2d(), "pivot": preview.transform.pivot.to_array(), "texture_lock": preview.transform.texture_lock, "texture_scale_lock": preview.transform.texture_scale_lock.unwrap_or(preview.transform.texture_lock), "bounds": bounds_json(session.object_bounds_in(&preview.document, &preview.ids)), "objects": preview.ids.iter().filter_map(|id| detail(&preview.document, *id).ok()).collect::<Vec<_>>()} )})})
}

pub(in crate::mcp) fn area_matches(matches: &map_core::region::Selection) -> Value {
    json!({"objects": matches.objects.iter().copied().map(id_string).collect::<Vec<_>>(), "faces": matches.faces.iter().map(|face| json!({"solid": face.solid, "side": face.side})).collect::<Vec<_>>()})
}

pub(in crate::mcp) fn area_state(session: &Session) -> Value {
    json!({"selection_filters":session.selection_filter_state(),"rule": area_rule_name(session.area_rule()), "mode": select_mode_name(session.area_mode()), "preview": session.area_preview().map(|preview| json!({"start": preview.start.position.to_array(), "end": preview.end.to_array(), "width": preview.start.size.0, "height": preview.start.size.1, "camera": camera_json(&preview.camera), "rule": area_rule_name(preview.rule), "mode": select_mode_name(preview.mode), "selection_mode": if preview.faces {"faces"} else {"objects"}, "matches": area_matches(&preview.matches)}))})
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FaceRef {
    pub solid: u32,
    pub side: u32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SelectFacesArgs {
    pub faces: Vec<FaceRef>,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectFaceGroupArgs {
    /// Omit to use the current selected faces.
    pub faces: Option<Vec<FaceRef>>,
    /// face (default): coplanar fitting; view: parallel projection along the current camera direction.
    pub projection: Option<String>,
    /// Face supplying UV directions and mirroring; defaults to the first sorted solid/side pair. Only for face projection.
    pub reference: Option<FaceRef>,
    /// Omit to keep the common material. Mixed materials require an explicit shared material.
    pub material: Option<String>,
    /// Horizontal/vertical tile counts across combined bounds, each greater than 0 and at most 1024. Default [1,1].
    pub repeats: Option<[f64; 2]>,
    #[serde(default)]
    pub preview: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VaryFaceUvArgs {
    pub faces: Option<Vec<FaceRef>>,
    pub seed: u32,
    pub max_shift_u: f64,
    pub max_shift_v: f64,
    #[serde(default)]
    pub preview: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EditFacesArgs {
    pub faces: Option<Vec<FaceRef>>,
    pub material: Option<String>,
    pub shift_u: Option<f64>,
    pub shift_v: Option<f64>,
    pub scale_u: Option<f64>,
    pub scale_v: Option<f64>,
    pub rotation: Option<f64>,
    /// Direct world-space U vector, without normalization. Null components keep each face's value. Cannot combine with alignment or rotation; scales/texel offsets stay independent.
    pub u_axis: Option<[Option<f64>; 3]>,
    /// Direct world-space V vector, allowing skew. Null components preserve values. Must span every target face together with U; no fallback projection.
    pub v_axis: Option<[Option<f64>; 3]>,
    /// World units per lighting pixel, 1–1024. Smaller values give denser authored lightmaps.
    pub lightmap_scale: Option<u32>,
    /// Replace groups with these numbers (1–32); [] clears. 25–32 are Source hard-edge groups.
    pub smoothing_groups: Option<Vec<u32>>,
    /// Add groups while preserving other memberships on each face.
    pub smoothing_add: Option<Vec<u32>>,
    /// Remove groups while preserving other memberships on each face.
    pub smoothing_remove: Option<Vec<u32>>,
    /// world or face. Resets the projection basis and rotation before optional rotation is applied.
    pub alignment: Option<String>,
    /// left, center or right within one material tile; changes offset only, after fit.
    pub justify_u: Option<String>,
    /// top, center or bottom within one material tile; changes offset only, after fit.
    pub justify_v: Option<String>,
    pub fit: Option<bool>,
    pub preview: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SampleFaceArgs {
    /// Omit to sample the single selected face. Captures committed values, not a preview.
    pub face: Option<FaceRef>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PaintFacesArgs {
    pub faces: Option<Vec<FaceRef>>,
    /// material (default), projection (copy world-space UVs), or wrap (rotate UVs across a shared edge).
    pub mode: Option<String>,
    /// Use commit_face_preview or cancel_face_preview to finish the shared draft.
    #[serde(default)]
    pub preview: bool,
}

pub(in crate::mcp) fn surface_sample_state(session: &Session) -> Value {
    session.surface_sample().map_or(Value::Null, |sample| json!({
        "face": {"solid": sample.face.solid, "side": sample.face.side},
        "material": sample.material, "uaxis": sample.projection.u.to_string(),
        "vaxis": sample.projection.v.to_string(), "rotation": sample.projection.rotation,
        "normal": sample.normal.to_array(), "vertices": sample.vertices.iter().map(|v| v.to_array()).collect::<Vec<_>>(),
        "displacement": sample.displaced,
        "source_unchanged": map_core::surface::sample(session.document(), sample.face).ok().as_ref() == Some(sample),
    }))
}

pub(in crate::mcp) fn face_id(face: &FaceRef) -> map_core::surface::FaceId {
    map_core::surface::FaceId {
        solid: face.solid,
        side: face.side,
    }
}

pub(in crate::mcp) fn face_state(session: &Session) -> Value {
    json!({"handle": session.face_preview_handle(), "mode": if session.face_mode() { "faces" } else { "objects" }, "preview": session.has_face_preview(),
        "preview_token":session.face_preview_token().map(|token|token.to_string()),
        "atlas":session.atlas_fit_state(),
        "sample": surface_sample_state(session),
        "preview_faces": session.preview_faces().into_iter().filter_map(|face| {
            let side = session.preview_side(face)?;
            Some(json!({"solid": face.solid, "side": face.side, "material": side.material,
                "shift_u": side.u_axis.offset, "shift_v": side.v_axis.offset,
                "scale_u": side.u_axis.scale, "scale_v": side.v_axis.scale, "rotation": side.rotation,
                "u_axis": side.u_axis.axis.to_array(), "v_axis": side.v_axis.axis.to_array(),
                "lightmap_scale": side.lightmap_scale, "smoothing_groups": map_core::surface::smoothing_numbers(side.smoothing_groups.unwrap_or(0)), "smoothing_mask": side.smoothing_groups.unwrap_or(0),
                "uaxis": side.u_axis.to_string(), "vaxis": side.v_axis.to_string()}))
        }).collect::<Vec<_>>(),
        "faces": session.selected_faces().into_iter().filter_map(|face| {
            let side = map_core::surface::side(session.document(), face)?;
            Some(json!({"solid": face.solid, "side": face.side, "owner": session.document().owner_of_solid(face.solid).map(id_string),
                "material": side.material, "shift_u": side.u_axis.offset, "shift_v": side.v_axis.offset,
                "scale_u": side.u_axis.scale, "scale_v": side.v_axis.scale, "rotation": side.rotation,
                "u_axis": side.u_axis.axis.to_array(), "v_axis": side.v_axis.axis.to_array(),
                "lightmap_scale": side.lightmap_scale, "smoothing_groups": map_core::surface::smoothing_numbers(side.smoothing_groups.unwrap_or(0)), "smoothing_mask": side.smoothing_groups.unwrap_or(0),
                "uaxis": side.u_axis.to_string(), "vaxis": side.v_axis.to_string(),
                "preview": session.preview_side(face).map(|side| json!({"material": side.material, "shift_u": side.u_axis.offset, "shift_v": side.v_axis.offset, "scale_u": side.u_axis.scale, "scale_v": side.v_axis.scale, "rotation": side.rotation,
                "u_axis": side.u_axis.axis.to_array(), "v_axis": side.v_axis.axis.to_array(), "lightmap_scale": side.lightmap_scale, "smoothing_groups": map_core::surface::smoothing_numbers(side.smoothing_groups.unwrap_or(0))}))}))
        }).collect::<Vec<_>>()})
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClipLine {
    pub projection: String,
    pub start: [f64; 2],
    pub end: [f64; 2],
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClipPlane {
    pub normal: Option<[f64; 3]>,
    pub distance: Option<f64>,
    /// Three points in VMF winding: (p0-p1) cross (p2-p1) points toward front.
    pub points: Option<[[f64; 3]; 3]>,
    /// A planar cut gesture using the shared grid, snapping and workplane.
    pub line: Option<ClipLine>,
}

impl ClipPlane {
    pub(in crate::mcp) fn resolve(self, session: &Session) -> Result<Plane, String> {
        match (self.normal, self.distance, self.points, self.line) {
            (Some(normal), Some(dist), None, None) => map_core::clip::normalized_plane(Plane {
                normal: vec3(normal),
                dist,
            })
            .map_err(|error| error.to_string()),
            (None, None, Some(points), None) => {
                let points = points.map(vec3);
                if points.iter().any(|point| !point.is_finite()) {
                    return Err("Cut points must be finite".into());
                }
                let plane = Plane::from_points(points).ok_or("Cut points must not be collinear")?;
                map_core::clip::normalized_plane(plane).map_err(|error| error.to_string())
            }
            (None, None, None, Some(line)) => session.clip_line_plane(
                Projection::parse(&line.projection)
                    .ok_or("projection must be top, front or side")?,
                glam::DVec2::from_array(line.start),
                glam::DVec2::from_array(line.end),
            ),
            _ => Err(
                "Provide exactly one cut plane: normal + distance, three points, or a planar line"
                    .into(),
            ),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClipArgs {
    pub ids: Option<Vec<String>>,
    pub plane: ClipPlane,
    /// front is the arrow/positive-normal side, back the opposite; defaults to both.
    pub keep: Option<String>,
    /// New cap material; omitted inherits each source brush's first material.
    pub material: Option<String>,
    #[serde(default)]
    pub preview: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClipToolArgs {
    pub enabled: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ClipHandleOptionsArgs {
    /// move, rotate or off; world axes, with pivot projected onto the cut plane.
    pub(in crate::mcp) mode: Option<String>,
    pub(in crate::mcp) snap: Option<bool>,
    /// Degrees per rotation step when snapping; 0.1 through 180.
    pub(in crate::mcp) angle_step: Option<f64>,
    pub(in crate::mcp) snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct UpdateClipDragArgs {
    pub(in crate::mcp) token: String,
    pub(in crate::mcp) pointer: Option<PointerArgs>,
    /// Total amounts since begin: world units for move, degrees for rotate. Axis/ring second amount must be zero.
    pub(in crate::mcp) amount: Option<[f64; 2]>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(in crate::mcp) struct ClipPointsArgs {
    /// Zero to three world points; three define a VMF-wound plane and preview the cut.
    pub(in crate::mcp) points: Vec<[f64; 3]>,
    pub(in crate::mcp) keep: Option<String>,
    /// Empty string restores inheritance; omitted retains the existing choice.
    pub(in crate::mcp) material: Option<String>,
    /// Use current grid/snap; defaults to true on a new draft.
    pub(in crate::mcp) snap: Option<bool>,
    pub(in crate::mcp) snapshot: String,
}

pub(in crate::mcp) fn clip_result(result: &map_core::clip::ClipResult) -> Value {
    json!({"cuts": result.cuts.iter().map(|cut| json!({"source": cut.source, "solids": cut.solids})).collect::<Vec<_>>(), "objects": result.objects.iter().copied().map(id_string).collect::<Vec<_>>(), "unchanged": result.unchanged})
}

pub(in crate::mcp) fn clip_state(session: &Session) -> Value {
    json!({"enabled": session.clip_tool(), "snapshot":session.clip_point_stamp(), "point_draft":session.clip_points_report(), "handle_options":session.clip_handles_report(), "drag":session.clip_drag_report(), "preview": session.clip_preview().map(|preview| json!({
        "plane": {"normal": preview.plane.normal.to_array(), "distance": preview.plane.dist},
        "keep": match preview.keep { map_core::clip::Keep::Front => "front", map_core::clip::Keep::Back => "back", map_core::clip::Keep::Both => "both" },
        "material": preview.material, "ids": preview.ids.iter().copied().map(id_string).collect::<Vec<_>>(),
        "result": clip_result(&preview.result),
        "pieces": preview.result.cuts.iter().flat_map(|cut| &cut.solids)
            .filter_map(|id| map_core::surface::solid(&preview.document, *id))
            .map(solid_detail).collect::<Vec<_>>()
    }))})
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClipboardCaptureArgs {
    pub ids: Option<Vec<String>>,
    pub snapshot: String,
    /// session (default) or system. System publication must succeed before Cut deletes anything.
    pub destination: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClipboardImportArgs {
    /// UTF-8 VMF text, at most 8 MiB. Omit only when system is true.
    pub text: Option<String>,
    pub system: Option<bool>,
    pub snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClipboardExportArgs {
    pub content_revision: u64,
    /// response (default) or system. System publication changes the computer's clipboard.
    pub destination: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SurfaceCornerRef {
    pub solid: u32,
    pub sides: Vec<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SurfacePlacementSourcesArgs {
    pub ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SurfacePlacementArgs {
    pub ids: Option<Vec<String>>,
    pub method: Option<String>,
    pub ray_origin: Option<[f64; 3]>,
    pub ray_direction: Option<[f64; 3]>,
    #[serde(default)]
    pub align: bool,
    pub offset: Option<f64>,
    pub texture_lock: Option<bool>,
    pub source_corner: Option<SurfaceCornerRef>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolygonArgs {
    /// Boundary order; closing corner is implicit. Simple concave outlines are decomposed.
    pub outline: Vec<[f64; 2]>,
    /// x draws in YZ, y in XZ, z (default) in XY.
    pub axis: Option<String>,
    pub base: f64,
    /// Positive extrusion distance, at least 0.125 units.
    pub depth: f64,
    pub material: Option<String>,
    /// Snap corners to the current shared grid; defaults to current snapping.
    pub snap: Option<bool>,
    #[serde(default)]
    pub preview: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PrimitiveArgs {
    /// wedge, cylinder, cone, sphere, arch or polygon. Editable convex VMF brushes.
    pub shape: String,
    pub mins: [f64; 3],
    pub maxs: [f64; 3],
    /// Extrusion/pole axis: x, y or z (default z). A y-axis arch lies in XZ.
    pub axis: Option<String>,
    /// Curved-shape sides/arch pieces, 3..64; default 12.
    pub segments: Option<u32>,
    /// Sphere latitude bands, 2..32; segments * bands <= 256; default 5.
    pub bands: Option<u32>,
    /// Arch inset subtracted from both ellipse radii; default 16 units.
    pub inset: Option<f64>,
    pub start_angle: Option<f64>,
    /// Arch angular span, 1..360 degrees; default 180.
    pub sweep_angle: Option<f64>,
    pub material: Option<String>,
    /// Polygon world-plane corners (YZ/XZ/XY by axis); supplied bounds must agree.
    pub outline: Option<Vec<[f64; 2]>>,
    /// Preview without changing the map, selection or history; default false.
    #[serde(default)]
    pub preview: bool,
}

impl PrimitiveArgs {
    pub(in crate::mcp) fn spec(&self) -> Result<map_core::primitive::Spec, String> {
        use map_core::primitive::{Axis, Shape, Spec};
        let mut spec = Spec {
            shape: Shape::parse(&self.shape).map_err(|error| error.to_string())?,
            mins: vec3(self.mins),
            maxs: vec3(self.maxs),
            axis: Axis::parse(self.axis.as_deref().unwrap_or("z"))
                .map_err(|error| error.to_string())?,
            ..Spec::default()
        };
        if let Some(value) = self.segments {
            spec.segments = value;
        }
        if let Some(value) = self.bands {
            spec.bands = value;
        }
        if let Some(value) = self.inset {
            spec.inset = value;
        }
        if let Some(value) = self.start_angle {
            spec.start_angle = value;
        }
        if let Some(value) = self.sweep_angle {
            spec.sweep_angle = value;
        }
        if let Some(value) = &self.material {
            spec.material = value.clone();
        }
        if let Some(outline) = &self.outline {
            spec.outline = outline
                .iter()
                .copied()
                .map(glam::DVec2::from_array)
                .collect();
        }
        Ok(spec)
    }
}

#[cfg(test)]
mod surface_corner_tests {
    use super::*;

    #[test]
    fn surface_corner_requests_preserve_explicit_identity_and_reject_raw_coordinates() {
        let valid = json!({"method":"brush_corner","source_corner":{"solid":12,"sides":[31,32,35]},"ray_origin":[1,2,3],"ray_direction":[0,0,-1]});
        let parsed = serde_json::from_value::<SurfacePlacementArgs>(valid.clone()).unwrap();
        assert_eq!(parsed.source_corner.unwrap().sides, [31, 32, 35]);
        let begin =
            json!({"method":"brush_corner","source_corner":{"solid":12,"sides":[31,32,35]}});
        assert!(serde_json::from_value::<BeginSurfacePlacementPickArgs>(begin).is_ok());
        for corner in [
            json!({"solid":12}),
            json!({"sides":[31,32,35]}),
            json!({"solid":12,"sides":[31,32,35],"position":[1,2,3]}),
        ] {
            let mut invalid = valid.clone();
            invalid["source_corner"] = corner;
            assert!(serde_json::from_value::<SurfacePlacementArgs>(invalid).is_err());
        }
    }
}

#[cfg(test)]
mod vector_tests {
    use super::*;

    #[test]
    fn direct_vector_parameters_accept_full_and_component_preserving_axes() {
        let args: EditFacesArgs = serde_json::from_value(
            json!({"u_axis": [2.0, null, 0.125], "v_axis": [0.25, -1.5, 0.0], "preview": true}),
        )
        .unwrap();
        assert_eq!(args.u_axis, Some([Some(2.0), None, Some(0.125)]));
        assert_eq!(args.v_axis, Some([Some(0.25), Some(-1.5), Some(0.0)]));
        for invalid in [
            json!({"u_axis": [1.0, 0.0]}),
            json!({"v_axis": [0.0, 1.0, 0.0, 2.0]}),
            json!({"u_axis": ["1", 0.0, 0.0]}),
        ] {
            assert!(serde_json::from_value::<EditFacesArgs>(invalid).is_err());
        }
    }
}
