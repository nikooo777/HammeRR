use super::super::*;
use crate::session::reference_images::Settings;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddReferenceImageArgs {
    path: String,
    name: String,
    snapshot: String,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateReferenceImageArgs {
    id: u32,
    snapshot: String,
    settings: Settings,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReferenceImageArgs {
    id: u32,
    snapshot: String,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReloadReferenceImageArgs {
    id: u32,
    snapshot: String,
    path: Option<String>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameReferenceImageArgs {
    id: u32,
    snapshot: String,
    aspect: f32,
}

#[tool_router(router=reference_image_tools,vis="pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect editor-only reference image planes, their settings, file paths, saved SHA256/dimensions, actual decoded source readiness/errors and action snapshot. Rechecks bounded original source metadata and invalidates changed/missing images; cached failures require explicit Reload image. Malformed/unsupported map metadata reports images=null and an error, never an empty recovery palette. Static PNG only; at most 8 planes, 4096 pixels per axis, 4 MP/image, 16 MP/map decoded RGBA8, 16 MiB/file. No game entities/geometry, image embedding or automatic packaging.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_reference_images(&self) -> ToolResult {
        ok(self.session().reference_images_state())
    }

    #[tool(
        description = "Add reference image through View → Reference images… → Add image. Pass name, physical PNG path and latest get_reference_images snapshot. Completely decodes bounded static PNG pixels and captures SHA256 before one undoable metadata edit. Starts centered at world origin in XY, longest edge256 units with image aspect, opacity0.5, no flips, shown in Top/Front/Side/Perspective. Actual image plane is two-sided, unlit and depth-tested; not a pickable map entity. Failed admission preserves source/history. Apply/cancel other drafts and gestures first. Save the map to persist; absolute source path remains external."
    )]
    pub(in crate::mcp) fn add_reference_image(
        &self,
        Parameters(args): Parameters<AddReferenceImageArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().add_reference_image(
                Actor::Agent,
                Path::new(&args.path),
                &args.name,
                &args.snapshot,
            )
        })
    }
    #[tool(
        description = "Apply reference image name, position XYZ in Source units, rotation XYZ Euler degrees from an XY plane (right+X/up+Y), positive width/height size in units, opacity0..1, flip[horizontal,vertical], and views[Top,Front,Side,Perspective]. Pass id/latest snapshot and complete settings. All finite values are validated atomically; exact names unique. One Undo edit; geometry/entity properties/selection stay unchanged. Source pixels/path/identity are unchanged. Other drafts/gestures refuse without cancellation."
    )]
    pub(in crate::mcp) fn update_reference_image(
        &self,
        Parameters(args): Parameters<UpdateReferenceImageArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().update_reference_image(
                Actor::Agent,
                args.id,
                args.settings,
                &args.snapshot,
            )
        })
    }
    #[tool(
        description = "Reload or relink one reference PNG using id/latest snapshot and optional replacement path. Fully decode and explicitly accept the new saved SHA256/dimensions in one undoable metadata edit while preserving pose/size/opacity/flips/views. Missing/invalid input never changes metadata/history and invalidates the old displayed image with an explicit error. Undo restores the old metadata identity, not external file bytes; changed old files remain unavailable. Use Reload after external edits or a missing source returns."
    )]
    pub(in crate::mcp) fn reload_reference_image(
        &self,
        Parameters(args): Parameters<ReloadReferenceImageArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().reload_reference_image(
                Actor::Agent,
                args.id,
                args.path.as_deref().map(Path::new),
                &args.snapshot,
            )
        })
    }
    #[tool(
        description = "Delete one editor reference image by id/latest snapshot in one Undo step; retains other metadata and source files. Save map persists removal. Other drafts/gestures refuse without cancellation."
    )]
    pub(in crate::mcp) fn delete_reference_image(
        &self,
        Parameters(args): Parameters<ReferenceImageArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .delete_reference_image(Actor::Agent, args.id, &args.snapshot)
        })
    }
    #[tool(
        description = "Frame the four saved world-space corners of a reference image using id/latest snapshot and finite viewport aspect0.01..100. Keeps map/history/selection unchanged; visible image still depends on source readiness, projection flags, opacity and current viewing direction. Active viewport gestures refuse."
    )]
    pub(in crate::mcp) fn frame_reference_image(
        &self,
        Parameters(args): Parameters<FrameReferenceImageArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().frame_reference_image(
                Actor::Agent,
                args.id,
                &args.snapshot,
                args.aspect,
            )?;
            Ok(json!({"framed":args.id}))
        })
    }
}
