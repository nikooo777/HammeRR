use super::super::*;
use crate::color_palette::{PaletteChange, PaletteSource};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ColorPaletteQuery {
    #[serde(default)]
    source: PaletteSource,
}

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
    #[serde(default)]
    source: PaletteSource,
    action: ColorPaletteAction,
    snapshot: String,
    name: Option<String>,
    new_name: Option<String>,
    color: Option<crate::color::Input>,
    property_token: Option<u64>,
}

#[tool_router(router=color_palette_tools,vis="pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect source=user (default) for shared persistent user colors, or source=map for this document's undoable saved colors in VMF editor metadata. Each source holds up to 16 exact named normalized RGB colors with RGB/HSV/HSL conversions. Reports include source, action snapshot, ready/error, path and limits. Map metadata errors return ready=false and entries=null, never a fabricated empty palette; malformed and unsupported metadata stays preserved. An unavailable user report may retain last known entries. ready=false blocks Use and writes. No brightness is stored. Reading does not reload external files or edit any map/draft.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_color_palette(
        &self,
        Parameters(args): Parameters<ColorPaletteQuery>,
    ) -> ToolResult {
        ok(self.session().color_palette_state_for(args.source))
    }

    #[tool(
        description = "Manage source=user (default) or source=map through the same Session actions as both color pickers. Supply the latest snapshot for that source. save requires a new exact name and color; replace requires an existing name and color; rename requires an existing name and distinct unused new_name, without color; only rename accepts new_name; delete requires an existing name. reload takes no name, new_name, color or property_token: it rereads user storage or refreshes current map metadata. use requires a name and returns {color,palette} without editing a map/draft; apply the returned color separately through set_entity_color_space or property draft operations. Save/Replace accept rgb255, rgb1, hsv or hsl and preserve normalized precision. Names allow 1–48 characters/128 UTF-8 bytes without controls or surrounding whitespace. Map writes are one undoable metadata edit; save_map persists them, Undo/Redo restores them, and map replacement resets them. Active geometry drafts/gestures refuse writes without cancelling them. If a property draft exists, map writes require its current property_token; only that valid draft's revision advances, preserving pending changes. Stale/replaced drafts refuse. User actions and Use/reload forbid property_token. User writes persist immediately through existing coordinated preference storage; external changes require Reload palette. Cancelling a color/property draft does not undo either source's explicit palette edits. Brightness stays independent. Invalid/unsupported storage is preserved and unavailable. User write_committed distinguishes a completed write whose final verification failed. UI labels: Palette source (User palette / Map palette), Saved colors, Saved color name, Save new color, Replace saved color, Rename saved color, Delete saved color, Reload palette / Refresh palette."
    )]
    pub(in crate::mcp) fn set_color_palette(
        &self,
        Parameters(args): Parameters<ColorPaletteArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().change_color_palette_for(
                Actor::Agent,
                args.source,
                PaletteChange {
                    action: args.action.name(),
                    snapshot: &args.snapshot,
                    name: args.name.as_deref(),
                    color: args.color,
                    new_name: args.new_name.as_deref(),
                    property_token: args.property_token,
                },
            )
        })
    }
}
