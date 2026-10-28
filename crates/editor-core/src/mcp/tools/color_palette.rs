use super::super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ColorPaletteAction {
    Save,
    Replace,
    Rename,
    Delete,
    Reload,
    Use,
}

impl ColorPaletteAction {
    fn name(&self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Replace => "replace",
            Self::Rename => "rename",
            Self::Delete => "delete",
            Self::Reload => "reload",
            Self::Use => "use",
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ColorPaletteArgs {
    action: ColorPaletteAction,
    snapshot: String,
    name: Option<String>,
    new_name: Option<String>,
    color: Option<crate::color::Input>,
}

#[tool_router(router=color_palette_tools,vis="pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the shared persistent user color palette: up to 16 named normalized RGB colors, converted RGB/HSV/HSL values, action snapshot, ready/error, inspected storage path, write_committed and limits. No brightness is stored. An unavailable report may retain last known entries; ready=false blocks Use and writes. Reading does not reload external changes or edit the map.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_color_palette(&self) -> ToolResult {
        ok(self.session().color_palette_state())
    }

    #[tool(
        description = "Manage saved colors through the same Session actions as the RGB/HSV/HSL picker. Supply the latest get_color_palette.snapshot. save requires a new exact name and color; replace requires an existing name and color; rename requires an existing name plus a distinct unused new_name and forbids color; only rename accepts new_name; delete requires an existing name; reload takes neither name nor color and explicitly rereads storage; use requires a name and returns {color,palette} without editing any map or draft. Apply the returned normalized color through set_entity_color_space or the existing property draft operations when wanted. Save/Replace accept rgb255, rgb1, hsv or hsl and retain normalized precision. Names allow 1–48 characters/128 UTF-8 bytes without controls or surrounding whitespace. Save/Replace/Rename/Delete persist immediately as shared user settings, independently of a later map draft Cancel. Brightness stays independent. Exact inspected file and directory checks reject external changes until Reload; participating Linux writers use a nonblocking lock. Invalid/unreadable files are preserved and unavailable. Errors never imply a saved palette: inspect ready/error and write_committed, which distinguishes a completed write whose final verification failed. UI labels: Saved colors, Use saved color, Saved color name, Save new color, Replace saved color, Rename saved color, Delete saved color, Reload palette."
    )]
    pub(in crate::mcp) fn set_color_palette(
        &self,
        Parameters(args): Parameters<ColorPaletteArgs>,
    ) -> ToolResult {
        run(|| {
            if matches!(&args.action, ColorPaletteAction::Rename) {
                if args.color.is_some() {
                    return Err("Rename does not accept a color".into());
                }
                return self.session().rename_color_palette(
                    &args.snapshot,
                    args.name
                        .as_deref()
                        .ok_or("Choose an existing color to rename")?,
                    args.new_name
                        .as_deref()
                        .ok_or("Enter a new saved color name")?,
                );
            }
            if args.new_name.is_some() {
                return Err("Only Rename accepts new_name".into());
            }
            self.session().change_color_palette(
                args.action.name(),
                &args.snapshot,
                args.name.as_deref(),
                args.color,
            )
        })
    }
}
