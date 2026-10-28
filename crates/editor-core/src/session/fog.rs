use map_core::fog::parse_static_ldr as parse;
use render::fog::Fog;
use serde_json::{Value, json};

use super::Session;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Selection {
    pub enabled: bool,
    pub controller: Option<u32>,
}

fn settings(fog: Fog) -> Value {
    json!({"color":fog.color(), "start":fog.start(), "end":fog.end(), "max_density":fog.max_density(), "enabled":fog.enabled()})
}

impl Session {
    pub fn fog_preview_snapshot(&self) -> String {
        format!(
            "{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.view_serial
        )
    }

    fn fog_controller(&self, id: u32) -> Result<Fog, String> {
        let entity = self
            .document
            .entity(id)
            .ok_or_else(|| format!("entity:{id} no longer exists; choose a fog controller"))?;
        if !entity
            .classname()
            .is_some_and(|name| name.eq_ignore_ascii_case("env_fog_controller"))
        {
            return Err(format!("entity:{id} is not an env_fog_controller"));
        }
        parse(entity).map_err(|error| format!("entity:{id}: {error}"))
    }

    pub fn fog_preview_state(&self) -> Value {
        self.fog_preview_report(self.fog_preview.enabled, self.fog_preview.controller)
    }

    pub fn fog_preview_report(&self, enabled: bool, controller: Option<u32>) -> Value {
        let selected = controller.map(|id| self.fog_controller(id));
        let issue = match &selected {
            Some(Err(error)) => Some(error.clone()),
            None if enabled => {
                Some("Choose an existing env_fog_controller; no controller is selected".into())
            }
            _ => None,
        };
        let fog = selected.and_then(Result::ok);
        let mut controllers = self.document.vmf().entities.iter()
            .filter(|entity| entity.classname().is_some_and(|name| name.eq_ignore_ascii_case("env_fog_controller")))
            .map(|entity| {
                let result = parse(entity);
                json!({"id":format!("entity:{}",entity.id), "targetname":entity.properties.get("targetname"),
                    "ready":result.is_ok(), "settings":result.as_ref().ok().copied().map(settings), "issue":result.err()})
            }).collect::<Vec<_>>();
        controllers.sort_by_key(|value| value["id"].as_str().unwrap().to_string());
        let summary = if let Some(issue) = &issue {
            issue.clone()
        } else if !enabled {
            "Fog preview is off".into()
        } else if fog.is_some_and(|fog| !fog.enabled()) {
            "Selected controller has fogenable=0; no fog is drawn".into()
        } else {
            "Static linear range fog · perspective Textured view".into()
        };
        json!({"snapshot":self.fog_preview_snapshot(), "enabled":enabled,
            "controller":controller.map(|id|format!("entity:{id}")), "ready":fog.is_some(),
            "active":enabled && issue.is_none() && fog.is_some_and(|fog|fog.enabled() && fog.max_density()>0.0),
            "settings":fog.map(settings), "issue":issue, "summary":summary, "controllers":controllers,
            "limits":"Static authored initial values only. View-depth ramp capped at max density; squared factor blends sRGB-decoded color in linear space. Perspective Textured surfaces/models only. Sky surfaces, background, helpers, selection and grid are unchanged. No radial/directional blend, far-Z clipping, HDR/zoom scaling, water fog, fog volumes, runtime I/O, transitions or automatic Master selection. Not a native parity claim.",
            "defaults":{"fogenable":0,"fogmaxdensity":1,"fogblend":0,"fogradial":0}})
    }

    pub fn set_fog_preview(
        &mut self,
        enabled: bool,
        controller: Option<u32>,
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.fog_preview_snapshot() {
            return Err("Map or view settings changed; refresh Fog preview before applying".into());
        }
        if enabled {
            let id = controller
                .ok_or("Choose an existing env_fog_controller; no controller is selected")?;
            self.fog_controller(id)?;
        }
        self.fog_preview = Selection {
            enabled,
            controller,
        };
        self.view_serial += 1;
        Ok(self.fog_preview_state())
    }

    pub fn fog_preview_for_render(
        &self,
        enabled: Option<bool>,
        controller: Option<u32>,
    ) -> Result<Option<Fog>, String> {
        let enabled = enabled.unwrap_or(self.fog_preview.enabled);
        let controller = controller.or(self.fog_preview.controller);
        if !enabled {
            return Ok(None);
        }
        let id =
            controller.ok_or("Choose an existing env_fog_controller; no controller is selected")?;
        self.fog_controller(id).map(Some)
    }

    pub fn fog_render_state(
        &self,
        enabled: Option<bool>,
        controller: Option<u32>,
    ) -> Result<(Option<Fog>, Value), String> {
        let fog = if enabled.is_none() && controller.is_none() {
            self.current_fog()
        } else {
            self.fog_preview_for_render(enabled, controller)?
        };
        let report = self.fog_preview_report(
            enabled.unwrap_or(self.fog_preview.enabled),
            controller.or(self.fog_preview.controller),
        );
        Ok((fog, report))
    }

    pub(super) fn current_fog(&self) -> Option<Fog> {
        self.fog_preview_for_render(None, None).ok().flatten()
    }
}
