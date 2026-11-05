use super::*;

pub(in crate::mcp) fn fog_controller_id(controller: Option<&str>) -> Result<Option<u32>, String> {
    controller
        .map(|id| match parse_id(id)? {
            ObjectId::Entity(id) => Ok(id),
            _ => Err("Fog controller must be an entity:<id>".into()),
        })
        .transpose()
}

pub(in crate::mcp) fn fog_overrides(
    enabled: Option<bool>,
    controller: Option<&str>,
    selection: Option<crate::session::fog::SelectionMode>,
    channel: Option<crate::session::fog::Channel>,
) -> Result<crate::session::fog::Overrides, String> {
    Ok(crate::session::fog::Overrides {
        enabled,
        controller: fog_controller_id(controller)?,
        selection,
        channel,
    })
}

pub(in crate::mcp) fn fog_parameters(view: ViewOptions) -> Value {
    view.fog.map_or(Value::Null, |fog| {
        let controller = fog.controller_profile();
        let water = fog.water_settings();
        json!({
            "kind":if water.is_some(){"water"}else{"controller"},
            "enabled":fog.enabled(),"max_density":fog.max_density(),"far_z":fog.far_z(),
            "controller":{
                "color_domain":"float_byte_rgb", "color":controller.color(),
                "color2":controller.color2(),"direction":controller.direction(),
                "received_direction":controller.received_direction(),
                "start":controller.start(),"end":controller.end(),
                "max_density":controller.max_density(),"enabled":controller.enabled(),
                "radial":controller.radial(),"blend":controller.blend(),
            },
            "water":water.map(|water|json!({
                "color_domain":"normalized_material_rgb","color":water.normalized_rgb(),
                "start":water.start(),"end":water.end(),"enabled":water.enabled(),
            })),
        })
    })
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FogPreviewArgs {
    pub enabled: bool,
    /// Explicit env_fog_controller entity:<id>. Required when enabling through this compatibility operation.
    pub controller: Option<String>,
    /// Current opaque snapshot from get_fog_preview.
    pub snapshot: String,
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FogPreviewAction {
    Settings,
    PrepareEnvironment,
    Input,
    Play,
    Pause,
    Seek,
    Advance,
    Reset,
    Cancel,
    Clear,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FogPreviewSettingsArgs {
    pub enabled: bool,
    /// Explicit env_fog_controller entity:<id>, or null to clear the explicit choice.
    pub controller: Option<String>,
    pub selection: crate::session::fog::SelectionMode,
    /// LDR is qualified; HDR requests remain explicitly unavailable in the current native profile.
    pub channel: crate::session::fog::Channel,
    /// Initial controller radial context: authored or css_official_map; omitted preserves the current setting.
    pub initial_radial_context: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FogPreviewControlArgs {
    pub action: FogPreviewAction,
    /// Current opaque snapshot from get_fog_preview.
    pub expected_snapshot: String,
    /// Settings only: complete shared settings, including explicit selection and channel.
    pub settings: Option<FogPreviewSettingsArgs>,
    /// Input only: env_fog_controller entity:<id> receiving the native input.
    pub controller: Option<String>,
    /// Input only: exact native input name from the shared controller input catalogue.
    pub input: Option<String>,
    /// Input only: raw native parameter; omitted for parameterless inputs.
    pub value: Option<String>,
    /// Seek only: requested preview time in seconds.
    pub time_seconds: Option<f64>,
    /// Advance only: elapsed preview time in seconds.
    pub seconds: Option<f64>,
    /// Prepare environment, Input, Seek, Advance or Reset: return while owned background work runs when needed.
    pub background: Option<bool>,
}
